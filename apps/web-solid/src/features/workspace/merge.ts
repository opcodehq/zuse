// The pull request's merge control: which methods the repository allows, the remembered pick, and what the
// split button says and does in each state. Never a disabled-looking button while the pull request is open.
import { statusLine } from "./feedback";
import type { ChangeRequestDetail } from "./types";

export type MergeMethod = "squash" | "merge" | "rebase";

export const MERGE_METHODS: readonly { value: MergeMethod; label: string; detail: string }[] = [
  { value: "squash", label: "Squash and merge", detail: "Combine all commits" },
  { value: "merge", label: "Merge commit", detail: "Preserve commit history" },
  { value: "rebase", label: "Rebase and merge", detail: "Reapply all commits" },
];

/** The repository's allowed methods; every method when the engine could not read them. */
export function allowedMethods(detail: Pick<ChangeRequestDetail, "mergeMethods">) {
  const allowed = detail.mergeMethods?.length ? detail.mergeMethods : null;
  return MERGE_METHODS.filter((method) => !allowed || allowed.includes(method.value));
}

/** The last method picked when the repository still allows it, else the first allowed one. */
export function pickMethod(allowed: readonly { value: MergeMethod }[], remembered: string | null): MergeMethod {
  return allowed.find((method) => method.value === remembered)?.value ?? allowed[0]?.value ?? "squash";
}

export type MergeAction = "merge" | "autoMerge" | "menu" | "markReady";

export interface MergeButton {
  label: string;
  action: MergeAction;
  primary: boolean;
}

/**
 * Ready: "Merge", primary. Checks or reviews outstanding: "Auto-merge", quiet but enabled. Auto-merge armed:
 * "Auto-merge on", which opens the menu to cancel it. Drafts offer "Ready for review". Merged or closed: none.
 */
export function mergeButton(detail: ChangeRequestDetail): MergeButton | null {
  if (detail.state.toUpperCase() !== "OPEN") return null;
  if (detail.isDraft) return { label: "Ready for review", action: "markReady", primary: false };
  if (detail.autoMergeRequest) return { label: "Auto-merge on", action: "menu", primary: false };
  if (statusLine(detail).ready) return { label: "Merge", action: "merge", primary: true };
  if (detail.mergeable.toUpperCase() === "CONFLICTING") return { label: "Merge", action: "merge", primary: false };
  return { label: "Auto-merge", action: "autoMerge", primary: false };
}
