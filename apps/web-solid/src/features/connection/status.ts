// What this tab is connected through. Area-only state; workspace data stays in the core store.
import { createStore } from "solid-js/store";
import type { LinkStatus } from "./link/wasm";

export type Mode = "owner" | "paired" | "account";

export interface AccountTarget {
  environmentId: string;
  expectedSubject: string;
  label: string;
}

export interface ConnectionArea {
  mode: Mode;
  /** The computer's address as the user should see it; never a capability. */
  origin: string;
  link: LinkStatus | null;
  account: AccountTarget | null;
}

export const [area, setArea] = createStore<ConnectionArea>({
  mode: "owner",
  origin: location.origin,
  link: null,
  account: null,
});

/** "Connected through Local network · 4 ms", or null for an owner socket. */
export function routeLine(): string | null {
  const link = area.link;
  if (!link?.label) return null;
  if (link.state !== "connected") return `Reconnecting · last connected through ${link.label}`;
  return link.rttMs === undefined ? `Connected through ${link.label}` : `Connected through ${link.label} · ${link.rttMs} ms`;
}
