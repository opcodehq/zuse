// Status, setup and removal surfaces shared by the browse page, the detail page and the installed list.
import { createSignal, Show } from "solid-js";
import { Icon } from "@/ui/Icon";
import { GlyphSpinner } from "@/ui/loaders";
import { Button, Dialog, IconButton } from "@/ui/primitives";
import { accountError, accountSession, login, signIn, signedIn } from "../account";
import { ErrorStrip, Segmented } from "../ui";
import { catalog, type PluginDefinition } from "./catalog";
import { LOCAL_CONFIG_TEMPLATE, connectRequest, localRequest, serverRequest, type ServerAuth } from "./setup";
import {
  activeAttempt,
  cancelAttempt,
  canManage,
  clearError,
  closeSetup,
  confirmRemoval,
  dismissRemoval,
  plugins,
  submitSetup,
  tenant,
} from "./state";

export function LoginPrompt() {
  return (
    <Show when={login()}>
      {(progress) => (
        <div class="pc-strip" role="status">
          <div class="pc-strip-copy">
            <strong>Finish signing in</strong>
            <span>{progress().userCode ? `Confirm the code ${progress().userCode} in your browser.` : "Complete sign-in in your browser."}</span>
          </div>
          <a class="btn" data-variant="outline" data-size="md" href={progress().url} target="_blank" rel="noopener noreferrer">
            Continue in browser
          </a>
        </div>
      )}
    </Show>
  );
}

/** Progress, notices and prompts both plugin views show above their lists. */
export function PluginStatus() {
  const error = () => plugins.error ?? accountError();
  const attemptName = () => {
    const id = plugins.attempt?.pluginId ?? "";
    return catalog(plugins.snapshot).find((p) => p.id === id)?.name ?? "plugin";
  };
  const authorization = () => {
    const url = plugins.attempt?.authorizationUrl;
    return url && url.startsWith("https://") ? url : null;
  };
  return (
    <>
      <LoginPrompt />
      <Show when={error()}>{(text) => <ErrorStrip onDismiss={plugins.error ? clearError : undefined}>{text()}</ErrorStrip>}</Show>
      <Show when={plugins.notice}>
        {(notice) => (
          <div class="pc-notice" role="status">
            {notice()}
          </div>
        )}
      </Show>
      <Show when={signedIn() && !canManage()}>
        <div class="pc-notice">Only workspace administrators can manage these connections.</div>
      </Show>
      <Show when={activeAttempt()}>
        <div class="pc-pending" role="status">
          <GlyphSpinner decorative />
          <span class="pc-pending-copy">Finish signing in to {attemptName()} in your browser. Zuse picks it up automatically.</span>
          <Show when={authorization()}>
            {(url) => (
              <a class="pc-pending-button" href={url()} target="_blank" rel="noopener noreferrer">
                Reopen browser
              </a>
            )}
          </Show>
          <button type="button" class="pc-pending-button" disabled={plugins.busy} onClick={cancelAttempt}>
            Cancel
          </button>
        </div>
      </Show>
      <ServerSetupDialog />
      <RemovalDialog />
    </>
  );
}

export function SignedOut() {
  return (
    <div class="pc-signed-out">
      <p>Sign in to connect your tools and use them across your agents.</p>
      <Button variant="outline" disabled={accountSession.busy || !!login()} onClick={() => void signIn()}>
        {accountSession.busy ? "Signing in…" : "Sign in"}
      </Button>
    </div>
  );
}

const AUTH_ITEMS: { value: ServerAuth; label: string }[] = [
  { value: "automatic", label: "Sign in or none" },
  { value: "bearer", label: "Bearer token" },
  { value: "header", label: "Custom header" },
];

function ServerSetupDialog() {
  const open = () => plugins.setup?.kind === "server";
  return (
    <Show when={open()}>
      <ServerForm />
    </Show>
  );
}

function ServerForm() {
  const target = tenant() ?? "";
  const personal = target.startsWith("personal:");
  const [local, setLocal] = createSignal<"remote" | "local">("remote");
  const [name, setName] = createSignal("");
  const [url, setUrl] = createSignal("");
  const [auth, setAuth] = createSignal<ServerAuth>("automatic");
  const [header, setHeader] = createSignal("");
  const [secret, setSecret] = createSignal("");
  const [config, setConfig] = createSignal(LOCAL_CONFIG_TEMPLATE);
  const [error, setError] = createSignal<string | null>(null);
  const organization = () => plugins.snapshot?.tenants.find((t) => t.id === target)?.name ?? "organization";
  const submit = (event?: Event) => {
    event?.preventDefault();
    if (plugins.busy) return;
    const result =
      local() === "local"
        ? localRequest(target, { name: name(), config: config() })
        : serverRequest(target, { name: name(), url: url(), auth: auth(), header: header(), secret: secret() });
    if ("error" in result) return setError(result.error);
    setError(null);
    submitSetup(result.ok);
  };
  const edited = <T,>(set: (value: T) => void) => (value: T) => {
    set(value);
    setError(null);
  };
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !plugins.busy && closeSetup()}
      title={local() === "local" ? "Add local MCP server" : "Add MCP server"}
      description={personal ? "Adding to your personal workspace." : `Adding to the ${organization()} organization. Members can use it once it's connected.`}
    >
      <form class="pc-form" onSubmit={submit} autocomplete="off">
        <Show when={personal}>
          <Segmented
            label="Connection type"
            value={local()}
            onChange={edited(setLocal)}
            items={[
              { value: "remote", label: "Remote" },
              { value: "local", label: "Local command" },
            ]}
          />
        </Show>
        <label class="pc-field">
          <span>Name</span>
          <input class="field" value={name()} placeholder="Internal tools" maxLength={120} onInput={(e) => edited(setName)(e.currentTarget.value)} autofocus />
        </label>
        <Show
          when={local() === "remote"}
          fallback={
            <label class="pc-field">
              <span>MCP configuration (JSON)</span>
              <textarea class="field pc-textarea" rows={8} spellcheck={false} value={config()} onInput={(e) => edited(setConfig)(e.currentTarget.value)} />
              <small>
                Runs on the computer hosting this Zuse engine, in your personal workspace. Paste one stdio server configuration, including command, args, and optional env. Desktop apps a server
                depends on must be installed there.
              </small>
            </label>
          }
        >
          <label class="pc-field">
            <span>Server URL</span>
            <input class="field pc-mono" type="url" inputMode="url" value={url()} placeholder="https://mcp.example.com/mcp" onInput={(e) => edited(setUrl)(e.currentTarget.value)} />
          </label>
          <div class="pc-field">
            <span id="plugin-auth-label">Authentication</span>
            <Segmented label="Authentication" value={auth()} onChange={edited(setAuth)} items={AUTH_ITEMS} />
            <Show when={auth() === "automatic"}>
              <small>Zuse detects whether the server needs you to sign in or is open to everyone.</small>
            </Show>
          </div>
          <Show when={auth() === "header"}>
            <label class="pc-field">
              <span>Header name</span>
              <input class="field pc-mono" value={header()} placeholder="X-API-Key" onInput={(e) => edited(setHeader)(e.currentTarget.value)} />
            </label>
          </Show>
          <Show when={auth() !== "automatic"}>
            <label class="pc-field">
              <span>{auth() === "bearer" ? "Token" : "Value"}</span>
              <input class="field pc-mono" type="password" autocomplete="new-password" value={secret()} placeholder="API key" onInput={(e) => edited(setSecret)(e.currentTarget.value)} />
              <small>Keys are encrypted on Zuse servers and never reach agent machines.</small>
            </label>
          </Show>
        </Show>
        <Show when={error()}>
          {(text) => (
            <div class="pc-form-error" role="alert">
              {text()}
            </div>
          )}
        </Show>
        <div class="dialog-footer">
          <Button variant="ghost" disabled={plugins.busy} onClick={closeSetup}>
            Cancel
          </Button>
          <button type="submit" class="pc-action" data-primary="true" disabled={plugins.busy}>
            <span class="pc-action-label">{plugins.busy ? "Working…" : "Add server"}</span>
          </button>
        </div>
      </form>
    </Dialog>
  );
}

/** The API-key row that replaces "Connect another account" on a keyed plugin's detail page. */
export function InlineKey(props: { plugin: PluginDefinition; label: string }) {
  const [secret, setSecret] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const submit = (event: Event) => {
    event.preventDefault();
    const target = tenant();
    if (!target || plugins.busy) return;
    const result = connectRequest(target, props.plugin, props.label, secret());
    if ("error" in result) return setError(result.error);
    submitSetup(result.ok);
  };
  return (
    <form class="pc-inline-key" onSubmit={submit} onKeyDown={(e) => e.key === "Escape" && closeSetup()}>
      <span class="pc-add-glyph" aria-hidden="true">
        <Icon name="plus" size={12} />
      </span>
      <input
        class="pc-inline-input"
        classList={{ "pc-mono": secret().length > 0 }}
        type="password"
        autocomplete="new-password"
        aria-label="API key"
        placeholder="Key for this account"
        value={secret()}
        autofocus
        onInput={(e) => {
          setSecret(e.currentTarget.value);
          setError(null);
        }}
      />
      <Button variant="ghost" onClick={closeSetup}>
        Cancel
      </Button>
      <button type="submit" class="pc-action" data-primary="true" disabled={plugins.busy || !secret().trim()}>
        <span class="pc-action-label">Connect</span>
      </button>
      <Show when={error()}>
        {(text) => (
          <div class="pc-form-error pc-inline-error" role="alert">
            {text()}
          </div>
        )}
      </Show>
    </form>
  );
}

function RemovalDialog() {
  const confirmation = () => plugins.confirmation;
  return (
    <Show when={confirmation()}>
      {(confirmed) => (
        <Dialog
          open
          onOpenChange={(next) => !next && !plugins.busy && dismissRemoval()}
          title={confirmed().kind === "server" ? `Remove ${confirmed().label}?` : "Disconnect plugin?"}
          description={
            confirmed().kind === "server"
              ? "This disconnects every account and removes the server from this workspace. Agents lose access right away."
              : `Agents lose access to ${confirmed().label} right away.`
          }
          footer={
            <>
              <Button variant="ghost" disabled={plugins.busy} onClick={dismissRemoval}>
                Cancel
              </Button>
              <Button variant="danger" class="pc-danger-solid" disabled={plugins.busy} onClick={confirmRemoval}>
                {plugins.busy ? "Removing…" : confirmed().kind === "server" ? "Remove server" : "Disconnect"}
              </Button>
            </>
          }
        >
          <Show when={plugins.error ?? plugins.notice}>
            {(text) => (
              <div class="pc-form-error" role="alert">
                {text()}
              </div>
            )}
          </Show>
        </Dialog>
      )}
    </Show>
  );
}

export function Refresh(props: { onClick: () => void; busy: boolean; label: string }) {
  return <IconButton icon="refresh" label={props.label} disabled={props.busy} onClick={() => props.onClick()} busy={props.busy} />;
}

