// Startup phase, elapsed time and status badges for cloud computers. Pure, so the copy is testable without a DOM.
import type { BadgeTone } from "../ui";
import { cloudIssue, GENERIC_ISSUE, type CloudIssue } from "./errors";
import type { Workspace } from "./types";

/** The server stops a start that has not come online after 10 minutes (crates/cloud allocation effects.rs). */
export const START_DEADLINE_MS = 10 * 60_000;
/** Past this a start reads as slow. */
export const SLOW_START_MS = 90_000;

/** "0:42", "12:05", "1:02:03". */
export function elapsed(ms: number) {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

/** What a starting computer is doing now, with desktop's step names (preparation.rs `progress`); null when not starting. */
export function startupLabel(row: Pick<Workspace, "state" | "startupPhase" | "preparation">): string | null {
  if (!["queued", "provisioning", "setup", "starting", "creating", "resuming"].includes(row.state)) return null;
  const phase = row.startupPhase ?? "";
  const running = ([["repository", "Syncing repository"], ["environment", "Preparing environment"], ["credentials", "Preparing agent access"], ["agent", "Starting agent"]] as const)
    .filter(([key]) => row.preparation?.[key]?.state === "running")
    .map(([, title]) => title);
  if (running.length) return running.join(" · ");
  if (phase === "starting-agent") return "Starting agent";
  if (phase === "syncing-repository") return "Syncing repository";
  return phase === "resuming" || row.state === "resuming" ? "Resuming computer" : "Starting computer";
}

/** The engine's connection messages in the same words as the startup steps. */
export function phaseText(message: string) {
  const text = message.trim().replace(/…$|\.\.\.$/, "");
  const known: Record<string, string> = {
    "Connecting to cloud workspace": "Connecting to the cloud computer",
    "Resuming cloud workspace": "Resuming computer",
    "Preparing cloud computer": "Preparing computer",
    "Connecting runtime": "Starting computer",
  };
  return known[text] ?? (text || "Connecting to the cloud computer");
}

/** Why a computer stopped: a start the server gave up on, a failed wake, or a failed or blocked row. Null otherwise. */
export function rowIssue(row: Workspace): CloudIssue | null {
  const recovery = row.allocationRecovery;
  if (recovery) return cloudIssue(recovery.errorCode) ?? cloudIssue("cloud_provider_rejected");
  const failedStep = (["repository", "environment", "credentials"] as const).map((key) => row.preparation?.[key]).find((step) => step?.state === "failed");
  if (row.state === "failed" || row.state === "blocked") return cloudIssue(failedStep?.errorCode) ?? cloudIssue(row.statusCode) ?? GENERIC_ISSUE;
  // A failed wake settles as paused and keeps its code (crates/cloud runtime/lifecycle.rs).
  if (row.state === "paused") return row.statusCode.startsWith("cloud_") ? cloudIssue(row.statusCode) ?? GENERIC_ISSUE : null;
  return null;
}

export function stateBadge(row: Workspace): { tone: BadgeTone; label: string } {
  if (row.allocationRecovery) return { tone: "error", label: "Failed" };
  switch (row.state) {
    case "ready":
    case "running":
      return { tone: "ok", label: "Running" };
    case "paused":
      return rowIssue(row) ? { tone: "error", label: "Couldn't wake" } : { tone: "outline", label: "Paused" };
    case "failed":
      return { tone: "error", label: "Failed" };
    case "blocked":
      return { tone: "error", label: "Blocked" };
    case "pausing":
      return { tone: "warn", label: "Pausing" };
    case "queued":
    case "starting":
    case "provisioning":
    case "setup":
    case "creating":
      return { tone: "warn", label: row.startupPhase === "resuming" ? "Waking" : "Starting" };
    case "resuming":
    case "waking":
      return { tone: "warn", label: "Waking" };
    case "archiving":
      return { tone: "warn", label: "Archiving" };
    case "deleting":
      return { tone: "warn", label: "Deleting" };
    default:
      return { tone: "outline", label: "Updating" };
  }
}
