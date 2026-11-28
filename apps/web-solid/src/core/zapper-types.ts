// Wire contracts from crates/protocol/src/zappers.rs (Zapper is the engine name).
export interface ZapperAvatar {
  mascot: string;
  color: string;
}
export interface ZapperModel {
  harness: string;
  model: string | null;
}
export interface ZapperRepository {
  deviceId: string;
  spaceId: string;
}
export interface ZapperConfig {
  name: string;
  purpose: string;
  homeDeviceId: string;
  repositories: ZapperRepository[];
  coordinatorModel: ZapperModel;
  workerDeviceId: string;
  maxConcurrentWorkers: number;
  maxFollowups: number;
  avatar?: ZapperAvatar;
  avatarImage?: string;
}
export type ZapperStatus = "active" | "paused" | "stopped" | "archived";
export interface ZapperWorker {
  chatId: string;
  deviceId: string;
  spaceId: string;
  objective: string;
  followups: number;
  queuePosition?: number;
}
export interface ZapperDecision {
  id: string;
  workerChatId: string | null;
  question: string;
  answer: string | null;
}
export type IssueStatus =
  | "backlog"
  | "todo"
  | "inProgress"
  | "needsYou"
  | "blocked"
  | "inReview"
  | "done"
  | "canceled";
export type IssuePriority = "urgent" | "high" | "normal" | "low";
export type IssueOwner = { type: "zapper" } | { type: "user" } | { type: "thread"; chatId: string };
export interface IssueDetails {
  title: string;
  description: string;
  status: IssueStatus;
  priority: IssuePriority;
  assigned: boolean;
  dueDate: string | null;
  labels: string[];
  relations: string[];
  owner?: IssueOwner;
  doneCriteria?: string;
  outOfScope?: string;
  parent?: string;
}
export type IssueDeliveryState = "queued" | "retrying" | "sent" | "answered" | "cancelled" | "held";
/** Where a person's comment went. "sent" is queue acceptance, not proof it was handled. */
export interface IssueDelivery {
  chatId: string;
  state: IssueDeliveryState;
  detail?: string;
  reply?: string;
}
/** One typed before/after value; plain text, never markup. */
export interface IssueChange {
  field: string;
  before?: string;
  after?: string;
}
export interface IssueActivity {
  id: string;
  actor?: string | null;
  createdAt: number;
  kind: string;
  text: string;
  replyTo?: string | null;
  delivery?: IssueDelivery;
  thread?: string;
  changes?: IssueChange[];
}
export interface ZapperIssue {
  id: string;
  number: number;
  details: IssueDetails;
  createdAt: number;
  updatedAt: number;
  threads: string[];
  pullRequests: { url: string; title: string }[];
  activity: IssueActivity[];
}
export interface Zapper {
  id: string;
  revision: number;
  activityRevision: number;
  config: ZapperConfig;
  status: ZapperStatus;
  coordinatorChatId: string;
  previousCoordinatorChats: string[];
  workers: ZapperWorker[];
  decisions: ZapperDecision[];
  issues: ZapperIssue[];
  /** Created folders, empty ones included; engines before folders omit it. */
  folders?: string[];
}
export interface ZapperFile {
  path: string;
  version: number;
  content: string;
  /** This version deleted the file; only histories list deletions. */
  deleted?: boolean;
}
export interface ZapperFileVersion {
  path: string;
  version: number;
}
export interface ZapperEvent {
  id: string;
  zapperId: string;
  workerChatId: string;
  kind: string;
  summary: string;
  acknowledged: boolean;
}
export type ZapperAction =
  | { type: "create" | "configure"; config: ZapperConfig }
  | { type: "setStatus"; status: ZapperStatus }
  | { type: "adoptWorker"; worker: ZapperWorker }
  | {
      type: "delegate";
      issueId?: string;
      worker: ZapperWorker;
      model: ZapperModel;
      base: string;
    }
  | { type: "messageWorker"; chatId: string; text: string; automatic: boolean }
  | { type: "stopWorker" | "archiveWorker" | "releaseWorker"; chatId: string }
  | { type: "decide"; decision: ZapperDecision }
  | {
      type: "writeFile";
      path: string;
      content: string;
      expectedVersion: number;
    }
  | {
      type: "restoreFile";
      path: string;
      version: number;
      expectedVersion: number;
    }
  | { type: "deleteFile"; path: string; expectedVersion: number }
  | { type: "moveFile"; path: string; to: string; expectedVersion: number }
  | { type: "createFolder"; path: string }
  | { type: "moveFolder"; path: string; to: string; files: ZapperFileVersion[] }
  | { type: "deleteFolder"; path: string; files: ZapperFileVersion[] }
  | {
      type: "createIssue" | "editIssue";
      issueId: string;
      details: IssueDetails;
    }
  | {
      type: "linkIssueThread" | "unlinkIssueThread";
      issueId: string;
      chatId: string;
    }
  | {
      type: "linkIssuePullRequest";
      issueId: string;
      url: string;
      title: string;
    }
  | { type: "unlinkIssuePullRequest"; issueId: string; url: string }
  | {
      type: "commentIssue";
      issueId: string;
      text: string;
      replyTo?: string | null;
    }
  | { type: "askUser"; issueId: string; question: string; recommendation?: string }
  | { type: "closeIssue"; issueId: string; outcome: "merged" | "dropped"; note: string }
  | { type: "freshConversation" | "delete" };
export interface ZapperCommand {
  commandId: string;
  zapperId: string;
  expectedRevision: number | null;
  action: ZapperAction;
}
