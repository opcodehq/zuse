import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import { accountError, accountLoaded, accountSession, bound, boundReply, errorText, login, newId, owner, ownerKey, signIn, type Owner } from "@/features/plugins-cloud/account";
import { connectionMethods, connectionName, connectionProviders, operationActive, orderedConnections, type AgentConnection, type ConnectionAgent, type ConnectionList, type ConnectionMethod, type LoginOperation } from "@/features/plugins-cloud/cloud/connections";
import { harnessIcon } from "@/ui/harness";
import { Icon } from "@/ui/Icon";
import { IconButton, Menu } from "@/ui/primitives";
import { createEffect, createMemo, createSignal, For, onCleanup, Show } from "solid-js";
import { ActionButton, Card, CopyField, ErrorStrip, InlineSpinner, Mark, Row, SectionTitle, Select, SettingsDialog, Switch } from "../widgets";
import "./connections.css";

const AGENTS: ConnectionAgent[] = ["claude", "codex", "grok"];
const methodsFor = (agent: ConnectionAgent): ConnectionMethod[] =>
  agent === "claude" ? ["setup-token", "api-key"] : ["device-code", "api-key"];
const alreadyConnected = (code?: string | null) => code === "agent_already_connected";

type Dialog =
  | { kind: "connect"; agent: ConnectionAgent; method: ConnectionMethod; row?: AgentConnection }
  | { kind: "rename"; row: AgentConnection }
  | { kind: "remove"; row: AgentConnection };

export function Connections() {
  const who = createMemo(owner);
  return (
    <>
      <SectionTitle title="Your connections" description="Threads you own run on these first, in this order" />
      <Show when={who()} keyed fallback={
        <div class="st-connections">
          <Card>
            <Show when={accountLoaded()} fallback={<Row title={<InlineSpinner label="Checking your account…" />} />}>
              <Row title="Sign in to manage connections" description="Cloud connections are saved to your Zuse account."
                children={<ActionButton disabled={accountSession.busy} onClick={() => void signIn()}>{login() ? "Continue sign-in" : "Sign in"}</ActionButton>} />
              <Show when={login()?.userCode}>{(code) => <Row title="Confirm this code in your browser"><CopyField value={code()} label="Copy sign-in code" /></Row>}</Show>
              <Show when={accountError()}>{(message) => <ErrorStrip>{message()}</ErrorStrip>}</Show>
            </Show>
          </Card>
        </div>
      }>
        {(current) => <ConnectionSettings who={current} />}
      </Show>
    </>
  );
}

function ConnectionSettings(props: { who: Owner }) {
  const [list, setList] = createSignal<ConnectionList>();
  const [listError, setListError] = createSignal("");
  const [actionError, setActionError] = createSignal("");
  const [notice, setNotice] = createSignal("");
  const [loading, setLoading] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [dialog, setDialog] = createSignal<Dialog>();
  const [input, setInput] = createSignal("");
  const [dialogError, setDialogError] = createSignal("");
  const [operation, setOperation] = createSignal<LoginOperation>();
  let requestKey = "";
  let loadVersion = 0;
  let disposed = false;
  onCleanup(() => { disposed = true; });

  const isPersonal = () => !props.who.organizationId;
  const personal = createMemo(() => orderedConnections((isPersonal() ? list()?.connections : list()?.personalConnections) ?? []));
  const organization = createMemo(() => orderedConnections((isPersonal() ? [] : list()?.connections) ?? []));
  const phase = () => {
    const value = dialog();
    return value?.kind === "connect" ? value : undefined;
  };
  const rowName = () => {
    const row = dialog()?.row;
    return row ? connectionName(row) : "";
  };

  async function call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    const who = props.who;
    if (ownerKey(owner()) !== ownerKey(who)) throw new Error("Your account changed. Reopen this view.");
    const reply = await rpc.call(method, bound(who, params));
    if (!boundReply(reply, who) || ownerKey(owner()) !== ownerKey(who) || disposed) {
      throw new Error("Your account changed. Reopen this view.");
    }
    return reply.data as T;
  }

  async function refresh() {
    const version = ++loadVersion;
    setLoading(true);
    try {
      const result = await call<ConnectionList>(M.CLOUD_AGENT_CONNECTIONS);
      if (disposed || version !== loadVersion) return;
      if (!Array.isArray(result?.connections)) throw new Error("The connection list could not be read. Try again.");
      setList(result);
      setListError("");
    } catch (error) {
      if (!disposed && version === loadVersion) setListError(errorText(error));
    } finally {
      if (!disposed && version === loadVersion) setLoading(false);
    }
  }

  createEffect(() => state.epoch, () => { void refresh(); });
  createEffect(() => personal().some((row) => row.state === "pending" || row.pendingLoginOperationId || operationActive(row.credentialUpdate)), (pending) => {
    if (!pending) return;
    const timer = setInterval(() => {
      if (!loading() && !busy()) void refresh();
    }, 2000);
    onCleanup(() => clearInterval(timer));
  });
  createEffect(() => operationActive(operation()) ? operation()?.operationId : undefined, (operationId) => {
    if (!operationId) return;
    let polling = false;
    const timer = setInterval(async () => {
      if (polling || busy()) return;
      polling = true;
      try {
        const next = await call<LoginOperation>(M.CLOUD_AGENT_LOGIN_STATUS, { operationId });
        if (disposed || operation()?.operationId !== operationId || !operationActive(operation()) || busy()) return;
        setDialogError("");
        acceptOperation(next);
        if (!operationActive(next)) void refresh();
      } catch (error) {
        if (!disposed && operation()?.operationId === operationId) setDialogError(errorText(error));
      } finally {
        polling = false;
      }
    }, 2000);
    onCleanup(() => clearInterval(timer));
  });

  function edit(value?: Dialog) {
    if (busy()) return;
    setDialog(value);
    setInput(value?.kind === "rename" ? value.row.alias ?? "" : "");
    setOperation(undefined);
    setDialogError("");
    setNotice("");
    requestKey = newId();
  }

  function acceptOperation(next: LoginOperation) {
    setOperation(next);
    if (operationActive(next)) return;
    if (next.state === "connected") {
      setDialog(undefined);
      setOperation(undefined);
      setNotice("Connection verified.");
    } else if (alreadyConnected(next.errorCode)) {
      setDialogError("Already connected. This provider account is already in your list.");
    } else if (next.state === "expired") {
      setDialogError("The verification expired. Try again.");
    } else if (next.state === "cancelled") {
      setDialogError("Verification was canceled.");
    } else {
      setDialogError("Could not verify this connection. Check the credential and try again.");
    }
  }

  async function mutate<T>(method: string, params: Record<string, unknown>, onSuccess?: (value: T) => void) {
    if (busy()) return;
    const inDialog = !!dialog();
    setBusy(true);
    setNotice("");
    setActionError("");
    setDialogError("");
    try {
      const value = await call<T>(method, params);
      onSuccess?.(value);
      await refresh();
    } catch (error) {
      if (disposed) return;
      (inDialog ? setDialogError : setActionError)(errorText(error));
      void refresh();
    } finally {
      if (!disposed) setBusy(false);
    }
  }

  function connect(row: AgentConnection) {
    edit({ kind: "connect", agent: row.agent, method: row.method, row });
    const update = row.credentialUpdate;
    if (update && operationActive(update)) setOperation(update);
    else if (row.pendingLoginOperationId) {
      setOperation({ operationId: row.pendingLoginOperationId, connectionId: row.connectionId, agent: row.agent, state: "queued" });
    }
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    const value = dialog();
    if (!value || busy() || operationActive(operation())) return;
    const entered = input().trim();
    if (value.kind === "rename" || value.kind === "remove") {
      const connectionId = value.row.connectionId;
      if (value.kind === "rename") {
        void mutate(M.CLOUD_AGENT_CONNECTION_UPDATE, { connectionId, alias: entered }, () => setDialog(undefined));
      } else {
        void mutate(M.CLOUD_AGENT_DISCONNECT, { connectionId }, () => setDialog(undefined));
      }
      return;
    }
    if (value.method !== "device-code" && !entered) return;
    if (operation()) {
      requestKey = newId();
      setOperation(undefined);
    }
    const params = { agent: value.agent, connectionId: value.row?.connectionId ?? null, idempotencyKey: requestKey };
    setInput("");
    if (value.method === "device-code") {
      void mutate<LoginOperation>(M.CLOUD_AGENT_LOGIN, params, acceptOperation);
    } else {
      void mutate<AgentConnection>(M.CLOUD_AGENT_CONFIGURE, { ...params, method: value.method, secret: entered }, (result) => {
        if (result.credentialUpdate) acceptOperation(result.credentialUpdate);
        else setDialog(undefined);
      });
    }
  }

  function closeDialog() {
    edit();
  }

  const canReconnect = (row: AgentConnection) =>
    row.method === "device-code" ? list()?.supportsDeviceReconnect === true : list()?.supportsCredentialReconfiguration === true;
  const title = () => dialog()?.kind === "rename" ? "Rename connection" : dialog()?.kind === "remove" ? "Remove connection"
    : phase()?.row ? (phase()?.method === "device-code" ? "Reconnect" : `Replace ${connectionMethods[phase()!.method].toLowerCase()}`)
    : "Add a connection";

  return (
    <div class="st-connections">
      <Show when={listError()}>{(message) => <ErrorStrip action={<ActionButton onClick={() => void refresh()}>Retry</ActionButton>}>{message()}</ErrorStrip>}</Show>
      <Show when={actionError()}>{(message) => <ErrorStrip>{message()}</ErrorStrip>}</Show>
      <Show when={notice()}><p class="st-connection-notice" role="status">{notice()}</p></Show>
      <Card>
        <div class="st-connection-head">
          <h2>{list() ? `${personal().length} ${personal().length === 1 ? "connection" : "connections"}` : "Personal connections"}</h2>
          <IconButton icon="refresh" size="sm" label="Refresh connections" disabled={loading() || busy()} onClick={() => void refresh()} />
          <Show when={isPersonal()}>
            <ActionButton icon="plus" disabled={!list() || busy()} onClick={() => edit({ kind: "connect", agent: "claude", method: "setup-token" })}>Add another</ActionButton>
          </Show>
        </div>
        <Show when={list() && !isPersonal()}><Row title="Switch to your personal account to manage these connections." /></Show>
        <Show when={list()} fallback={<Show when={loading()}><Row title={<InlineSpinner label="Loading connections…" />} /></Show>}>
          <ol class="st-connection-list">
            <For each={personal()} keyed={(row) => row.connectionId} fallback={<li><Row title="No personal connections yet" description="Add a Claude, Codex, or Grok account to use in cloud chats." /></li>}>
              {(row, index) => <ConnectionRow row={row()} editable={isPersonal()} busy={busy()} index={index()} count={personal().length}
                canReconnect={canReconnect(row())} onMove={(position) => void mutate(M.CLOUD_AGENT_CONNECTION_UPDATE, { connectionId: row().connectionId, position })}
                onToggle={(enabled) => void mutate(M.CLOUD_AGENT_CONNECTION_UPDATE, { connectionId: row().connectionId, enabled })}
                onConnect={() => connect(row())}
                onRename={() => edit({ kind: "rename", row: row() })}
                onRemove={() => edit({ kind: "remove", row: row() })} />}
            </For>
          </ol>
        </Show>
      </Card>
      <section class="st-connection-org" aria-labelledby="st-org-connections">
        <h2 id="st-org-connections">Organization connections</h2>
        <p>Connections managed by your organization.</p>
        <Card>
          <Show when={!isPersonal()} fallback={<Row title="No organization selected" description="Organization connections appear when you’re in an organization." />}>
            <Show when={list()} fallback={<Show when={loading()}><Row title="Loading organization connections…" /></Show>}>
              <ol class="st-connection-list">
                <For each={organization()} keyed={(row) => row.connectionId} fallback={<li><Row title="No organization connections yet" /></li>}>
                  {(row) => <ConnectionRow row={row()} />}
                </For>
              </ol>
            </Show>
          </Show>
        </Card>
      </section>
      <SettingsDialog open={!!dialog()} onClose={closeDialog} title={title()}
        description={dialog()?.kind === "remove" ? `Remove ${rowName()}? Cloud chats that use this connection will need to reconnect.`
          : dialog()?.kind === "rename" ? "Leave the alias blank to use the provider’s account name."
          : phase()?.row ? rowName() : "Choose a provider and how you want to sign in."}>
        <form class="st-connection-form" onSubmit={submit}>
          <Show when={dialog()?.kind === "rename"}>
            <label class="st-connection-field">Alias
              <input class="field" value={input()} maxlength={120} onInput={(e) => setInput(e.currentTarget.value)} />
            </label>
          </Show>
          <Show when={phase()}>
            {(flow) => <>
              <Show when={!flow().row && !operationActive(operation())}>
                <div class="st-connection-form-choices">
                  <div class="st-connection-field"><span>Provider</span>
                    <Select label="Provider" value={flow().agent} options={AGENTS.map((agent) => ({ value: agent, label: connectionProviders[agent] }))} disabled={busy()}
                      onChange={(agent) => edit({ kind: "connect", agent, method: methodsFor(agent)[0]! })} />
                  </div>
                  <div class="st-connection-field"><span>Method</span>
                    <Select label="Method" value={flow().method} options={methodsFor(flow().agent).map((method) => ({ value: method, label: connectionMethods[method] }))} disabled={busy()}
                      onChange={(method) => edit({ ...flow(), method })} />
                  </div>
                </div>
              </Show>
              <Show when={operationActive(operation())} fallback={
                <Show when={flow().method !== "device-code"}>
                  <label class="st-connection-field">{connectionMethods[flow().method]}
                    <input class="field" type="password" autocomplete="off" spellcheck="false" maxlength={32768} value={input()} disabled={busy()}
                      onInput={(event) => setInput(event.currentTarget.value)} required />
                  </label>
                  <Show when={flow().method === "setup-token"}><p class="st-desc">Run <code>claude setup-token</code> in a terminal, then paste the token here.</p></Show>
                </Show>
              }>
                <div class="st-connection-verification">
                  <span class="st-connection-progress" role="status">
                    <InlineSpinner label="Verification in progress" />
                    {operation()?.verificationUrl ? "Waiting for sign-in…" : "Verifying…"}
                  </span>
                  <Show when={operation()?.verificationCode}><p class="st-desc">Copy this code, then open the provider’s sign-in page.</p></Show>
                  <Show when={operation()?.verificationCode}>{(code) => <CopyField label="Copy verification code" value={code()} />}</Show>
                  <Show when={operation()?.verificationUrl?.startsWith("https://") ? operation()?.verificationUrl : undefined}>
                    {(url) => <a class="st-action" data-tone="filled" href={url()} target="_blank" rel="noopener noreferrer">Open sign-in page <Icon name="arrow-up-right" size={13} /></a>}
                  </Show>
                </div>
              </Show>
            </>}
          </Show>
          <Show when={dialogError()}>{(message) => <ErrorStrip>{message()}</ErrorStrip>}</Show>
          <div class="dialog-footer">
            <Show when={operationActive(operation())} fallback={
              <>
                <ActionButton tone="quiet" disabled={busy()} onClick={closeDialog}>Cancel</ActionButton>
                <ActionButton type="submit" disabled={busy() || (!!phase() && phase()?.method !== "device-code" && !input().trim())} tone={dialog()?.kind === "remove" ? "danger" : "filled"}>
                  {busy() ? "Working…" : dialog()?.kind === "rename" ? "Save alias" : dialog()?.kind === "remove" ? "Remove" : phase()?.method === "device-code" ? "Start sign-in" : "Verify connection"}
                </ActionButton>
              </>
            }>
              <ActionButton tone="quiet" disabled={busy()} onClick={closeDialog}>Close</ActionButton>
              <ActionButton disabled={busy()} onClick={() => {
                const operationId = operation()?.operationId;
                if (operationId) void mutate<LoginOperation>(M.CLOUD_AGENT_LOGIN_CANCEL, { operationId }, acceptOperation);
              }}>Cancel verification</ActionButton>
            </Show>
          </div>
        </form>
      </SettingsDialog>
    </div>
  );
}

function ConnectionRow(props: {
  row: AgentConnection; editable?: boolean; busy?: boolean; index?: number; count?: number; canReconnect?: boolean;
  onMove?: (position: number) => void; onToggle?: (enabled: boolean) => void; onConnect?: () => void; onRename?: () => void; onRemove?: () => void;
}) {
  const name = () => connectionName(props.row);
  const verification = () => !!props.row.pendingLoginOperationId || operationActive(props.row.credentialUpdate);
  const state = () => verification() ? "pending" : props.row.state;
  const status = () => alreadyConnected(props.row.errorCode) ? "Already connected" : ({
    connected: "Connected", pending: "Verifying", "reconnect-required": "Signed out", error: "Failed", revoked: "Removed",
  })[state()];
  const reconnect = () => (state() === "reconnect-required" || state() === "error") && !alreadyConnected(props.row.errorCode);
  return (
    <li class="st-connection-row" data-editable={props.editable ? "true" : undefined} data-enabled={props.row.enabled !== false ? "true" : "false"}>
      <Show when={props.editable}>
        <div class="st-connection-order">
          <IconButton icon="alt-arrow-up" size="sm" label={`Move ${name()} up`} disabled={props.busy || !props.index || props.row.position === undefined} onClick={() => props.onMove?.(props.index! - 1)} />
          <IconButton icon="alt-arrow-down" size="sm" label={`Move ${name()} down`} disabled={props.busy || props.index === (props.count ?? 0) - 1 || props.row.position === undefined} onClick={() => props.onMove?.(props.index! + 1)} />
        </div>
      </Show>
      <Mark name={harnessIcon(props.row.agent === "claude" ? "claude-code" : props.row.agent)} size={18} />
      <div class="st-connection-identity">
        <strong title={name()}>{name()}</strong>
        <span>{connectionProviders[props.row.agent]} · {connectionMethods[props.row.method]}</span>
      </div>
      <div class="st-connection-state">
        <span class="st-connection-status" data-state={state()}><i aria-hidden="true" />{status()}</span>
        <Show when={props.editable && props.canReconnect && (verification() || reconnect())}>
          <ActionButton tone="quiet" disabled={props.busy} onClick={props.onConnect}>{verification() ? "Continue" : "Reconnect"}</ActionButton>
        </Show>
      </div>
      <Show when={props.editable}>
        <Switch checked={props.row.enabled !== false} label={`Use ${name()} for new cloud chats`} onChange={(enabled) => props.onToggle?.(enabled)} disabled={props.busy || props.row.enabled === undefined} />
        <Menu placement="bottom-end" trigger={(attrs) => <IconButton {...attrs} icon="menu-dots" size="sm" label={`Actions for ${name()}`} disabled={props.busy} />}
          items={[
            { label: "Rename", icon: "pen", disabled: props.row.position === undefined, onSelect: props.onRename },
            { label: verification() ? "View verification" : "Reconnect", icon: "refresh", disabled: !props.canReconnect || (state() === "pending" && !verification()) || alreadyConnected(props.row.errorCode), onSelect: props.onConnect },
            { label: props.row.method === "setup-token" ? "Replace token" : "Replace key", disabled: props.row.method === "device-code" || !props.canReconnect || state() === "pending" || alreadyConnected(props.row.errorCode), onSelect: props.onConnect },
            { separator: true, label: "" },
            { label: "Remove", danger: true, onSelect: props.onRemove },
          ]} />
      </Show>
      <Show when={!props.editable && props.row.enabled === false}><span class="st-desc st-connection-disabled">Disabled</span></Show>
    </li>
  );
}
