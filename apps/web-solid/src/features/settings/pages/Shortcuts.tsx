// Keyboard shortcuts in effect in this browser. Lists what core/keys has registered, so a shortcut only
// appears once something handles it. Bindings are fixed here; the browser reserves several desktop chords.
import { For, Show } from "solid-js";
import { formatKeys, listShortcuts } from "@/core/keys";
import { t } from "../i18n";
import { Card, Group, SectionTitle } from "../widgets";

interface Binding {
  label: string;
  keys: string[];
}

const SETTINGS: Binding[] = [
  { label: "Move between sections", keys: ["↑", "↓"] },
  { label: "First or last section", keys: ["Home", "End"] },
  { label: "Close settings", keys: ["Esc"] },
];

function Rows(props: { rows: Binding[]; empty?: string }) {
  return (
    <Card>
      <Show when={props.rows.length} fallback={<div class="st-empty">{props.empty}</div>}>
        <dl style={{ margin: 0 }}>
          <For each={props.rows}>
            {(row) => (
              <div class="st-row">
                <dt class="st-row-text st-row-title">
                  {row.label}
                </dt>
                <dd class="st-row-control" style={{ margin: 0, gap: "4px" }}>
                  <For each={row.keys}>{(key) => <kbd class="st-kbd">{key}</kbd>}</For>
                </dd>
              </div>
            )}
          </For>
        </dl>
      </Show>
    </Card>
  );
}

export function Shortcuts() {
  // Read once: the registry is a plain set filled when the app starts.
  const app: Binding[] = listShortcuts()
    .map((s) => ({ label: s.label, keys: [formatKeys(s.keys)] }))
    .sort((a, b) => a.label.localeCompare(b.label));
  return (
    <>
      <SectionTitle
        title={t("settings-settings_page_keyboard_shortcuts", "Keyboard shortcuts")}
        description={t("native-keyboard_shortcuts_for_the_browser_workspace", "Keyboard shortcuts for the browser workspace.")}
      />
      <div class="st-stack">
        <Group title="Application" description="Ctrl is ⌘ on macOS. Shortcuts the browser keeps for itself, such as new window and new tab, stay with the browser.">
          <Rows rows={app} empty="No application shortcuts are registered yet." />
        </Group>
        <Group title={t("common-settings", "Settings")}>
          <Rows rows={SETTINGS} />
        </Group>
      </div>
    </>
  );
}
