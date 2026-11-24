import { createMemo, createSignal, For, Show } from "solid-js";
import { Icon } from "@/ui/Icon";
import type { IssueRelationKind, Zapper, ZapperIssue } from "@/core/zapper-types";
import { IssueStatusIcon, issueStatuses } from "./IssueMetadata";
import { issueRelations, relationKinds } from "./issue-relations";
import { act } from "./state";

/** The detail view's Relations field: grouped Blocking / Blocked by / Related rows, removable, with search to add one. */
export function IssueRelations(props: { zapper: Zapper; issue: ZapperIssue; onOpen?: (id: string) => void }) {
  const [adding, setAdding] = createSignal(false);
  const [kind, setKind] = createSignal<IssueRelationKind>("blocks");
  const [query, setQuery] = createSignal("");
  const rows = createMemo(() => issueRelations(props.zapper, props.issue.id));
  const candidates = createMemo(() => {
    const q = query().trim().toLowerCase();
    return props.zapper.issues.filter(other =>
      other.id !== props.issue.id
      && !rows().some(row => row.issue.id === other.id && row.kind === kind())
      && (!q || `ZAP-${other.number} ${other.details.title}`.toLowerCase().includes(q)),
    ).slice(0, 8);
  });
  const add = (target: string) => {
    act(props.zapper.id, { type: "linkIssueRelation", issueId: props.issue.id, kind: kind(), targetId: target });
    setAdding(false);
    setQuery("");
  };
  return (
    <section class="zp-relations" aria-label="Relations">
      <h3>Relations</h3>
      <For each={relationKinds.filter(group => rows().some(row => row.kind === group.kind))}>{group => (
        <div class="zp-relation-group">
          <span class="zp-relation-label">{group.label}</span>
          <div class="zp-relation-rows">
            <For each={rows().filter(row => row.kind === group.kind)}>{row => (
              <div class="zp-relation">
                <button type="button" class="zp-relation-link" title={issueStatuses[row.issue.details.status].label} onClick={() => props.onOpen?.(row.issue.id)}>
                  <IssueStatusIcon status={row.issue.details.status} />
                  <span class="zp-relation-number">ZAP-{row.issue.number}</span>
                  <span class="zp-relation-title">{row.issue.details.title}</span>
                </button>
                <button type="button" class="zp-relation-remove" aria-label={`Remove relation to ZAP-${row.issue.number}`} onClick={() => act(props.zapper.id, { type: "unlinkIssueRelation", issueId: props.issue.id, targetId: row.issue.id })}>
                  <Icon name="close" size={12} />
                </button>
              </div>
            )}</For>
          </div>
        </div>
      )}</For>
      <Show when={adding()} fallback={<button type="button" class="zp-relation-add" onClick={() => setAdding(true)}><Icon name="plus" size={12} />Add relation</button>}>
        <div class="zp-relation-picker" onKeyDown={event => { if (event.key === "Escape") { setAdding(false); setQuery(""); } }}>
          <div class="zp-relation-kinds" role="radiogroup" aria-label="Relation type">
            <For each={relationKinds}>{option => (
              <button type="button" role="radio" aria-checked={kind() === option.kind} data-active={kind() === option.kind || undefined} onClick={() => setKind(option.kind)}>{option.label}</button>
            )}</For>
          </div>
          <label class="zp-search"><Icon name="palette-search" size={13} /><input aria-label="Search issues to relate" placeholder="Search issues…" value={query()} onInput={event => setQuery(event.currentTarget.value)} autofocus /></label>
          <div class="zp-relation-candidates">
            <For each={candidates()} fallback={<span class="zp-muted">No matching issues</span>}>{other => (
              <button type="button" class="zp-relation-link" onClick={() => add(other.id)}>
                <IssueStatusIcon status={other.details.status} />
                <span class="zp-relation-number">ZAP-{other.number}</span>
                <span class="zp-relation-title">{other.details.title}</span>
              </button>
            )}</For>
          </div>
        </div>
      </Show>
    </section>
  );
}
