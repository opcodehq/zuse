// Which mood a Zapper's mascot shows, from statuses the client already derived.
// Mirrors mood_of in crates/ui/src/shell/zapper_avatar.rs; see there for the order.
import type { Zapper } from "@/core/zapper-types";
import type { Indicator } from "@/features/sidebar/model";
import type { Mood } from "./mascot-motion";

/**
 * `coordinator` is undefined while its chat isn't synced; statuses arrive staleness-gated.
 * There is no loading mood: nothing marks a Zapper as starting, and a thread's "Starting"
 * label only means no data has arrived yet, which a stuck thread looks the same as.
 */
export function moodFor(zapper: Zapper, homeOnline: boolean, coordinator: Indicator | undefined, workers: Indicator[]): Mood {
  if (zapper.status !== "active") return "resting";
  if (!homeOnline) return "offline";
  const asks = zapper.decisions.some(d => d.answer === null)
    || zapper.issues.some(issue => issue.details.status === "needsYou")
    || coordinator === "awaitingInput"
    || workers.includes("awaitingInput");
  if (asks) return "needsYou";
  if (coordinator === "errored") return "blocked";
  if (coordinator === "working") return "working";
  if (workers.includes("working")) return "delegating";
  if (coordinator === "completed") return "done";
  return "awake";
}
