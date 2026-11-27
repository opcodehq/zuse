// One geometric icon per issue status, keyed by the status type so renamed labels keep their icon.
import type { IssueStatus } from "@/core/zapper-types";
import "./status-icon.css";

export const ISSUE_STATUS_ORDER: IssueStatus[] = ["needsYou", "inReview", "inProgress", "blocked", "todo", "backlog", "done", "canceled"];

export const ISSUE_STATUS_LABELS: Record<IssueStatus, string> = {
  needsYou: "Needs you",
  inReview: "In review",
  inProgress: "In progress",
  blocked: "Blocked",
  todo: "Todo",
  backlog: "Backlog",
  done: "Done",
  canceled: "Canceled",
};

const RING = '<circle cx="8" cy="8" r="6.25" fill="none" stroke="currentColor" stroke-width="1.5"/>';
const SHAPES: Record<IssueStatus, string> = {
  needsYou: `${RING}<path d="M8 4.75v3.75" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/><circle cx="8" cy="10.9" r="0.95" fill="currentColor"/>`,
  inReview: `${RING}<path d="M8 4.5A3.5 3.5 0 1 1 4.5 8H8Z" fill="currentColor"/>`,
  inProgress: `${RING}<path d="M8 4.5a3.5 3.5 0 0 1 0 7Z" fill="currentColor"/>`,
  blocked: '<path d="M1.75 10h4l-4 4.25h4M7.75 5.75h3.25l-3.25 3.5H11M12 2h2.25L12 4.5h2.25" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>',
  todo: RING,
  backlog: '<circle cx="8" cy="8" r="6.25" fill="none" stroke="currentColor" stroke-width="1.5" stroke-dasharray="2.45 2.45"/>',
  done: '<circle cx="8" cy="8" r="7" fill="currentColor"/><path class="status-icon-knock" d="M5.25 8.2l1.8 1.8 3.7-3.9" fill="none" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>',
  canceled: `${RING}<path d="M5.9 5.9l4.2 4.2M10.1 5.9l-4.2 4.2" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>`,
};

export const isIssueStatus = (status: string): status is IssueStatus => Object.hasOwn(SHAPES, status);

/** The icon's markup, for surfaces that build HTML strings (transcript chips). */
export function statusIconHtml(status: IssueStatus, size = 14) {
  return `<svg class="status-icon" data-status="${status}" width="${size}" height="${size}" viewBox="0 0 16 16" aria-hidden="true">${SHAPES[status]}</svg>`;
}

export function StatusIcon(props: { status: IssueStatus; size?: number; label?: string; class?: string }) {
  return (
    <svg
      class={["status-icon", props.class]}
      data-status={props.status}
      width={props.size ?? 14}
      height={props.size ?? 14}
      viewBox="0 0 16 16"
      role={props.label ? "img" : undefined}
      aria-label={props.label}
      aria-hidden={props.label ? undefined : "true"}
      // eslint-disable-next-line solid/no-innerhtml -- fixed shapes defined above
      innerHTML={SHAPES[props.status]}
    />
  );
}
