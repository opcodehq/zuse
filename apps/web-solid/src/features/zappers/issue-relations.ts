import type { IssueRelationKind, Zapper, ZapperIssue } from "@/core/zapper-types";

export const relationKinds: { kind: IssueRelationKind; label: string }[] = [
  { kind: "blocks", label: "Blocking" },
  { kind: "blockedBy", label: "Blocked by" },
  { kind: "related", label: "Related" },
];

/** The engine's Zapper::issue_relations: one blocking link read from both sides, related named on either side. */
export function issueRelations(zapper: Zapper, id: string): { kind: IssueRelationKind; issue: ZapperIssue }[] {
  const issue = zapper.issues.find(i => i.id === id);
  if (!issue) return [];
  const rows: { kind: IssueRelationKind; issue: ZapperIssue }[] = [];
  for (const other of zapper.issues) {
    if (other.id === id) continue;
    const kind: IssueRelationKind | null = (issue.details.blocks ?? []).includes(other.id) ? "blocks"
      : (other.details.blocks ?? []).includes(id) ? "blockedBy"
      : issue.details.relations.includes(other.id) || other.details.relations.includes(id) ? "related"
      : null;
    if (kind) rows.push({ kind, issue: other });
  }
  const order = relationKinds.map(k => k.kind);
  return rows.sort((a, b) => order.indexOf(a.kind) - order.indexOf(b.kind) || a.issue.number - b.issue.number);
}
