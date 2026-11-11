// One model menu for new-chat defaults and thread naming: models grouped under each enabled agent.
import { Select as KSelect } from "@kobalte/core/select";
import { createEffect, createMemo, Show } from "solid-js";
import { Icon } from "@/ui/Icon";
import { data, harnessList, loadModels } from "./data";
import { descriptorEnabled, harnessMark, harnessTint } from "./harness";

interface Choice {
  key: string;
  harness: string | null;
  model: string | null;
  label: string;
  detail?: string;
  disabled?: boolean;
}
interface Group {
  label: string;
  harness: string | null;
  options: Choice[];
}

export function ModelPicker(props: {
  harness: string | null;
  model: string | null;
  /** Label of the "no specific model" choice; omitted when one must be chosen. */
  noneLabel?: string;
  /** Label for an agent's own default model. */
  agentDefaultLabel?: string;
  onChange: (harness: string | null, model: string | null) => void;
  label: string;
  disabled?: boolean;
  placeholder?: string;
  width?: number;
}) {
  const offered = createMemo(() => harnessList().filter((h) => h.id !== "mock" && descriptorEnabled(h) && h.installed !== false));
  createEffect(() => {
    for (const h of offered()) void loadModels(h.id);
  });

  const groups = createMemo<Group[]>(() => {
    const out: Group[] = [];
    if (props.noneLabel) out.push({ label: "", harness: null, options: [{ key: "none", harness: null, model: null, label: props.noneLabel }] });
    for (const h of offered()) {
      const models = data.models[h.id] ?? [];
      const options: Choice[] = [];
      if (props.agentDefaultLabel) options.push({ key: `${h.id}|`, harness: h.id, model: null, label: props.agentDefaultLabel });
      for (const m of models) options.push({ key: `${h.id}|${m.id}`, harness: h.id, model: m.id, label: m.label || m.id, detail: m.description });
      // The saved choice stays visible even while its catalog is loading or no longer lists it.
      if (props.harness === h.id && props.model && !models.some((m) => m.id === props.model))
        options.push({ key: `${h.id}|${props.model}`, harness: h.id, model: props.model, label: props.model });
      if (!options.length) {
        const error = data.modelErrors[h.id];
        options.push({ key: `${h.id}|?`, harness: h.id, model: null, label: error ? "Models unavailable" : "Loading models…", detail: error, disabled: true });
      }
      out.push({ label: h.name || h.id, harness: h.id, options });
    }
    return out;
  });

  const selectedKey = () => (props.harness ? `${props.harness}|${props.model ?? ""}` : props.noneLabel ? "none" : null);
  const selected = createMemo(() => {
    const key = selectedKey();
    for (const group of groups()) for (const option of group.options) if (option.key === key) return option;
    return null;
  });
  const triggerLabel = () => {
    const option = selected();
    if (!option) return props.placeholder ?? "Choose a model";
    if (!option.harness) return option.label;
    const name = harnessList().find((h) => h.id === option.harness)?.name ?? option.harness;
    return option.model ? option.label : `${name} · ${option.label}`;
  };

  return (
    <KSelect<Choice, Group>
      options={groups()}
      optionValue="key"
      optionTextValue="label"
      optionDisabled="disabled"
      optionGroupChildren="options"
      value={selected()}
      onChange={(option) => option && option.key !== selectedKey() && props.onChange(option.harness, option.model)}
      disabled={props.disabled}
      gutter={4}
      placement="bottom-end"
      sectionComponent={(section) => (
        <Show when={section.section.rawValue.label}>
          <KSelect.Section class="st-select-section">
            <Show when={section.section.rawValue.harness}>
              {(id) => (
                <span style={{ color: harnessTint(id()) }}>
                  <Icon name={harnessMark(id())} size={12} />
                </span>
              )}
            </Show>
            {section.section.rawValue.label}
          </KSelect.Section>
        </Show>
      )}
      itemComponent={(item) => (
        <KSelect.Item item={item.item} class="st-select-item">
          <span class="st-select-check">
            <KSelect.ItemIndicator>
              <Icon name="check" size={14} />
            </KSelect.ItemIndicator>
          </span>
          <span class="st-select-item-text">
            <KSelect.ItemLabel>{item.item.rawValue.label}</KSelect.ItemLabel>
            <Show when={item.item.rawValue.detail}>
              <KSelect.ItemDescription class="st-select-detail">{item.item.rawValue.detail}</KSelect.ItemDescription>
            </Show>
          </span>
        </KSelect.Item>
      )}
    >
      <KSelect.Trigger class="st-select" aria-label={props.label} style={{ width: `${props.width ?? 256}px`, "max-width": "100%" }}>
        <Show when={selected()?.harness}>
          {(id) => (
            <span style={{ color: harnessTint(id()), display: "inline-flex" }}>
              <Icon name={harnessMark(id())} size={14} />
            </span>
          )}
        </Show>
        <span class="st-select-value">{triggerLabel()}</span>
        <KSelect.Icon class="st-select-icon">
          <Icon name="alt-arrow-down" size={14} />
        </KSelect.Icon>
      </KSelect.Trigger>
      <KSelect.Portal>
        <KSelect.Content class="st-select-menu st-model-menu">
          <KSelect.Listbox class="st-select-list" />
        </KSelect.Content>
      </KSelect.Portal>
    </KSelect>
  );
}

/** Agents that can start a chat right now, for empty states. */
export const offeredAgents = () => harnessList().filter((h) => h.id !== "mock" && descriptorEnabled(h) && h.installed !== false);
