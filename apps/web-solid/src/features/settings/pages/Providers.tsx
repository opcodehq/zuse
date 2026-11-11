// Providers: which agents the composer offers, their setup, accounts, updates and custom ACP agents
// (crates/ui/src/settings/harnesses.rs, accounts.rs, custom_acp.rs).
import { createEffect, createMemo, createSignal, For, on, onMount, Show } from "solid-js";
import { state } from "@/core/store";
import { Icon } from "@/ui/Icon";
import { Menu } from "@/ui/primitives";
import { type AgentAccount, data, loadAccounts, loadCustomAgents } from "../data";
import {
  addOptionLabel,
  canStartUpdate,
  checkedLabel,
  cliName,
  harnessMark,
  harnessTint,
  type HarnessDescriptor,
  type HarnessUpdateStatus,
  installCommand,
  installHint,
  isCustomAcp,
  keepsOneLogin,
  loginCommand,
  loginOptions,
  maskedEmail,
  providerNote,
  reportsUsage,
  signsIn,
  switchesAccounts,
  UPDATE_POLICIES,
  type UpdatePolicy,
  updateDetail,
  updateInProgress,
} from "../harness";
import { t } from "../i18n";
import { accountAction, cancelLogin, login, loginActive, loginStatus, reopenLoginPage, retryLogin, startLogin, submitCode } from "../login";
import {
  applyUpdate,
  cancelInstall,
  cancelUpdate,
  checkUpdates,
  effectiveEnabled,
  install,
  loadHarnesses,
  newestCheck,
  providers,
  setEnabled,
  setPolicy,
  updateFor,
  visibleHarnesses,
} from "../providers";
import { ActionButton, Badge, Card, CopyField, ErrorStrip, Group, InlineSpinner, Mark, Meta, SectionTitle, Select, SettingsDialog, Switch, WarningStrip } from "../widgets";
import { CustomAgents } from "./CustomAgents";

type Tone = "ready" | "warning" | "error" | "muted";

function providerStatus(d: HarnessDescriptor, enabled: boolean, update: HarnessUpdateStatus | undefined): [Tone, string] {
  if (d.installed === false) return ["warning", installHint(d.id, enabled, !!d.canInstall)];
  const detail = update ? updateDetail(update) : undefined;
  if (update?.phase === "failed") return ["error", detail ?? ""];
  const [tone, label]: [Tone, string] = enabled ? ["ready", "Ready"] : ["muted", "Off"];
  return [tone, detail ? `${label} · ${detail}` : label];
}

export function Providers() {
  const [expanded, setExpanded] = createSignal<string | null>(null);
  const refresh = () => {
    void loadHarnesses();
    void checkUpdates();
    void loadAccounts(true);
    void loadCustomAgents();
  };
  onMount(() => {
    void loadHarnesses();
    void loadAccounts(true);
    void loadCustomAgents();
  });
  createEffect(on(() => state.epoch, () => {
    void loadHarnesses();
    void loadAccounts(true);
    void loadCustomAgents();
  }, { defer: true }));

  // Custom agents have their own card.
  const rows = createMemo(() => visibleHarnesses().filter((d) => !isCustomAcp(d.id)));
  const enabledCount = createMemo(() => visibleHarnesses().filter((d) => effectiveEnabled(d)).length);
  const checking = () => providers.loading || providers.checking;
  const status = () => {
    if (checking()) return "Checking…";
    if (providers.listError) return "Probe failed";
    return checkedLabel(newestCheck());
  };

  return (
    <>
      <SectionTitle
        title={t("settings-settings_page_providers", "Providers")}
        description={t(
          "native-verify_what_s_installed_signed_in_and_which_subscription_each_provider_runs_on",
          "Verify what's installed, signed in, and which subscription each provider runs on.",
        )}
      />
      <div class="st-stack">
        <Group
          title={t("settings-settings_page_agent_providers", "Agent providers")}
          description={t(
            "settings-settings_page_enable_the_coding_agents_you_use_verify_their_local_setup_and_control",
            "Enable the coding agents you use, verify their local setup, and control which models appear in Zuse.",
          )}
          action={
            <>
              <span class="st-desc" style={{ "font-size": "10px" }} aria-live="polite">
                {status()}
              </span>
              <button
                type="button"
                class="st-icon-action"
                data-size="md"
                aria-label={t("settings-settings_page_refresh_provider_status", "Refresh provider status")}
                title={t("settings-settings_page_refresh_provider_status", "Refresh provider status")}
                disabled={checking()}
                onClick={refresh}
              >
                <Icon name="refresh" size={14} />
              </button>
            </>
          }
        >
          <Show when={data.updatesError}>
            {(text) => <ErrorStrip action={<ActionButton onClick={refresh}>{t("native-retry_updates", "Retry updates")}</ActionButton>}>Agent updates: {text()}</ErrorStrip>}
          </Show>
          <Show when={providers.error}>{(text) => <ErrorStrip>{text()}</ErrorStrip>}</Show>
          <Show
            when={state.harnesses.length}
            fallback={
              <Show
                when={providers.listError}
                fallback={
                  <Card style={{ padding: "16px", gap: "12px" }} aria-busy="true" aria-label="Loading providers">
                    <For each={[0, 1, 2, 3]}>{() => <div class="st-skeleton" style={{ height: "28px" }} />}</For>
                  </Card>
                }
              >
                {(text) => <ErrorStrip action={<ActionButton onClick={() => void loadHarnesses()}>Retry</ActionButton>}>{text()}</ErrorStrip>}
              </Show>
            }
          >
            <Card>
              <For each={rows()}>
                {(d) => (
                  <ProviderRow
                    descriptor={d}
                    enabledCount={enabledCount()}
                    expanded={expanded() === d.id}
                    onToggleDetails={() => setExpanded(expanded() === d.id ? null : d.id)}
                    onDisabled={() => expanded() === d.id && setExpanded(null)}
                  />
                )}
              </For>
            </Card>
          </Show>
        </Group>
        <CustomAgents />
      </div>
    </>
  );
}

function ProviderRow(props: { descriptor: HarnessDescriptor; enabledCount: number; expanded: boolean; onToggleDetails: () => void; onDisabled: () => void }) {
  const d = () => props.descriptor;
  const name = () => d().name || d().id;
  const installed = () => d().installed !== false;
  const enabled = () => effectiveEnabled(d());
  // The last runnable agent stays on: the composer needs something to run.
  const lastEnabled = () => enabled() && props.enabledCount === 1 && installed();
  const interactive = () => !lastEnabled() && (enabled() || installed());
  const update = () => updateFor(d().id);
  const version = () => (installed() ? update()?.installedVersion : undefined);
  const status = () => providerStatus(d(), enabled(), update());
  const canExpand = () => enabled() || !installed();
  const open = () => canExpand() && props.expanded;
  const installing = () => providers.installing === d().id;
  const offersInstall = () => d().id !== "mock" && !installed() && !!d().canInstall;
  const detailsId = () => `provider-details-${d().id}`;

  const updateChip = () => {
    const u = update();
    if (!u) return null;
    if ((u.phase === "available" || u.phase === "manual-action-required") && u.canApply) return "update" as const;
    if (u.phase === "waiting-for-idle" || u.phase === "preparing" || u.phase === "downloading") return "cancel" as const;
    return null;
  };

  return (
    <div class="st-provider">
      <div class="st-provider-head">
        {/* Clicking the row toggles details like desktop; the caret button is the keyboard control. */}
        <div
          class="st-provider-trigger"
          data-expandable={canExpand() ? "true" : undefined}
          onClick={(event) => {
            if (canExpand() && !(event.target as HTMLElement).closest("button")) props.onToggleDetails();
          }}
        >
          <Mark name={harnessMark(d().id)} tint={harnessTint(d().id)} dim={!enabled()} />
          <span class="st-row-text" style={{ opacity: enabled() ? undefined : 0.6, "min-width": 0 }}>
            <span class="st-provider-name">
              <span class="st-row-title">{name()}</span>
              <Show when={version()}>
                <span class="st-version">v{version()}</span>
              </Show>
              <Show when={updateChip()}>
                {(chip) => {
                  const tip = () =>
                    chip() === "update" ? (update()?.latestVersion ? `Update to v${update()!.latestVersion}` : "Update to the latest version") : "Cancel update";
                  return (
                    <button
                      type="button"
                      class="st-update-chip"
                      data-tone={chip() === "cancel" ? "cancel" : undefined}
                      aria-label={tip()}
                      title={tip()}
                      disabled={chip() === "update" && (!canStartUpdate(update()!) || !!providers.updateAction)}
                      onClick={() => void (chip() === "update" ? applyUpdate(d().id) : cancelUpdate(d().id))}
                    >
                      <Icon name={chip() === "update" ? "arrow-up" : "close"} size={14} />
                      <Show when={chip() === "update"}>{update()?.latestVersion ? `v${update()!.latestVersion}` : "Update"}</Show>
                    </button>
                  );
                }}
              </Show>
            </span>
            <span class="st-status">
              <Show when={!installing()} fallback={<InlineSpinner label={`Installing ${name()}`} />}>
                <span class="st-dot" data-tone={status()[0]} aria-hidden="true" />
              </Show>
              <span>{installing() ? `Installing ${name()}…` : status()[1]}</span>
            </span>
          </span>
        </div>
        <Show when={offersInstall() && !installing()}>
          <ActionButton tone="quiet" disabled={!!providers.installing} onClick={() => void install(d().id)}>
            {t("providers-provider_card_install", "Install")}
          </ActionButton>
        </Show>
        <Show when={installing()}>
          <ActionButton tone="quiet" onClick={() => void cancelInstall()}>
            Cancel
          </ActionButton>
        </Show>
        <Switch
          label={name()}
          checked={enabled()}
          disabled={!interactive()}
          description={lastEnabled() ? "The last enabled agent stays on." : undefined}
          onChange={(value) => {
            void setEnabled(d().id, value);
            if (!value) props.onDisabled();
          }}
        />
        <button
          type="button"
          class="st-icon-action"
          data-size="md"
          aria-label={`${name()} preferences`}
          aria-expanded={canExpand() ? open() : undefined}
          disabled={!canExpand()}
          onClick={() => props.onToggleDetails()}
        >
          <Icon name={open() ? "alt-arrow-up" : "alt-arrow-down"} size={14} />
        </button>
      </div>
      <Show when={open()}>
        <div class="st-provider-body" id={detailsId()}>
          <Show when={installed()} fallback={<InstallDetails harness={d().id} />}>
            <Show when={signsIn(d().id)} fallback={<CliSignIn harness={d().id} />}>
              <Accounts harness={d().id} />
            </Show>
            <AgentOptions harness={d().id} />
          </Show>
        </div>
      </Show>
    </div>
  );
}

function InstallDetails(props: { harness: string }) {
  const command = () => installCommand(props.harness);
  return (
    <div style={{ display: "flex", "flex-direction": "column", gap: "6px" }}>
      <div class="st-sub-label">{t("providers-provider_card_install", "Install")}</div>
      <Show when={command()} fallback={<div class="st-copy"><code>Install this agent on the execution host, then refresh its status.</code></div>}>
        {(cmd) => <CopyField value={cmd()} label={t("native-copy_install_command", "Copy install command")} />}
      </Show>
    </div>
  );
}

function CliSignIn(props: { harness: string }) {
  const command = () => loginCommand(props.harness);
  return (
    <Show when={props.harness === "muse" && command()}>
      {(cmd) => (
        <div style={{ display: "flex", "flex-direction": "column", gap: "6px" }}>
          <div class="st-sub-label">{t("common-signIn", "Sign in")}</div>
          <CopyField value={cmd()} label="Copy command" />
          <div class="st-desc">Muse Code manages sign-in. Run `muse login` in a terminal, or save a Meta API key with `muse auth set`. Zuse doesn't pass META_API_KEY through.</div>
        </div>
      )}
    </Show>
  );
}

function AgentOptions(props: { harness: string }) {
  const [open, setOpen] = createSignal(false);
  const update = () => updateFor(props.harness);
  const caption = (): [string, "attention" | "danger" | undefined] => {
    switch (update()?.phase) {
      case "available":
        return ["An update is available", "attention"];
      case "failed":
        return ["An update needs attention", "danger"];
      case "manual-action-required":
        return ["An update needs manual action", "attention"];
      case "waiting-for-idle":
        return ["Update waiting for this agent to finish", "attention"];
      case "preparing":
      case "downloading":
      case "installing":
      case "verifying":
        return ["Agent update in progress", "attention"];
      case "checking":
        return ["Checking for updates…", undefined];
      default:
        return ["CLI updates and update policy", undefined];
    }
  };
  return (
    <Show when={update()}>
      {(u) => (
        <div style={{ display: "flex", "flex-direction": "column", gap: "8px" }}>
          <button type="button" class="st-options-toggle" aria-expanded={open()} aria-label={t("native-agent_options", "Agent options")} onClick={() => setOpen(!open())}>
            <Icon name={open() ? "alt-arrow-down" : "alt-arrow-right"} size={14} />
            <span class="st-row-text">
              <span class="st-row-title">Agent options</span>
              <span class="st-desc" data-tone={caption()[1]}>
                {caption()[0]}
              </span>
            </span>
          </button>
          <Show when={open()}>
            <div style={{ "padding-left": "30px" }}>
              <div class="st-details-label">Updates</div>
              <div class="st-details-row">
                <div class="st-row-text">
                  <div class="st-row-title">Update policy</div>
                  <div class="st-desc">{UPDATE_POLICIES.find((p) => p.value === (u().policy ?? "notify"))?.description}</div>
                </div>
                <Select<UpdatePolicy>
                  label="Update policy"
                  value={u().policy ?? "notify"}
                  options={UPDATE_POLICIES.map((p) => ({ value: p.value, label: p.label }))}
                  onChange={(policy) => void setPolicy(props.harness, policy)}
                  width={128}
                  disabled={!!providers.updateAction}
                />
              </div>
              <Show when={u().phase === "failed" && u().error?.message}>
                <div class="st-details-row st-desc" role="alert" style={{ color: "var(--danger)" }}>
                  {u().error!.message}
                </div>
              </Show>
              <Show when={u().phase === "manual-action-required" && u().manualCommand}>
                {(cmd) => (
                  <div class="st-details-row" style={{ "flex-direction": "column", "align-items": "stretch" }}>
                    <div class="st-desc">Run this on the computer to update {cliName(props.harness)}:</div>
                    <CopyField value={cmd()} label="Copy update command" />
                  </div>
                )}
              </Show>
              <Show when={updateInProgress(u().phase) && u().progress?.message}>
                <div class="st-details-row st-desc">{u().progress!.message}</div>
              </Show>
            </div>
          </Show>
        </div>
      )}
    </Show>
  );
}

/** Compact absolute reset moment: a clock time within ~22h, a weekday within a week, else month and day. */
function formatReset(resetsAt: string | null | undefined, now = Date.now()) {
  if (!resetsAt) return null;
  const at = new Date(resetsAt);
  const hours = (at.getTime() - now) / 3_600_000;
  if (hours < 22) return `resets ${at.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}`;
  if (hours < 24 * 7) return `resets ${at.toLocaleDateString(undefined, { weekday: "short" })}`;
  return `resets ${at.toLocaleDateString(undefined, { month: "short", day: "numeric" })}`;
}

const level = (fraction: number) => (fraction >= 0.95 ? "critical" : fraction >= 0.8 ? "warn" : undefined);

function accountLabel(account: AgentAccount) {
  const name = account.displayName?.trim();
  if (name && name !== account.email) return name;
  return account.email ? maskedEmail(account.email) : "Unknown account";
}

function Accounts(props: { harness: string }) {
  const rows = () => data.accounts?.accounts.filter((a) => a.harness === props.harness) ?? [];
  const warnings = () => data.accounts?.warnings.filter((w) => w.harness === props.harness) ?? [];
  const flow = () => (login.flow?.harness === props.harness ? login.flow : null);
  const [removing, setRemoving] = createSignal<AgentAccount | null>(null);
  const blocked = () => !!login.busyAccount || loginActive();
  return (
    <div style={{ display: "flex", "flex-direction": "column" }}>
      <div style={{ display: "flex", "align-items": "center", gap: "4px" }}>
        <span class="st-details-label">Accounts</span>
        <Show when={data.accountsLoading && data.accounts}>
          <span style={{ "margin-left": "6px" }}>
            <InlineSpinner label="Refreshing accounts" />
          </span>
        </Show>
        <span style={{ flex: 1 }} />
        <button
          type="button"
          class="st-icon-action"
          style={{ "margin-right": "-4px" }}
          aria-label={t("settings-model_connections_refresh", "Refresh accounts")}
          title="Refresh usage"
          disabled={data.accountsLoading}
          onClick={() => void loadAccounts(true)}
        >
          <Icon name="refresh" size={13} />
        </button>
      </div>
      <For each={warnings()}>{(w) => <WarningStrip>{w.message}</WarningStrip>}</For>
      <Show when={providerNote(props.harness)}>{(note) => <div class="st-desc" style={{ "margin-top": "4px" }}>{note()}</div>}</Show>
      <Show
        when={data.accounts}
        fallback={
          <Show
            when={data.accountsError}
            fallback={
              <div class="st-accounts" aria-busy="true" aria-label="Loading accounts">
                <div class="st-account-row">
                  <div class="st-skeleton" style={{ width: "28px", height: "28px", "border-radius": "50%" }} />
                  <div class="st-skeleton" style={{ flex: 1, height: "14px" }} />
                </div>
              </div>
            }
          >
            {(text) => (
              <ErrorStrip action={<ActionButton aria-label={t("native-retry_loading_accounts", "Retry loading accounts")} onClick={() => void loadAccounts(true)}>Retry</ActionButton>}>
                {text()}
              </ErrorStrip>
            )}
          </Show>
        }
      >
        <Show when={rows().length}>
          <div class="st-accounts" style={{ "margin-top": "4px" }} role="list" aria-label={`${props.harness} accounts`}>
            <For each={rows()}>{(account) => <AccountRow account={account} blocked={blocked()} onRemove={() => setRemoving(account)} />}</For>
          </div>
        </Show>
        <Show when={login.actionError}>{(text) => <div class="st-dialog-error" role="alert">{text()}</div>}</Show>
        <Show when={!rows().length || flow()}>
          <InlineSignIn harness={props.harness} />
        </Show>
        <Show when={rows().length && !keepsOneLogin(props.harness) && !flow()}>
          <div class="st-add-options">
            <For each={loginOptions(props.harness)}>
              {(option) => (
                <button type="button" class="st-add-link" disabled={blocked()} onClick={() => void startLogin(props.harness, option.provider)}>
                  <Icon name="plus" size={12} />
                  {addOptionLabel(props.harness, option, false)}
                </button>
              )}
            </For>
          </div>
        </Show>
      </Show>
      <RemoveDialog account={removing()} onClose={() => setRemoving(null)} />
    </div>
  );
}

function AccountRow(props: { account: AgentAccount; blocked: boolean; onRemove: () => void }) {
  const a = () => props.account;
  const label = () => accountLabel(a());
  const initials = () =>
    (a().displayName || a().email || "?")
      .split(/\s+/)
      .slice(0, 2)
      .map((s) => s.charAt(0))
      .join("")
      .toUpperCase();
  const missing = () => {
    if (!reportsUsage(a().harness) || a().usageWindows?.length) return null;
    if (!a().switchable && switchesAccounts(a().harness)) return a().harness === "claude-code" ? "Credentials unavailable" : "Couldn't identify this login";
    if (a().usageError) return a().usageError!;
    return data.accountsLoading ? "Checking usage…" : "Usage unavailable";
  };
  const resets = () =>
    (a().usageWindows ?? [])
      .map((w) => {
        const reset = formatReset(w.resetsAt);
        return reset ? `${w.label}: ${reset}` : w.label;
      })
      .join(" · ");
  const busy = () => login.busyAccount === a().id;
  const canSwitch = () => !a().active && !!a().switchable && !props.blocked;
  // Personal Claude orgs are named "<email>'s Organization": noise under the email itself.
  const organization = () => {
    const org = a().organization;
    return org && !(a().email && org.startsWith(a().email!)) ? org : null;
  };
  return (
    <div
      class="st-account-row"
      role="listitem"
      data-active={a().active ? "true" : undefined}
      data-switchable={canSwitch() ? "true" : undefined}
      title={canSwitch() ? "Switch to this account" : undefined}
      aria-busy={busy()}
      onClick={(event) => {
        if (canSwitch() && !(event.target as HTMLElement).closest("button")) void accountAction(a(), "switch");
      }}
    >
      <div class="st-account-avatar" aria-hidden="true">
        {initials()}
      </div>
      <div class="st-account-main">
        <div style={{ display: "flex", "align-items": "center", gap: "6px", "min-width": 0 }}>
          <span class="st-account-email">{label()}</span>
          <Show when={a().active}>
            <Badge tone="active">In use</Badge>
          </Show>
        </div>
        {/* A missing-usage reason joins the meta line, so a failed probe never changes the row's height. */}
        <Meta parts={[a().planLabel, organization(), a().authKind === "api-key" && "API key", missing()]} />
      </div>
      <Show when={reportsUsage(a().harness)}>
        <div class="st-usage" title={resets() || undefined}>
          <Show when={!missing()}>
            <For each={a().usageWindows ?? []}>
              {(w) => {
                const fraction = () => Math.min(1, Math.max(0, w.usedFraction));
                return (
                  <div class="st-meter" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(fraction() * 100)} aria-label={`${w.label} usage`}>
                    <span class="st-meter-label">{w.label}</span>
                    <span class="st-meter-bar">
                      <Show when={fraction() > 0}>
                        <span class="st-meter-fill" data-level={level(fraction())} style={{ width: `${Math.max(fraction(), 0.015) * 100}%`, display: "block" }} />
                      </Show>
                    </span>
                    <span class="st-meter-pct" data-level={level(fraction())}>
                      {Math.round(fraction() * 100)}%
                    </span>
                  </div>
                );
              }}
            </For>
          </Show>
        </div>
      </Show>
      <div style={{ width: "24px", "flex-shrink": 0 }}>
        <Show when={busy()}>
          <InlineSpinner label="Working" />
        </Show>
        <Show when={a().switchable && !busy()}>
          <Menu
            placement="bottom-end"
            items={[
              ...(a().active ? [] : [{ label: "Switch to this account", disabled: props.blocked, onSelect: () => void accountAction(a(), "switch") }]),
              { label: "Remove account", danger: true, disabled: props.blocked, onSelect: props.onRemove },
            ]}
            trigger={(triggerProps) => (
              <button {...triggerProps} type="button" class="st-icon-action" aria-label={`Actions for ${label()}`} disabled={props.blocked}>
                <Icon name="more-horizontal" size={14} />
              </button>
            )}
          />
        </Show>
      </div>
    </div>
  );
}

function RemoveDialog(props: { account: AgentAccount | null; onClose: () => void }) {
  const [last, setLast] = createSignal<AgentAccount | null>(null);
  createEffect(() => props.account && setLast(props.account));
  const account = () => props.account ?? last();
  return (
    <SettingsDialog
      open={!!props.account}
      onClose={props.onClose}
      title={`Remove ${account() ? accountLabel(account()!) : "account"}?`}
      description={
        account()?.active
          ? "Removes this saved account and signs the CLI out of this login. Chats may need you to sign in again. Transcripts are kept."
          : "Removes this saved login from Settings. Sign in again to use this account. The active CLI login and transcripts are kept."
      }
      footer={
        <>
          <ActionButton tone="quiet" onClick={props.onClose}>
            Cancel
          </ActionButton>
          <ActionButton
            tone="danger"
            onClick={() => {
              const target = props.account;
              props.onClose();
              if (target) void accountAction(target, "remove");
            }}
          >
            Remove
          </ActionButton>
        </>
      }
    />
  );
}

const onEngineMachine = () => ["localhost", "127.0.0.1", "[::1]"].includes(location.hostname);

function InlineSignIn(props: { harness: string }) {
  const flow = () => (login.flow?.harness === props.harness ? login.flow : null);
  const command = () => loginCommand(props.harness);
  const [code, setCode] = createSignal("");
  return (
    <div class="st-inline-signin">
      <Show
        when={flow()}
        fallback={
          <>
            <div class="st-sub-label">{t("common-signIn", "Sign in")}</div>
            <div class="st-row-control" style={{ "flex-wrap": "wrap" }}>
              <For each={loginOptions(props.harness)}>
                {(option) => (
                  <ActionButton tone="solid" class="st-signin-primary" disabled={loginActive() || !!login.busyAccount} onClick={() => void startLogin(props.harness, option.provider)}>
                    {option.provider ? `Sign in with ${option.label}` : `Sign in to ${option.label}`}
                  </ActionButton>
                )}
              </For>
              <Show when={command()}>
                <span class="st-desc" style={{ "font-size": "10px" }}>
                  {loginOptions(props.harness).length ? "or run" : "Run"} <code class="st-mono">{command()}</code>
                </span>
              </Show>
            </div>
          </>
        }
      >
        {(f) => (
          <Show
            when={f().step.kind !== "failed"}
            fallback={
              <>
                <div class="st-signin-error" role="alert">
                  {f().step.kind === "failed" ? (f().step as { message: string }).message : ""}
                </div>
                <div class="st-row-control">
                  <ActionButton onClick={retryLogin}>Try again</ActionButton>
                  <ActionButton tone="quiet" onClick={cancelLogin}>
                    Dismiss
                  </ActionButton>
                </div>
                <Show when={command()}>
                  <span class="st-desc">
                    Or run manually <code class="st-mono">{command()}</code>
                  </span>
                </Show>
              </>
            }
          >
            <div class="st-signin-strip" role="status">
              <InlineSpinner label="Signing in" />
              <span class="st-signin-text">{f().step.kind === "paste" ? "Paste the authorization code to finish signing in." : loginStatus(f())}</span>
              <Show when={f().url}>
                <ActionButton tone="quiet" icon="arrow-up-right" onClick={reopenLoginPage}>
                  Open browser again
                </ActionButton>
              </Show>
              <ActionButton tone="quiet" onClick={cancelLogin}>
                Cancel
              </ActionButton>
            </div>
            <Show when={f().step.kind === "paste"}>
              <form
                class="st-row-control"
                onSubmit={(e) => {
                  e.preventDefault();
                  void submitCode(code()).then(() => setCode(""));
                }}
              >
                <input
                  class="st-field"
                  style={{ flex: 1, width: "auto" }}
                  aria-label="Authorization code"
                  placeholder="Paste the authorization code"
                  autocomplete="off"
                  value={code()}
                  onInput={(e) => setCode(e.currentTarget.value)}
                />
                <ActionButton type="submit" disabled={(f().step as { submitting?: boolean }).submitting || !code().trim()}>
                  {(f().step as { submitting?: boolean }).submitting ? "Working…" : "Complete sign-in"}
                </ActionButton>
              </form>
              <Show when={(f().step as { error?: string | null }).error}>{(text) => <div class="st-dialog-error" role="alert">{text()}</div>}</Show>
            </Show>
            <Show when={f().callbackPort && !onEngineMachine()}>
              <div class="st-desc">
                The sign-in page finishes at localhost:{f().callbackPort} on the computer running Zuse. If this browser is on another device, finish signing in
                from a browser on that computer, or run <code class="st-mono">{command() ?? "the agent's login command"}</code> there.
              </div>
            </Show>
          </Show>
        )}
      </Show>
    </div>
  );
}
