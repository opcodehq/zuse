// One model menu for new-chat defaults and thread naming: models grouped under each enabled agent.
import { ModelSelect } from "@/ui/ModelSelect";
import { createEffect, createMemo } from "solid-js";
import { data, harnessList, loadModels } from "./data";
import { descriptorEnabled } from "./harness";

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
  createEffect(() => offered().map(harness => harness.id), () => {
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
  return <ModelSelect label={props.label} value={selectedKey() ?? ""} disabled={props.disabled} placeholder={props.placeholder} width={props.width ?? 256}
    options={groups().flatMap(group => group.options.map(option => ({ key: option.key, label: option.label, provider: option.harness ?? "", providerLabel: group.label, description: option.detail, disabled: option.disabled })))}
    onChange={key => { const option = groups().flatMap(group => group.options).find(option => option.key === key); if (option && key !== selectedKey()) props.onChange(option.harness, option.model); }} />;
}

/** Agents that can start a chat right now, for empty states. */
export const offeredAgents = () => harnessList().filter((h) => h.id !== "mock" && descriptorEnabled(h) && h.installed !== false);
