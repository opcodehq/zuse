import { createEffect, createRoot } from "solid-js";
import { createStore, reconcile } from "solid-js";
import { onConnected, rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { bound, boundReply, errorText, newId, openExternal, owner, ownerKey, type Owner } from "../account";
import { capDollars, capMicros, cloudSubscribed } from "./money";
import { cloudErrorText, defaultProvider, readCatalog } from "./plans";
import type {
  AccountImage,
  BillingSummary,
  BillingUsageItem,
  CloudAgent,
  CloudCapabilities,
  CloudContextProject,
  CloudProject,
  GithubCatalog,
  GithubRepository,
  Head,
  PlanCatalog,
  PrepaidBalance,
  Workspace,
} from "./types";

interface Billing {
  loaded: boolean;
  loading: boolean;
  subscribed: boolean | null;
  subscriptionLoading: boolean;
  /** Plans for sale; the static plans until entitlements load. */
  catalog: PlanCatalog | null;
  /** Offer whose checkout is opening. */
  checkoutBusy: string | null;
  summary: BillingSummary | null;
  usage: BillingUsageItem[];
  cursor: string | null;
  error: string | null;
  notice: string | null;
  portalBusy: boolean;
  credits: PrepaidBalance | null;
  creditsBusy: boolean;
  creditAmount: number;
  creditError: string | null;
  capInput: string;
  capBusy: boolean;
}
interface Repositories {
  loaded: boolean;
  projects: CloudProject[];
  github: GithubCatalog | null;
  listing: boolean;
  githubLoading: boolean;
  connecting: string[];
  removing: string | null;
  error: string | null;
  notice: string | null;
}
interface Image {
  value: AccountImage | null;
  loading: boolean;
  building: boolean;
  deleting: boolean;
  error: string | null;
}
interface Workspaces {
  loaded: boolean;
  loading: boolean;
  rows: Workspace[];
  error: string | null;
  /** Workspace id → the action in flight. Settled entries are removed by setting them undefined. */
  pending: Record<string, WorkspaceAction | undefined>;
  rowError: { id: string; message: string } | null;
}
export interface Launch {
  key: string;
  text: string;
  repository: string;
  state: "sending" | "accepted" | "failed" | "uncertain";
  message: string | null;
  workspaceId: string | null;
}
interface Context {
  loaded: boolean;
  loading: boolean;
  projects: CloudContextProject[];
  agents: CloudAgent[];
  capabilities: CloudCapabilities | null;
  error: string | null;
}
interface History {
  workspaceId: string | null;
  sessionId: string | null;
  head: Head | null;
  loading: boolean;
  error: string | null;
}

const initial = () => ({
  billing: {
    loaded: false,
    loading: false,
    subscribed: null,
    subscriptionLoading: false,
    catalog: null,
    checkoutBusy: null,
    summary: null,
    usage: [],
    cursor: null,
    error: null,
    notice: null,
    portalBusy: false,
    credits: null,
    creditsBusy: false,
    creditAmount: 2500,
    creditError: null,
    capInput: "",
    capBusy: false,
  } as Billing,
  repos: { loaded: false, projects: [], github: null, listing: false, githubLoading: false, connecting: [], removing: null, error: null, notice: null } as Repositories,
  image: { value: null, loading: false, building: false, deleting: false, error: null } as Image,
  workspaces: { loaded: false, loading: false, rows: [], error: null, pending: {}, rowError: null } as Workspaces,
  context: { loaded: false, loading: false, projects: [], agents: [], capabilities: null, error: null } as Context,
  launches: [] as Launch[],
  history: { workspaceId: null, sessionId: null, head: null, loading: false, error: null } as History,
});

const [cloud, setCloud] = createStore(initial());
export { cloud };

let generation = 0;
// A refresh must not overwrite a cap the user is still editing.
let capEdited = false;
const timers = new Set<ReturnType<typeof setTimeout>>();
const later = (ms: number, run: () => void) => {
  const timer = setTimeout(() => {
    timers.delete(timer);
    run();
  }, ms);
  timers.add(timer);
};

function reset() {
  generation++;
  capEdited = false;
  for (const timer of timers) clearTimeout(timer);
  timers.clear();
  setCloud(draft => { reconcile(initial())(draft); });
}

createRoot(() => {
  createEffect(() => ownerKey(owner()), (key, previous) => { if (key !== previous) reset(); });
});
onConnected(() => {
  // A dropped connection fails in-flight work; clear busy flags so the user can check and retry.
  generation++;
  setCloud(draft => {
    (() => { Object.assign(draft["billing"], { loading: false, subscriptionLoading: false, checkoutBusy: null, portalBusy: false, creditsBusy: false, capBusy: false }); })();
    (() => { Object.assign(draft["repos"], { listing: false, githubLoading: false, connecting: [], removing: null }); })();
    (() => { Object.assign(draft["image"], { loading: false, building: false, deleting: false }); })();
    (() => { Object.assign(draft["workspaces"], { loading: false, pending: {} }); })();
    (() => { draft["context"]["loading"] = false; })();
    (() => { draft["history"]["loading"] = false; })();
  });
});

/** Calls an account-bound method; resolves to null when the owner changed meanwhile. */
async function call<T>(method: string, params: Record<string, unknown>, who: Owner, verify = true): Promise<{ value: T } | { error: string } | null> {
  const mine = generation;
  try {
    const reply = await rpc.call<unknown>(method, bound(who, params));
    if (mine !== generation || ownerKey(owner()) !== ownerKey(who)) return null;
    if (verify && !boundReply(reply, who)) return { error: "Your account changed. Reopen this view." };
    return { value: reply as T };
  } catch (error) {
    if (mine !== generation || ownerKey(owner()) !== ownerKey(who)) return null;
    return { error: cloudErrorText(errorText(error)) };
  }
}

const data = <T>(reply: unknown) => (reply as { data?: T }).data;

// Billing ----------------------------------------------------------------------------------------

export async function loadBilling(more = false) {
  const who = owner();
  if (!who || cloud.billing.loading) return;
  if (!more) {
    void loadSubscription();
    void loadCredits(false);
    if (!who.organizationId) void loadWorkspaces();
  }
  setCloud(draft => { Object.assign(draft["billing"], { loaded: true, loading: true, error: null }); });
  const [summary, page] = await Promise.all([
    call<unknown>(M.CLOUD_BILLING_SUMMARY, {}, who),
    call<unknown>(M.CLOUD_BILLING_USAGE, { limit: 100, cursor: more ? cloud.billing.cursor : null }, who),
  ]);
  if (!summary || !page) return;
  setCloud(draft => { draft["billing"]["loading"] = false; });
  if ("error" in summary || "error" in page) {
    const message = "error" in summary ? summary.error : (page as { error: string }).error;
    // Unsubscribed accounts have no billing period; the hosting card already says so.
    if (cloud.billing.subscribed !== false || message !== "An active cloud billing period is required.") setCloud(draft => { draft["billing"]["error"] = message; });
    return;
  }
  const parsed = data<BillingSummary>(summary.value);
  const usage = data<{ items?: BillingUsageItem[]; nextCursor?: string | null }>(page.value);
  if (!parsed || typeof parsed.overageCapMicros !== "number" || !usage || !Array.isArray(usage.items)) {
    setCloud(draft => { draft["billing"]["error"] = "Cloud billing returned an invalid response. Refresh to retry."; });
    return;
  }
  setCloud(draft => {
    if (!more) (() => { Object.assign(draft["billing"], capEdited ? { usage: [] } : { usage: [], capInput: capDollars(parsed.overageCapMicros) }); })();
    const seen = new Set(draft.billing.usage.map((item) => item.entryId));
    (() => { draft["billing"]["usage"] = ((list) => [...list, ...usage.items!.filter((item) => !seen.has(item.entryId))])(draft["billing"]["usage"]); })();
    (() => { Object.assign(draft["billing"], { cursor: usage.nextCursor ?? null, summary: parsed }); })();
  });
}

async function loadSubscription() {
  const who = owner();
  if (!who || cloud.billing.subscriptionLoading) return;
  setCloud(draft => { draft["billing"]["subscriptionLoading"] = true; });
  const result = await call<unknown>(M.CLOUD_BILLING_ENTITLEMENTS, {}, who);
  if (!result) return;
  setCloud(draft => { draft["billing"]["subscriptionLoading"] = false; });
  if ("error" in result) return setCloud(draft => { draft["billing"]["error"] = result.error; });
  const value = data(result.value);
  const subscribed = cloudSubscribed(value, Date.now());
  setCloud(draft => {
    (() => { draft["billing"]["subscribed"] = subscribed; })();
    (() => { (() => { const next = readCatalog(value); if (draft["billing"]["catalog"] == null) draft["billing"]["catalog"] = next; else reconcile(next)(draft["billing"]["catalog"]); })(); })();
    if (subscribed === false && draft.billing.error === "An active cloud billing period is required.") (() => { draft["billing"]["error"] = null; })();
    if (subscribed === null) (() => { draft["billing"]["error"] = "Could not read the subscription. Refresh to retry."; })();
  });
}

/** Opens the billing portal, where the subscription, payment details and invoices are managed. */
export async function openBilling() {
  const who = owner();
  if (!who || cloud.billing.portalBusy) return;
  setCloud(draft => { Object.assign(draft["billing"], { portalBusy: true, error: null }); });
  const result = await call<unknown>(M.CLOUD_BILLING_PORTAL, {}, who);
  if (!result) return;
  setCloud(draft => { draft["billing"]["portalBusy"] = false; });
  if ("error" in result) return setCloud(draft => { draft["billing"]["error"] = result.error; });
  const url = data<{ portalUrl?: unknown }>(result.value)?.portalUrl;
  if (typeof url === "string" && url.startsWith("https://")) openExternal(url);
  else setCloud(draft => { draft["billing"]["error"] = "The billing portal returned an invalid link."; });
}

/**
 * Buys or switches to a plan. New subscriptions open checkout; a plan change opens the billing portal link the
 * server returns. Starting Cloud during a trial ends the trial at once and returns no link, so billing just refreshes.
 */
export async function checkout(offerId: string) {
  const who = owner();
  if (!who || cloud.billing.checkoutBusy) return;
  setCloud(draft => { Object.assign(draft["billing"], { checkoutBusy: offerId, error: null, notice: null }); });
  const result = await call<unknown>(M.CLOUD_BILLING_CHECKOUT, { offerId }, who);
  if (!result) return;
  setCloud(draft => { draft["billing"]["checkoutBusy"] = null; });
  if ("error" in result) return setCloud(draft => { draft["billing"]["error"] = result.error; });
  const reply = data<{ checkoutUrl?: unknown; trialConverted?: unknown }>(result.value);
  if (typeof reply?.checkoutUrl === "string" && reply.checkoutUrl.startsWith("https://")) openExternal(reply.checkoutUrl);
  else if (reply?.trialConverted === true) {
    setCloud(draft => { draft["billing"]["notice"] = "Cloud has started."; });
    void loadBilling();
  } else setCloud(draft => { draft["billing"]["error"] = "Checkout returned an invalid link. Try again."; });
}

export async function loadCredits(buy: boolean) {
  const who = owner();
  if (!who || cloud.billing.creditsBusy) return;
  if (buy && !cloud.billing.credits?.available) return;
  setCloud(draft => { Object.assign(draft["billing"], { creditsBusy: true, creditError: null }); });
  const result = await call<unknown>(buy ? M.CLOUD_PREPAID_CHECKOUT : M.CLOUD_PREPAID_BALANCE, buy ? { amountCents: cloud.billing.creditAmount } : {}, who);
  if (!result) return;
  setCloud(draft => { draft["billing"]["creditsBusy"] = false; });
  if ("error" in result) return setCloud(draft => { draft["billing"]["creditError"] = result.error; });
  if (buy) {
    const url = data<{ checkoutUrl?: unknown }>(result.value)?.checkoutUrl;
    if (typeof url === "string" && url.startsWith("https://")) openExternal(url);
    else setCloud(draft => { draft["billing"]["creditError"] = "The provider returned an invalid credit checkout link."; });
    return;
  }
  const balance = data<PrepaidBalance>(result.value);
  if (balance && typeof balance.available === "boolean" && typeof balance.creditCents === "number") setCloud(draft => { (() => { const next = balance; if (draft["billing"]["credits"] == null) draft["billing"]["credits"] = next; else reconcile(next)(draft["billing"]["credits"]); })(); });
  else setCloud(draft => { draft["billing"]["creditError"] = "Could not read the prepaid balance. Refresh to retry."; });
}

export const selectCredit = (amount: number) => setCloud(draft => { draft["billing"]["creditAmount"] = amount; });
export const setCapInput = (value: string) => {
  capEdited = true;
  setCloud(draft => { draft["billing"]["capInput"] = value; });
};

// The same idempotency key is reused while the requested amount is unchanged, so a retry is safe.
let capRequest: { amount: number; key: string } | null = null;
export async function saveCap() {
  const who = owner();
  if (!who || cloud.billing.capBusy || cloud.billing.loading) return;
  const amount = capMicros(cloud.billing.capInput);
  if (amount === null) return setCloud(draft => { draft["billing"]["error"] = "Enter a nonnegative dollar amount with up to six decimal places."; });
  if (capRequest?.amount !== amount) capRequest = { amount, key: newId() };
  setCloud(draft => { Object.assign(draft["billing"], { capBusy: true, error: null, notice: null }); });
  const result = await call<unknown>(M.CLOUD_BILLING_CAP, { overageCapMicros: amount, idempotencyKey: capRequest.key }, who);
  if (!result) return;
  setCloud(draft => { draft["billing"]["capBusy"] = false; });
  if ("error" in result) {
    setCloud(draft => { draft["billing"]["error"] = result.error; });
    return;
  }
  const summary = data<BillingSummary>(result.value);
  if (!summary || typeof summary.overageCapMicros !== "number") {
    return setCloud(draft => { draft["billing"]["error"] = "Could not verify the saved cap. Refresh before changing it again."; });
  }
  capRequest = null;
  capEdited = false;
  setCloud(draft => { Object.assign(draft["billing"], { summary, capInput: capDollars(summary.overageCapMicros), notice: "Monthly overage cap saved." }); });
}

// Workspaces -------------------------------------------------------------------------------------

export const workspaceLabel = (row: Workspace) => (row.title?.trim() ? row.title : row.branch);
export const isArchived = (row: Workspace) => ["archiving", "archived"].includes(row.state) || row.desiredState === "archived";
export const isDeleting = (row: Workspace) => row.desiredState === "deleted" && row.state !== "deleted";
export const cleanupPending = (row: Workspace) => row.localMirrorCleanup?.state === "pending";
export const cleanupFailed = (row: Workspace) => row.localMirrorCleanup?.state === "error";
export const deleteFailed = (row: Workspace) => isDeleting(row) && row.statusCode.startsWith("cloud_");
export function archiveStatus(row: Workspace) {
  const status = row.archiveStatus;
  return status && status.workspaceId === row.workspaceId && status.archived && isArchived(row) && !isDeleting(row) ? status : null;
}
const recoveryPending = (row: Workspace) => {
  const status = archiveStatus(row);
  return !!status && !status.errorCode && ["uploading", "verifying", "retiring", "restoring"].includes(status.phase ?? "");
};
const STARTING = new Set(["starting", "provisioning", "creating"]);
const needsPoll = (rows: Workspace[]) =>
  rows.some((row) => row.state === "archiving" || isDeleting(row) || cleanupPending(row) || recoveryPending(row) || ["starting", "pausing", "provisioning", "creating"].includes(row.state));

export async function loadWorkspaces() {
  const who = owner();
  if (!who || cloud.workspaces.loading) return;
  setCloud(draft => { Object.assign(draft["workspaces"], { loaded: true, loading: true, error: null }); });
  const result = await call<{ workspaces?: Workspace[] }>(M.CLOUD_WORKSPACES, {}, who, false);
  if (!result) return;
  setCloud(draft => { draft["workspaces"]["loading"] = false; });
  if ("error" in result) return setCloud(draft => { draft["workspaces"]["error"] = result.error; });
  const reply = result.value as { accountId?: string | null; workspaces?: Workspace[] };
  if (reply.accountId !== who.accountId || !Array.isArray(reply.workspaces)) {
    return setCloud(draft => { draft["workspaces"]["error"] = "Could not load cloud workspaces. Refresh to retry."; });
  }
  // Titles arrive with saved history; keep ones already read when the list omits them.
  const titles = new Map(cloud.workspaces.rows.map((row) => [row.workspaceId, row.title]));
  const rows = reply.workspaces.map((row) => (row.title == null && titles.get(row.workspaceId) ? { ...row, title: titles.get(row.workspaceId) } : row));
  setCloud(draft => {
    (() => { (() => { const next = rows; if (draft["workspaces"]["rows"] == null) draft["workspaces"]["rows"] = next; else reconcile(next, "workspaceId")(draft["workspaces"]["rows"]); })(); })();
    // An accepted first message leaves the queue once its workspace has started; the chats list carries it from there.
    const started = new Set(rows.filter((row) => !STARTING.has(row.state)).map((row) => row.workspaceId));
    if (draft.launches.some((item) => item.state === "accepted" && item.workspaceId && started.has(item.workspaceId))) {
      (() => { draft["launches"] = ((list) => list.filter((item) => !(item.state === "accepted" && item.workspaceId && started.has(item.workspaceId))))(draft["launches"]); })();
    }
  });
  if (cloud.history.workspaceId && !rows.some((row) => row.workspaceId === cloud.history.workspaceId)) closeHistory();
  if (needsPoll(rows)) {
    const mine = generation;
    later(3000, () => mine === generation && void loadWorkspaces());
  }
}

function updateRow(row: Workspace) {
  setCloud(draft => { draft["workspaces"]["rows"] = ((rows) => {
    const index = rows.findIndex((old) => old.workspaceId === row.workspaceId);
    if (index < 0) return [row, ...rows];
    const next = rows.slice();
    next[index] = { ...row, title: row.title ?? rows[index]!.title };
    return next;
  })(draft["workspaces"]["rows"]); });
}

// One command id per workspace action, kept until it resolves, so a retry repeats the same request.
const commands = new Map<string, string>();
export type WorkspaceAction = "restore" | "delete" | "cleanup" | "pause";
export const workspaceBusy = () => Object.values(cloud.workspaces.pending).some(Boolean);
async function workspaceAction(id: string, action: WorkspaceAction) {
  const who = owner();
  if (!who || cloud.workspaces.pending[id]) return;
  const method = { restore: M.CLOUD_UNARCHIVE, delete: M.CLOUD_DELETE, cleanup: M.CLOUD_CLEANUP_MIRROR, pause: M.CLOUD_PAUSE }[action];
  const commandKey = `${action}:${id}`;
  let command = commands.get(commandKey);
  if (!command) commands.set(commandKey, (command = newId()));
  setCloud(draft => { Object.assign(draft["workspaces"], { rowError: null }); });
  setCloud(draft => { draft["workspaces"]["pending"][id] = action; });
  const params = action === "cleanup" ? { workspaceId: id } : { workspaceId: id, commandId: command };
  const result = await call<{ workspace?: Workspace }>(method, params, who, false);
  if (!result) return;
  // Store setters merge objects, so a removed key must be cleared explicitly.
  setCloud(draft => { draft["workspaces"]["pending"][id] = undefined; });
  if ("error" in result) return setCloud(draft => { draft["workspaces"]["rowError"] = { id, message: result.error }; });
  commands.delete(commandKey);
  const workspace = (result.value as { workspace?: Workspace } | null)?.workspace;
  if (workspace && typeof workspace.workspaceId === "string") updateRow(workspace);
  if (action === "restore" && cloud.history.workspaceId === id) closeHistory();
  void loadWorkspaces();
}

export const restoreWorkspace = (id: string) => workspaceAction(id, "restore");
export const deleteWorkspace = (id: string) => workspaceAction(id, "delete");
export const retryCleanup = (id: string) => workspaceAction(id, "cleanup");
export const pauseWorkspace = (id: string) => workspaceAction(id, "pause");

// Saved history ----------------------------------------------------------------------------------

let historyRevision = 0;
export async function openHistory(workspaceId: string, sessionId: string | null = null) {
  const who = owner();
  if (!who) return;
  const mine = ++historyRevision;
  setCloud(draft => { Object.assign(draft["history"], { workspaceId, sessionId, head: sessionId ? cloud.history.head : null, loading: true, error: null }); });
  const result = await call<{ head?: Head; workspace?: Workspace }>(M.CLOUD_HISTORY, sessionId ? { workspaceId, sessionId } : { workspaceId }, who, false);
  if (!result || mine !== historyRevision) return;
  setCloud(draft => { draft["history"]["loading"] = false; });
  if ("error" in result) return setCloud(draft => { draft["history"]["error"] = result.error; });
  const head = result.value.head;
  if (!head || head.workspaceId !== workspaceId || !Array.isArray(head.transcript?.reset)) {
    return setCloud(draft => { draft["history"]["error"] = "Saved cloud history is invalid."; });
  }
  setCloud(draft => {
    (() => { (() => { const next = head; if (draft["history"]["head"] == null) draft["history"]["head"] = next; else reconcile(next)(draft["history"]["head"]); })(); })();
    (() => { draft["history"]["sessionId"] = head.sessionId; })();
    const row = draft.workspaces.rows.find((r) => r.workspaceId === workspaceId);
    if (row && head.chat?.title) updateRow({ ...row, title: head.chat.title });
  });
}
export function closeHistory() {
  historyRevision++;
  setCloud(draft => { (() => { const next = { workspaceId: null, sessionId: null, head: null, loading: false, error: null }; if (draft["history"] == null) draft["history"] = next; else reconcile(next)(draft["history"]); })(); });
}

// Launching cloud chats --------------------------------------------------------------------------

export async function loadContext() {
  const who = owner();
  if (!who || cloud.context.loading) return;
  setCloud(draft => { Object.assign(draft["context"], { loaded: true, loading: true, error: null }); });
  const result = await call<unknown>(M.CLOUD_CONTEXT, {}, who, false);
  if (!result) return;
  setCloud(draft => { draft["context"]["loading"] = false; });
  if ("error" in result) return setCloud(draft => { draft["context"]["error"] = result.error; });
  const value = result.value as { accountId?: string; projects?: CloudContextProject[]; agentCatalog?: { providers?: CloudAgent[] }; computerCapabilities?: CloudCapabilities | null };
  if (value.accountId !== who.accountId) return setCloud(draft => { draft["context"]["error"] = "Your account changed. Reopen this view."; });
  setCloud(draft => { Object.assign(draft["context"], {
    projects: Array.isArray(value.projects) ? value.projects : [],
    agents: Array.isArray(value.agentCatalog?.providers) ? value.agentCatalog!.providers! : [],
    capabilities: value.computerCapabilities && typeof value.computerCapabilities === "object" ? value.computerCapabilities : null,
  }); });
}

export interface LaunchDraft {
  project: CloudContextProject;
  agent: string;
  model: string;
  branch: string;
  runtimeMode: "approval-required" | "auto" | "full-access";
  text: string;
}
const drafts = new Map<string, Record<string, unknown>>();

/** Starts a cloud chat. A failed or uncertain launch keeps its idempotency key for a safe retry. */
export async function launch(draft: LaunchDraft | null, retryKey?: string) {
  const who = owner();
  if (!who) return;
  if (who.organizationId) {
    setCloud(draft => { draft["context"]["error"] = "Cloud chats start from your personal account."; });
    return;
  }
  let key = retryKey;
  if (!key) {
    if (!draft) return;
    key = newId();
    const project = draft.project;
    drafts.set(key, {
      projectId: project.projectId,
      // New computers run where the server says; the project's bound provider is the fallback.
      providerId: defaultProvider(cloud.context.capabilities) ?? project.providerId,
      baseRef: project.defaultBranch ?? "main",
      ...(draft.branch.trim() ? { branch: draft.branch.trim() } : {}),
      agent: draft.agent,
      model: draft.model,
      runtimeMode: draft.runtimeMode,
      idempotencyKey: key,
      firstMessage: draft.text,
    });
    setCloud(storeDraft => { storeDraft["launches"] = ((list) => [
      { key: key!, text: draft.text, repository: project.displayName ?? project.name ?? "Repository", state: "sending", message: "Accepting first message…", workspaceId: null },
      ...list,
    ])(storeDraft["launches"]); });
  } else {
    setCloud(draft => { for (const row of draft["launches"]) if (((row) => row.key === key)(row)) Object.assign(row, { state: "sending", message: "Accepting first message…" }); });
  }
  const body = drafts.get(key);
  if (!body) return;
  const mine = generation;
  try {
    const reply = await rpc.call<{ accountId?: string; workspace?: Workspace | { workspace?: Workspace } }>(M.CLOUD_CREATE, { ...body, expectedAccountId: who.accountId });
    if (mine !== generation) return;
    if (reply?.accountId !== who.accountId) throw new Error("Your account changed. Reopen this view.");
    const raw = reply.workspace as (Workspace & { workspace?: Workspace }) | undefined;
    const workspace = raw?.workspace ?? raw;
    drafts.delete(key);
    setCloud(draft => { for (const row of draft["launches"]) if (((row) => row.key === key)(row)) Object.assign(row, { state: "accepted", message: "Preparing cloud workspace…", workspaceId: workspace?.workspaceId ?? null }); });
    if (workspace?.workspaceId) updateRow(workspace as Workspace);
    void loadWorkspaces();
  } catch (error) {
    if (mine !== generation) return;
    const uncertain = (error as { outcomeUnknown?: boolean }).outcomeUnknown === true;
    setCloud(draft => { for (const row of draft["launches"]) if (((row) => row.key === key)(row)) Object.assign(row, {
      state: uncertain ? "uncertain" : "failed",
      message: uncertain ? "The connection dropped before the cloud confirmed this chat. Retry sends the same request, so it cannot start twice." : cloudErrorText(errorText(error)),
    }); });
  }
}

export function dismissLaunch(key: string) {
  drafts.delete(key);
  setCloud(draft => { draft["launches"] = ((list) => list.filter((row) => row.key !== key))(draft["launches"]); });
}

// Repositories and image -------------------------------------------------------------------------

export function canManageRepositories() {
  const who = owner();
  return !!who && (!who.organizationId || cloud.repos.github?.canManageInstallations === true);
}

export async function loadRepositories() {
  const who = owner();
  if (!who || cloud.repos.listing || cloud.repos.githubLoading) return;
  setCloud(draft => { Object.assign(draft["repos"], { loaded: true, listing: true, githubLoading: true, error: null }); });
  void loadImage();
  // Image builds send the provider new computers run on.
  if (!cloud.context.loaded) void loadContext();
  const [list, github] = await Promise.all([call<unknown>(M.CLOUD_PROJECTS, {}, who), call<unknown>(M.CLOUD_GITHUB, {}, who)]);
  if (!list || !github) return;
  setCloud(draft => {
    (() => { Object.assign(draft["repos"], { listing: false, githubLoading: false }); })();
    if ("error" in list) (() => { draft["repos"]["error"] = list.error; })();
    else {
      const projects = data<{ projects?: CloudProject[] }>(list.value)?.projects;
      if (Array.isArray(projects)) (() => { (() => { const next = projects; if (draft["repos"]["projects"] == null) draft["repos"]["projects"] = next; else reconcile(next, "projectId")(draft["repos"]["projects"]); })(); })();
      else (() => { draft["repos"]["error"] = "Repository list is unavailable. Refresh and try again."; })();
    }
    if ("error" in github) (() => { draft["repos"]["error"] = github.error; })();
    else {
      const catalog = data<GithubCatalog>(github.value);
      if (catalog && Array.isArray(catalog.installations) && Array.isArray(catalog.repositories)) (() => { (() => { const next = catalog; if (draft["repos"]["github"] == null) draft["repos"]["github"] = next; else reconcile(next)(draft["repos"]["github"]); })(); })();
      else (() => { draft["repos"]["error"] = "GitHub repository list is unavailable. Refresh and try again."; })();
    }
  });
}

const connectKeys = new Map<string, string>();
/** Connects repositories one at a time, as desktop does, so each result is verified before the next. */
export async function connectRepositories(repos: GithubRepository[]) {
  const who = owner();
  if (!who || cloud.repos.connecting.length || !canManageRepositories()) return;
  setCloud(draft => { Object.assign(draft["repos"], { connecting: repos.map((r) => r.nameWithOwner), error: null, notice: null }); });
  for (const repo of repos) {
    const name = repo.nameWithOwner.toLowerCase();
    let key = connectKeys.get(name);
    if (!key) connectKeys.set(name, (key = newId()));
    const result = await call<unknown>(
      M.CLOUD_PROJECT_CONNECT,
      { repositoryUrl: repo.httpsUrl, defaultBranch: repo.defaultBranch, visibility: repo.isPrivate ? "private" : "public", idempotencyKey: key },
      who,
    );
    if (!result) return;
    if ("error" in result) {
      setCloud(draft => { Object.assign(draft["repos"], { connecting: [], error: result.error }); });
      return;
    }
    const project = data<CloudProject>(result.value);
    if (!project?.projectId) {
      setCloud(draft => { Object.assign(draft["repos"], { connecting: [], error: "The repository result could not be verified. Refresh before retrying." }); });
      return;
    }
    connectKeys.delete(name);
    setCloud(draft => {
      (() => { draft["repos"]["projects"] = ((list) => [...list.filter((p) => p.projectId !== project.projectId), project])(draft["repos"]["projects"]); })();
      (() => { draft["repos"]["connecting"] = ((list) => list.filter((n) => n !== repo.nameWithOwner))(draft["repos"]["connecting"]); })();
      (() => { draft["repos"]["notice"] = "Repository added."; })();
    });
  }
  void loadImage();
}

export async function removeRepository(projectId: string) {
  const who = owner();
  if (!who || cloud.repos.removing || !canManageRepositories()) return;
  setCloud(draft => { Object.assign(draft["repos"], { removing: projectId, error: null, notice: null }); });
  const result = await call<unknown>(M.CLOUD_PROJECT_REMOVE, { projectId }, who);
  if (!result) return;
  setCloud(draft => { draft["repos"]["removing"] = null; });
  if ("error" in result) return setCloud(draft => { draft["repos"]["error"] = result.error; });
  const project = data<CloudProject>(result.value);
  if (!project?.projectId) return setCloud(draft => { draft["repos"]["error"] = "The repository result could not be verified. Refresh before retrying."; });
  setCloud(draft => {
    (() => { draft["repos"]["projects"] = ((list) => list.filter((p) => p.projectId !== project.projectId))(draft["repos"]["projects"]); })();
    (() => { draft["repos"]["notice"] = "Repository removed. Existing chats are preserved."; })();
  });
  void loadImage();
}

let imageFailures = 0;
export async function loadImage() {
  const who = owner();
  if (!who || cloud.image.loading || cloud.image.building || cloud.image.deleting) return;
  setCloud(draft => { draft["image"]["loading"] = true; });
  receiveImage(await call<unknown>(M.CLOUD_IMAGE, {}, who), "image");
}

function receiveImage(result: { value: unknown } | { error: string } | null, kind: "image" | "build" | "delete") {
  if (!result) return;
  setCloud(draft => { Object.assign(draft["image"], { loading: false, building: false, deleting: false }); });
  if ("error" in result) {
    imageFailures++;
    setCloud(draft => { draft["image"]["error"] = result.error; });
  } else {
    const image = data<AccountImage>(result.value);
    if (!image || typeof image.state !== "string" || !Array.isArray(image.builds)) {
      imageFailures++;
      setCloud(draft => { draft["image"]["error"] = "Image status could not be verified. Refresh and try again."; });
    } else {
      imageFailures = 0;
      if (kind === "build") buildKey = null;
      setCloud(draft => {
        (() => { (() => { const next = image; if (draft["image"]["value"] == null) draft["image"]["value"] = next; else reconcile(next)(draft["image"]["value"]); })(); })();
        (() => { draft["image"]["error"] = null; })();
      });
    }
  }
  const value = result && "value" in result ? data<AccountImage>(result.value) : cloud.image.value;
  if (value && (value.state === "building" || value.storage?.state === "deleting") && imageFailures < 3) {
    const mine = generation;
    later(2000, () => mine === generation && void loadImage());
  }
}

let buildKey: { mode: string; key: string } | null = null;
export const buildRetry = () => buildKey !== null;
export async function buildImage(mode: "update" | "rebuild") {
  const who = owner();
  const value = cloud.image.value;
  if (!who || cloud.image.loading || cloud.image.building || cloud.image.deleting || !canManageRepositories() || !cloud.repos.projects.length) return;
  if (!value || value.state === "building" || value.state === "auth-broken") return;
  if (!buildKey) buildKey = { mode, key: newId() };
  setCloud(draft => { Object.assign(draft["image"], { building: true, error: null }); });
  // Without a known provider the server builds for its default.
  const providerId = defaultProvider(cloud.context.capabilities) ?? value.providerId ?? null;
  receiveImage(
    await call<unknown>(M.CLOUD_IMAGE_BUILD, { mode: buildKey.mode, ...(providerId ? { providerId } : {}), idempotencyKey: buildKey.key }, who),
    "build",
  );
}

export async function deleteImage(snapshotId: string) {
  const who = owner();
  if (!who || cloud.image.loading || cloud.image.building || cloud.image.deleting || !canManageRepositories()) return;
  if (cloud.image.value?.storage?.snapshotId !== snapshotId) return;
  setCloud(draft => { Object.assign(draft["image"], { deleting: true, error: null }); });
  receiveImage(await call<unknown>(M.CLOUD_IMAGE_DELETE, { snapshotId }, who), "delete");
}

export const clearRepoNotice = () => setCloud(draft => { Object.assign(draft["repos"], { notice: null, error: null }); });
export const clearBillingNotice = () => setCloud(draft => { Object.assign(draft["billing"], { notice: null, error: null }); });
