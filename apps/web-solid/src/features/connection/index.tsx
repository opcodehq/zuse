// Connection gate: chooses the transport (owner, paired, account), shows connecting, authentication,
// pairing and reconnect states, and keeps the workspace visible with a banner once it has loaded.
import "./connection.css";
import "./target";
import { For, type JSX, lazy, Match, Show, Switch } from "solid-js";
import { retryConnection } from "@/core/connection";
import { setState, state } from "@/core/store";
import { Icon } from "@/ui/Icon";
import { Spinner } from "@/ui/primitives";
import { returnToController } from "./account";
import { forgetCurrent, linkFor, openInThisTab, savedComputers, selectedLink } from "./computers";
import { AddComputer } from "./add";
import { retryPaired } from "./paired";
import { area } from "./status";
import { Btn } from "./ui";

// Only Settings → Remote access shows it, so it loads with that page.
export const DevicesSettings = lazy(() => import("./devices").then((m) => ({ default: m.DevicesSettings })));
export { AccountSignIn, signInToAccount, signOutOfAccount } from "./signin";
export { AddComputer, addComputer } from "./add";

const reconnectingText = () =>
  area.mode === "paired" ? "Reconnecting to paired computer…" : area.mode === "account" ? "Reconnecting to account computer…" : "Reconnecting to your engine…";

export function ConnectionGate(props: { children: JSX.Element }) {
  const ready = () => state.connection.kind === "connected" || state.workspaceSynced;
  return (
    <div class="cx cx-gate">
      <Show when={ready()}>
        <Banners />
      </Show>
      <Show when={ready()} fallback={<Screen />}>
        <div class="cx-gate-frame">{props.children}</div>
      </Show>
    </div>
  );
}

function Banners() {
  const status = () => state.connection;
  const message = () => ("message" in status() ? (status() as { message?: string }).message : undefined);
  const shown = () => status().kind !== "connected" || area.link?.state === "reattaching" || state.uncertainMutation;
  return (
    <Show when={shown()}>
      <div class="cx-banners">
        <Switch>
          <Match when={status().kind === "reconnecting" || status().kind === "connecting"}>
            <div class="cx-banner" role="status">
              <Spinner label="Reconnecting" tone="muted" cell={1.5} />
              <span class="cx-banner-text" title={message()}>
                {reconnectingText()} Local edits are preserved; actions are not resent.
              </span>
              <Btn size="sm" tone="quiet" onClick={() => retryConnection()}>Try again</Btn>
            </div>
          </Match>
          <Match when={status().kind === "authRequired" || status().kind === "pairingRequired" || status().kind === "accountRequired"}>
            <div class="cx-banner" data-tone="warning" role="alert">
              <Icon name="danger-triangle" size={14} />
              <span class="cx-banner-text" title={message()}>{blockedTitle()} · {message() ?? blockedText()}</span>
              <BlockedActions />
            </div>
          </Match>
          <Match when={area.link?.state === "reattaching"}>
            <div class="cx-banner" role="status">
              <Spinner label="Reconnecting" tone="muted" cell={1.5} />
              <span class="cx-banner-text">Reconnecting to paired computer… Requests continue where they stopped.</span>
              <Btn size="sm" tone="quiet" onClick={() => retryPaired()}>Try again</Btn>
            </div>
          </Match>
        </Switch>
        <Show when={state.uncertainMutation}>
          <div class="cx-banner" data-tone="warning" role="alert">
            <Icon name="danger-triangle" size={14} />
            <span class="cx-banner-text">An action may have completed before the connection closed. Check its current state before trying it again.</span>
            <Btn size="sm" tone="quiet" onClick={() => setState("uncertainMutation", false)}>Dismiss</Btn>
          </div>
        </Show>
      </div>
    </Show>
  );
}

function blockedTitle() {
  switch (state.connection.kind) {
    case "pairingRequired":
      return "Pairing required";
    case "accountRequired":
      return "Account access required";
    default:
      return "Authentication required";
  }
}

function blockedText() {
  switch (state.connection.kind) {
    case "pairingRequired":
      return "Open a new invitation from the other computer.";
    case "accountRequired":
      return "Return to the controller and select this computer again.";
    default:
      return "Open the access link printed by zuse serve.";
  }
}

function BlockedActions() {
  return (
    <Switch>
      <Match when={state.connection.kind === "accountRequired"}>
        <Btn size="sm" onClick={() => returnToController()}>Return to controller</Btn>
      </Match>
      <Match when={state.connection.kind === "pairingRequired" && selectedLink()}>
        <Btn size="sm" tone="quiet" onClick={() => retryConnection()}>Try again</Btn>
        <Btn size="sm" onClick={() => forgetCurrent()}>Forget pairing</Btn>
      </Match>
    </Switch>
  );
}

// Full-window gate, from desktop's gate cards (shell.rs OrgGate and signed-out cards).
function Screen() {
  const message = () => (state.connection as { message?: string }).message;
  return (
    <main class="cx-screen">
      <div class="cx-screen-body" role="group" aria-labelledby="cx-screen-title">
        <span class="cx-screen-mark"><Icon name="zuse-logo" size={36} /></span>
        <Switch>
          <Match when={state.connection.kind === "connecting" || state.connection.kind === "connected"}>
            <h1 class="cx-screen-title" id="cx-screen-title">Connecting to your environment…</h1>
            <p class="cx-screen-text">Authentication and connection recovery happen automatically.</p>
            <div class="cx-screen-actions"><Spinner label="Connecting" /></div>
          </Match>
          <Match when={state.connection.kind === "reconnecting"}>
            <h1 class="cx-screen-title" id="cx-screen-title">{reconnectingText()}</h1>
            <p class="cx-screen-text">Zuse keeps trying. Check that the computer is on and reachable.</p>
            <Show when={message()}>{(text) => <p class="cx-screen-detail">{text()}</p>}</Show>
            <div class="cx-screen-actions" data-row="true">
              <Btn size="lg" onClick={() => retryConnection()}>Try again</Btn>
              <Spinner label="Reconnecting" />
            </div>
          </Match>
          <Match when={state.connection.kind === "accountRequired"}>
            <h1 class="cx-screen-title" id="cx-screen-title">Account access required</h1>
            <p class="cx-screen-text">{message() ?? blockedText()}</p>
            <div class="cx-screen-actions" data-row="true">
              <Btn size="lg" tone="solid" onClick={() => returnToController()}>Return to controller</Btn>
            </div>
          </Match>
          <Match when={state.connection.kind === "pairingRequired"}>
            <h1 class="cx-screen-title" id="cx-screen-title">Pairing required</h1>
            <p class="cx-screen-text">{message() ?? blockedText()}</p>
            <div class="cx-screen-actions">
              <AddComputer />
              <Show when={selectedLink()}>
                <div class="cx-screen-row">
                  <Btn size="lg" onClick={() => retryConnection()}>Try again</Btn>
                  <Btn size="lg" tone="quiet" onClick={() => forgetCurrent()}>Forget pairing</Btn>
                </div>
              </Show>
            </div>
          </Match>
          <Match when={state.connection.kind === "authRequired"}>
            <h1 class="cx-screen-title" id="cx-screen-title">Authentication required</h1>
            <p class="cx-screen-text">{message() ?? blockedText()}</p>
            <div class="cx-screen-actions">
              <SavedList />
              <AddComputer />
            </div>
          </Match>
        </Switch>
      </div>
    </main>
  );
}

function SavedList() {
  return (
    <Show when={savedComputers().length > 0}>
      <div class="cx-screen-list" role="list" aria-label="Computers in this browser">
        <For each={savedComputers()}>
          {(computer) => (
            <div class="cx-row" role="listitem">
              <span class="cx-glyph"><Icon name="monitor" size={16} /></span>
              <div class="cx-details">
                <div class="cx-details-title">{computer.origin}</div>
                <div class="cx-details-text">Saved in this browser</div>
              </div>
              <Btn aria-label={`Open ${computer.origin}`} onClick={() => openSaved(computer.id)}>Open</Btn>
            </div>
          )}
        </For>
      </div>
    </Show>
  );
}

// The saved list holds only public metadata; the capability is read from storage on demand.
function openSaved(id: string) {
  const link = linkFor(id);
  if (link) openInThisTab(link);
}
