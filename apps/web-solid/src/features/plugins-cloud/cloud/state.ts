// Cloud data for the signed-in account. Every request names its owner; replies for another owner are dropped.
import { batch, createEffect, createRoot, on } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { onConnected, rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { bound, boundReply, errorText, newId, openExternal, owner, ownerKey, type Owner } from "../account";
import { capDollars, capMicros, cloudSubscribed, STANDARD_OFFER } from "./money";
import type {
  AccountImage,
  BillingSummary,
  BillingUsageItem,
  CloudAgent,
  CloudContextProject,
  CloudProject,
  GithubCatalog,
  GithubRepository,
  Head,
  PrepaidBalance,
  Workspace,
} from "./types";

interface Billing {
  loaded: boolean;
  loading: boolean;
  subscribed: boolean | null;
  subscriptionLoading: boolean;
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
  context: { loaded: false, loading: false, projects: [], agents: [], error: null } as Context,
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
  setCloud(reconcile(initial()));
}

createRoot(() => {
  createEffect(on(() => ownerKey(owner()), (key, previous) => key !== previous && reset()));
});
onConnected(() => {
  // A dropped connection fails in-flight work; clear busy flags so the user can check and retry.
  generation++;
  batch(() => {
    setCloud("billing", { loading: false, subscriptionLoading: false, portalBusy: false, creditsBusy: false, capBusy: false });
    setCloud("repos", { listing: false, githubLoading: false, connecting: [], removing: null });
    setCloud("image", { loading: false, building: false, deleting: false });
    setCloud("workspaces", { loading: false, pending: {} });
    setCloud("context", "loading", false);
    setCloud("history", "loading", false);
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
    return { error: errorText(error) };
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
  setCloud("billing", { loaded: true, loading: true, error: null });
  const [summary, page] = await Promise.all([
    call<unknown>(M.CLOUD_BILLING_SUMMARY, {}, who),
    call<unknown>(M.CLOUD_BILLING_USAGE, { limit: 10, cursor: more ? cloud.billing.cursor : null }, who),
  ]);
  if (!summary || !page) return;
  setCloud("billing", "loading", false);
  if ("error" in summary || "error" in page) {
    const message = "error" in summary ? summary.error : (page as { error: string }).error;
    // Unsubscribed accounts have no billing period; the hosting card already says so.
    if (cloud.billing.subscribed !== false || message !== "An active cloud billing period is required.") setCloud("billing", "error", message);
    return;
  }
  const parsed = data<BillingSummary>(summary.value);
  const usage = data<{ items?: BillingUsageItem[]; nextCursor?: string | null }>(page.value);
  if (!parsed || typeof parsed.overageCapMicros !== "number" || !usage || !Array.isArray(usage.items)) {
    setCloud("billing", "error", "Cloud billing returned an invalid response. Refresh to retry.");
    return;
  }
  batch(() => {
    if (!more) setCloud("billing", capEdited ? { usage: [] } : { usage: [], capInput: capDollars(parsed.overageCapMicros) });
    const seen = new Set(cloud.billing.usage.map((item) => item.entryId));
    setCloud("billing", "usage", (list) => [...list, ...usage.items!.filter((item) => !seen.has(item.entryId))]);
    setCloud("billing", { cursor: usage.nextCursor ?? null, summary: parsed });
  });
}

async function loadSubscription() {
  const who = owner();
  if (!who || cloud.billing.subscriptionLoading) return;
  setCloud("billing", "subscriptionLoading", true);
  const result = await call<unknown>(M.CLOUD_BILLING_ENTITLEMENTS, {}, who);
  if (!result) return;
  setCloud("billing", "subscriptionLoading", false);
  if ("error" in result) return setCloud("billing", "error", result.error);
  const subscribed = cloudSubscribed(data(result.value), Date.now());
  batch(() => {
    setCloud("billing", "subscribed", subscribed);
    if (subscribed === false && cloud.billing.error === "An active cloud billing period is required.") setCloud("billing", "error", null);
    if (subscribed === null) setCloud("billing", "error", "Could not read the subscription. Refresh to retry.");
  });
}

/** Opens checkout when unsubscribed, otherwise the billing portal. */
export async function openBilling(forcePortal: boolean) {
  const who = owner();
  if (!who || cloud.billing.portalBusy) return;
  const subscribed = forcePortal || cloud.billing.subscribed;
  if (subscribed === null) return;
  setCloud("billing", { portalBusy: true, error: null });
  const result = await call<unknown>(subscribed ? M.CLOUD_BILLING_PORTAL : M.CLOUD_BILLING_CHECKOUT, subscribed ? {} : { offerId: STANDARD_OFFER }, who);
  if (!result) return;
  setCloud("billing", "portalBusy", false);
  if ("error" in result) return setCloud("billing", "error", result.error);
  const url = data<Record<string, unknown>>(result.value)?.[subscribed ? "portalUrl" : "checkoutUrl"];
  if (typeof url === "string" && url.startsWith("https://")) openExternal(url);
  else setCloud("billing", "error", "The billing provider returned an invalid portal link.");
}

export async function loadCredits(buy: boolean) {
  const who = owner();
  if (!who || cloud.billing.creditsBusy) return;
  if (buy && !cloud.billing.credits?.available) return;
  setCloud("billing", { creditsBusy: true, creditError: null });
  const result = await call<unknown>(buy ? M.CLOUD_PREPAID_CHECKOUT : M.CLOUD_PREPAID_BALANCE, buy ? { amountCents: cloud.billing.creditAmount } : {}, who);
  if (!result) return;
  setCloud("billing", "creditsBusy", false);
  if ("error" in result) return setCloud("billing", "creditError", result.error);
  if (buy) {
    const url = data<{ checkoutUrl?: unknown }>(result.value)?.checkoutUrl;
    if (typeof url === "string" && url.startsWith("https://")) openExternal(url);
    else setCloud("billing", "creditError", "The provider returned an invalid credit checkout link.");
    return;
  }
  const balance = data<PrepaidBalance>(result.value);
  if (balance && typeof balance.available === "boolean" && typeof balance.creditCents === "number") setCloud("billing", "credits", reconcile(balance));
  else setCloud("billing", "creditError", "Could not read the prepaid balance. Refresh to retry.");
}

export const selectCredit = (amount: number) => setCloud("billing", "creditAmount", amount);
export const setCapInput = (value: string) => {
  capEdited = true;
  setCloud("billing", "capInput", value);
};

// The same idempotency key is reused while the requested amount is unchanged, so a retry is safe.
let capRequest: { amount: number; key: string } | null = null;
export async function saveCap() {
  const who = owner();
  if (!who || cloud.billing.capBusy || cloud.billing.loading) return;
  const amount = capMicros(cloud.billing.capInput);
  if (amount === null) return setCloud("billing", "error", "Enter a nonnegative dollar amount with up to six decimal places.");
  if (capRequest?.amount !== amount) capRequest = { amount, key: newId() };
  setCloud("billing", { capBusy: true, error: null, notice: null });
  const result = await call<unknown>(M.CLOUD_BILLING_CAP, { overageCapMicros: amount, idempotencyKey: capRequest.key }, who);
  if (!result) return;
  setCloud("billing", "capBusy", false);
  if ("error" in result) {
    setCloud("billing", "error", result.error);
    return;
  }
  const summary = data<BillingSummary>(result.value);
  if (!summary || typeof summary.overageCapMicros !== "number") {
    return setCloud("billing", "error", "Could not verify the saved cap. Refresh before changing it again.");
  }
  capRequest = null;
  capEdited = false;
  setCloud("billing", { summary, capInput: capDollars(summary.overageCapMicros), notice: "Monthly overage cap saved." });
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
  setCloud("workspaces", { loaded: true, loading: true, error: null });
  const result = await call<{ workspaces?: Workspace[] }>(M.CLOUD_WORKSPACES, {}, who, false);
  if (!result) return;
  setCloud("workspaces", "loading", false);
  if ("error" in result) return setCloud("workspaces", "error", result.error);
  const reply = result.value as { accountId?: string | null; workspaces?: Workspace[] };
  if (reply.accountId !== who.accountId || !Array.isArray(reply.workspaces)) {
    return setCloud("workspaces", "error", "Could not load cloud workspaces. Refresh to retry.");
  }
  // Titles arrive with saved history; keep ones already read when the list omits them.
  const titles = new Map(cloud.workspaces.rows.map((row) => [row.workspaceId, row.title]));
  const rows = reply.workspaces.map((row) => (row.title == null && titles.get(row.workspaceId) ? { ...row, title: titles.get(row.workspaceId) } : row));
  batch(() => {
    setCloud("workspaces", "rows", reconcile(rows, { key: "workspaceId", merge: true }));
    // An accepted first message leaves the queue once its workspace has started; the chats list carries it from there.
    const started = new Set(rows.filter((row) => !STARTING.has(row.state)).map((row) => row.workspaceId));
    if (cloud.launches.some((item) => item.state === "accepted" && item.workspaceId && started.has(item.workspaceId))) {
      setCloud("launches", (list) => list.filter((item) => !(item.state === "accepted" && item.workspaceId && started.has(item.workspaceId))));
    }
  });
  if (cloud.history.workspaceId && !rows.some((row) => row.workspaceId === cloud.history.workspaceId)) closeHistory();
  if (needsPoll(rows)) {
    const mine = generation;
    later(3000, () => mine === generation && void loadWorkspaces());
  }
}

function updateRow(row: Workspace) {
  setCloud("workspaces", "rows", (rows) => {
    const index = rows.findIndex((old) => old.workspaceId === row.workspaceId);
    if (index < 0) return [row, ...rows];
    const next = rows.slice();
    next[index] = { ...row, title: row.title ?? rows[index]!.title };
    return next;
  });
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
  setCloud("workspaces", { rowError: null });
  setCloud("workspaces", "pending", id, action);
  const params = action === "cleanup" ? { workspaceId: id } : { workspaceId: id, commandId: command };
  const result = await call<{ workspace?: Workspace }>(method, params, who, false);
  if (!result) return;
  // Store setters merge objects, so a removed key must be cleared explicitly.
  setCloud("workspaces", "pending", id, undefined);
  if ("error" in result) return setCloud("workspaces", "rowError", { id, message: result.error });
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
  setCloud("history", { workspaceId, sessionId, head: sessionId ? cloud.history.head : null, loading: true, error: null });
  const result = await call<{ head?: Head; workspace?: Workspace }>(M.CLOUD_HISTORY, sessionId ? { workspaceId, sessionId } : { workspaceId }, who, false);
  if (!result || mine !== historyRevision) return;
  setCloud("history", "loading", false);
  if ("error" in result) return setCloud("history", "error", result.error);
  const head = result.value.head;
  if (!head || head.workspaceId !== workspaceId || !Array.isArray(head.transcript?.reset)) {
    return setCloud("history", "error", "Saved cloud history is invalid.");
  }
  batch(() => {
    setCloud("history", "head", reconcile(head));
    setCloud("history", "sessionId", head.sessionId);
    const row = cloud.workspaces.rows.find((r) => r.workspaceId === workspaceId);
    if (row && head.chat?.title) updateRow({ ...row, title: head.chat.title });
  });
}
export function closeHistory() {
  historyRevision++;
  setCloud("history", reconcile({ workspaceId: null, sessionId: null, head: null, loading: false, error: null }));
}

// Launching cloud chats --------------------------------------------------------------------------

export async function loadContext() {
  const who = owner();
  if (!who || cloud.context.loading) return;
  setCloud("context", { loaded: true, loading: true, error: null });
  const result = await call<{ projects?: CloudContextProject[]; agentCatalog?: { providers?: CloudAgent[] } }>(M.CLOUD_CONTEXT, {}, who, false);
  if (!result) return;
  setCloud("context", "loading", false);
  if ("error" in result) return setCloud("context", "error", result.error);
  const value = result.value as { accountId?: string; projects?: CloudContextProject[]; agentCatalog?: { providers?: CloudAgent[] } };
  if (value.accountId !== who.accountId) return setCloud("context", "error", "Your account changed. Reopen this view.");
  setCloud("context", {
    projects: Array.isArray(value.projects) ? value.projects : [],
    agents: Array.isArray(value.agentCatalog?.providers) ? value.agentCatalog!.providers! : [],
  });
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
    setCloud("context", "error", "Cloud chats start from your personal account.");
    return;
  }
  let key = retryKey;
  if (!key) {
    if (!draft) return;
    key = newId();
    const project = draft.project;
    drafts.set(key, {
      projectId: project.projectId,
      providerId: project.providerId,
      baseRef: project.defaultBranch ?? "main",
      ...(draft.branch.trim() ? { branch: draft.branch.trim() } : {}),
      agent: draft.agent,
      model: draft.model,
      runtimeMode: draft.runtimeMode,
      idempotencyKey: key,
      firstMessage: draft.text,
    });
    setCloud("launches", (list) => [
      { key: key!, text: draft.text, repository: project.displayName ?? project.name ?? "Repository", state: "sending", message: "Accepting first message…", workspaceId: null },
      ...list,
    ]);
  } else {
    setCloud("launches", (row) => row.key === key, { state: "sending", message: "Accepting first message…" });
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
    setCloud("launches", (row) => row.key === key, { state: "accepted", message: "Preparing cloud workspace…", workspaceId: workspace?.workspaceId ?? null });
    if (workspace?.workspaceId) updateRow(workspace as Workspace);
    void loadWorkspaces();
  } catch (error) {
    if (mine !== generation) return;
    const uncertain = (error as { outcomeUnknown?: boolean }).outcomeUnknown === true;
    setCloud("launches", (row) => row.key === key, {
      state: uncertain ? "uncertain" : "failed",
      message: uncertain ? "The connection dropped before the cloud confirmed this chat. Retry sends the same request, so it cannot start twice." : errorText(error),
    });
  }
}

export function dismissLaunch(key: string) {
  drafts.delete(key);
  setCloud("launches", (list) => list.filter((row) => row.key !== key));
}

// Repositories and image -------------------------------------------------------------------------

export function canManageRepositories() {
  const who = owner();
  return !!who && (!who.organizationId || cloud.repos.github?.canManageInstallations === true);
}

export async function loadRepositories() {
  const who = owner();
  if (!who || cloud.repos.listing || cloud.repos.githubLoading) return;
  setCloud("repos", { loaded: true, listing: true, githubLoading: true, error: null });
  void loadImage();
  const [list, github] = await Promise.all([call<unknown>(M.CLOUD_PROJECTS, {}, who), call<unknown>(M.CLOUD_GITHUB, {}, who)]);
  if (!list || !github) return;
  batch(() => {
    setCloud("repos", { listing: false, githubLoading: false });
    if ("error" in list) setCloud("repos", "error", list.error);
    else {
      const projects = data<{ projects?: CloudProject[] }>(list.value)?.projects;
      if (Array.isArray(projects)) setCloud("repos", "projects", reconcile(projects, { key: "projectId" }));
      else setCloud("repos", "error", "Repository list is unavailable. Refresh and try again.");
    }
    if ("error" in github) setCloud("repos", "error", github.error);
    else {
      const catalog = data<GithubCatalog>(github.value);
      if (catalog && Array.isArray(catalog.installations) && Array.isArray(catalog.repositories)) setCloud("repos", "github", reconcile(catalog));
      else setCloud("repos", "error", "GitHub repository list is unavailable. Refresh and try again.");
    }
  });
}

const connectKeys = new Map<string, string>();
/** Connects repositories one at a time, as desktop does, so each result is verified before the next. */
export async function connectRepositories(repos: GithubRepository[]) {
  const who = owner();
  if (!who || cloud.repos.connecting.length || !canManageRepositories()) return;
  setCloud("repos", { connecting: repos.map((r) => r.nameWithOwner), error: null, notice: null });
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
      setCloud("repos", { connecting: [], error: result.error });
      return;
    }
    const project = data<CloudProject>(result.value);
    if (!project?.projectId) {
      setCloud("repos", { connecting: [], error: "The repository result could not be verified. Refresh before retrying." });
      return;
    }
    connectKeys.delete(name);
    batch(() => {
      setCloud("repos", "projects", (list) => [...list.filter((p) => p.projectId !== project.projectId), project]);
      setCloud("repos", "connecting", (list) => list.filter((n) => n !== repo.nameWithOwner));
      setCloud("repos", "notice", "Repository added.");
    });
  }
  void loadImage();
}

export async function removeRepository(projectId: string) {
  const who = owner();
  if (!who || cloud.repos.removing || !canManageRepositories()) return;
  setCloud("repos", { removing: projectId, error: null, notice: null });
  const result = await call<unknown>(M.CLOUD_PROJECT_REMOVE, { projectId }, who);
  if (!result) return;
  setCloud("repos", "removing", null);
  if ("error" in result) return setCloud("repos", "error", result.error);
  const project = data<CloudProject>(result.value);
  if (!project?.projectId) return setCloud("repos", "error", "The repository result could not be verified. Refresh before retrying.");
  batch(() => {
    setCloud("repos", "projects", (list) => list.filter((p) => p.projectId !== project.projectId));
    setCloud("repos", "notice", "Repository removed. Existing chats are preserved.");
  });
  void loadImage();
}

let imageFailures = 0;
export async function loadImage() {
  const who = owner();
  if (!who || cloud.image.loading || cloud.image.building || cloud.image.deleting) return;
  setCloud("image", "loading", true);
  receiveImage(await call<unknown>(M.CLOUD_IMAGE, {}, who), "image");
}

function receiveImage(result: { value: unknown } | { error: string } | null, kind: "image" | "build" | "delete") {
  if (!result) return;
  setCloud("image", { loading: false, building: false, deleting: false });
  if ("error" in result) {
    imageFailures++;
    setCloud("image", "error", result.error);
  } else {
    const image = data<AccountImage>(result.value);
    if (!image || typeof image.state !== "string" || !Array.isArray(image.builds)) {
      imageFailures++;
      setCloud("image", "error", "Image status could not be verified. Refresh and try again.");
    } else {
      imageFailures = 0;
      if (kind === "build") buildKey = null;
      batch(() => {
        setCloud("image", "value", reconcile(image));
        setCloud("image", "error", null);
      });
    }
  }
  const value = cloud.image.value;
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
  setCloud("image", { building: true, error: null });
  receiveImage(await call<unknown>(M.CLOUD_IMAGE_BUILD, { mode: buildKey.mode, providerId: "boxd", idempotencyKey: buildKey.key }, who), "build");
}

export async function deleteImage(snapshotId: string) {
  const who = owner();
  if (!who || cloud.image.loading || cloud.image.building || cloud.image.deleting || !canManageRepositories()) return;
  if (cloud.image.value?.storage?.snapshotId !== snapshotId) return;
  setCloud("image", { deleting: true, error: null });
  receiveImage(await call<unknown>(M.CLOUD_IMAGE_DELETE, { snapshotId }, who), "delete");
}

export const clearRepoNotice = () => setCloud("repos", { notice: null, error: null });
export const clearBillingNotice = () => setCloud("billing", { notice: null, error: null });
