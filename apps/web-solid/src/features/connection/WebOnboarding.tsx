// Hosted browsers have their own account and cloud setup; never inspect or import the user's desktop.
import { hostedBrowser } from "@/core/auth";
import { state } from "@/core/store";
import { navigate } from "@/core/view";
import { accountLoaded, accountError, accountSession, login, ownerKey, owner, signedIn, signIn } from "@/features/plugins-cloud/account";
import { Hosting } from "@/features/plugins-cloud/cloud/Billing";
import { Setup } from "@/features/plugins-cloud/cloud/Setup";
import { Chats } from "@/features/plugins-cloud/cloud/Chats";
import { cloud, loadBilling, loadContext } from "@/features/plugins-cloud/cloud/state";
import { Icon } from "@/ui/Icon";
import type { JSX } from "@solidjs/web";
import { createEffect, createSignal, For, Match, Show, Switch } from "solid-js";
import "@/features/plugins-cloud/plugins-cloud.css";
import "./onboarding.css";

const steps = ["Welcome", "Account", "Cloud plan", "Repositories", "First chat"];
export function WebOnboarding(props: { children: JSX.Element }) {
  const [step, setStep] = createSignal(0);
  const [dismissed, setDismissed] = createSignal(false);
  const connected = () => state.connection.kind === "connected";
  const visible = () => hostedBrowser() && !location.hash.startsWith("#cloud=") && !dismissed();
  // Completion belongs to the signed-in account, not the machine or another user's browser session.
  createEffect(() => ownerKey(owner()), key => {
    setStep(0);
    setDismissed(false);
    if (!key) return;
    try { setDismissed(localStorage.getItem(`zuse.web.onboarding.v1:${key}`) === "complete"); } catch { /* Private storage is optional. */ }
  });
  createEffect(() => [signedIn(), step(), state.epoch, hostedBrowser(), connected()] as const, ([signed, current, , hosted, online]) => {
    if (!hosted || !signed || !online) return;
    if (current === 1) setStep(2);
    if (current >= 2) void loadBilling();
    if (current === 4) void loadContext();
  });
  const finish = () => {
    const key = ownerKey(owner());
    if (!key || !signedIn()) return;
    try { localStorage.setItem(`zuse.web.onboarding.v1:${key}`, "complete"); } catch { /* The current tab still completes setup. */ }
    navigate({ kind: "settings", section: "cloud" });
    setDismissed(true);
  };
  return <Show when={visible()} fallback={props.children}>
    <main class="web-onboarding">
      <header class="web-onboarding-header"><Icon name="zuse-logo" size={30} /><span>Zuse for web</span><span class="web-onboarding-caption">Your workspace, in your browser</span></header>
      <ol class="web-onboarding-steps" aria-label="Web setup progress"><For each={steps}>{(label, index) => <li aria-current={step() === index() ? "step" : undefined} data-complete={step() > index() ? "true" : undefined}>{index() + 1}. {label}</li>}</For></ol>
      <section class="web-onboarding-body" aria-label={steps[step()]}>
        <Switch>
          <Match when={step() === 0}>
            <h1>Start building from your browser</h1><p>Sign in, connect a GitHub repository, and start a coding agent in a private cloud workspace. Keep chats, threads, Zappers, files, and terminals together. No desktop app required.</p>
            <button type="button" class="btn" disabled={!connected()} onClick={() => setStep(signedIn() ? 2 : 1)}>Get started</button>
          </Match>
          <Match when={step() === 1}>
            <h1>Sign in to Zuse</h1><p>Your account keeps your cloud repositories and conversations available across browsers.</p>
            <Show when={login()?.userCode}>{code => <p role="status">Confirm this code on the sign-in page: <strong>{code()}</strong></p>}</Show>
            <Show when={accountError()}>{message => <p role="alert">{message()}</p>}</Show>
            <button type="button" class="btn" disabled={!connected() || !accountLoaded() || accountSession.busy} onClick={() => void signIn()}>{accountSession.busy ? "Starting sign-in…" : login() ? "Continue sign-in" : "Sign in"}</button>
          </Match>
          <Match when={step() === 2}>
            <h1>Choose your cloud plan</h1><p>Choose the hosting plan for your private workspaces.</p><Hosting />
            <button type="button" class="btn" disabled={!connected()} onClick={() => void loadBilling()}>Refresh plan</button>
            <button type="button" class="btn" disabled={!connected() || cloud.billing.subscribed !== true} onClick={() => setStep(3)}>Continue</button>
          </Match>
          <Match when={step() === 3}>
            <h1>Prepare your repositories</h1><p>Connect GitHub, choose your repositories, and build your private cloud image.</p><Setup />
            <button type="button" class="btn" disabled={!connected() || !cloud.repos.projects.length} onClick={() => setStep(4)}>Continue to your first chat</button>
          </Match>
          <Match when={step() === 4}>
            <h1>Start your first cloud chat</h1><p>Choose a prepared repository and an available model, then tell your agent what to build.</p><Chats />
            <button type="button" class="btn" onClick={finish}>Open Zuse</button>
          </Match>
        </Switch>
        <Show when={step() > 0}><button type="button" class="btn" onClick={() => setStep(current => Math.max(0, current - 1))}>Back</button></Show>
        <Show when={signedIn()}><button type="button" class="btn" onClick={finish}>Finish setup later</button></Show>
      </section>
    </main>
  </Show>;
}
