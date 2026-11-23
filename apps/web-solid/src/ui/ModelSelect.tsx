// Desktop model-menu metrics and shared glyphs, usable with any engine catalog.
import { Popover } from "@kobalte/core/popover";
import { createMemo, createSignal, For, Show } from "solid-js";
import { Icon } from "./Icon";
import { harnessIcon, harnessTint } from "./harness";
import "./model-select.css";
export interface ModelChoice { key: string; label: string; provider: string; providerLabel: string; description?: string; disabled?: boolean }
export function ModelSelect(props: { options: ModelChoice[]; value: string; onChange: (key: string) => void; label: string; disabled?: boolean; placeholder?: string; width?: number }) {
  const [open, setOpen] = createSignal(false);
  const [query, setQuery] = createSignal("");
  const [provider, setProvider] = createSignal("");
  const selected = () => props.options.find(row => row.key === props.value);
  const providers = createMemo(() => [...new Map(props.options.map(row => [row.provider, row.providerLabel])).entries()].filter(row => row[0]));
  const rows = createMemo(() => props.options.filter(row => (!provider() || row.provider === provider()) && `${row.label} ${row.providerLabel} ${row.description ?? ""}`.toLowerCase().includes(query().toLowerCase())));
  const pick = (row: ModelChoice) => { if (row.disabled) return; props.onChange(row.key); setOpen(false); };
  return <Popover open={open()} onOpenChange={value => { setOpen(value); if (value) { setQuery(""); setProvider(""); } }} placement="bottom-start" gutter={6}>
    <Popover.Trigger class="model-select-trigger" aria-label={props.label} disabled={props.disabled} style={{ width: props.width ? `${props.width}px` : undefined }}>
      <Icon name={harnessIcon(selected()?.provider, "tool-agent")} size={14} /><span>{selected()?.label ?? props.placeholder ?? "Choose a model"}</span><Icon name="alt-arrow-down" size={12} />
    </Popover.Trigger>
    <Popover.Portal><Popover.Content class="model-select-popover" aria-label={props.label}>
      <div class="model-select-rail" aria-label="Filter by provider">
        <button type="button" class="model-select-provider" aria-pressed={!provider() ? "true" : "false"} onClick={() => setProvider("")}>All</button>
        <For each={providers()}>{row => <button type="button" class="model-select-provider" aria-label={row[1] || "Defaults"} title={row[1] || "Defaults"} aria-pressed={provider() === row[0] ? "true" : "false"} onClick={() => setProvider(row[0])}><Icon name={harnessIcon(row[0], "tool-agent")} size={16} style={{ color: harnessTint(row[0]) }} /></button>}</For>
      </div>
      <div class="model-select-body">
        <label class="model-select-search"><Icon name="palette-search" size={14} /><input autofocus aria-label="Search models" placeholder="Search models…" value={query()} onInput={e => setQuery(e.currentTarget.value)} onKeyDown={e => { if (e.key === "ArrowDown") { e.preventDefault(); e.currentTarget.closest('.model-select-body')?.querySelector<HTMLButtonElement>('.model-select-row:not(:disabled)')?.focus(); } if (e.key === "Enter" && rows().length === 1) { e.preventDefault(); pick(rows()[0]!); } }} /><kbd>esc</kbd></label>
        <div class="model-select-list" onKeyDown={e => {
          if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
          e.preventDefault();
          const buttons = [...e.currentTarget.querySelectorAll<HTMLButtonElement>('.model-select-row:not(:disabled)')];
          const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
          buttons[e.key === 'Home' ? 0 : e.key === 'End' ? buttons.length - 1 : (index + (e.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length]?.focus();
        }}>
          <Show when={rows().length} fallback={<p class="model-select-empty">No models found.</p>}>
            <For each={rows()}>{(row, index) => <>
              <Show when={index() === 0 || rows()[index() - 1]?.provider !== row.provider}><div class="model-select-heading"><Icon name={harnessIcon(row.provider, "tool-agent")} size={14} /><span>{row.providerLabel || "Defaults"}</span><Show when={selected()?.provider === row.provider}><small>Current</small></Show></div></Show>
              <button type="button" class="model-select-row" disabled={row.disabled} aria-pressed={props.value === row.key ? "true" : "false"} onClick={() => pick(row)} title={row.description}><span>{row.label}</span><Show when={props.value === row.key}><Icon name="check" size={13} /></Show></button>
            </>}</For>
          </Show>
        </div>
        <footer class="model-select-footer"><span>↑ ↓ to navigate</span><span>↵ to select</span></footer>
      </div>
    </Popover.Content></Popover.Portal>
  </Popover>;
}
