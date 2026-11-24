// The footer (crates/viewport sidebar_controls.rs plus crates/ui's settings trigger): General-account
// plan, provider usage limits, the running-agent count, Git and Settings.
import { formatKeys } from "@/core/keys";
import { navigate, view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { Tooltip } from "@/ui/primitives";
import { percentLeft } from "@/ui/usage-meter";
import { UsageMeter } from "@/ui/UsageMeter";
import { For, Match, Show, Switch } from "solid-js";
import { account, accountAction, accountDisabled, type AgentAccount, available, loadUsage, planLabel, providerAccount, PROVIDERS, signedIn } from "./account";
import { toggleRightPanel, toggleSettings } from "./actions";
import { EntriesMenu, type Entry } from "./menus";
import { workingThreads } from "./model";
import { now } from "./util";

function AccountButton() {
  const label = () => planLabel();
  const upgrade = () => label() === "Upgrade";
  const tip = () => account.accountError ?? account.planError ?? label();
  return (
    <Tooltip content={tip()}>
      <button
        type="button"
        class="sb-fbtn"
        data-kind={upgrade() ? "upgrade" : "account"}
        disabled={accountDisabled()}
        aria-label={label()}
        aria-description={tip() !== label() ? tip() : undefined}
        onClick={accountAction}
      >
        <Show when={!signedIn() && label() === "Sign in"}>
          <Icon name="global" size={14} />
        </Show>
        <Show when={label() === "Cloud"}>
          <Icon name="cloud-dither" size={14} />
        </Show>
        <span>{label()}</span>
      </button>
    </Tooltip>
  );
}

function ProviderDetail(props: { name: string; icon: string; account: AgentAccount | undefined }) {
  return (
    <>
      <div class="sb-detail-head">
        <Icon name={props.icon} size={16} />
        <div>
          <div class="sb-detail-name">{props.name}</div>
          <div class="sb-detail-plan">{props.account?.planLabel || "Usage limits"}</div>
        </div>
      </div>
      <Show when={props.account?.usageError}>{(error) => <div class="sb-usage-note" style={{ padding: "0 0 8px" }}>{error()}</div>}</Show>
      <Switch>
        <Match when={props.account?.usageWindows.length}>
          <For each={props.account!.usageWindows}>
            {(window) => <UsageMeter window={window} variant="detail" />}
          </For>
        </Match>
        <Match when={true}>
          <div class="sb-detail-empty">
            {account.pending.usage
              ? "Loading usage…"
              : props.account
                ? "Usage limits are unavailable for this account."
                : "Sign in to this provider to see usage limits."}
          </div>
        </Match>
      </Switch>
    </>
  );
}

function usageEntries(): Entry[] {
  const entries: Entry[] = [{ kind: "heading", label: "Usage limits" }];
  for (const provider of PROVIDERS) {
    entries.push({
      kind: "custom",
      label: provider.name,
      icon: provider.icon,
      get meta() {
        const left = percentLeft(providerAccount(provider.harness)?.usageWindows[0]?.usedFraction);
        return left === null ? undefined : `${left}% left`;
      },
      content: () => <ProviderDetail name={provider.name} icon={provider.icon} account={providerAccount(provider.harness)} />,
    });
  }
  entries.push(
    { kind: "separator" },
    { kind: "item", label: "Refresh usage", icon: "refresh", keepOpen: true, get disabled() { return account.pending.usage; }, onSelect: () => loadUsage(true) },
    { kind: "item", label: "Full usage", icon: "dashboard-speed", onSelect: () => navigate({ kind: "usage" }) },
  );
  return entries;
}

export function Footer() {
  const working = () => workingThreads(now());
  const settingsOpen = () => view.route.kind === "settings";
  const gitOpen = () => (view.route.kind === "chat" || view.route.kind === "zappers") && view.rightPanel === "git";
  return (
    <div class="sb-foot">
      <Show when={!(signedIn() && account.subscribed === null && !account.planError)}>
        <AccountButton />
      </Show>
      <div class="sb-foot-spacer" />
      <Show when={account.usageError}>
        {(error) => (
          <span class="sb-usage-meta" role="status" title={error()} style={{ "margin-left": 0 }}>
            <Icon name="danger-triangle" size={12} />
          </span>
        )}
      </Show>
      <EntriesMenu
        label="Usage limits"
        placement="top-end"
        class="sb-usage"
        entries={usageEntries}
        onOpenChange={(open) => open && loadUsage(false)}
        trigger={(p) => (
          <button type="button" {...p} class="sb-fbtn" title="Usage limits" disabled={!available()} onPointerEnter={() => loadUsage(false)}>
            <Icon name="dashboard-speed" size={16} />
          </button>
        )}
      />
      <span class="sb-agents" role="status" aria-label={`${available() ? working() : "Unknown"} agents working`} title="Agents working">
        <Icon name="bot" size={14} />
        {available() ? working() : "—"}
      </span>
      <Tooltip content={view.selectedChatId ? (gitOpen() ? "Close Git" : "Git") : "Open a chat to see its Git status"}>
        <button
          type="button"
          class="sb-fbtn"
          aria-label="Git"
          aria-pressed={(gitOpen()) ? "true" : "false"}
          disabled={!view.selectedChatId}
          onClick={() => toggleRightPanel("git")}
        >
          <Icon name="git-branch" size={14} />
        </button>
      </Tooltip>
      <Tooltip content={`${settingsOpen() ? "Close settings" : "Settings"} · ${formatKeys("mod+,")}`}>
        <button type="button" class="sb-fbtn" aria-label={settingsOpen() ? "Close settings" : "Settings"} aria-pressed={(settingsOpen()) ? "true" : "false"} onClick={toggleSettings}>
          <Icon name="dither-gear" size={14} />
        </button>
      </Tooltip>
    </div>
  );
}
