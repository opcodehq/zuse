import { createMemo } from "solid-js";
import { harnessList, modelList, offeredHarnesses } from "@/features/composer/catalog";
import type { ZapperModel } from "@/core/zapper-types";
import { ModelSelect, type ModelChoice } from "@/ui/ModelSelect";
import { Menu } from "@/ui/primitives";
import { Icon } from "@/ui/Icon";
import type { JSX } from "@solidjs/web";

export function ZapperSelect(props: { label: string; value: string; options: { value: string; label: string; icon?: string; leading?: JSX.Element }[]; onChange: (value: string) => void; disabled?: boolean }) {
  const selected = () => props.options.find(row => row.value === props.value);
  return <Menu placement="bottom-start" trigger={attrs => <button {...attrs} type="button" class="zp-select" aria-label={props.label} disabled={props.disabled}>{selected()?.leading ?? <Icon name={selected()?.icon ?? "list"} size={13} />}<span>{selected()?.label ?? props.value}</span><Icon name="alt-arrow-down" size={12} /></button>} items={props.options.map(row => ({ label: row.label, icon: row.icon, leading: row.leading, onSelect: () => props.onChange(row.value) }))} />;
}

/** Catalogs come from the selected execution device, never the browser/controller. */
export function ZapperModelPicker(props: { deviceId: string; value: ZapperModel; onChange: (value: ZapperModel) => void; label: string }) {
  const options = createMemo(() => {
    const offered = harnessList(props.deviceId);
    const rows: ModelChoice[] = [];
    if (offered.status === "ready") for (const harness of offeredHarnesses(offered.value)) {
      rows.push({ key: `${harness.id}|`, label: "Agent default", provider: harness.id, providerLabel: harness.name || harness.id });
      const models = modelList(props.deviceId, harness.id);
      if (models.status === "ready") for (const model of models.value) rows.push({ key: `${harness.id}|${model.id}`, label: model.label, provider: harness.id, providerLabel: harness.name || harness.id, description: model.description ?? undefined });
    }
    const key = `${props.value.harness}|${props.value.model ?? ""}`;
    if (!rows.some(row => row.key === key)) rows.push({key, label: props.value.model || "Agent default", provider: props.value.harness, providerLabel: props.value.harness});
    return rows;
  });
  return <ModelSelect modal label={props.label} value={`${props.value.harness}|${props.value.model ?? ""}`} options={options()} onChange={key => { const index = key.indexOf("|"); props.onChange({harness: key.slice(0,index), model: key.slice(index+1) || null}); }} />;
}
