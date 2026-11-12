// The footer's General-account plan and provider usage limits (crates/viewport sidebar_controls.rs).
import { createEffect, createRoot, on } from "solid-js";
import { createStore } from "solid-js/store";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import { navigate } from "@/core/view";

interface UsageWindow {
  label: string;
  usedFraction: number;
  resetsAt?: string | null;
}

export interface AgentAccount {
  id: string;
  harness: string;
  active: boolean;
  planLabel?: string | null;
  usageWindows: UsageWindow[];
  usageError?: string | null;
}

export const PROVIDERS = [
  { harness: "claude-code", name: "Claude", icon: "claude-mark" },
  { harness: "codex", name: "Codex", icon: "openai-mark" },
  { harness: "grok", name: "Grok", icon: "grok-mark" },
  { harness: "gemini", name: "Gemini", icon: "gemini-mark" },
  { harness: "kiro", name: "Kiro", icon: "kiro-mark" },
] as const;

const OFFER = "cloud-workspace-standard-v1";

interface AccountState {
  subscribed: boolean | null;
  planError: string | null;
  accountError: string | null;
  pending: { plan: boolean; signin: boolean; checkout: boolean; usage: boolean };
  accounts: AgentAccount[];
  usageLoadedAt: number;
  usageError: string | null;
}

const [account, setAccount] = createStore<AccountState>({
  subscribed: null,
  planError: null,
  accountError: null,
  pending: { plan: false, signin: false, checkout: false, usage: false },
  accounts: [],
  usageLoadedAt: 0,
  usageError: null,
});
export { account };

type Owner = { accountId: string; organizationId: string | null };

export function owner(): Owner | null {
  const value = state.account as { user?: { id?: unknown }; orgId?: unknown } | null;
  const id = value?.user?.id;
  if (typeof id !== "string") return null;
  return { accountId: id, organizationId: typeof value?.orgId === "string" ? value.orgId : null };
}

const ownerKey = () => {
  const o = owner();
  return o ? `${o.accountId}\u0000${o.organizationId ?? ""}` : "";
};

export const signedIn = () => owner() !== null;
export const signingIn = () => account.pending.signin || !!(state.account as { login?: unknown } | null)?.login;
export const available = () => state.connection.kind === "connected";

function cloudSubscribed(data: unknown, at: number): boolean | null {
  const list = (data as { entitlements?: unknown } | null)?.entitlements;
  if (!Array.isArray(list)) return null;
  return list.some((ent: { offerId?: string; status?: string; paidThrough?: number | null }) =>
    ent.offerId === OFFER && ["pending", "active", "grace"].includes(ent.status ?? "") && (typeof ent.paidThrough !== "number" || ent.paidThrough > at),
  );
}

function sameOwner(value: { accountId?: unknown; organizationId?: unknown }, expected: Owner) {
  return value.accountId === expected.accountId && (value.organizationId ?? null) === expected.organizationId;
}

function loadPlan() {
  const expected = owner();
  if (!expected || !available() || account.pending.plan) return;
  const key = ownerKey();
  setAccount("pending", "plan", true);
  rpc
    .call<{ accountId?: string; organizationId?: string | null; data?: unknown }>(M.CLOUD_BILLING_ENTITLEMENTS, {
      expectedAccountId: expected.accountId,
      expectedOrganizationId: expected.organizationId,
    })
    .then(
      (value) => {
        if (key !== ownerKey()) return;
        if (!sameOwner(value, expected)) {
          setAccount({ planError: "The account changed. Reopen Cloud settings to refresh." });
          return;
        }
        const subscribed = cloudSubscribed(value.data, Date.now());
        setAccount({ subscribed, planError: subscribed === null ? "The Cloud plan is unavailable. Open Cloud settings to retry." : null });
      },
      (error: unknown) => {
        if (key === ownerKey()) setAccount("planError", error instanceof Error ? error.message : String(error));
      },
    )
    .finally(() => setAccount("pending", "plan", false));
}

createRoot(() => {
  createEffect(
    on([ownerKey, () => state.epoch, available], () => {
      setAccount({ subscribed: null, planError: null, accountError: null, accounts: [], usageLoadedAt: 0, usageError: null });
      loadPlan();
    }),
  );
});

export type PlanLabel = "Loading account" | "Account unavailable" | "Signing in" | "Sign in" | "Account" | "Cloud" | "Upgrade" | "Checking plan";

export function planLabel(): PlanLabel {
  if (!available()) return "Account unavailable";
  if (!state.account) return "Loading account";
  if (signingIn()) return "Signing in";
  if (!signedIn()) return "Sign in";
  if (account.planError) return "Account";
  if (account.subscribed === true) return "Cloud";
  if (account.subscribed === false) return "Upgrade";
  return "Checking plan";
}

export function accountDisabled() {
  return (
    !available() ||
    !state.account ||
    signingIn() ||
    account.pending.checkout ||
    (signedIn() && account.subscribed === null && !account.planError)
  );
}

function openSecure(url: unknown) {
  if (typeof url === "string" && url.startsWith("https://")) window.open(url, "_blank", "noopener");
  else setAccount("accountError", "No secure sign-in or checkout link was returned. Open Settings to retry.");
}

export function accountAction() {
  if (accountDisabled()) return;
  setAccount("accountError", null);
  const expected = owner();
  if (!expected) {
    setAccount("pending", "signin", true);
    rpc
      .call<{ url?: string }>(M.CLOUD_ACCOUNT_SIGN_IN, {})
      .then((value) => openSecure(value?.url), (error: unknown) => setAccount("accountError", error instanceof Error ? error.message : String(error)))
      .finally(() => setAccount("pending", "signin", false));
  } else if (account.subscribed === false && !account.planError) {
    setAccount("pending", "checkout", true);
    rpc
      .call<{ accountId?: string; organizationId?: string | null; data?: { checkoutUrl?: string } }>(M.CLOUD_BILLING_CHECKOUT, {
        offerId: OFFER,
        expectedAccountId: expected.accountId,
        expectedOrganizationId: expected.organizationId,
      })
      .then(
        (value) => (sameOwner(value, expected) ? openSecure(value.data?.checkoutUrl) : setAccount("accountError", "The account changed. Reopen Cloud settings to refresh.")),
        (error: unknown) => setAccount("accountError", error instanceof Error ? error.message : String(error)),
      )
      .finally(() => setAccount("pending", "checkout", false));
  } else navigate({ kind: "settings", section: "cloud" });
}

export const providerAccount = (harness: string) => account.accounts.find((a) => a.harness === harness && a.active);

/** Refresh quickly while something is missing, otherwise at most once a minute. */
export function loadUsage(force = false) {
  if (!available() || account.pending.usage) return;
  const missing =
    !!account.usageError ||
    PROVIDERS.some((p) => {
      const a = providerAccount(p.harness);
      return !a || !a.usageWindows.length || !!a.usageError;
    });
  const stale = Date.now() - account.usageLoadedAt >= (missing ? 5_000 : 60_000);
  if (!stale && !force) return;
  const key = ownerKey();
  setAccount("pending", "usage", true);
  rpc
    .call<{ accounts?: AgentAccount[] }>(M.LIST_AGENT_ACCOUNTS, { forceUsage: force })
    .then(
      (value) => {
        if (key !== ownerKey()) return;
        if (!Array.isArray(value?.accounts)) {
          setAccount({ usageError: "Could not read provider usage. Refresh to retry.", usageLoadedAt: Date.now() });
          return;
        }
        setAccount({
          accounts: value.accounts.map((a) => ({ ...a, usageWindows: Array.isArray(a.usageWindows) ? a.usageWindows : [] })),
          usageLoadedAt: Date.now(),
          usageError: null,
        });
      },
      (error: unknown) => {
        if (key === ownerKey()) setAccount({ usageError: error instanceof Error ? error.message : String(error), usageLoadedAt: Date.now() });
      },
    )
    .finally(() => setAccount("pending", "usage", false));
}

export function resetLabel(resetsAt: string | null | undefined, at: number) {
  const when = resetsAt ? Date.parse(resetsAt) : NaN;
  if (Number.isNaN(when) || when <= at) return "Reset unavailable";
  const minutes = Math.max(0, Math.floor((when - at) / 60_000));
  if (minutes >= 1440) return `Resets in ${Math.floor(minutes / 1440)}d ${Math.floor(minutes / 60) % 24}h`;
  if (minutes >= 60) return `Resets in ${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
  return `Resets in ${minutes}m`;
}

export const percentLeft = (w: UsageWindow) => Math.round((1 - Math.min(1, Math.max(0, w.usedFraction))) * 100);
