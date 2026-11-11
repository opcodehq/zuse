// Loads crates/web-link (built by scripts/build-link.mjs) only when a paired computer is used.

export interface LinkStatus {
  state: "connecting" | "connected" | "reattaching";
  kind?: "loopback" | "lan" | "tailnet" | "public" | "relay";
  label?: string;
  rttMs?: number;
  resumable: boolean;
  compressed: boolean;
}

export type LinkEnd = "unreachable" | "authentication" | "lost" | "endedByHost" | "closed";

export interface Effects {
  open: { socket: number; url: string }[];
  close: number[];
  write: { socket: number; records: Uint8Array[] }[];
  deliver: string[];
  waitMs: number;
  status?: LinkStatus;
  saved?: string;
  engineInfo?: string;
  connected: boolean;
  ended?: LinkEnd;
  accepted?: boolean;
}

export interface WebLink {
  start(): Effects;
  socketOpened(socket: number, now: number): Effects;
  record(socket: number, record: Uint8Array, now: number): Effects;
  socketClosed(socket: number, now: number): Effects;
  send(message: string, now: number): Effects;
  tick(now: number): Effects;
  networkChanged(network: string, now: number): Effects;
  close(now: number): Effects;
  free(): void;
}

export interface LinkModule {
  WebLink: { open(link: string, saved: string | null | undefined, network: string, now: number): WebLink };
  inspectLink(link: string): { origin: string; id: string; kind: string };
  linkFromFragment(origin: string, fragment: string): string;
  normalizeLink(link: string): string;
}

interface Generated extends LinkModule {
  default: () => Promise<unknown>;
}

// A glob, not an import, so typecheck and builds still work before the WebAssembly is built.
const generated = import.meta.glob<Generated>("./pkg/zuse_web_link.js");

let loading: Promise<LinkModule> | null = null;

export function loadLink(): Promise<LinkModule> {
  loading ??= (async () => {
    const load = Object.values(generated)[0];
    if (!load) throw new Error("This build does not include paired connections. Rebuild with bun run build.");
    const module = await load();
    await module.default();
    return module;
  })();
  loading.catch(() => (loading = null));
  return loading;
}
