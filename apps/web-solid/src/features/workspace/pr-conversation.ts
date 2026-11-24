import type { ChangeRequestCheck, ChangeRequestDetail } from "./types";

export function checkStatus(check: ChangeRequestCheck) {
  return (check.conclusion || check.state || check.status || "").toUpperCase();
}

export function prStatus(detail: ChangeRequestDetail): string {
  const state = detail.state.toUpperCase();
  if (state === "MERGED") return "This pull request has been merged.";
  if (state === "CLOSED") return "This pull request is closed without merging.";
  const notes: string[] = [];
  if (detail.isDraft) notes.push("This pull request is still a draft.");
  if (detail.mergeable.toUpperCase() === "CONFLICTING") notes.push("This branch has conflicts that must be resolved.");
  const checks = detail.statusCheckRollup;
  const failed = checks.filter(check => /^(FAILURE|ERROR|TIMED_OUT|ACTION_REQUIRED|CANCELLED|STARTUP_FAILURE)$/.test(checkStatus(check))).length;
  const pending = checks.filter(check => !/^(SUCCESS|NEUTRAL|SKIPPED|FAILURE|ERROR|TIMED_OUT|ACTION_REQUIRED|CANCELLED|STARTUP_FAILURE)$/.test(checkStatus(check))).length;
  if (failed) notes.push(`${failed} ${failed === 1 ? "check is" : "checks are"} failing.`);
  if (pending) notes.push(`${pending} ${pending === 1 ? "check is" : "checks are"} still pending.`);
  if (!checks.length) notes.push("No checks have been reported.");
  else if (!failed && !pending) notes.push("All reported checks have finished without failures.");
  const review = detail.reviewDecision.toUpperCase();
  if (review === "CHANGES_REQUESTED") notes.push("Changes were requested in review.");
  else if (review === "REVIEW_REQUIRED") notes.push("A review is required.");
  else if (review === "APPROVED") notes.push("The review is approved.");
  else notes.push("Review approval is not confirmed.");
  const threads = detail.reviewThreads?.filter(thread => !thread.isResolved).length;
  if (threads) notes.push(`${threads} review ${threads === 1 ? "thread remains" : "threads remain"} unresolved.`);
  if (detail.autoMergeRequest) notes.push(`Auto-merge is enabled (${detail.autoMergeRequest.mergeMethod.toLowerCase()}).`);
  return notes.join(" ");
}

export function prQuestion(url: string, intent: "summary" | "blockers") {
  const question = intent === "summary" ? "Summarize this pull request and what needs my attention" : "Explain the current blockers and the next safe step for this pull request";
  return `${question}: ${url}\n\nRead the current diff, checks and reviews before answering. This is a request for analysis only; do not merge, enable auto-merge, close, or otherwise change the pull request.`;
}

export function appendPrQuestion(draft: string, question: string) {
  return draft.trim() ? `${draft}\n\n${question}` : question;
}
