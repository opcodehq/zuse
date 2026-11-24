import { For } from "solid-js";
import { Icon } from "@/ui/Icon";
import type { IssuePriority, IssueStatus } from "@/core/zapper-types";
import "./issue-metadata.css";

export const issueStatuses: Record<IssueStatus, { label: string; icon: string }> = {
  backlog: { label: "Backlog", icon: "issue-backlog" },
  todo: { label: "Todo", icon: "issue-todo" },
  inProgress: { label: "In progress", icon: "issue-progress" },
  needsYou: { label: "Needs you", icon: "issue-attention" },
  blocked: { label: "Blocked", icon: "issue-blocked" },
  inReview: { label: "In review", icon: "issue-todo" },
  done: { label: "Done", icon: "issue-done" },
  canceled: { label: "Canceled", icon: "issue-canceled" },
};
export function IssueStatusIcon(props: { status: IssueStatus }) {
  return <Icon class="issue-state-icon" data-status={props.status} name={issueStatuses[props.status].icon} size={14} />;
}
export function IssuePriorityIcon(props: { priority: IssuePriority }) {
  return (
    <span class="issue-priority-icon" data-priority={props.priority} aria-hidden="true">
      {props.priority === "urgent" ? <Icon name="priority-urgent" size={14} /> : (
        <span class="issue-priority-bars">
          <For each={[1, 2, 3]}>{bar => (
            <i data-active={bar <= (props.priority === "low" ? 1 : props.priority === "normal" ? 2 : 3) ? "true" : undefined} />
          )}</For>
        </span>
      )}
    </span>
  );
}
export function IssueWorking(props: { working: boolean }) {
  return (
    <span class="issue-working" data-working={props.working || undefined}
      role={props.working ? "img" : undefined}
      aria-label={props.working ? "Assigned agent is working" : undefined}
      aria-hidden={props.working ? undefined : "true"}>
      <For each={[0, 1, 2, 3, 4, 5, 6, 7, 8]}>{cell => <i style={{ "animation-delay": `${cell * -90}ms` }} />}</For>
    </span>
  );
}
