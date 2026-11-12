// Host-owned visual sessions (crates/protocol/src/visual_session.rs, as crates/viewport/src/visuals.rs drives them).
// A control targets the revision the reader saw; disconnect and navigation retire sessions instead of replaying input.
import { createEffect, createRoot, on } from "solid-js";
import { createStore, produce, reconcile } from "solid-js/store";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import type { Subscription } from "@/core/rpc";
import { state } from "@/core/store";
import { view } from "@/core/view";

const VERSION = 1;
const MAX_OPEN = 8;

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export type JsonObject = { [key: string]: Json };
export type Argument = { kind: "none" } | { kind: "text"; value: string } | { kind: "bool"; value: boolean } | { kind: "number"; value: number } | { kind: "point"; value: [number, number] };

export interface VisualView {
  chatId: string;
  revision: number;
  tree: JsonObject | null;
  error: string | null;
  busy: boolean;
  /** The connection dropped: the last tree stays on screen, inert, until the reconnect reopens it. */
  offline: boolean;
  sliderTarget: { path: string; value: number } | null;
  /** Draw commands per canvas path. */
  canvases: Record<string, { commands: Json[]; error: string | null }>;
}

const [visuals, setVisuals] = createStore<Record<string, VisualView>>({});
export { visuals };

interface CanvasInternals {
  handler: number;
  height: number;
  animate: boolean;
  element: HTMLElement | null;
  visible: boolean;
  pointer: { x: number; y: number; down: boolean } | null;
  signature: string | null;
  requested: number;
  pending: boolean;
}
interface Internals {
  owner: Subscription | null;
  lease: string | null;
  started: number;
  queuedSlider: { path: string; argument: Argument; node: JsonObject } | null;
  canvases: Map<string, CanvasInternals>;
}
const sessions = new Map<string, Internals>();
let timer: ReturnType<typeof setInterval> | undefined;

export const visualKey = (chatId: string, visualId: string) => `${chatId}\u0000${visualId}`;

export const isObject = (value: Json | undefined): value is JsonObject => !!value && typeof value === "object" && !Array.isArray(value);

/** A node's variant name and payload: `{ kind: { Stack: {...} } }` or a bare unit variant. */
export function variant(node: Json | undefined): [string, Json] | null {
  if (!isObject(node)) return null;
  const kind = node.kind;
  if (typeof kind === "string") return [kind, null];
  if (!isObject(kind)) return null;
  const name = Object.keys(kind)[0];
  return name ? [name, kind[name]!] : null;
}

function input(node: Json | undefined): [number, string] | null {
  const found = variant(node);
  if (!found || !isObject(found[1])) return null;
  const [kind, value] = found;
  const field = kind === "Button" || kind === "Stack" || kind === "Canvas" ? "on_click" : kind === "Tabs" || kind === "Toggle" || kind === "Slider" ? "on_change" : null;
  const handler = field ? value[field] : undefined;
  return typeof handler === "number" ? [handler, kind] : null;
}

/** Tree paths are `/child` indices; tables take `/row/column`. */
export function nodeAt(tree: Json | null, path: string): JsonObject | null {
  let node: Json | undefined = tree;
  if (path === "") return isObject(node) ? node : null;
  const indices = path.replace(/^\//, "").split("/").map(Number);
  for (let i = 0; i < indices.length; i++) {
    const found = variant(node);
    if (!found || !isObject(found[1])) return null;
    const [kind, value] = found;
    if (kind === "Table") {
      const row = (value.rows as Json[] | undefined)?.[indices[i]!];
      node = Array.isArray(row) ? row[indices[++i]!] : undefined;
    } else {
      node = (value.children as Json[] | undefined)?.[indices[i]!];
    }
  }
  return isObject(node) ? node : null;
}

function collectCanvases(node: Json | undefined, path: string, out: Map<string, CanvasInternals>, previous: Map<string, CanvasInternals>) {
  const found = variant(node);
  if (!found || !isObject(found[1])) return;
  const [kind, value] = found;
  if (kind === "Canvas") {
    const height = typeof value.height === "number" && Number.isFinite(value.height) ? Math.min(720, Math.max(40, value.height)) : 200;
    const old = previous.get(path);
    out.set(path, {
      handler: typeof value.draw === "number" ? value.draw : 0,
      height,
      animate: value.animate === true,
      element: old?.element ?? null,
      visible: old?.visible ?? false,
      pointer: old?.pointer ?? null,
      signature: null,
      requested: Date.now(),
      pending: false,
    });
  }
  (Array.isArray(value.children) ? value.children : []).forEach((child, i) => collectCanvases(child, `${path}/${i}`, out, previous));
  (Array.isArray(value.rows) ? value.rows : []).forEach((row, r) =>
    (Array.isArray(row) ? row : []).forEach((cell, c) => collectCanvases(cell, `${path}/${r}/${c}`, out, previous)),
  );
}

function sameSlider(a: JsonObject, b: JsonObject) {
  const x = variant(a);
  const y = variant(b);
  if (!x || !y || x[0] !== "Slider" || y[0] !== "Slider" || !isObject(x[1]) || !isObject(y[1])) return false;
  return a.key === b.key && ["label", "min", "max", "step"].every((k) => (x[1] as JsonObject)[k] === (y[1] as JsonObject)[k]);
}

/** Keep what was shown while the connection is down; the host session is gone either way. */
function suspend(key: string) {
  const session = sessions.get(key);
  if (!session) return;
  const owner = session.owner;
  session.owner = null;
  session.lease = null;
  session.queuedSlider = null;
  owner?.cancel();
  if (visuals[key])
    setVisuals(key, produce((v) => {
      v.offline = true;
      v.busy = false;
      v.sliderTarget = null;
    }));
}

function fail(key: string, message: string) {
  const session = sessions.get(key);
  session?.owner?.cancel();
  if (session) {
    session.owner = null;
    session.queuedSlider = null;
  }
  if (visuals[key])
    setVisuals(key, produce((v) => {
      v.error = message.slice(0, 2000);
      v.busy = false;
      v.sliderTarget = null;
    }));
}

const deviceOf = (chatId: string) => state.chats[chatId]?.deviceId ?? state.localDeviceId ?? "";

/** Visual parts on screen; off-screen sessions give their slot to a new one. */
const mounted = new Set<string>();
export function trackMounted(key: string) {
  mounted.add(key);
  return () => mounted.delete(key);
}

export function openVisual(chatId: string, visualId: string) {
  const key = visualKey(chatId, visualId);
  closeVisual(key);
  if (state.connection.kind !== "connected") return false;
  if (Object.keys(visuals).length >= MAX_OPEN) {
    const idle = Object.keys(visuals).find((other) => !mounted.has(other));
    if (!idle) return false;
    closeVisual(idle);
  }
  const session: Internals = { owner: null, lease: null, started: performance.now(), queuedSlider: null, canvases: new Map() };
  sessions.set(key, session);
  setVisuals(key, { chatId, revision: 0, tree: null, error: null, busy: false, offline: false, sliderTarget: null, canvases: {} });
  session.owner = rpc.subscribe<JsonObject>(
    M.OPEN_VISUAL,
    { chatId, visualId, targetDeviceId: deviceOf(chatId) },
    (item) => receiveFrame(key, item),
    () => {
      if (sessions.get(key) !== session || !session.owner) return;
      if (!rpc.connected) suspend(key);
      else fail(key, "Visual session ended. Reopen it to start again.");
    },
  );
  timer ??= setInterval(refreshDraws, 100);
  return true;
}

export function closeVisual(key: string) {
  const session = sessions.get(key);
  if (session) {
    const owner = session.owner;
    session.owner = null;
    owner?.cancel();
    sessions.delete(key);
  }
  if (visuals[key]) setVisuals(key, undefined!);
  if (!sessions.size && timer) {
    clearInterval(timer);
    timer = undefined;
  }
}

function receiveFrame(key: string, item: JsonObject) {
  const session = sessions.get(key);
  const current = visuals[key];
  if (!session || !current) return;
  const valid =
    item.version === VERSION &&
    typeof item.lease === "string" &&
    item.lease.length <= 128 &&
    typeof item.revision === "number" &&
    isObject(item.tree) &&
    (session.lease === null || (session.lease === item.lease && item.revision === current.revision + 1));
  if (!valid) return fail(key, typeof item.error === "string" ? item.error : "The host returned an invalid visual frame.");
  session.lease = item.lease as string;
  const previous = session.canvases;
  session.canvases = new Map();
  collectCanvases(item.tree, "", session.canvases, previous);
  const revision = item.revision as number;
  setVisuals(key, produce((v) => {
    v.revision = revision;
    v.busy = false;
    v.sliderTarget = null;
    v.canvases = Object.fromEntries([...session.canvases.keys()].map((path) => [path, v.canvases[path] ?? { commands: [], error: null }]));
  }));
  setVisuals(key, "tree", reconcile(item.tree as JsonObject, { merge: true }));
  const queued = session.queuedSlider;
  session.queuedSlider = null;
  if (queued) {
    const node = nodeAt(visuals[key]!.tree, queued.path);
    if (node && sameSlider(node, queued.node)) control(key, queued.path, revision, queued.argument);
  }
}

/** Run a node's handler. Sliders coalesce: while a reply is pending, only the latest value is kept. */
export function control(key: string, path: string, revision: number, argument: Argument) {
  if (state.connection.kind !== "connected") return;
  const session = sessions.get(key);
  const current = visuals[key];
  if (!session || !current || current.error || current.revision !== revision) return;
  const node = nodeAt(current.tree, path);
  const found = input(node ?? undefined);
  if (!node || !found) return;
  const [handler, kind] = found;
  const slider = kind === "Slider";
  if (slider && argument.kind === "number") setVisuals(key, "sliderTarget", { path, value: argument.value });
  if (current.busy) {
    if (slider) session.queuedSlider = { path, argument, node: JSON.parse(JSON.stringify(node)) as JsonObject };
    return;
  }
  if (!slider) setVisuals(key, "sliderTarget", null);
  const payload = variant(node)?.[1];
  if (slider && argument.kind === "number" && isObject(payload) && payload.value === argument.value) {
    setVisuals(key, "sliderTarget", null);
    return;
  }
  if (!session.lease) return;
  setVisuals(key, "busy", true);
  rpc
    .call(M.CONTROL_VISUAL, { lease: session.lease, revision, handler, argument, targetDeviceId: deviceOf(current.chatId) })
    .catch(() => {
      // Only the owner's next tree unlocks interaction; an uncertain control is never retried.
      if (sessions.get(key) === session) fail(key, "The control response was interrupted. Reopen the visual before trying again.");
    });
}

export function canvasInternals(key: string, path: string) {
  return sessions.get(key)?.canvases.get(path);
}

const reducedMotion = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

function refreshDraws() {
  if (state.connection.kind !== "connected" || document.visibilityState !== "visible") return;
  const now = Date.now();
  for (const [key, session] of sessions) {
    const current = visuals[key];
    if (!current || current.error || current.busy || !session.lease) continue;
    if ([...session.canvases.values()].some((c) => c.pending)) continue;
    const candidates = [...session.canvases.entries()].sort((a, b) => a[1].requested - b[1].requested);
    for (const [path, canvas] of candidates) {
      const width = Math.min(2048, Math.max(0, canvas.element?.clientWidth ?? 0));
      if (width < 1 || !canvas.visible || current.canvases[path]?.error) continue;
      const signature = JSON.stringify([width, canvas.height, canvas.pointer]);
      if (signature === canvas.signature && (!canvas.animate || reducedMotion() || now - canvas.requested < 100)) continue;
      canvas.signature = signature;
      canvas.requested = now;
      canvas.pending = true;
      const revision = current.revision;
      rpc
        .call<{ revision?: number; commands?: Json[] }>(M.DRAW_VISUAL, {
          lease: session.lease,
          revision,
          handler: canvas.handler,
          width,
          height: canvas.height,
          time: Math.min(86400, (performance.now() - session.started) / 1000),
          pointer: canvas.pointer,
          targetDeviceId: deviceOf(current.chatId),
        })
        .then(
          (reply) => {
            canvas.pending = false;
            if (sessions.get(key) !== session || visuals[key]?.revision !== revision || !visuals[key]!.canvases[path]) return;
            if (reply?.revision === revision && Array.isArray(reply.commands)) setVisuals(key, "canvases", path, { commands: reply.commands, error: null });
            else setVisuals(key, "canvases", path, "error", "The canvas could not draw this frame.");
          },
          () => {
            canvas.pending = false;
            if (sessions.get(key) === session && visuals[key]?.canvases[path]) setVisuals(key, "canvases", path, "error", "The canvas could not draw this frame.");
          },
        );
      break;
    }
  }
}

function closeAll() {
  for (const key of [...sessions.keys()]) closeVisual(key);
}

function suspendAll() {
  for (const key of sessions.keys()) suspend(key);
}

// Navigation and reconnects retire every session; reopening starts fresh. A drop only suspends them.
createRoot(() => {
  createEffect(on(() => [view.selectedChatId, state.epoch] as const, closeAll, { defer: true }));
  createEffect(on(() => state.connection.kind, (kind) => kind !== "connected" && suspendAll(), { defer: true }));
});
