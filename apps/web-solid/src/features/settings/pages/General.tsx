// General: account, language, theme, notifications, thread naming, worktrees and the engine version
// (crates/ui/src/settings/general.rs and the groups shortcuts.rs composes around it).
import { createEffect, createSignal, For, on, onMount, Show } from "solid-js";
import { M } from "@/core/methods";
import { rpc } from "@/core/connection";
import { state } from "@/core/store";
import { setView, view } from "@/core/view";
import { signInToAccount } from "@/features/connection";
import { signOutOfAccount } from "@/features/connection/signin";
import { Icon } from "@/ui/Icon";
import { toast } from "@/ui/primitives";
import { data, message } from "../data";
import { browseFolders, closeFolders, general, joinPath, loadNaming, loadWorktrees, parentPath, saveNaming, saveWorktrees, setGeneral } from "../general";
import { activeLocale, languageLoadError, LOCALES, t } from "../i18n";
import { ModelPicker } from "../ModelPicker";
import { PRESET_LABELS, PRESETS, pref, setPref, type CompletionPreset } from "../prefs";
import { playCompletion } from "../sound";
import { ActionButton, Card, ErrorStrip, Group, Row, SectionTitle, radioKeys, Segmented, Select, SettingsDialog, Switch } from "../widgets";

export function General() {
  return (
    <>
      <SectionTitle
        title={t("settings-settings_page_general", "General")}
        description={t("native-workspace_defaults_and_your_personal_app_preferences", "Workspace defaults and your personal app preferences.")}
      />
      <div class="st-stack">
        <Account />
        <Language />
        <Theme />
        <Notices />
        <ThreadNaming />
        <Worktrees />
        <About />
      </div>
    </>
  );
}

interface AccountStatus {
  user?: { id?: string; email?: string; name?: string | null } | null;
  login?: { url?: string; userCode?: string } | null;
  error?: string | null;
}

function Account() {
  const status = () => (state.account ?? {}) as AccountStatus;
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [revealed, setRevealed] = createSignal(false);
  const [editing, setEditing] = createSignal(false);
  let nameInput: HTMLInputElement | undefined;
  const user = () => status().user ?? null;
  const waiting = () => !!status().login;
  const available = () => state.connection.kind === "connected";
  const saved = () => {
    const entry = view.preferences.accountDisplayName as { userId?: string; name?: string } | undefined;
    return entry && entry.userId === user()?.id ? (entry.name ?? "") : "";
  };
  const name = () => saved().trim() || user()?.name?.trim() || user()?.email || t("settings-settings_page_not_signed_in", "Not signed in");
  const email = () => user()?.email ?? null;

  const run = async (action: () => Promise<string | null>) => {
    if (busy() || !available()) return;
    setBusy(true);
    setError(null);
    setError(await action());
    setBusy(false);
  };
  const commitName = () => {
    const id = user()?.id;
    if (id && nameInput) setView("preferences", "accountDisplayName", { userId: id, name: nameInput.value.trim() });
    setEditing(false);
  };

  const action = () => (
    <ActionButton disabled={busy() || waiting() || !available()} onClick={() => void run(user() ? signOutOfAccount : signInToAccount)}>
      {user() ? t("common-signOut", "Sign out") : busy() || waiting() ? "Signing in…" : t("common-signIn", "Sign in")}
    </ActionButton>
  );

  const EmailButton = (p: { value: string }) => (
    <button
      type="button"
      class="st-email"
      data-hidden={revealed() ? undefined : "true"}
      aria-label={revealed() ? `Email ${p.value}. Hide email` : "Show email"}
      aria-pressed={revealed()}
      onClick={() => setRevealed(!revealed())}
    >
      {p.value}
    </button>
  );

  return (
    <Group
      title={t("settings-settings_page_account", "Account")}
      description={t(
        "settings-settings_page_sign_in_to_sync_your_account_across_devices_and_soon_drive_remote_agen",
        "Sign in to sync your account across devices and (soon) drive remote agents from your phone.",
      )}
    >
      <Card>
        <Show
          when={user()}
          fallback={
            <Row
              title={!available() ? t("native-account_unavailable", "Account unavailable") : t("settings-settings_page_not_signed_in", "Not signed in")}
              description={
                !available()
                  ? t("native-reconnect_account", "Reconnect this computer to load the saved account session.")
                  : t(
                      "settings-settings_page_you_re_using_zuse_beta_locally_without_an_account_sign_in_to_sync_and",
                      "You're using Zuse (Beta) locally without an account. Sign in to sync and unlock remote agents.",
                    )
              }
            >
              <Show when={available()}>{action()}</Show>
            </Row>
          }
        >
          <div class="st-account">
            <div class="st-avatar" aria-hidden="true">
              {name().charAt(0).toUpperCase() || "?"}
            </div>
            <div class="st-row-text">
              <Show
                when={!editing()}
                fallback={
                  <input
                    ref={nameInput}
                    class="st-field"
                    style={{ "max-width": "220px" }}
                    aria-label={t("settings-settings_page_your_name", "Your name")}
                    placeholder={t("settings-settings_page_your_name", "Your name")}
                    value={saved() || user()?.name || ""}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") commitName();
                      if (e.key === "Escape") {
                        e.preventDefault();
                        e.stopPropagation();
                        setEditing(false);
                      }
                    }}
                    onBlur={commitName}
                  />
                }
              >
                <div style={{ display: "flex", "align-items": "center", gap: "6px", "min-width": "0" }}>
                  <Show when={email() === name()} fallback={<span class="st-account-name">{name()}</span>}>
                    <EmailButton value={email()!} />
                  </Show>
                  <button
                    type="button"
                    class="st-icon-action"
                    aria-label={t("settings-settings_page_edit_display_name", "Edit display name")}
                    title="Edit the name shown in this browser"
                    onClick={() => {
                      setEditing(true);
                      nameInput?.focus();
                      nameInput?.select();
                    }}
                  >
                    <Icon name="pen" size={14} />
                  </button>
                </div>
              </Show>
              <Show when={email() && email() !== name()}>
                <div>
                  <EmailButton value={email()!} />
                </div>
              </Show>
            </div>
            {action()}
          </div>
        </Show>
        <Show when={status().login}>
          {(pending) => (
            <div class="st-login">
              <div class="st-desc">
                {pending().userCode ? (
                  <>
                    Enter <span class="st-login-code">{pending().userCode}</span> in your browser to finish signing in.
                  </>
                ) : (
                  t("providers-accounts_waiting", "Complete sign-in in your browser.")
                )}
              </div>
              <div class="st-row-control">
                <Show when={pending().url?.startsWith("https://") ? pending().url : null}>
                  {(url) => (
                    <ActionButton onClick={() => window.open(url(), "_blank", "noopener")}>{t("native-continue_in_browser", "Continue in browser")}</ActionButton>
                  )}
                </Show>
                <ActionButton tone="quiet" disabled={busy()} onClick={() => void run(signOutOfAccount)}>
                  Cancel
                </ActionButton>
              </div>
            </div>
          )}
        </Show>
        <Show when={error() || status().error}>
          {(text) => (
            <div class="st-login st-dialog-error" role="alert">
              {text()}
            </div>
          )}
        </Show>
      </Card>
    </Group>
  );
}

function Language() {
  const selected = () => (LOCALES.some((l) => l.code === view.preferences.language) ? view.preferences.language : "system");
  const choices = () => [{ code: "system", name: t("common-system", "System") }, ...LOCALES];
  const title = () => t("native-language_settings", "Language");
  return (
    <Group title={title()}>
      <Card>
        <div class="st-languages">
          <div class="st-chips" role="radiogroup" aria-label={title()}>
            <For each={choices()}>
              {(choice) => (
                <button
                  type="button"
                  role="radio"
                  class="st-chip"
                  lang={choice.code === "system" ? undefined : choice.code}
                  aria-checked={selected() === choice.code}
                  tabIndex={selected() === choice.code ? 0 : -1}
                  onClick={() => setView("preferences", "language", choice.code)}
                  onKeyDown={(e) => radioKeys(e, choices().map((c) => c.code), selected(), (code) => setView("preferences", "language", code))}
                >
                  {choice.name}
                </button>
              )}
            </For>
          </div>
          <Show when={activeLocale() !== "en"}>
            <div class="st-desc">{t("common-draftNotice", "Translation preview: native-speaker review is pending.")}</div>
            <div class="st-desc">In this browser, only Settings and Usage are translated so far.</div>
          </Show>
          <Show when={languageLoadError()}>{(text) => <div class="st-desc" role="alert">{text()}</div>}</Show>
        </div>
      </Card>
    </Group>
  );
}

export const THEMES = [
  { value: "system" as const, label: () => t("common-system", "System") },
  { value: "light" as const, label: () => t("settings-settings_page_light", "Light") },
  { value: "dark" as const, label: () => t("settings-settings_page_dark", "Dark") },
];

function Theme() {
  const description = () => t("settings-settings_page_choose_the_app_theme_or_follow_your_system_setting", "Choose the app theme, or follow your system setting.");
  return (
    <Group title={t("settings-settings_page_appearance", "Appearance")} description={description()}>
      <Card>
        <Row title={t("settings-settings_page_theme", "Theme")} description={description()} top>
          <Segmented
            label={t("settings-settings_page_theme", "Theme")}
            value={view.preferences.theme}
            options={THEMES.map((o) => ({ value: o.value, label: o.label() }))}
            onChange={(value) => setView("preferences", "theme", value)}
          />
        </Row>
      </Card>
    </Group>
  );
}

function Notices() {
  const sound = () => pref("sessionSounds") && pref("completionSound");
  return (
    <Group title={t("settings-settings_page_notifications", "Notifications")}>
      <Card>
        <Row
          top
          title={t("settings-settings_page_provider_update_notifications", "Provider update notifications")}
          description={t(
            "settings-settings_page_provider_update_notifications_description",
            "Show a notification when a newer version of an agent provider is available. Off by default.",
          )}
        >
          <Switch
            label={t("settings-settings_page_provider_update_notifications", "Provider update notifications")}
            checked={pref("providerUpdateNotifications")}
            onChange={(value) => setPref("providerUpdateNotifications", value)}
          />
        </Row>
        <div class="st-row" style={{ "flex-direction": "column", "align-items": "stretch", gap: "8px" }}>
          <div style={{ display: "flex", gap: "10px", "align-items": "flex-start" }}>
            <div class="st-row-text">
              <div class="st-row-title">{t("settings-settings_page_agent_completion_sound", "Agent completion sound")}</div>
              <div class="st-desc">
                {t(
                  "settings-settings_page_play_a_short_sound_when_any_agent_turn_finishes_including_agents_worki",
                  "Play a short sound when any agent turn finishes, including agents working in background chats.",
                )}
              </div>
            </div>
            <Switch
              label={t("settings-settings_page_agent_completion_sound", "Agent completion sound")}
              checked={sound()}
              onChange={(enable) => {
                setPref("completionSound", enable);
                if (enable) setPref("sessionSounds", true);
              }}
            />
          </div>
          <CompletionSoundPicker disabled={!sound()} />
        </div>
      </Card>
    </Group>
  );
}

function CompletionSoundPicker(props: { disabled: boolean }) {
  return (
    <div class="st-sound" data-off={props.disabled ? "true" : undefined}>
      <Icon name="volume-loud" size={16} />
      <Select<CompletionPreset>
        label="Completion sound preset"
        value={pref("completionPreset")}
        options={PRESETS.map((p) => ({ value: p, label: PRESET_LABELS[p] }))}
        onChange={(value) => {
          setPref("completionPreset", value);
          playCompletion(value);
        }}
        width={160}
        disabled={props.disabled}
      />
      <ActionButton disabled={props.disabled} onClick={() => playCompletion(pref("completionPreset"))}>
        {t("settings-settings_page_preview", "Preview")}
      </ActionButton>
    </div>
  );
}

function ThreadNaming() {
  onMount(() => void loadNaming());
  createEffect(on(() => state.epoch, () => void loadNaming(), { defer: true }));
  const enabled = () => state.connection.kind === "connected" && !!general.naming && !general.namingSaving;
  return (
    <Group
      title={t("native-thread_naming", "Thread naming")}
      description={t(
        "native-choose_which_agent_and_model_automatically_name_new_threads_on_this_computer",
        "Choose which agent and model automatically name new threads on this computer.",
      )}
    >
      <Card>
        <Row
          top
          title={t("native-title_model", "Title model")}
          description={
            general.naming?.harness
              ? "A small model keeps titles fast and cheap."
              : t(
                  "native-follow_the_thread_s_agent_or_choose_a_specific_model_this_does_not_change_your_chat_model",
                  "Follow the thread’s agent, or choose a specific model. This does not change your chat model.",
                )
          }
        >
          <ModelPicker
            label="Thread naming model"
            harness={general.naming?.harness ?? null}
            model={general.naming?.model ?? null}
            noneLabel={t("native-follow_thread_agent", "Follow thread agent")}
            agentDefaultLabel="Agent default"
            placeholder={general.namingSaving ? "Saving…" : general.namingLoading ? "Loading…" : "Unavailable"}
            disabled={!enabled()}
            onChange={(harness, model) => void saveNaming(harness, model)}
          />
        </Row>
        <Show when={general.naming?.harness || general.namingError}>
          <div class="st-row" style={{ "justify-content": "flex-end" }}>
            <Show when={general.namingError && !general.naming}>
              <ActionButton onClick={() => void loadNaming()}>Retry</ActionButton>
            </Show>
            <Show when={general.naming?.harness}>
              <ActionButton disabled={!enabled()} onClick={() => void saveNaming(null, null)}>
                {t("native-follow_thread_agent", "Follow thread agent")}
              </ActionButton>
            </Show>
          </div>
        </Show>
      </Card>
      <Show when={general.namingError}>{(text) => <ErrorStrip>{text()}</ErrorStrip>}</Show>
    </Group>
  );
}

function Worktrees() {
  onMount(() => void loadWorktrees());
  createEffect(on(() => state.epoch, () => void loadWorktrees(), { defer: true }));
  const [draft, setDraft] = createSignal<string | null>(null);
  const status = () => general.worktrees;
  const locked = () => !!status()?.environmentOverride;
  const custom = () => status()?.settings.useCustomDirectory ?? false;
  const editable = () => !!status() && !locked() && !general.worktreesSaving && state.connection.kind === "connected";
  const path = () => draft() ?? status()?.settings.customDirectory ?? "";
  const [localError, setLocalError] = createSignal<string | null>(null);

  const save = async (use: boolean, directory: string | null) => {
    setLocalError(null);
    if (use && !directory?.trim()) {
      setLocalError("Choose a folder or enter an absolute path first.");
      return;
    }
    if (await saveWorktrees(use, directory?.trim() || null)) {
      setDraft(null);
      closeFolders();
    }
  };

  return (
    <Group title={t("settings-settings_repository_worktrees", "Worktrees")} description="Where Zuse creates new worktrees on this computer.">
      <Card>
        <Show
          when={status()}
          fallback={
            <Row
              title={general.worktreesError ? "Couldn't load worktree settings" : "Loading…"}
              description={general.worktreesError ?? undefined}
            >
              <Show when={general.worktreesError}>
                <ActionButton onClick={() => void loadWorktrees()}>Retry</ActionButton>
              </Show>
            </Row>
          }
        >
          {(s) => (
            <>
              <Row
                title="Location"
                description={
                  <>
                    <span class="st-mono">{s().effectiveDirectory}</span>
                    <Show when={locked()}>
                      <span> · Set by ZUSE_WORKTREES_DIR</span>
                    </Show>
                  </>
                }
              >
                <Show when={custom() && !locked()}>
                  <ActionButton tone="quiet" disabled={!editable()} onClick={() => void save(false, null)}>
                    Reset to default
                  </ActionButton>
                </Show>
              </Row>
              <Row title="Use a custom folder" description={`Default: ${s().defaultDirectory}`}>
                <Switch
                  label="Use a custom worktree folder"
                  checked={custom() || draft() !== null}
                  disabled={!editable()}
                  onChange={(on) => {
                    setLocalError(null);
                    setGeneral("worktreesError", null);
                    if (on) setDraft(s().settings.customDirectory ?? "");
                    else {
                      setDraft(null);
                      closeFolders();
                      if (custom()) void save(false, s().settings.customDirectory ?? null);
                    }
                  }}
                />
              </Row>
              <Show when={custom() || draft() !== null}>
                <div class="st-row" style={{ "flex-direction": "column", "align-items": "stretch", gap: "8px" }}>
                  <div style={{ display: "flex", gap: "8px" }}>
                    <input
                      class="st-field"
                      data-mono="true"
                      aria-label="Worktree folder"
                      placeholder="Choose a folder for new worktrees"
                      value={path()}
                      disabled={!editable()}
                      onInput={(e) => setDraft(e.currentTarget.value)}
                      onKeyDown={(e) => e.key === "Enter" && void save(true, path())}
                    />
                    <ActionButton disabled={!editable()} onClick={() => void browseFolders(path() || s().effectiveDirectory, true)}>
                      Browse…
                    </ActionButton>
                    <ActionButton tone="solid" disabled={!editable() || (path() === (s().settings.customDirectory ?? "") && custom())} onClick={() => void save(true, path())}>
                      {general.worktreesSaving ? "Saving…" : "Save"}
                    </ActionButton>
                  </div>
                  <FolderBrowser onChoose={(chosen) => setDraft(chosen)} />
                </div>
              </Show>
            </>
          )}
        </Show>
      </Card>
      <Show when={localError() || (status() && general.worktreesError)}>{(text) => <ErrorStrip>{text()}</ErrorStrip>}</Show>
    </Group>
  );
}

function FolderBrowser(props: { onChoose: (path: string) => void }) {
  const listing = () => general.folders;
  return (
    <Show when={listing() || general.foldersLoading || general.foldersError}>
      <div class="st-folders">
        <div class="st-folders-head">
          <button
            type="button"
            class="st-icon-action"
            aria-label="Parent folder"
            disabled={!listing() || !parentPath(listing()!.path)}
            onClick={() => {
              const parent = listing() && parentPath(listing()!.path);
              if (parent) void browseFolders(parent);
            }}
          >
            <Icon name="arrow-up" size={14} />
          </button>
          <button type="button" class="st-icon-action" aria-label="Home folder" onClick={() => void browseFolders()}>
            <Icon name="home" size={14} />
          </button>
          <span title={listing()?.path}>{listing()?.path ?? ""}</span>
        </div>
        <div class="st-folders-list" role="list" aria-label="Folders" aria-busy={general.foldersLoading}>
          <Show when={!general.foldersLoading} fallback={<div class="st-empty">Loading folders…</div>}>
            <Show when={!general.foldersError} fallback={<div class="st-empty">{general.foldersError}</div>}>
              <For each={listing()?.entries.filter((e) => e.isDir) ?? []} fallback={<div class="st-empty">No subfolders</div>}>
                {(entry) => (
                  <button type="button" role="listitem" class="st-folder" onClick={() => void browseFolders(joinPath(listing()!.path, entry.name))}>
                    <Icon name={entry.isRepo ? "git-branch" : "folder"} size={14} />
                    {entry.name}
                  </button>
                )}
              </For>
              <Show when={listing()?.truncated}>
                <div class="st-empty">More folders exist than are shown here.</div>
              </Show>
            </Show>
          </Show>
        </div>
        <div class="st-folders-foot">
          <ActionButton tone="quiet" onClick={closeFolders}>
            Cancel
          </ActionButton>
          <ActionButton
            disabled={!listing()}
            onClick={() => {
              props.onChoose(listing()!.path);
              closeFolders();
            }}
          >
            Use this folder
          </ActionButton>
        </div>
      </div>
    </Show>
  );
}

function About() {
  const [applying, setApplying] = createSignal(false);
  const [confirming, setConfirming] = createSignal(false);
  const status = () => data.appUpdate;
  const apply = async () => {
    if (applying()) return;
    setApplying(true);
    try {
      await rpc.call(M.APPLY_UPDATE, {});
      setConfirming(false);
      toast("Update installed. Zuse restarts to finish.");
    } catch (error) {
      setConfirming(false);
      toast(`Couldn't install the update — ${message(error)}`, "danger");
    } finally {
      setApplying(false);
    }
  };
  return (
    <Show when={status()}>
      {(s) => (
        <Group title={t("native-about_zuse", "About Zuse")}>
          <Card>
            <Row
              title={t("native-app_updates", "App updates")}
              description={
                s().error
                  ? // The engine's reason is a long transport chain; it stays available on hover.
                    <span title={s().error}>Version {s().currentVersion} · Couldn't check for updates</span>
                  : s().updateAvailable && s().latestVersion
                    ? `Version ${s().currentVersion} · v${s().latestVersion} available`
                    : `Version ${s().currentVersion} · Up to date${s().checkedAt ? ` as of ${new Date(s().checkedAt!).toLocaleString()}` : ""}`
              }
            >
              <Show when={s().updateAvailable && s().latestVersion}>
                <ActionButton onClick={() => setConfirming(true)}>Update to v{s().latestVersion}</ActionButton>
              </Show>
            </Row>
          </Card>
          <SettingsDialog
            open={confirming()}
            onClose={() => !applying() && setConfirming(false)}
            title={`Update Zuse to v${s().latestVersion ?? ""}?`}
            description="The engine on this computer installs the update and restarts. Running agents stop, and this page reconnects when it is back."
            footer={
              <>
                <ActionButton tone="quiet" disabled={applying()} onClick={() => setConfirming(false)}>
                  Cancel
                </ActionButton>
                <ActionButton tone="solid" disabled={applying()} onClick={() => void apply()}>
                  {applying() ? "Updating…" : "Update and restart"}
                </ActionButton>
              </>
            }
          />
        </Group>
      )}
    </Show>
  );
}
