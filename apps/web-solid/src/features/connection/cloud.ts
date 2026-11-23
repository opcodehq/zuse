import { uuid } from "@/core/util";
// A cloud workspace uses the same UI and RPC framing as a local engine. The
// cookie-authenticated controller owns credentials and forwards the RPC stream.
import { ConnectionBlocked } from "@/core/connection";
import { M } from "@/core/methods";
import { isRead, RpcClient, type Subscription } from "@/core/rpc";
import { OwnerTransport, type Transport } from "@/core/transport";

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
export class CloudTransport implements Transport {
  onMessage: ((text: string) => void) | null = null;
  onClose: ((reason: string) => void) | null = null;
  private socket = new OwnerTransport();
  private controller = new RpcClient();
  private subscriptions = new Map<number, Subscription>();
  private environment: Subscription | undefined;
  private account: Subscription | undefined;
  private closed = false;
  private lose(reason: string) {
    if (this.closed) return;
    this.close();
    this.onClose?.(reason);
  }
  constructor(private target: CloudTarget) {}
  private bound() { return { workspaceId: this.target.workspaceId, expectedAccountId: this.target.accountId, expectedOrganizationId: this.target.organizationId }; }
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
      await new Promise<void>((resolve, reject) => {
        let ready = false;
        this.environment = this.controller.subscribe<{ state: string; message: string }>(M.CLOUD_ENVIRONMENT, { ...this.bound(), interactiveResume: true }, value => {
          if (value.state === "ready") { ready = true; resolve(); }
          else if (ready) this.lose(value.message || "Cloud workspace disconnected");
          else if (["failed", "blocked", "paused"].includes(value.state)) reject(new Error(value.message));
        }, error => { if (ready) this.lose(error?.message ?? "Cloud environment ended"); else reject(error ?? new Error("Cloud environment ended")); });
      });
    } catch (error) { this.close(); throw error; }
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
    this.environment?.cancel();
    this.account?.cancel();
    for (const subscription of this.subscriptions.values()) subscription.cancel();
    this.subscriptions.clear();
    this.controller.detach("Cloud connection closed");
    this.socket.close();
  }
}
