// Cloud workspaces page (crates/ui/src/settings/cloud_billing.rs): account gate, hosting, then Setup / Usage as on
// desktop. Chats is web-only: desktop starts cloud chats from its composer and lists them in the sidebar.
import { createEffect, createSignal, Match, on, Show, Switch } from "solid-js";
import { accountError, accountSession, login, owner, ownerKey, signIn, signedIn } from "../account";
import { PageTitle, Segmented } from "../ui";
import { Archives } from "./Archives";
import { Hosting, Usage } from "./Billing";
import { Chats } from "./Chats";
import { History } from "./History";
import { ImageStorage, Setup } from "./Setup";
import { cloud, loadBilling } from "./state";

export type CloudTab = "setup" | "usage" | "chats";
const [tab, setTab] = createSignal<CloudTab>("setup");
export const selectCloudTab = (value: CloudTab) => setTab(value);
// The tab resets when the account changes, not each time the page mounts.
let shownFor = "";

export function CloudSettings() {
  createEffect(
    on(
      () => ownerKey(owner()),
      (key) => {
        if (!key) return;
        if (shownFor && shownFor !== key) setTab("setup");
        shownFor = key;
        if (!cloud.billing.loaded) void loadBilling();
      },
    ),
  );
  const choose = (value: CloudTab) => {
    setTab(value);
    if (value === "usage") void loadBilling();
  };
  return (
    <Show when={!cloud.history.workspaceId} fallback={<History />}>
      <div class="pc-page">
        <PageTitle title="Cloud workspaces (Beta)" subtitle="Connect GitHub and your coding agents, then keep work running when this app is closed." />
        <Show when={signedIn()} fallback={<AccountGate />}>
          <Hosting />
          <Show when={cloud.billing.subscribed === true}>
            <Segmented
              class="pc-cloud-tabs"
              label="Cloud workspace settings"
              value={tab()}
              onChange={choose}
              items={[
                { value: "setup", label: "Setup" },
                { value: "usage", label: "Usage" },
                { value: "chats", label: "Chats" },
              ]}
            />
            <Switch>
              <Match when={tab() === "chats"}>
                <Chats />
              </Match>
              <Match when={tab() === "setup"}>
                <Setup />
              </Match>
              <Match when={tab() === "usage"}>
                <ImageStorage />
                <Usage />
              </Match>
            </Switch>
          </Show>
          <Show when={!owner()?.organizationId}>
            <Archives />
          </Show>
        </Show>
      </div>
    </Show>
  );
}

function AccountGate() {
  const label = () => (accountSession.busy ? "Signing in…" : login() ? "Continue sign-in" : "Sign in");
  return (
    <div class="pc-gate">
      <div class="pc-gate-copy">
        <strong>Sign in to set up cloud workspaces</strong>
        <span>Choose a plan, connect repositories, and authorize your coding agents from one account.</span>
        <small>Local chats stay on this computer and remain available without an account.</small>
        <Show when={accountError()}>
          {(error) => (
            <span class="pc-error-text" role="alert">
              {error()}
            </span>
          )}
        </Show>
        <Show when={login()?.userCode}>{(code) => <span>Confirm the code {code()} in your browser.</span>}</Show>
      </div>
      <button type="button" class="pc-action" data-primary="true" disabled={accountSession.busy} onClick={() => void signIn()}>
        <span class="pc-action-label">{label()}</span>
      </button>
    </div>
  );
}
