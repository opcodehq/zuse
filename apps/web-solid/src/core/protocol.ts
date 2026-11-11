// Wire shapes transcribed from crates/protocol and crates/model. Field names are camelCase on the wire.

export type Iso = string;

export interface ChatConfig {
  harness: string;
  model?: string | null;
  reasoning?: string | null;
  modelOptions?: Record<string, unknown>;
  sandbox: string;
  plan?: boolean;
  [key: string]: unknown;
}

export interface Chat {
  id: string;
  deviceId: string;
  title?: string | null;
  archived: boolean;
  cwd?: string | null;
  branch?: string | null;
  checkoutId?: string | null;
  config?: ChatConfig | null;
  lastMessagePreview?: string | null;
  lastMessageAt?: Iso | null;
  createdAt: Iso;
  spaceId?: string | null;
  lastSeenAt?: Iso | null;
  parentChatId?: string | null;
  threadId?: string | null;
  [key: string]: unknown;
}

export interface Space {
  id: string;
  deviceId: string;
  path: string;
  name?: string | null;
  gitDetected: boolean;
  checkoutId?: string | null;
  repositoryId?: string | null;
  remoteRepository?: string | null;
  createdAt: Iso;
}

export interface Device {
  id: string;
  name: string;
  platform: string;
  lastSeenAt?: Iso | null;
  createdAt?: Iso | null;
  [key: string]: unknown;
}

export interface Repo {
  path: string;
  name: string;
  defaultBranch?: string | null;
}

/** A merged live/remote session row from WatchSessions. */
export interface Session {
  chatId: string;
  status?: "idle" | "working" | "awaitingInput" | "error" | string;
  [key: string]: unknown;
}

export interface Harness {
  id: string;
  name?: string;
  enabled?: boolean;
  installed?: boolean;
  [key: string]: unknown;
}

export type MessageRole = "user" | "assistant" | "system";
export type MessageStatus = "streaming" | "complete" | "aborted";

export interface ToolCall {
  name: string;
  input?: unknown;
  [key: string]: unknown;
}

export type MessagePart =
  | { kind: "agentContext"; id: string; agent: unknown; previousAgent?: unknown }
  | { kind: "text"; id: string; text: string }
  | { kind: "image"; id: string; path: string; name: string; mimeType: string }
  | { kind: "visual"; id: string; path: string; title: string }
  | { kind: "uiBlock"; id: string; spec: string }
  | { kind: "reasoning"; id: string; text: string }
  | {
      kind: "tool";
      id: string;
      call: ToolCall;
      isError?: boolean;
      resolved?: boolean;
      output?: string;
      diff?: unknown;
      outputRef?: string;
      outputBytes?: number;
      diffRef?: string;
      diffStats?: unknown[];
      subagentRef?: string;
      subagentStatus?: string;
      subagentTail?: string;
    }
  | { kind: "input"; id: string; requestId: string; questions: unknown[]; resolved?: boolean }
  | { kind: "error"; id: string; message: string }
  | { kind: "fork"; id: string; sourceChatId: string; sourceTitle: string };

export interface SessionMessageEntry {
  id: string;
  role: MessageRole;
  parts: MessagePart[];
  /** Epoch milliseconds. */
  createdAt: number;
  deviceId: string;
  status?: MessageStatus;
  continuationOf?: string;
  durationMs?: number;
}

export interface TextAppend {
  entry: string;
  part: string;
  text: string;
  /** UTF-8 byte length of the part's text after appending. */
  len: number;
}

export type TranscriptFrame =
  | { reset: SessionMessageEntry[] }
  | {
      upsert?: { after: string | null; entry: SessionMessageEntry }[];
      append?: TextAppend[];
      remove?: string[];
      count: number;
    };

export interface ContextUsage {
  used?: number;
  window?: number | null;
  [key: string]: unknown;
}

export type TranscriptUpdate = TranscriptFrame & { contextUsage?: ContextUsage | null };
