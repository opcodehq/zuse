// Wire shapes for the projects area, transcribed from crates/protocol (project_creation.rs, entities.rs).

export type ProjectTemplate = "empty" | "nextjs" | "turborepo";

export type ProjectSource = { kind: "clone"; url: string } | { kind: "create"; template: ProjectTemplate; publish: boolean };

export interface StartProjectCreation {
  id: string;
  name: string;
  parent: string;
  source: ProjectSource;
}

export type ProjectCreationStatus = "running" | "ready" | "failed" | "cancelled" | "interrupted";

export interface ProjectCreation {
  id: string;
  deviceId: string;
  destination: string;
  request: StartProjectCreation;
  stage: string;
  status: ProjectCreationStatus;
  percent: number | null;
  filesReady: boolean;
  error: string | null;
  warning: string | null;
  publishError: string | null;
}

export interface GithubProjectStatus {
  installed: boolean;
  authenticated: boolean;
}

export interface GithubProjectRepository {
  nameWithOwner: string;
  description: string | null;
  sshUrl: string;
  httpsUrl: string;
  private: boolean;
}

export interface GithubProjectRepositories {
  repositories: GithubProjectRepository[];
  nextPage: number | null;
}

export interface FolderEntry {
  name: string;
  isDir: boolean;
  isRepo: boolean;
}

export interface FolderListing {
  path: string;
  entries: FolderEntry[];
  truncated?: boolean;
}

export interface HomeFolderMatch {
  path: string;
  relative: string;
  isRepo: boolean;
}

export interface HomeFolderSearchResult {
  matches: HomeFolderMatch[];
  indexing: boolean;
}

export interface DriveEntry {
  name: string;
  path: string;
}

export interface WorktreeSettings {
  useCustomDirectory: boolean;
  customDirectory: string | null;
}

export interface WorktreeSettingsStatus {
  settings: WorktreeSettings;
  defaultDirectory: string;
  effectiveDirectory: string;
  environmentOverride: string | null;
}

export type ProjectActionIcon = "play" | "test" | "lint" | "configure" | "build" | "debug";

export interface ProjectAction {
  id: string;
  name: string;
  command: string;
  icon: ProjectActionIcon;
  runOnWorktreeCreate: boolean;
}

export interface ProjectActionDraft {
  name: string;
  command: string;
  icon: ProjectActionIcon;
  runOnWorktreeCreate: boolean;
}

export interface ProjectActionsSnapshot {
  spaceId: string;
  actions: ProjectAction[];
  importableActions: ProjectActionDraft[];
  projectFileIssue: string | null;
}

export interface TerminalSession {
  id: string;
  cwd: string;
  shell: string;
}

export interface ProjectActionRun {
  actionId: string;
  actionName: string;
  terminal: TerminalSession;
}

export interface FileArchiveStatus {
  chatId: string;
  phase: string | null;
  storageReclaimed?: boolean;
  changedPaths?: string[];
}

export type Loadable<T> =
  | { state: "idle" }
  | { state: "loading" }
  | { state: "ready"; value: T }
  | { state: "error"; message: string };

// Fresh objects each time: a store merges into the object it holds, so shared constants would be mutated.
export const idle = (): Loadable<never> => ({ state: "idle" });
export const loading = (): Loadable<never> => ({ state: "loading" });
export const ready = <T,>(value: T): Loadable<T> => ({ state: "ready", value });
export const failed = (message: string): Loadable<never> => ({ state: "error", message });
export const readyValue = <T,>(value: Loadable<T>): T | undefined => (value.state === "ready" ? value.value : undefined);
