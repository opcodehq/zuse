// Notifications: session sounds and browser notifications (crates/ui/src/settings/notifications.rs).
import { Show } from "solid-js";
import { notificationPermission, requestNotificationPermission } from "../alerts";
import { t } from "../i18n";
import { pref, setPref, type SettingsPrefs } from "../prefs";
import { playAttention, playCompletion, playRequest } from "../sound";
import { ActionButton, Card, Group, Row, SectionTitle, Switch, WarningStrip } from "../widgets";

type Toggle = { [K in keyof SettingsPrefs]: SettingsPrefs[K] extends boolean ? K : never }[keyof SettingsPrefs];

const PARENT_OFF = "Unavailable while its parent setting is off";

function ToggleRow(props: { name: Toggle; title: string; description?: string; parent?: boolean; label?: string; preview?: () => void }) {
  const inert = () => props.parent === false;
  return (
    <Row title={props.title} description={props.description} class={inert() ? "st-inert" : undefined}>
      <Show when={props.preview}>
        {(play) => (
          <ActionButton tone="quiet" disabled={inert() || !pref(props.name)} aria-label={`Preview ${props.label ?? props.title}`} onClick={() => play()()}>
            {t("settings-settings_page_preview", "Preview")}
          </ActionButton>
        )}
      </Show>
      <Switch
        label={props.label ?? props.title}
        checked={pref(props.name)}
        disabled={inert()}
        description={inert() ? PARENT_OFF : undefined}
        onChange={(value) => setPref(props.name, value)}
      />
    </Row>
  );
}

/** Desktop order: banners first, then sounds (crates/ui/src/settings/notifications.rs). */
export function Notifications() {
  const sounds = () => pref("sessionSounds");
  const banners = () => pref("browserNotifications") && notificationPermission() === "granted";
  const enableBanners = async (on: boolean) => {
    if (!on) return setPref("browserNotifications", false);
    const result = notificationPermission() === "granted" ? "granted" : await requestNotificationPermission();
    setPref("browserNotifications", result === "granted");
  };
  return (
    <>
      <SectionTitle
        title={t("settings-settings_page_notifications", "Notifications")}
        description={t("native-choose_when_zuse_alerts_you_about_agent_activity", "Choose when Zuse alerts you about agent activity.")}
      />
      <div class="st-stack">
        <Group title="Desktop" description="System banners from this browser when a run finishes, fails or needs your input.">
          <Card>
            <Row
              title={t("native-desktop_notifications", "Desktop notifications")}
              description={
                notificationPermission() === "unsupported"
                  ? "This browser doesn't support notifications."
                  : notificationPermission() === "denied"
                    ? "Notifications are blocked for this site. Allow them in your browser's site settings."
                    : notificationPermission() === "granted"
                      ? undefined
                      : "Your browser asks for permission the first time."
              }
            >
              <Switch
                label={t("native-desktop_notifications", "Desktop notifications")}
                checked={banners()}
                disabled={notificationPermission() === "unsupported" || notificationPermission() === "denied"}
                onChange={(on) => void enableBanners(on)}
              />
            </Row>
            <ToggleRow
              name="notifyInBackgroundOnly"
              title={t("native-only_when_in_the_background", "Only when in the background")}
              label="Only notify when Zuse is in the background"
              parent={banners()}
            />
            <ToggleRow
              name="providerUpdateNotifications"
              title={t("native-agent_updates", "Agent updates")}
              description="Show a notice when monitored agent CLIs have updates."
              label="Agent update notifications"
            />
          </Card>
          <Show when={pref("browserNotifications") && notificationPermission() !== "granted" && notificationPermission() !== "unsupported"}>
            <WarningStrip>Notifications are turned on here, but this browser hasn't allowed them for this site.</WarningStrip>
          </Show>
        </Group>
        <Group title="Sounds">
          <Card>
            <ToggleRow name="sessionSounds" title={t("native-session_sounds", "Session sounds")} />
            <ToggleRow
              name="completionSound"
              title={t("native-task_completed", "Task completed")}
              label="Task completed sound"
              parent={sounds()}
              preview={() => playCompletion(pref("completionPreset"))}
            />
            <ToggleRow name="inputSound" title={t("native-input_required", "Input required")} label="Input required sound" parent={sounds()} preview={playRequest} />
            <ToggleRow
              name="attentionSound"
              title={t("native-errors_and_disconnections", "Errors and disconnections")}
              label="Errors and disconnections sound"
              parent={sounds()}
              preview={playAttention}
            />
          </Card>
        </Group>
      </div>
    </>
  );
}
