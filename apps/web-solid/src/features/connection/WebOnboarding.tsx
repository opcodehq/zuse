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
  const primary = () => ["Get started", accountSession.busy ? "Signing in…" : "Sign in to Zuse", "Continue", "Continue", "Open Zuse"][step()];
  const disabled = () => !connected() || (step() === 1 && (!accountLoaded() || accountSession.busy)) || (step() === 2 && cloud.billing.subscribed !== true) || (step() === 3 && !cloud.repos.projects.length);
  const advance = () => {
    if (disabled()) return;
    if (step() === 0) setStep(signedIn() ? 2 : 1);
    else if (step() === 1) void signIn();
    else if (step() === 4) finish();
    else setStep(current => current + 1);
  };
  return <Show when={visible()} fallback={props.children}>
    <main class="web-onboarding">
      <div class="web-onboarding-column" data-step={step()}>
        <header class="web-onboarding-header">
          <span class="web-onboarding-wordmark">Zuse <span>for web</span></span>
          <div class="web-onboarding-progress"><span class="web-onboarding-track"><span style={{ width: `${(step() + 1) / steps.length * 100}%` }} /></span><span>Step {step() + 1} of {steps.length}</span></div>
        </header>
        <section class="web-onboarding-body" aria-label={steps[step()]}>
          <div class="web-onboarding-mark"><Icon name={step() === 0 ? "zuse-logo" : step() === 1 ? "user" : step() === 2 ? "cloud" : step() === 3 ? "folder" : "chat-round"} size={35} /></div>
          <Switch>
            <Match when={step() === 0}>
              <h1>Your workspace, wherever you are.</h1><p>Your agents, projects, and conversations. Together in Zuse, right from your browser.</p>
              <div class="web-onboarding-points"><For each={[
                ["tool-agent", "Work with the coding agents you already use"],
                ["worktree", "Run tasks in parallel, each in its own workspace"],
                ["cloud", "Keep work running when you close your browser"],
              ]}>{point => <div><Icon name={point[0] ?? "cloud"} size={17} /><span>{point[1]}</span></div>}</For></div>
              <span class="web-onboarding-note">No desktop app needed.</span>
            </Match>
            <Match when={step() === 1}>
              <h1>Make yourself at home.</h1><p>Sign in to bring your cloud projects and conversations with you. Your workspace stays private to your account.</p>
              <div class="web-onboarding-account"><Icon name="cloud" size={22} /><div><strong>Your Zuse account</strong><span>One account across desktop and web</span></div><Icon name="lock" size={14} /></div>
              <Show when={login()?.userCode}>{code => <p role="status">Confirm the code <strong>{code()}</strong> on the sign-in page.</p>}</Show>
              <Show when={accountError()}>{message => <p role="alert">{message()}</p>}</Show>
              <span class="web-onboarding-note">You’ll continue securely in your browser.</span>
            </Match>
            <Match when={step() === 2}>
              <h1>A home for your cloud work.</h1><p>Choose a plan for your private workspaces. You stay in control of your usage.</p><Hosting />
              <button type="button" class="web-onboarding-link" disabled={!connected()} onClick={() => void loadBilling()}>Refresh plan <Icon name="refresh" size={12} /></button>
            </Match>
            <Match when={step() === 3}>
              <h1>Bring a project.</h1><p>Connect GitHub, choose your repositories, and prepare your cloud workspace.</p><Setup />
            </Match>
            <Match when={step() === 4}>
              <h1>What would you like to build?</h1><p>Choose your repository and agent. Start a conversation, and take it from there.</p><Chats />
            </Match>
          </Switch>
        </section>
        <footer class="web-onboarding-footer">
          <Show when={step() > 0} fallback={<span class="web-onboarding-note">Built for your workflow</span>}><button type="button" class="web-onboarding-back" aria-label="Back" onClick={() => setStep(current => Math.max(0, current - 1))}><Icon name="arrow-left" size={15} /></button></Show>
          <div><Show when={signedIn()}><button type="button" class="web-onboarding-skip" onClick={finish}>Set up later</button></Show><button type="button" class="web-onboarding-primary" disabled={disabled()} onClick={advance}>{primary()}<Icon name="arrow-right" size={14} /></button></div>
        </footer>
      </div>
    </main>
  </Show>;
}
