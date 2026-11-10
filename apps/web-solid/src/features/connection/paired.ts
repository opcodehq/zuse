// The paired encrypted transport. crates/web-link decides routes, takeovers and resume; this file
// owns the WebSockets and the timer and carries out its effects.
import { ConnectionBlocked } from "@/core/connection";
import type { Transport } from "@/core/transport";
import { describeLink, remember, saveRoutes, savedRoutes } from "./computers";
import { loadLink, type Effects, type LinkEnd, type WebLink } from "./link/wasm";
import { setArea } from "./status";

// Browsers expose no interface list, so every network looks alike; network events still re-probe.
const NETWORK = "browser";
const MAX_BUFFERED = 20 * 1024 * 1024;
const now = () => Math.max(1, Math.round(performance.now()));

/** Engine identity seen by this tab, so a link that starts pointing elsewhere is caught. */
let knownDevice: string | null = null;
/** Open paired transports, so the reattaching banner can ask them to try every route now. */
const live = new Set<PairedTransport>();

export function retryPaired() {
  for (const transport of live) transport.retryNow();
}

// A revoked pairing is refused before the handshake, which a browser cannot tell apart from an
// unreachable host. When the host itself answers over HTTP but keeps refusing the pairing, ask for
// a new invitation instead of retrying forever. Relays answer for absent hosts, so only direct routes count.
const REFUSALS_BEFORE_ASKING = 3;
let refusals = 0;
const REVOKED = "This computer no longer accepts this pairing. It may have been revoked. Open a new invitation from the other computer.";

async function hostAnswers(origin: string): Promise<boolean> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 3000);
  try {
    await fetch(`${origin}/`, { mode: "no-cors", cache: "no-store", credentials: "omit", signal: controller.signal });
    return true;
  } catch {
    return false;
  } finally {
    clearTimeout(timer);
  }
}

const endMessages: Record<LinkEnd, string> = {
  unreachable: "Could not reach the paired computer",
  authentication: "This pairing is no longer accepted. It may have been revoked or expired.",
  lost: "The paired connection was lost",
  endedByHost: "The paired computer ended this session",
  closed: "Paired connection closed",
};

export class PairedTransport implements Transport {
  onClose: ((reason: string) => void) | null = null;
  private receiver: ((text: string) => void) | null = null;
  private early: string[] = [];
  private link: WebLink | null = null;
  private sockets = new Map<number, WebSocket>();
  private timer: ReturnType<typeof setTimeout> | undefined;
  private opening: { resolve: () => void; reject: (error: Error) => void } | null = null;
  private ended = false;
  private readonly id: string;
  private direct = false;

  constructor(private readonly exported: string) {
    this.id = describeLink(exported)?.id ?? "";
  }

  get onMessage() {
    return this.receiver;
  }
  set onMessage(next: ((text: string) => void) | null) {
    this.receiver = next;
    if (!next) return;
    for (const text of this.early.splice(0)) next(text);
  }

  async open(): Promise<void> {
    const module = await loadLink();
    this.direct = ["loopback", "lan", "tailnet"].includes(module.inspectLink(this.exported).kind);
    const link = module.WebLink.open(this.exported, savedRoutes(this.id), NETWORK, now());
    this.link = link;
    const opened = new Promise<void>((resolve, reject) => (this.opening = { resolve, reject }));
    window.addEventListener("online", this.networkChanged);
    document.addEventListener("visibilitychange", this.networkChanged);
    live.add(this);
    this.apply(link.start());
    return opened;
  }

  send(text: string) {
    if (!this.link || this.ended) throw new Error("The paired computer is not connected");
    const effects = this.link.send(text, now());
    this.apply(effects);
    if (!effects.accepted) throw new Error("The paired computer is not connected");
  }

  close() {
    if (!this.link || this.ended) return;
    this.apply(this.link.close(now()));
  }

  private readonly networkChanged = () => {
    if (document.visibilityState === "hidden") return;
    this.retryNow();
  };

  retryNow() {
    if (this.link && !this.ended) this.apply(this.link.networkChanged(NETWORK, now()));
  }

  private apply(effects: Effects) {
    if (!this.link) return;
    for (const id of effects.close) this.drop(id);
    for (const { socket, url } of effects.open) this.dial(socket, url);
    for (const { socket, records } of effects.write) {
      const ws = this.sockets.get(socket);
      if (!ws || ws.readyState !== WebSocket.OPEN) continue;
      if (ws.bufferedAmount > MAX_BUFFERED) {
        this.lost(socket);
        continue;
      }
      for (const record of records) ws.send(record);
    }
    for (const text of effects.deliver) {
      if (this.receiver) this.receiver(text);
      else this.early.push(text);
    }
    if (effects.status) setArea("link", effects.status);
    if (effects.saved && this.id) saveRoutes(this.id, effects.saved);
    if (effects.engineInfo && !this.verify(effects.engineInfo)) return;
    if (effects.connected && this.opening) {
      refusals = 0;
      remember(this.exported);
      this.opening.resolve();
      this.opening = null;
    }
    if (effects.ended) {
      this.finish(effects.ended);
      return;
    }
    if (this.ended) return;
    clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      if (this.link && !this.ended) this.apply(this.link.tick(now()));
    }, Math.max(1, effects.waitMs));
  }

  private verify(info: string): boolean {
    let device: string | undefined;
    try {
      device = (JSON.parse(info) as { deviceId?: string }).deviceId;
    } catch {
      device = undefined;
    }
    if (!device) return true;
    if (knownDevice && knownDevice !== device) {
      this.fail(
        new ConnectionBlocked({
          kind: "pairingRequired",
          message: "This pairing now reaches a different computer. Reload when your local edits are safe.",
        }),
      );
      return false;
    }
    knownDevice = device;
    return true;
  }

  private dial(id: number, url: string) {
    let ws: WebSocket;
    try {
      // An HTTPS page cannot open plain ws:// routes; the link treats that route as failed.
      ws = new WebSocket(url);
    } catch {
      queueMicrotask(() => this.lost(id));
      return;
    }
    ws.binaryType = "arraybuffer";
    this.sockets.set(id, ws);
    ws.onopen = () => {
      if (this.sockets.get(id) === ws && this.link) this.apply(this.link.socketOpened(id, now()));
    };
    ws.onmessage = (event: MessageEvent) => {
      if (this.sockets.get(id) !== ws || !this.link) return;
      if (event.data instanceof ArrayBuffer) this.apply(this.link.record(id, new Uint8Array(event.data), now()));
      else this.lost(id);
    };
    ws.onclose = ws.onerror = () => {
      if (this.sockets.get(id) === ws) this.lost(id);
    };
  }

  private lost(id: number) {
    this.drop(id);
    if (this.link && !this.ended) this.apply(this.link.socketClosed(id, now()));
  }

  private drop(id: number) {
    const ws = this.sockets.get(id);
    if (!ws) return;
    this.sockets.delete(id);
    ws.onopen = ws.onmessage = ws.onclose = ws.onerror = null;
    try {
      ws.close();
    } catch {
      // Already closing.
    }
  }

  private finish(end: LinkEnd) {
    const message = endMessages[end];
    if (end === "authentication") return this.fail(new ConnectionBlocked({ kind: "pairingRequired", message }));
    if (end !== "unreachable" || !this.opening || !this.direct || ++refusals < REFUSALS_BEFORE_ASKING) return this.fail(new Error(message));
    // Stop the link now; opening settles once the host has been checked.
    const opening = this.opening;
    this.fail(new Error(message), false);
    const origin = describeLink(this.exported)?.origin ?? "";
    void hostAnswers(origin).then((answers) => {
      if (answers) refusals = 0;
      opening.reject(answers ? new ConnectionBlocked({ kind: "pairingRequired", message: REVOKED }) : new Error(message));
    });
  }

  private fail(error: Error, settle = true) {
    if (this.ended) return;
    this.ended = true;
    clearTimeout(this.timer);
    for (const id of [...this.sockets.keys()]) this.drop(id);
    window.removeEventListener("online", this.networkChanged);
    document.removeEventListener("visibilitychange", this.networkChanged);
    live.delete(this);
    setArea("link", null);
    const link = this.link;
    this.link = null;
    queueMicrotask(() => link?.free());
    if (this.opening) {
      if (settle) this.opening.reject(error);
      this.opening = null;
    } else {
      this.onClose?.(error.message);
    }
  }
}
