// What this tab is connected through. Area-only state; workspace data stays in the core store.
import { createStore } from "solid-js";
import type { CloudIssue } from "@/features/plugins-cloud/cloud/errors";
import type { LinkStatus } from "./link/wasm";

export type Mode = "owner" | "paired" | "account" | "cloud";

export interface AccountTarget {
  environmentId: string;
  expectedSubject: string;
  label: string;
}

/** Why a cloud computer stopped: asleep, or a start or wake that failed. */
export type CloudStop =
  | { kind: "paused" }
  | { kind: "failed"; issue: CloudIssue; operationId: string | null };

export interface CloudStatus {
  /** The latest startup step, in the words of desktop's progress steps. */
  phase: string;
  /** When the current start or reconnect began; null once ready. */
  since: number | null;
  stop: CloudStop | null;
}

export interface ConnectionArea {
  mode: Mode;
  /** The computer's address as the user should see it; never a capability. */
  origin: string;
  link: LinkStatus | null;
  account: AccountTarget | null;
  cloud: CloudStatus;
}

export const [area, setArea] = createStore<ConnectionArea>({
  mode: "owner",
  origin: location.origin,
  link: null,
  account: null,
  cloud: { phase: "Connecting to the cloud computer", since: null, stop: null },
});

/** "Connected through Local network · 4 ms", or null for an owner socket. */
export function routeLine(): string | null {
  const link = area.link;
  if (!link?.label) return null;
  if (link.state !== "connected") return `Reconnecting · last connected through ${link.label}`;
  return link.rttMs === undefined ? `Connected through ${link.label}` : `Connected through ${link.label} · ${link.rttMs} ms`;
}
