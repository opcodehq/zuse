import { uuid } from "@/core/util";
// A cloud workspace uses the same UI and RPC framing as a local engine. The
// cookie-authenticated controller owns credentials and forwards the RPC stream.
import { ConnectionBlocked, retryConnection } from "@/core/connection";
import { M } from "@/core/methods";
import { isRead, RpcClient, type Subscription } from "@/core/rpc";
import { OwnerTransport, type Transport } from "@/core/transport";
import { cloudIssue, codeIn, GENERIC_ISSUE, type CloudIssue } from "@/features/plugins-cloud/cloud/errors";
import { phaseText, rowIssue, START_DEADLINE_MS, startupLabel } from "@/features/plugins-cloud/cloud/progress";
import type { Workspace } from "@/features/plugins-cloud/cloud/types";
import { area, setArea, type CloudStatus, type CloudStop } from "./status";

export interface CloudTarget { workspaceId: string; accountId: string; organizationId: string | null }
export function readCloudTarget(): CloudTarget | null {
  if (!location.hash.startsWith("#cloud=")) return null;
  const value = JSON.parse(decodeURIComponent(location.hash.slice(7))) as CloudTarget;
  if (!value || typeof value.workspaceId !== "string" || !value.workspaceId || value.workspaceId.length > 512 || typeof value.accountId !== "string" || !value.accountId || (value.organizationId !== null && typeof value.organizationId !== "string")) throw new Error("Invalid cloud workspace link");
  return value;
}
export function openCloudWorkspace(target: CloudTarget) {
  const url = new URL(location.pathname, location.origin);
  url.hash = `cloud=${encodeURIComponent(JSON.stringify(target))}`;
  const child = window.open(url, "_blank");
  if (child) child.opener = null;
  else throw new Error("Allow pop-ups to open this cloud workspace.");
}

// Opening the tab, Resume and Retry start may wake a paused computer; automatic reconnects never do, so a computer
// that went to sleep while the tab was open shows the paused screen instead of waking and using machine minutes.
let interactive = true;
let retryOperation: string | null = null;
const POLL_MS = 3000;
const LATE_START: CloudIssue = { title: "The computer didn't start in time", detail: "It didn't come online within 10 minutes. Your chat and first message are saved, so you can retry the start.", recovery: "retry" };
const UNREACHABLE: CloudIssue = { title: "Couldn't reach the cloud computer", detail: "Zuse couldn't connect to it for 10 minutes. Your chat is saved; try again when you're back online.", recovery: "retry" };

const setCloud = (patch: Partial<CloudStatus>) => setArea(draft => { Object.assign(draft.cloud, patch); });
function stopped(stop: CloudStop) {
  setCloud({ stop, since: null });
  return new ConnectionBlocked({ kind: "cloudStopped", message: stop.kind === "paused" ? "This cloud chat is paused." : stop.issue.detail });
}
function issueFrom(message: string, title: string): CloudIssue {
  const code = codeIn(message);
  if (code) return cloudIssue(code) ?? GENERIC_ISSUE;
  return { title, detail: message || GENERIC_ISSUE.detail, recovery: "retry" };
}

/** Wakes a paused computer, or retries a failed start with the server's recovery operation when it offered one. */
export function resumeCloud(operationId: string | null = null) {
  interactive = true;
  retryOperation = operationId;
  setCloud({ stop: null, since: Date.now(), phase: "Connecting to the cloud computer" });
  retryConnection();
}
/** Leaves this cloud chat for the app's Cloud page. */
export function backToCloud() {
  location.assign(location.pathname + location.search);
}

export class CloudTransport implements Transport {
  onMessage: ((text: string) => void) | null = null;
  onClose: ((reason: string) => void) | null = null;
  private socket = new OwnerTransport();
  private controller = new RpcClient();
  private subscriptions = new Map<number, Subscription>();
  private environment: Subscription | undefined;
  private account: Subscription | undefined;
  private timers: ReturnType<typeof setTimeout>[] = [];
  private closed = false;
  private lose(reason: string) {
    if (this.closed) return;
    this.close();
    if (area.cloud.since === null) setCloud({ since: Date.now() });
    this.onClose?.(reason);
  }
  constructor(private target: CloudTarget) {}
  private bound() { return { workspaceId: this.target.workspaceId, expectedAccountId: this.target.accountId, expectedOrganizationId: this.target.organizationId }; }
  private async row(): Promise<Workspace | null> {
    try {
      const reply = await this.controller.call<{ workspace?: Workspace }>(M.CLOUD_WORKSPACE, this.bound());
      return reply?.workspace?.workspaceId === this.target.workspaceId ? reply.workspace : null;
    } catch {
      return null;
    }
  }
  async open() {
    await this.socket.open();
    this.socket.onClose = reason => { this.lose(reason); };
    this.controller.attach(this.socket);
    try {
      let accountReady = false;
      await new Promise<void>((resolve, reject) => {
        this.account = this.controller.subscribe<{ user?: { id: string }; orgId?: string | null }>(M.CLOUD_ACCOUNT_STATUS, {}, value => {
          if (value.user?.id !== this.target.accountId || (value.orgId ?? null) !== this.target.organizationId) {
            const error = new ConnectionBlocked({ kind: "accountRequired", message: "Sign in on the controller and open this workspace again." });
            reject(error);
            this.lose(error.message);
          } else { accountReady = true; resolve(); }
        }, error => {
          if (accountReady) this.lose(error?.message ?? "Account connection ended");
          else reject(error ?? new Error("Account connection ended"));
        });
      });
      if (retryOperation) {
        const operationId = retryOperation;
        retryOperation = null;
        try {
          await this.controller.call(M.CLOUD_ALLOCATION_RETRY, { ...this.bound(), operationId, commandId: uuid() });
        } catch (error) {
          throw stopped({ kind: "failed", issue: issueFrom(error instanceof Error ? error.message : "", "Couldn't retry the start"), operationId: null });
        }
      }
      if (area.cloud.since === null) setCloud({ since: Date.now() });
      await this.waitReady();
    } catch (error) { this.close(); throw error; }
  }
  /** Follows the environment until it is ready, and reads the workspace row for the startup step and why it stopped. */
  private waitReady() {
    return new Promise<void>((resolve, reject) => {
      let settled = false;
      let ready = false;
      let rowLabel: string | null = null;
      let starting = false;
      // A paused row only means a failed wake once this wait has seen it awake.
      let woke = false;
      const settle = (run: () => void) => {
        if (settled) return;
        settled = true;
        for (const timer of this.timers) clearTimeout(timer);
        this.timers = [];
        run();
      };
      const fail = (stop: CloudStop) => settle(() => reject(stopped(stop)));
      const check = (row: Workspace | null, final: string | null) => {
        if (settled) return;
        if (row) {
          if (row.state !== "paused") woke = true;
          rowLabel = startupLabel(row);
          if (rowLabel) setCloud({ phase: rowLabel });
          const issue = rowIssue(row);
          if (row.allocationRecovery) return fail({ kind: "failed", issue: issue!, operationId: row.allocationRecovery.retryAvailable === false ? null : row.allocationRecovery.operationId });
          if (issue && (row.state !== "paused" || woke || final !== null)) return fail({ kind: "failed", issue, operationId: null });
        }
        if (final !== null) fail({ kind: "failed", issue: issueFrom(final, "Couldn't connect to the cloud computer"), operationId: null });
      };
      let polling = false;
      const poll = () => {
        if (polling || settled) return;
        polling = true;
        const tick = () => {
          void this.row().then(row => {
            check(row, null);
            if (!settled) this.timers.push(setTimeout(tick, POLL_MS));
          });
        };
        tick();
      };
      const deadline = Math.max(0, (area.cloud.since ?? Date.now()) + START_DEADLINE_MS - Date.now());
      this.timers.push(setTimeout(() => void this.row().then(row => {
        check(row, null);
        fail({ kind: "failed", issue: starting ? LATE_START : UNREACHABLE, operationId: null });
      }), deadline));
      this.environment = this.controller.subscribe<{ state: string; message: string }>(M.CLOUD_ENVIRONMENT, { ...this.bound(), interactiveResume: interactive }, value => {
        if (ready) {
          if (value.state !== "ready") this.lose(value.message || "Cloud workspace disconnected");
          return;
        }
        switch (value.state) {
          case "ready":
            ready = true;
            interactive = false;
            setCloud({ since: null, stop: null });
            settle(resolve);
            return;
          case "paused":
            void this.row().then(row => {
              const issue = row && rowIssue(row);
              fail(issue ? { kind: "failed", issue, operationId: null } : { kind: "paused" });
            });
            return;
          case "failed":
          case "blocked":
            void this.row().then(row => check(row, value.message ?? ""));
            return;
          case "starting":
            starting = true;
            poll();
            if (!rowLabel) setCloud({ phase: phaseText(value.message) });
            return;
          default:
            if (!rowLabel) setCloud({ phase: value.state === "reconnecting" ? "Reconnecting to the cloud computer" : phaseText(value.message) });
        }
      }, error => {
        if (ready) this.lose(error?.message ?? "Cloud environment ended");
        else settle(() => reject(error ?? new Error("Cloud environment ended")));
      });
    });
  }
  send(text: string) {
    const frame = JSON.parse(text) as { id: number; method: string; params: unknown; cancel?: boolean };
    if (frame.cancel) { this.subscriptions.get(frame.id)?.cancel(); this.subscriptions.delete(frame.id); return; }
    // Account/cloud settings remain on the controller; engine tools use the workspace.
    const controllerMethod = frame.method.startsWith("Cloud") || frame.method === M.SELECT_ORG;
    const params = frame.params as Record<string, unknown> | null;
    const commandId = params?.cloudCommandId ?? params?.commandId ?? (!isRead(frame.method, frame.params) ? uuid() : undefined);
    const subscription = this.controller.subscribe<unknown>(controllerMethod ? frame.method : M.CLOUD_RPC,
      controllerMethod ? frame.params : { ...this.bound(), requestId: frame.id, logicalCommandId: commandId, method: frame.method, params: frame.params },
      value => {
        this.onMessage?.(JSON.stringify(controllerMethod ? { id: frame.id, item: value } : value));
      }, error => {
        this.subscriptions.delete(frame.id);
        this.onMessage?.(JSON.stringify({ id: frame.id, ...(error ? { err: error.message } : { done: true }) }));
      });
    this.subscriptions.set(frame.id, subscription);
  }
  close() {
    this.closed = true;
    for (const timer of this.timers) clearTimeout(timer);
    this.timers = [];
    this.environment?.cancel();
    this.account?.cancel();
    for (const subscription of this.subscriptions.values()) subscription.cancel();
    this.subscriptions.clear();
    this.controller.detach("Cloud connection closed");
    this.socket.close();
  }
}
