import type { IssueStatus } from "@/core/zapper-types";
import { issueStatuses } from "@/features/zappers/IssueMetadata";

const glyphs = import.meta.glob("../../../../../crates/ui/assets/icons/issue-*.svg", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

export function issueReferencePresentation(status: string) {
  if (!Object.hasOwn(issueStatuses, status)) return undefined;
  const presentation = issueStatuses[status as IssueStatus];
  const svg = glyphs[`../../../../../crates/ui/assets/icons/${presentation.icon}.svg`];
  return svg ? { status, label: presentation.label, svg } : undefined;
}
