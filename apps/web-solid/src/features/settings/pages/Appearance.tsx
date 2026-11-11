// Appearance: theme cards and motion (crates/ui/src/settings/appearance.rs).
import { For, Show } from "solid-js";
import { setView, view } from "@/core/view";
import { systemReducedMotion } from "../alerts";
import { t } from "../i18n";
import { pref, type ReduceMotion, setPref } from "../prefs";
import { Card, Group, radioKeys, Row, SectionTitle, Select } from "../widgets";
import { THEMES } from "./General";

function Preview(props: { scheme: "light" | "dark" }) {
  return (
    <div class="st-preview" data-scheme={props.scheme}>
      <div class="st-preview-side">
        <div class="st-preview-line" style={{ width: "80%" }} />
        <div class="st-preview-line" style={{ width: "60%" }} />
        <div class="st-preview-line" style={{ width: "70%" }} />
      </div>
      <div class="st-preview-main">
        <div class="st-preview-accent" />
        <div class="st-preview-composer" />
      </div>
    </div>
  );
}

const MOTION: { value: ReduceMotion; label: string }[] = [
  { value: "system", label: "System" },
  { value: "on", label: "On" },
  { value: "off", label: "Off" },
];

function motionHelper(choice: ReduceMotion, systemReduced: boolean) {
  if (choice === "on") return "Animations skip straight to their final state.";
  if (choice === "off") return "Animations play even if the system asks for less motion.";
  return systemReduced ? "Following reduced system motion." : "Following the system, which currently allows motion.";
}

export function Appearance() {
  const title = () => t("settings-settings_page_theme", "Theme");
  return (
    <>
      <SectionTitle
        title={t("settings-settings_page_appearance", "Appearance")}
        description={t("native-themes_colors_fonts_and_motion_for_this_app", "Themes, colors, fonts and motion for this app.")}
      />
      <div class="st-stack">
        <Group title={title()} description={t("native-follow_your_system_or_pin_a_light_or_dark_scheme", "Follow your system, or pin a light or dark scheme.")}>
          <Card>
            <div class="st-options" role="radiogroup" aria-label={title()}>
              <For each={THEMES}>
                {(option) => (
                  <button
                    type="button"
                    role="radio"
                    class="st-option"
                    aria-checked={view.preferences.theme === option.value}
                    tabIndex={view.preferences.theme === option.value ? 0 : -1}
                    onClick={() => setView("preferences", "theme", option.value)}
                    onKeyDown={(e) => radioKeys(e, THEMES.map((o) => o.value), view.preferences.theme, (v) => setView("preferences", "theme", v))}
                  >
                    <div class="st-option-preview">
                      <Show when={option.value === "system"} fallback={<Preview scheme={option.value === "light" ? "light" : "dark"} />}>
                        <Preview scheme="light" />
                        <Preview scheme="dark" />
                      </Show>
                    </div>
                    <span class="st-option-label">
                      <span class="st-radio" aria-hidden="true" />
                      {option.label()}
                    </span>
                  </button>
                )}
              </For>
            </div>
          </Card>
        </Group>
        <Group title={t("native-motion", "Motion")}>
          <Card>
            <Row title="Reduce motion" description={motionHelper(pref("reduceMotion"), systemReducedMotion())}>
              <Select<ReduceMotion> label="Reduce motion" value={pref("reduceMotion")} options={MOTION} onChange={(value) => setPref("reduceMotion", value)} width={128} />
            </Row>
          </Card>
        </Group>
      </div>
    </>
  );
}
