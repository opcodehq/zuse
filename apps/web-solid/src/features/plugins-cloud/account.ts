// The Zuse account behind plugins and Cloud. The engine streams its status into state.account.
import { createStore } from "solid-js/store";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { RpcError } from "@/core/rpc";
import { state } from "@/core/store";

export interface AccountUser {
  id: string;
  email: string;
  name?: string | null;
}
export interface LoginProgress {
  url: string;
  userCode?: string;
  expiresAt: number;
}
export interface AccountStatus {
  user?: AccountUser | null;
  login?: LoginProgress | null;
  error?: string | null;
  orgId?: string | null;
}

/** Account and organization every account-bound request must name. */
export interface Owner {
  accountId: string;
  organizationId: string | null;
}

export const account = (): AccountStatus | null => state.account as AccountStatus | null;
export const accountLoaded = () => state.account !== null;
export const user = () => account()?.user ?? null;
export const signedIn = () => !!user()?.id;
export const login = () => {
  const progress = account()?.login;
  return progress && progress.url.startsWith("https://") ? progress : null;
};

export function owner(): Owner | null {
  const id = user()?.id;
  if (!id) return null;
  return { accountId: id, organizationId: account()?.orgId ?? null };
}
export const ownerKey = (value: Owner | null) => (value ? `${value.accountId}\u0000${value.organizationId ?? ""}` : "");

/** Adds the expected identity so the engine rejects requests that outlive an account switch. */
export function bound(value: Owner, params: Record<string, unknown> = {}) {
  return { ...params, expectedAccountId: value.accountId, expectedOrganizationId: value.organizationId };
}

/** Replies are only trusted when they name the account that asked. */
export function boundReply(reply: unknown, value: Owner): reply is { accountId: string; organizationId?: string | null; data?: unknown } {
  const row = reply as { accountId?: unknown; organizationId?: unknown } | null;
  return !!row && row.accountId === value.accountId && (row.organizationId ?? null) === value.organizationId;
}

const [session, setSession] = createStore({ busy: false, error: null as string | null });
export { session as accountSession };

export function errorText(error: unknown, fallback = "The request failed. Try again."): string {
  if (error instanceof RpcError) return error.message;
  if (error instanceof Error && error.message) return error.message;
  return fallback;
}

/** Remote browsers confirm a device code; the engine never follows a loopback redirect here. */
export async function signIn() {
  if (session.busy || signedIn()) return;
  const pending = login();
  if (pending) {
    window.open(pending.url, "_blank", "noopener");
    return;
  }
  setSession({ busy: true, error: null });
  try {
    const progress = await rpc.call<LoginProgress>(M.CLOUD_ACCOUNT_SIGN_IN, {});
    if (progress?.url?.startsWith("https://")) window.open(progress.url, "_blank", "noopener");
  } catch (error) {
    setSession("error", errorText(error, "Could not start sign-in. Try again."));
  } finally {
    setSession("busy", false);
  }
}

export async function signOut() {
  if (session.busy) return;
  setSession({ busy: true, error: null });
  try {
    await rpc.call(M.CLOUD_ACCOUNT_SIGN_OUT, {});
  } catch (error) {
    setSession("error", errorText(error, "Could not sign out. Try again."));
  } finally {
    setSession("busy", false);
  }
}

export const accountError = () => session.error ?? account()?.error ?? null;

// randomUUID is missing on plain-HTTP LAN origins; getRandomValues is not.
export function newId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6]! & 0x0f) | 0x40;
  bytes[8] = (bytes[8]! & 0x3f) | 0x80;
  const hex = [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export function openExternal(url: string) {
  if (url.startsWith("https://")) window.open(url, "_blank", "noopener");
}
