// Agent account sign-in and account actions (crates/ui/src/settings/accounts.rs): one flow at a time,
// started, then waiting on the browser (or a pasted code) until it lands or fails.
import { onConnected, refreshHarnesses, rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { toast } from "@/ui/primitives";
import { createStore, reconcile, snapshot as snapshotStore } from "solid-js";
import { type AccountsSnapshot, type AgentAccount, data, loadAccounts, message, setData, uncertain } from "./data";
import { isOpenableLoginUrl, loginOptions, providerName } from "./harness";

export type LoginStep =
  | { kind: "browser"; message: string | null }
  | { kind: "paste"; submitting: boolean; error: string | null }
  | { kind: "failed"; message: string };

export interface LoginFlow {
  harness: string;
  provider?: string;
  attempt: number;
  loginId: string | null;
  url: string | null;
  callbackPort: number | null;
  step: LoginStep;
}

interface LoginStart {
  loginId: string;
  url: string;
  mode: "paste-code" | "browser";
  callbackPort?: number | null;
}
interface LoginPoll {
  status: "pending" | "done" | "error";
  message?: string | null;
  url?: string | null;
  callbackPort?: number | null;
}

interface LoginState {
  flow: LoginFlow | null;
  /** Account id with an in-flight switch or remove. */
  busyAccount: string | null;
  actionError: string | null;
}

export const [login, setLogin] = createStore<LoginState>({ flow: null, busyAccount: null, actionError: null });

let attempts = 0;
let pollTimer: ReturnType<typeof setTimeout> | undefined;

onConnected(() => {
  clearTimeout(pollTimer);
  setLogin(draft => { Object.assign(draft, { flow: null, busyAccount: null, actionError: null }); });
});

export function loginTitle(flow: LoginFlow) {
  const option = loginOptions(flow.harness).find((o) => o.provider === flow.provider);
  return option?.provider ? `Sign in to ${option.label} for ${providerName(flow.harness)}` : `Sign in to ${providerName(flow.harness)}`;
}

export function loginStatus(flow: LoginFlow) {
  if (flow.step.kind === "browser" && flow.step.message) return flow.step.message;
  if (flow.step.kind === "browser" && flow.url) return "Waiting for you to finish in the browser…";
  return "Starting sign-in…";
}

export const loginActive = () => !!login.flow && login.flow.step.kind !== "failed";

const current = (attempt: number) => login.flow?.attempt === attempt;

function openUrl(url: string, pending?: Window | null) {
  if (!isOpenableLoginUrl(url)) {
    pending?.close();
    return;
  }
  if (pending && !pending.closed) {
    pending.opener = null;
    pending.location.href = url;
  } else window.open(url, "_blank", "noopener");
}

export async function startLogin(harness: string, provider?: string) {
  if (loginActive() || login.busyAccount) return;
  clearTimeout(pollTimer);
  const attempt = ++attempts;
  setLogin(draft => { draft["flow"] = { harness, provider, attempt, loginId: null, url: null, callbackPort: null, step: { kind: "browser", message: null } }; });
  // Opened inside the click so pop-up blockers allow it; pointed at the sign-in page once known.
  const pending = window.open("about:blank", "_blank");
  let start: LoginStart;
  try {
    start = await rpc.call<LoginStart>(M.START_AGENT_LOGIN, provider ? { harness, provider } : { harness });
  } catch (error) {
    pending?.close();
    if (current(attempt)) setLogin(draft => { draft["flow"]!["step"] = { kind: "failed", message: `Couldn't start the sign-in: ${message(error)}` }; });
    return;
  }
  if (!current(attempt)) {
    pending?.close();
    void rpc.call(M.CANCEL_AGENT_LOGIN, { loginId: start.loginId }).catch(() => {});
    return;
  }
  const url = start.url || null;
  if (url) openUrl(url, pending);
  else if (start.mode === "paste-code") pending?.close();
  setLogin(draftState => {
    const flow = draftState["flow"]!;
    if (!flow) return;
    flow.loginId = start.loginId;
    flow.url = url && isOpenableLoginUrl(url) ? url : null;
    flow.callbackPort = start.callbackPort ?? null;
    flow.step = start.mode === "paste-code" ? { kind: "paste", submitting: false, error: null } : { kind: "browser", message: null };
  });
  // A page that only becomes known in a later poll keeps the blank window for it.
  if (start.mode === "browser") schedulePoll(attempt, start.loginId, url ? null : pending);
}

function schedulePoll(attempt: number, loginId: string, pending: Window | null) {
  pollTimer = setTimeout(async () => {
    if (!current(attempt) || login.flow?.step.kind !== "browser") return pending?.close();
    let poll: LoginPoll;
    try {
      poll = await rpc.call<LoginPoll>(M.POLL_AGENT_LOGIN, { loginId });
    } catch (error) {
      pending?.close();
      if (current(attempt)) setLogin(draft => { draft["flow"]!["step"] = { kind: "failed", message: `Lost track of the sign-in: ${message(error)}` }; });
      return;
    }
    if (!current(attempt)) return pending?.close();
    if (poll.url && poll.url !== login.flow?.url && isOpenableLoginUrl(poll.url)) {
      openUrl(poll.url, pending);
      pending = null;
      setLogin(draft => { Object.assign(draft["flow"]!, { url: poll.url, callbackPort: poll.callbackPort ?? login.flow?.callbackPort ?? null }); });
    }
    if (poll.status === "done") {
      pending?.close();
      finish();
      return;
    }
    if (poll.status === "error") {
      pending?.close();
      setLogin(draft => { draft["flow"]!["step"] = { kind: "failed", message: poll.message || "The sign-in failed." }; });
      return;
    }
    setLogin(draft => { draft["flow"]!["step"] = { kind: "browser", message: poll.message ?? null }; });
    schedulePoll(attempt, loginId, pending);
  }, 1500);
}

function finish() {
  const name = login.flow ? providerName(login.flow.harness) : "";
  setLogin(draft => { draft["flow"] = null; });
  toast(`Signed in to ${name}`);
  void loadAccounts(true);
  void refreshHarnesses();
}

export async function submitCode(code: string) {
  const flow = login.flow;
  if (!flow?.loginId || flow.step.kind !== "paste" || flow.step.submitting || !code.trim()) return;
  const attempt = flow.attempt;
  setLogin(draft => { draft["flow"]!["step"] = { kind: "paste", submitting: true, error: null }; });
  try {
    await rpc.call(M.COMPLETE_AGENT_LOGIN, { loginId: flow.loginId, code: code.trim() });
    if (current(attempt)) finish();
  } catch (error) {
    if (current(attempt)) setLogin(draft => { draft["flow"]!["step"] = { kind: "paste", submitting: false, error: message(error) }; });
  }
}

export function cancelLogin() {
  const flow = login.flow;
  if (!flow) return;
  clearTimeout(pollTimer);
  setLogin(draft => { draft["flow"] = null; });
  if (flow.loginId) void rpc.call(M.CANCEL_AGENT_LOGIN, { loginId: flow.loginId }).catch(() => {});
  void loadAccounts(false);
}

export function retryLogin() {
  const flow = login.flow;
  if (!flow) return;
  setLogin(draft => { draft["flow"] = null; });
  void startLogin(flow.harness, flow.provider);
}

export function reopenLoginPage() {
  const url = login.flow?.url;
  if (url) openUrl(url);
}

function isSnapshot(value: unknown): value is AccountsSnapshot {
  return !!value && typeof value === "object" && Array.isArray((value as AccountsSnapshot).accounts);
}

/** The optimistic half of a switch: the account becomes the live login of its group. */
function markSwitched(account: AgentAccount) {
  setData(draftState => {
    const snapshot = draftState["accounts"]!;
    for (const row of snapshot?.accounts ?? [])
      if (row.harness === account.harness && (row.provider ?? null) === (account.provider ?? null)) row.active = row.id === account.id;
  });
}

export async function accountAction(account: AgentAccount, action: "switch" | "remove") {
  if (login.busyAccount || loginActive() || !account.switchable) return;
  // Store proxies can't be cloned; copy the plain snapshot underneath.
  const previous = data.accounts ? structuredClone(snapshotStore(data.accounts)) : null;
  setLogin(draft => { Object.assign(draft, { busyAccount: account.id, actionError: null }); });
  if (action === "switch") markSwitched(account);
  try {
    const reply = await rpc.call(action === "switch" ? M.ACTIVATE_AGENT_ACCOUNT : M.FORGET_AGENT_ACCOUNT, {
      id: account.id,
      accountId: account.id,
      harness: account.harness,
    });
    if (isSnapshot(reply)) setData(draft => { { const next = reply; if (draft["accounts"] == null) draft["accounts"] = next; else reconcile(next, "id")(draft["accounts"]); } });
    else void loadAccounts(false);
  } catch (error) {
    if (previous) setData(draft => { { const next = previous; if (draft["accounts"] == null) draft["accounts"] = next; else reconcile(next, "id")(draft["accounts"]); } });
    setLogin(draft => { draft["actionError"] = uncertain(error) ? "The connection dropped before the engine answered. Refresh accounts before trying again." : message(error); });
  } finally {
    setLogin(draft => { draft["busyAccount"] = null; });
  }
}
