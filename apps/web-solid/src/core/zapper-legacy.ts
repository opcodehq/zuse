// Zapper calls go out under their names from before Zots were renamed
// Zappers: this hosted client can reach an engine that predates the rename,
// and every engine accepts the old names. Mirrors
// `zuse_protocol::zappers::legacy`. Legacy, remove after one release.
import { M } from "./methods";

const METHOD: Record<string, string> = {
  [M.CONTROL_ZAPPER_COMMAND]: "ZotCommand",
  [M.CONTROL_WATCH_ZAPPERS]: "WatchZots",
  [M.CONTROL_ZAPPER_FILES]: "ZotFiles",
  [M.CONTROL_ZAPPER_EVENTS]: "ZotEvents",
  [M.CONTROL_ZAPPER_ISSUE]: "ZotIssue",
  [M.CONTROL_ACK_ZAPPER_EVENTS]: "AckZotEvents",
};
const ID_FIELD = "zotId";
const WORKER_PREFIX = "zapper-worker-";
const LEGACY_WORKER_PREFIX = "zot-worker-";

/** The name to send a Zapper method under. */
export const wireMethod = (method: string) => METHOD[method] ?? method;

/** Request fields as engines from before the rename read them. */
export function wireParams(params: Record<string, unknown>): Record<string, unknown> {
  const { zapperId, ...rest } = params;
  const out: Record<string, unknown> = zapperId === undefined ? rest : { ...rest, [ID_FIELD]: zapperId };
  const action = out.action as { type?: string; worker?: { chatId?: string } } | undefined;
  const chatId = action?.type === "delegate" ? action.worker?.chatId : undefined;
  if (chatId?.startsWith(WORKER_PREFIX))
    out.action = { ...action, worker: { ...action!.worker, chatId: LEGACY_WORKER_PREFIX + chatId.slice(WORKER_PREFIX.length) } };
  return out;
}

/** Events from any engine, with the current `zapperId` field. */
export function eventsFromWire<T>(events: T[]): T[] {
  return events.map((event) => {
    const { [ID_FIELD]: legacy, ...rest } = event as Record<string, unknown>;
    return (legacy === undefined ? event : { zapperId: legacy, ...rest }) as T;
  });
}
