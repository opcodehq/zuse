// Checkout diff sizes for the sidebar's "+N −N". Summary-only frames: no patches cross the wire.
import { createStore, reconcile } from "solid-js/store";
import { onConnected, rpc } from "@/core/connection";
import { M } from "@/core/methods";
import type { Chat } from "@/core/protocol";

interface DiffStat {
  checkoutId: string;
  deviceId: string;
  cwd: string;
  additions: number;
  deletions: number;
}

const [stats, setStats] = createStore<Record<string, DiffStat>>({});

onConnected(() =>
  rpc.subscribe<unknown>(M.WATCH_CHECKOUT_DIFFS, { summaryOnly: true }, (frame) => {
    if (!Array.isArray(frame)) return;
    // Checkouts missing from a frame keep their last known size, as desktop does.
    const next: Record<string, DiffStat> = {};
    for (const raw of frame as Partial<DiffStat>[]) {
      if (typeof raw?.checkoutId !== "string") continue;
      next[raw.checkoutId] = {
        checkoutId: raw.checkoutId,
        deviceId: raw.deviceId ?? "",
        cwd: raw.cwd ?? "",
        additions: raw.additions ?? 0,
        deletions: raw.deletions ?? 0,
      };
    }
    setStats(reconcile({ ...stats, ...next }));
  }),
);

/** A chat's diff as (additions, deletions): checkout id first, then device + cwd, then cwd. */
export function diffFor(chat: Chat): { additions: number; deletions: number } | null {
  let found = chat.checkoutId ? stats[chat.checkoutId] : undefined;
  if (!found && chat.cwd) {
    const all = Object.values(stats);
    found = all.find((s) => s.deviceId === chat.deviceId && s.cwd === chat.cwd) ?? all.find((s) => s.cwd === chat.cwd);
  }
  if (!found || (found.additions === 0 && found.deletions === 0)) return null;
  return { additions: found.additions, deletions: found.deletions };
}
