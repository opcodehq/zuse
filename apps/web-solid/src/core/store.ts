// The one client-side store. The engine owns the data; this is its normalized projection.
import { createStore, reconcile } from "solid-js/store";
import type { Chat, ContextUsage, Device, Harness, Repo, Session, SessionMessageEntry, Space } from "./protocol";

export type ConnectionStatus =
  | { kind: "connecting" }
  | { kind: "connected" }
  | { kind: "reconnecting"; attempt: number; message?: string }
  | { kind: "authRequired"; message?: string }
  | { kind: "pairingRequired"; message?: string }
  | { kind: "accountRequired"; message?: string };

export interface Transcript {
  chatId: string;
  ready: boolean;
  messages: SessionMessageEntry[];
  contextUsage: ContextUsage | null;
  notice: string | null;
  /** The connection epoch of the last reset frame; matches `state.epoch` once this connection's history arrived. */
  syncedEpoch?: number;
}

export interface AppState {
  connection: ConnectionStatus;
  /** Bumps on every new connection, so views can drop work from an earlier one. */
  epoch: number;
  localDeviceId: string | null;
  account: Record<string, unknown> | null;
  auth: unknown;
  chats: Record<string, Chat>;
  sessions: Record<string, Session>;
  spaces: Record<string, Space>;
  devices: Record<string, Device>;
  repos: Repo[];
  harnesses: Harness[];
  workspaceSynced: boolean;
  transcript: Transcript | null;
  queue: Record<string, unknown[]>;
  /** Set when a connection dropped while a mutation was in flight; cleared when the user dismisses it. */
  uncertainMutation: boolean;
}

export const [state, setState] = createStore<AppState>({
  connection: { kind: "connecting" },
  epoch: 0,
  localDeviceId: null,
  account: null,
  auth: null,
  chats: {},
  sessions: {},
  spaces: {},
  devices: {},
  repos: [],
  harnesses: [],
  workspaceSynced: false,
  transcript: null,
  queue: {},
  uncertainMutation: false,
});

const byId = <T,>(rows: T[], key: (row: T) => string) => Object.fromEntries(rows.map((row) => [key(row), row]));

/** Replace one keyed collection from a full snapshot, keeping unchanged rows referentially stable. */
export function applySnapshot(domain: "chats" | "sessions" | "spaces" | "devices", rows: unknown) {
  const list = Array.isArray(rows) ? rows : [];
  const key = domain === "sessions" ? (row: Session) => row.chatId : (row: { id: string }) => row.id;
  setState(domain, reconcile(byId(list as never[], key as (row: never) => string), { merge: true }));
}

export const chatList = () => Object.values(state.chats);
export const sessionFor = (chatId: string) => state.sessions[chatId];
