// Wire shapes used only by the workspace area, transcribed from crates/protocol (camelCase on the wire).

export type GitFileState =
  | "unchanged"
  | "added"
  | "modified"
  | "deleted"
  | "renamed"
  | "copied"
  | "unmerged"
  | "untracked"
  | "typeChanged";

export interface GitFileStatus {
  path: string;
  oldPath?: string | null;
  index: GitFileState;
  worktree: GitFileState;
}

export interface CheckoutGitStatus {
  checkoutId: string;
  deviceId: string;
  revision: string;
  complete: boolean;
  files: GitFileStatus[];
}

export interface GitWorkspace {
  chatId: string;
  checkoutId: string;
  cwd: string;
  branch: string;
  headSha?: string | null;
  upstream?: string | null;
  ahead: number;
  behind: number;
  files: GitFileStatus[];
  complete: boolean;
  revision: string;
  createdPullRequestUrl?: string | null;
}

export interface SourceControlFrame {
  reset?: boolean;
  snapshot?: GitWorkspace | null;
  updatedAt?: number | null;
  refreshing: boolean;
  error?: string | null;
  lastFetchAt?: number | null;
  fetchError?: string | null;
}

export type GitOperation =
  | { action: "stage"; paths: string[] }
  | { action: "unstage"; paths: string[] }
  | { action: "commit"; message: string }
  | { action: "push" }
  | { action: "pull" }
  | { action: "fetch" }
  | { action: "createBranch"; name: string }
  | { action: "switchBranch"; name: string }
  | { action: "createPullRequest"; title: string; body: string; base: string; baseRepository: string | null; draft: boolean };

export interface DiffFileSummary {
  path: string;
  oldPath?: string | null;
  status: string;
  additions: number;
  deletions: number;
  binary?: boolean;
}

export interface CheckoutDiff {
  checkoutId: string;
  deviceId: string;
  cwd: string;
  patch: string;
  files: DiffFileSummary[];
  additions: number;
  deletions: number;
  truncated: boolean;
  checksum: string;
  updatedAt: string;
}

export type WorkspaceEntryKind = "file" | "directory" | "symlink";

export interface WorkspaceEntry {
  mutationRevision?: string | null;
  path: string;
  name: string;
  kind: WorkspaceEntryKind;
  size?: number | null;
  modifiedAt?: string | null;
  ignored: boolean;
  readOnly: boolean;
}

export interface WorkspaceDirectoryPage {
  checkoutId?: string | null;
  directory: string;
  entries: WorkspaceEntry[];
  nextCursor?: string | null;
  truncated: boolean;
}

export interface WorkspaceFileSearchMatch {
  path: string;
  name: string;
  kind: WorkspaceEntryKind;
  score: number;
}

export type ReadOnlyReason =
  | "archived"
  | "binary"
  | "unsupportedEncoding"
  | "mixedLineEndings"
  | "symlink"
  | "tooLarge"
  | "permissionDenied"
  | "notRegularFile"
  | "outsideWorkspace";

export interface WorkspaceFileText {
  checkoutId: string;
  path: string;
  text?: string | null;
  contentHash?: string | null;
  size: number;
  modifiedAt?: string | null;
  encoding: "utf8" | "utf8Bom" | "binary" | "unsupported";
  lineEnding?: "lf" | "crlf" | "mixed" | "none" | null;
  readOnlyReason?: ReadOnlyReason | null;
  truncated: boolean;
}

export type WriteOutcome =
  | { status: "written"; file: { path: string; contentHash: string; size: number; modifiedAt?: string | null } }
  | { status: "conflict"; reason: "changed" | "deleted" | "replaced" | "notRegularFile"; currentContentHash?: string | null };

export interface WorkspaceFileChanges {
  sequence: number;
  resyncRequired: boolean;
  changes: { kind: "created" | "modified" | "removed" | "renamed"; path: string; oldPath?: string | null }[];
}

export interface TerminalSession {
  id: string;
  cwd: string;
  shell: string;
}

export type TerminalEvent =
  | { type: "data"; seq: number; data: string }
  | { type: "exit"; seq: number; exitCode: number; signal?: string | null };

export interface GitHistoryRef {
  kind: "branch" | "remote" | "tag";
  label: string;
}

export interface GitHistoryCommit {
  sha: string;
  parentShas: string[];
  subject: string;
  authorName: string;
  authorEmail: string;
  authoredAt: string;
  refs: GitHistoryRef[];
}

export interface GitHistoryPage {
  commits: GitHistoryCommit[];
  branchTips?: GitHistoryCommit[];
  headSha?: string | null;
  nextCursor?: number | null;
  totalCount?: number | null;
  headCommitCount?: number | null;
  comparison?: { base: string; ahead: number; behind: number } | null;
}

export type ChangeRequestState = "open" | "closed" | "merged";

export interface ChangeRequestSummary {
  provider: string;
  number: number;
  title: string;
  url: string;
  state: ChangeRequestState;
  baseRef: string;
  headRef: string;
  isDraft?: boolean;
  reviewDecision?: string;
}

export interface CheckoutChangeRequestStatus {
  error?: string | null;
  checkoutId: string;
  deviceId: string;
  cwd: string;
  branch: string;
  changeRequest: ChangeRequestSummary | null;
  updatedAt: string;
}

export interface ChangeRequestComment {
  id: string;
  url: string;
  author: { login: string };
  body: string;
  state: string;
  createdAt: string;
  submittedAt: string;
  diffHunk: string;
}

export interface ChangeRequestCheck {
  name: string;
  context: string;
  status: string;
  conclusion: string;
  state: string;
  detailsUrl: string;
  targetUrl: string;
}

export interface ChangeRequestDetail {
  title: string;
  body: string;
  url: string;
  number: number;
  author: { login: string };
  baseRefName: string;
  baseRefOid?: string;
  headRefName: string;
  headRefOid: string;
  ci?: { state: CiState; totalCount: number };
  state: string;
  isDraft: boolean;
  reviewDecision: string;
  mergeable: string;
  additions: number;
  deletions: number;
  comments: ChangeRequestComment[];
  reviews: ChangeRequestComment[];
  reviewThreads?: { id: string; isResolved: boolean; isOutdated: boolean; path: string; line?: number | null; comments: ChangeRequestComment[] }[] | null;
  files: { path: string; additions: number; deletions: number }[];
  statusCheckRollup: ChangeRequestCheck[];
  autoMergeRequest?: { mergeMethod: string } | null;
  viewerLogin?: string | null;
}

export type CiState = "noChecks" | "pending" | "passed" | "skipped" | "failed" | "unknown";

export interface ChangeRequestListItem {
  repository?: string;
  headRefOid?: string;
  ci?: { state: CiState; totalCount: number };
  number: number;
  title: string;
  url: string;
  state: ChangeRequestState;
  isDraft: boolean;
  author: { login: string };
  additions: number;
  deletions: number;
  updatedAt: string;
  reviewDecision?: string;
}

export interface ChangeRequestPage {
  items: ChangeRequestListItem[];
  nextCursor?: string | null;
  totalCount?: number | null;
}

export type ChangeRequestUpdate =
  | { kind: "merge"; method: "squash" | "merge" | "rebase" }
  | { kind: "enableAutoMerge"; method: "squash" | "merge" | "rebase" }
  | { kind: "disableAutoMerge" }
  | { kind: "markReady" }
  | { kind: "markDraft" }
  | { kind: "close" }
  | { kind: "reopen" };

export interface PreviewService {
  id: string;
  projectId: string;
  projectName: string;
  projectCwd: string;
  deviceId: string;
  deviceName: string;
  hostname: string;
  name: string;
  port: number;
  pid: number;
  cwd: string;
  startedAt: number;
  zuseOwned: boolean;
}

export interface PreviewSnapshot {
  services: PreviewService[];
  proxyPort: number;
  error?: string | null;
  projectName?: string | null;
  remote?: boolean;
}

export interface BackgroundTask {
  id: string;
  kind: "shell" | "agent" | "other";
  description: string;
  command?: string | null;
  startedAt: string;
  endedAt?: string | null;
  outcome?: string | null;
  stoppable?: boolean;
}
