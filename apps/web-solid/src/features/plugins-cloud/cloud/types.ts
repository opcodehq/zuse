// Cloud wire shapes from crates/protocol (cloud_billing, cloud_projects, cloud) and crates/model/src/cloud.rs.
import type { SessionMessageEntry } from "@/core/protocol";

export type BillingStatus = "active" | "grace" | "billing-hold" | "ended" | "manual";
export interface BillingSummary {
  currency: string;
  status: BillingStatus;
  periodStart: number;
  periodEnd: number;
  basePriceMicros: number;
  includedProviderCostMicros: number;
  providerCostMicros: number;
  storageCostMicros: number;
  includedUsedMicros: number;
  includedRemainingMicros: number;
  overageProviderCostMicros: number;
  overageChargeMicros: number;
  overageCapMicros: number;
  markupBasisPoints: number;
  currentInvoiceEstimateMicros: number;
  usageProvisional: boolean;
}
export interface BillingUsageItem {
  entryId: string;
  resourceKind: string;
  usageKind?: string | null;
  resourceId: string;
  provider: string;
  startedAt: number;
  endedAt: number;
  vcpuCount: number;
  memoryMib: number;
  providerCostMicros: number;
  status: string;
}
export interface PrepaidBalance {
  available: boolean;
  creditCents: number;
  debitCents: number;
  currency: string;
}

export interface ProjectBuildStatus {
  buildId: string;
  providerId: string;
  state: string;
  errorCode?: string | null;
  createdAt: number;
  updatedAt: number;
}
export interface CloudProject {
  projectId: string;
  repositoryIdentity: string;
  repositoryUrl: string;
  displayName: string;
  defaultBranch: string;
  visibility: "public" | "private";
  state: string;
  createdAt: number;
  updatedAt: number;
}
export interface GithubRepository {
  nameWithOwner: string;
  description?: string | null;
  sshUrl: string;
  httpsUrl: string;
  isPrivate: boolean;
  defaultBranch: string;
  updatedAt: string;
  ownerAvatarUrl?: string | null;
}
export interface GithubInstallation {
  installationId: number;
  accountLogin: string;
  accountType: string;
  avatarUrl?: string | null;
  repositorySelection: string;
  suspended: boolean;
}
export interface GithubCatalog {
  configured: boolean;
  canManageInstallations?: boolean | null;
  user?: { login: string; name: string; email: string } | null;
  installations: GithubInstallation[];
  repositories: GithubRepository[];
}

export interface ImageRepository {
  projectId: string;
  repositoryIdentity: string;
  displayName: string;
  defaultBranch: string;
  sourceCommit?: string | null;
}
export interface ImageBuild {
  buildId: string;
  state: string;
  mode: "update" | "rebuild";
  active: boolean;
  progressPhase?: string | null;
  errorCode?: string | null;
  logText?: string | null;
  runtimeVersion: string;
  configurationDigest: string;
  repositories: ImageRepository[];
  createdAt: number;
  updatedAt: number;
}
export interface ImageStorage {
  snapshotId: string;
  state: string;
  monthlyCostMicros: number;
  billingEnabled: boolean;
  lastError?: string | null;
}
export interface AccountImage {
  storage?: ImageStorage | null;
  state: string;
  generation?: string | null;
  providerId?: string | null;
  runtimeVersion?: string | null;
  progressPhase?: string | null;
  errorCode?: string | null;
  repositories: ImageRepository[];
  builds: ImageBuild[];
  builtAt?: number | null;
  updatedAt: number;
}

export type ArchivePhase = "uploading" | "verifying" | "retiring" | "restoring" | "retained" | "retired" | "failed" | string;
export interface ArchiveStatus {
  workspaceId: string;
  archived: boolean;
  machineRetained: boolean;
  backupVerified: boolean;
  computeRetired: boolean;
  retentionDeadline?: number | null;
  phase?: ArchivePhase | null;
  verifiedChunks: number;
  totalChunks: number;
  errorCode?: string | null;
}
export interface Workspace {
  workspaceId: string;
  chatId: string;
  initialSessionId: string;
  branch: string;
  state: string;
  desiredState: string;
  statusCode: string;
  revision: number;
  title?: string | null;
  rpcProtocol?: string | null;
  localMirrorCleanup?: { state: string; removed: number; preserved: number; error?: string | null } | null;
  archiveStatus?: ArchiveStatus | null;
  projectId?: string;
}

export interface HeadChat {
  id: string;
  title?: string | null;
  [key: string]: unknown;
}
export interface Head {
  workspaceId: string;
  sessionId: string;
  chat: HeadChat;
  conversations: HeadChat[];
  transcript: { reset?: SessionMessageEntry[] };
  queue: unknown;
  queuePaused: boolean;
}

export interface CloudModel {
  id: string;
  name?: string;
  label?: string;
  [key: string]: unknown;
}
export interface CloudAgent {
  id: string;
  name: string;
  defaultModel: string;
  models: CloudModel[];
}
export interface CloudContextProject {
  projectId: string;
  providerId: string;
  name?: string;
  displayName?: string;
  defaultBranch?: string;
  supportsModelConfiguration?: boolean;
  prepared?: boolean;
  [key: string]: unknown;
}
