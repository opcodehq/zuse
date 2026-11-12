// Default models: how new chats start (crates/ui/src/settings/defaults.rs). The composer reads the same
// remembered defaults, so a choice here is what the next new chat offers.
import { Show } from "solid-js";
import { automaticReviewSupported, defaults, type Sandbox, updateDefaults } from "@/features/composer/catalog";
import { data } from "../data";
import { providerName } from "../harness";
import { t } from "../i18n";
import { ModelPicker, offeredAgents } from "../ModelPicker";
import { PERMISSION_MODES } from "../prefs";
import { Card, Group, Row, SectionTitle, Select, WarningStrip } from "../widgets";

export function Defaults() {
  const harness = () => defaults().harness ?? null;
  const model = () => (harness() ? (defaults().models[harness()!]?.id ?? null) : null);
  const sandbox = (): Sandbox => defaults().sandbox ?? "workspace-write";
  const harnessName = () => (harness() ? (offeredAgents().find((h) => h.id === harness())?.name ?? providerName(harness()!)) : null);

  return (
    <>
      <SectionTitle
        title={t("settings-settings_page_default_models", "Default models")}
        description={t("native-choose_how_new_chats_start", "Choose how new chats start.")}
      />
      <div class="st-stack">
        <Group
          title={t("settings-settings_page_chat_defaults", "Chat defaults")}
          description={t(
            "settings-settings_page_these_choices_apply_when_you_start_a_new_chat_you_can_still_change_eit",
            "These choices apply when you start a new chat. You can still change either one from the composer.",
          )}
        >
          <Card>
            <Row
              top
              title={t("settings-settings_page_default_model", "Default model")}
              description={harnessName() ? `Model for new chats · ${harnessName()}` : "Model for new chats."}
            >
              <ModelPicker
                label="Default model"
                harness={harness()}
                model={model()}
                agentDefaultLabel="Agent default"
                placeholder={offeredAgents().length ? "Choose a model" : "No agents enabled"}
                disabled={!offeredAgents().length}
                onChange={(nextHarness, nextModel) =>
                  updateDefaults((draft) => {
                    if (!nextHarness) return;
                    draft.harness = nextHarness;
                    if (nextModel) {
                      const label = data.models[nextHarness]?.find((m) => m.id === nextModel)?.label ?? nextModel;
                      draft.models[nextHarness] = { id: nextModel, label };
                    } else delete draft.models[nextHarness];
                  })
                }
              />
            </Row>
            <Row
              top
              title={t("settings-settings_page_default_permission_mode", "Default permission mode")}
              description={t(
                "settings-settings_page_how_new_chats_handle_tool_calls_each_chat_can_override_this_from_the_c",
                "How new chats handle tool calls. Each chat can override this from the composer.",
              )}
            >
              <Select<Sandbox>
                label="Default permission mode"
                value={sandbox()}
                options={PERMISSION_MODES.map((mode) => ({ value: mode.value, label: mode.label, detail: mode.description }))}
                onChange={(value) => updateDefaults((draft) => (draft.sandbox = value))}
                width={256}
              />
            </Row>
          </Card>
          <Show when={sandbox() === "auto" && harness() && !automaticReviewSupported(harness()!)}>
            <WarningStrip>{harnessName()} can't review actions on its own, so its new chats ask for approval instead.</WarningStrip>
          </Show>
        </Group>
      </div>
    </>
  );
}
