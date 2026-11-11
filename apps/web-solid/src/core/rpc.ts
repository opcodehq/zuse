// Request admission and stream lifetime over one transport. Mirrors crates/rpc framing.
import { retryableRead } from "./methods";
import type { Transport } from "./transport";

interface ServerFrame {
  id: number;
  ok?: unknown;
  err?: string;
  item?: unknown;
  done?: boolean;
}

export class RpcError extends Error {
  /** True when a mutation may have reached the engine before the connection dropped. */
  readonly outcomeUnknown: boolean;
  constructor(message: string, outcomeUnknown = false) {
    super(message);
    this.outcomeUnknown = outcomeUnknown;
  }
}

// The generated list misses many feeds and lookups; these change nothing on the engine, so a dropped
// connection must not report them as actions that may have completed.
const READ_NAME = /^(Watch|Get|List|Search)[A-Z]/;
const EXTRA_READS = new Set([
  "CheckHarnessUpdates", "DrawVisual", "EngineInfo", "EngineReady", "LocalImportStatus", "ObservePrWatch", "OpenVisual",
  "PollAgentLogin", "ReadWorkspaceImage", "ResolveGitAvatars", "SimulatorFrames", "SimulatorScreenshot", "SimulatorState",
  "SyncStatus", "UpdateStatus", "WarmWorkspaceSearch",
]);
export const isRead = (method: string, params: unknown) => retryableRead(method, params) || READ_NAME.test(method) || EXTRA_READS.has(method);

type Pending =
  | { kind: "call"; method: string; params: unknown; resolve: (value: unknown) => void; reject: (error: RpcError) => void }
  | { kind: "stream"; method: string; params: unknown; onItem: (value: unknown) => void; onEnd: (error?: RpcError) => void };

export interface Subscription {
  cancel(): void;
}

export class RpcClient {
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  private transport: Transport | null = null;

  /** Attach a freshly opened transport. Earlier work is failed, never replayed. */
  attach(transport: Transport) {
    this.detach("Reconnected");
    this.transport = transport;
    transport.onMessage = (text) => this.receive(text);
  }

  /** The connection is gone: reads report a retryable failure, mutations an unknown outcome. */
  /** Returns how many mutations were in flight, whose outcome is now unknown. */
  detach(reason: string): number {
    this.transport = null;
    const pending = [...this.pending.values()];
    this.pending.clear();
    let uncertainCount = 0;
    for (const entry of pending) {
      const uncertain = !isRead(entry.method, entry.params);
      if (uncertain) uncertainCount++;
      const error = new RpcError(
        uncertain ? `${reason}. The action may have completed; check before trying again.` : reason,
        uncertain,
      );
      if (entry.kind === "call") entry.reject(error);
      else entry.onEnd(error);
    }
    return uncertainCount;
  }

  get connected() {
    return this.transport !== null;
  }

  call<T = unknown>(method: string, params: unknown = {}): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const id = this.send(method, params, reject);
      if (id === null) return;
      this.pending.set(id, { kind: "call", method, params, resolve: resolve as (value: unknown) => void, reject });
    });
  }

  subscribe<T = unknown>(
    method: string,
    params: unknown,
    onItem: (value: T) => void,
    onEnd: (error?: RpcError) => void = () => {},
  ): Subscription {
    const id = this.send(method, params, onEnd);
    if (id === null) return { cancel() {} };
    this.pending.set(id, { kind: "stream", method, params, onItem: onItem as (value: unknown) => void, onEnd });
    return {
      cancel: () => {
        if (!this.pending.delete(id)) return;
        try {
          this.transport?.send(JSON.stringify({ id, cancel: true }));
        } catch {
          // The stream ends with the connection anyway.
        }
      },
    };
  }

  private send(method: string, params: unknown, fail: (error: RpcError) => void): number | null {
    if (!this.transport) {
      fail(new RpcError("The engine is not connected. This request was not sent."));
      return null;
    }
    const id = this.nextId++;
    try {
      this.transport.send(JSON.stringify({ id, method, params }));
    } catch {
      fail(new RpcError("The engine is not connected. This request was not sent."));
      return null;
    }
    return id;
  }

  private receive(text: string) {
    for (const line of text.split("\n")) {
      if (!line.trim()) continue;
      let frame: ServerFrame;
      try {
        frame = JSON.parse(line) as ServerFrame;
      } catch {
        continue;
      }
      const entry = this.pending.get(frame.id);
      if (!entry) continue;
      if (entry.kind === "call") {
        if (frame.err !== undefined) {
          this.pending.delete(frame.id);
          entry.reject(new RpcError(frame.err));
        } else if ("ok" in frame) {
          this.pending.delete(frame.id);
          entry.resolve(frame.ok);
        } else if ("item" in frame) {
          // A stream answered a unary call: its first item is the reply.
          this.pending.delete(frame.id);
          this.transport?.send(JSON.stringify({ id: frame.id, cancel: true }));
          entry.resolve(frame.item);
        }
        continue;
      }
      if ("item" in frame) entry.onItem(frame.item);
      else if ("ok" in frame) entry.onItem(frame.ok);
      if (frame.err !== undefined || frame.done) {
        this.pending.delete(frame.id);
        entry.onEnd(frame.err !== undefined ? new RpcError(frame.err) : undefined);
      }
    }
  }
}
