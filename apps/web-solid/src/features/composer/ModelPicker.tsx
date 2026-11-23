// Agent, model and effort picker: the toolbar's model pill and its popover.
// The popover mirrors crates/ui/src/pickers/model_menu.rs with crates/viewport/src/model_picker_style.rs (provider rail,
// search, grouped rows), plus the separate 240px card from crates/ui/src/pickers/model_detail.rs.
import { navigate, setView, view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { Spinner, toast } from "@/ui/primitives";
import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { Popover } from "@kobalte/core/popover";
import { Tooltip as KTooltip } from "@kobalte/core/tooltip";
import { type JSX } from "@solidjs/web";
import { createEffect, createMemo, createSignal, For, onSettled, Show, untrack } from "solid-js";
import { brandIcon, BrandMark } from "./assets";
import { catalog, changeConfig, clampReasoning, defaults, effortOption, fastModeValues, harnessList, modelList, offeredHarnesses, reasoningLabel, resolve, revalidateCatalog, selectedChat, setCatalog, targetDeviceId, updateDefaults, type HarnessInfo, type Model, type ModelOption, type Resolved, } from "./catalog";
import { focusComposer } from "./drafts";

const mac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));
const harnessName = (h: HarnessInfo) => h.name || h.id;

/** One model row; `selectedOnly` is the chat's model when the fresh catalog no longer lists it. */
interface Row { harness: HarnessInfo; model: Model; selectedOnly?: boolean; favoriteSection?: boolean }

const [open, setOpen] = createSignal(false);
let focusModelSearch = () => {};
// A pick or Escape hands the caret back to the prompt; clicking elsewhere leaves focus there.
let returnFocus = false;
/** Open the picker from a shortcut, a slash command or the palette. */
export const openModelPicker = () => { returnFocus = false; setOpen(true); };

function apply(change: Parameters<typeof changeConfig>[0]) {
  changeConfig(change).catch((error) => toast(`Couldn't change chat settings: ${errorText(error)}`, "danger"));
}

/** The effort slider's stops: the reasoning ladder, else an option shaped like one, so every model with an effort gets the same slider. */
interface Effort { labels: string[]; selected: number; pick: (index: number) => void }
function effortOf(r: Resolved, change: typeof apply = apply): Effort | null {
  if (r.ladder.length) {
    const ladder = r.ladder;
    return {
      labels: ladder.map(reasoningLabel),
      selected: Math.max(0, r.reasoning ? ladder.indexOf(r.reasoning) : 0),
      pick: (index) => ladder[index] && ladder[index] !== r.reasoning && change({ reasoning: ladder[index] }),
    };
  }
  const option = r.model && effortOption(r.model);
  if (!option) return null;
  const current = r.options[option.id] ?? option.defaultChoice;
  const at = option.choices.findIndex((c) => c.id === current);
  const selected = at >= 0 ? at : Math.max(0, option.choices.findIndex((c) => c.id === option.defaultChoice));
  return {
    labels: option.choices.map((c) => c.label),
    selected,
    pick: (index) => {
      const choice = option.choices[index];
      if (choice && index !== selected) apply({ option: { id: option.id, value: choice.id, isDefault: choice.id === option.defaultChoice } });
    },
  };
}

/** The model's fast-mode option, its on/off values and whether it is on. */
function fastOf(r: Resolved, change: typeof apply = apply) {
  const option = r.model?.options?.find((o) => fastModeValues(o));
  const values = option && fastModeValues(option);
  if (!option || !values) return null;
  const on = (r.options[option.id] ?? option.defaultChoice) === values.on;
  return { option, on, flip: () => change({ option: { id: option.id, value: on ? values.off : values.on, isDefault: (on ? values.off : values.on) === option.defaultChoice } }) };
}

/** Alt+T: step through the selected model's effort, wrapping back to the lowest. */
export function cycleReasoning() {
  const effort = effortOf(resolve());
  if (!effort || effort.labels.length < 2) return;
  effort.pick((effort.selected + 1) % effort.labels.length);
}

/** Alt+O: flip the model's fast-mode option, when it has one. */
export function toggleFastMode() {
  fastOf(resolve())?.flip();
}

export function ModelPill() {
  const r = createMemo(resolve);
  const fast = () => !!fastOf(r())?.on;
  const label = () => r().modelLabel ?? r().harnessInfo?.name ?? (r().noAgents ? "No agents available" : undefined);
  return (
    <Popover open={open()} onOpenChange={next => { if (next) returnFocus = false; setOpen(next); }} placement="top-end" gutter={8}>
      <Popover.Trigger
        class="cmp-model"
        aria-label={label() ? `Model: ${label()}${r().reasoning ? `, ${reasoningLabel(r().reasoning!)} reasoning` : ""}` : "Choose a model"}
        title={`Switch model (${mac ? "⌥P" : "Alt+P"})`}
      >
        <Show when={r().harness} fallback={<Show when={!r().noAgents}><Spinner label="Loading agents" /></Show>}>
          <BrandMark harness={r().harness} size={12} />
        </Show>
        <Show when={label()} fallback={<span class="cmp-ghost-bar skeleton" />}>
          <span class="cmp-model-label">{label()}</span>
        </Show>
        <Show when={fast()}>
          <Icon name="fast-tier-bold" size={12} label="Fast mode" />
        </Show>
        <Show when={r().reasoning && r().ladder.length}>
          <span class="cmp-model-suffix">{reasoningLabel(r().reasoning!)}</span>
        </Show>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          class="cmp-popover cmp-models"
          aria-label="Model picker"
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            focusModelSearch();
            // Untracked: this runs inside the focus scope's effect, which would remount on every catalog change.
            untrack(revalidateCatalog);
          }}
          onEscapeKeyDown={() => { returnFocus = true; setOpen(false); requestAnimationFrame(focusComposer); }}
          onCloseAutoFocus={(event) => {
            if (!returnFocus) return;
            event.preventDefault();
            focusComposer();
          }}
        >
          <PickerBody close={() => setOpen(false)} />
        </Popover.Content>
      </Popover.Portal>
    </Popover>
  );
}

/** Explicit new-chat default, independent of the active chat and recent picks. */
const isDefaultModel = (harness: string, model: string) => view.preferences.defaultModel?.provider === harness && view.preferences.defaultModel?.model === model;
function makeDefault(row: Row) {
  setView(draft => { draft.preferences.defaultModel = {provider:row.harness.id,model:row.model.id,label:row.model.label}; });
  const r = resolve();
  // On the new-chat canvas the current pick stays put while the default moves.
  if (!selectedChat() && r.harness) setCatalog(draft => { Object.assign(draft["draft"]!, { harness: r.harness, model: r.modelId }); });
  updateDefaults((d) => {
    d.harness = row.harness.id;
    d.models[row.harness.id] = { id: row.model.id, label: row.model.label };
  });
}

function PickerBody(props: { close: () => void }) {
  const r = createMemo(resolve);
  const device = () => targetDeviceId();
  const list = () => harnessList(device());
  const locked = () => r().locked;
  // The rail: agents on offer, plus the chat's own agent when it is installed but switched off. A locked chat keeps its own.
  const providers = createMemo(() => {
    const l = list();
    if (l.status !== "ready") return [];
    const offered = offeredHarnesses(l.value);
    const current = r().harness;
    const own = current && !offered.some((h) => h.id === current) ? l.value.find((h) => h.id === current && h.installed !== false) : undefined;
    const all = own ? [own, ...offered] : offered;
    return locked() ? all.filter((h) => h.id === current) : all;
  });
  const [scope, setScope] = createSignal<string | null>(null);
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal(0);
  // The keyboard cursor only shows once the keyboard moves it; the pointer has its own hover wash.
  const [keyboard, setKeyboard] = createSignal(false);
  let search: HTMLInputElement | undefined;
  let listEl: HTMLDivElement | undefined;
  const needle = () => query().trim().toLowerCase();
  const grouped = () => scope() === null && !needle();
  // All, unsearched, leads with the current agent; a tab shows one agent; a search spans the tab it runs on.
  const inScope = createMemo(() => {
    const list = providers().filter((h) => scope() === null || h.id === scope());
    if (!grouped()) return list;
    const current = r().harness;
    return [...list.filter((h) => h.id === current), ...list.filter((h) => h.id !== current)];
  });
  const catalogs = createMemo(() => inScope().map((h) => ({ harness: h, models: modelList(device(), h.id) })));
  const rows = createMemo<Row[]>(() => {
    const q = needle();
    const match = (id: string, label: string) => !q || id.toLowerCase().includes(q) || label.toLowerCase().includes(q);
    const out: Row[] = [];
    for (const { harness, models } of catalogs()) {
      if (models.status !== "ready") continue;
      for (const model of models.value) if (match(model.id, model.label)) out.push({ harness, model });
      const id = r().modelId;
      if (harness.id === r().harness && id && !models.value.some((m) => m.id === id)) {
        const label = r().modelLabel ?? id;
        if (match(id, label)) out.push({ harness, model: { id, label }, selectedOnly: true });
      }
    }
    if (grouped()) return [...out.filter(row => defaults().favorites.includes(`${row.harness.id}|${row.model.id}`)).map(row=>({...row,favoriteSection:true})),...out];
    return out;
  });
  const loading = () => list().status === "loading" || catalogs().some((c) => c.models.status === "loading");
  // A provider's failure belongs to its own tab; All keeps listing the healthy ones.
  const failed = () => {
    if (scope() === null) return undefined;
    const entry = catalogs().find((c) => c.models.status === "error");
    return entry && { harness: entry.harness, message: (entry.models as { message: string }).message };
  };
  const isSelected = (row: Row) => row.harness.id === r().harness && row.model.id === r().modelId;
  const isLive = (harness: string) => modelList(device(), harness).status === "ready";
  const label = () => {
    const n = rows().length;
    if (grouped()) return "MODELS";
    return `${n} ${scope() === null ? (n === 1 ? "MATCH" : "MATCHES") : n === 1 ? "MODEL" : "MODELS"}`;
  };

  createEffect(rows, (list) => { void (setActive(Math.max(0, list.findIndex(isSelected)))); });
  createEffect(active, (index) => { void (keyboard() && listEl?.querySelector(`[data-index="${index}"]`)?.scrollIntoView({ block: "nearest" })); });
  focusModelSearch = () => search?.focus();
  queueMicrotask(() => search?.focus());

  const pick = (row: Row | undefined) => {
    if (!row || row.selectedOnly) return;
    if (locked() && row.harness.id !== r().harness) return;
    apply(row.harness.id !== r().harness ? { harness: row.harness.id, model: row.model.id } : { model: row.model.id });
    returnFocus = true;
    props.close();
    requestAnimationFrame(focusComposer);
  };
  const chooseScope = (next: string | null) => {
    setScope(next);
    setKeyboard(false);
    search?.focus();
  };

  const onKeyDown = (event: KeyboardEvent) => {
    const total = rows().length;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      setKeyboard(true);
      if (total) setActive((i) => (i + (event.key === "ArrowDown" ? 1 : -1) + total) % total);
    } else if (event.key === "Enter" && (event.target === search || event.target === document.body)) {
      event.preventDefault();
      pick(rows()[active()]);
    } else if ((mac ? event.metaKey : event.ctrlKey) && /^[1-9]$/.test(event.key)) {
      event.preventDefault();
      pick(rows()[Number(event.key) - 1]);
    }
  };

  // Catalog refreshes can replace the focused field while the popover is
  // opening. Keep keyboard actions with this picker during that brief gap.
  onSettled(() => {
    const recover = (event: KeyboardEvent) => {
      if (event.target !== document.body) return;
      if (event.key === "Escape") {
        returnFocus = true;
        props.close();
        requestAnimationFrame(focusComposer);
      } else {
        search?.focus();
        onKeyDown(event);
      }
    };
    window.addEventListener("keydown", recover);
    return () => window.removeEventListener("keydown", recover);
  });
  return (
    <div class="cmp-models-frame" onKeyDown={onKeyDown}>
      <div class="cmp-model-list-card"><Show
        when={list().status !== "error"}
        fallback={
          <div class="cmp-models-body">
            <RetryNote message={(list() as { message: string }).message} onRetry={() => harnessList(device(), true)} />
          </div>
        }
      >
        <Show when={list().status === "loading" || providers().length} fallback={<NoAgents close={props.close} />}>
          <div class="cmp-models-rail" role="tablist" aria-label="Agents" aria-orientation="vertical">
            <button type="button" role="tab" class="cmp-rail-tab" aria-label="All models" aria-selected={(scope() === null) ? "true" : "false"} onClick={() => chooseScope(null)}>
              All
            </button>
            <For each={providers()}>
              {(h) => (
                <button
                  type="button"
                  role="tab"
                  class="cmp-rail-tab"
                  aria-selected={(scope() === h.id) ? "true" : "false"}
                  aria-label={`${harnessName(h)} models`}
                  title={harnessName(h)}
                  onClick={() => chooseScope(h.id)}
                >
                  <Icon name={brandIcon(h.id)} size={14} />
                  <Show when={isLive(h.id)}><span class="cmp-rail-live" /></Show>
                </button>
              )}
            </For>
          </div>
          <div class="cmp-models-body">
            <div class="cmp-models-search">
              <label class="cmp-models-field">
                <Icon name="magnifer" size={14} />
                <input
                  ref={element => { search = element; queueMicrotask(() => element.focus()); }}
                  value={query()}
                  placeholder="Search models…"
                  aria-label="Search models"
                  aria-controls="cmp-model-list"
                  aria-activedescendant={rows().length ? `cmp-model-${active()}` : undefined}
                  onInput={(event) => {
                    setQuery(event.currentTarget.value);
                    // A query shows which row Enter will pick.
                    setKeyboard(!!event.currentTarget.value.trim());
                    queueMicrotask(() => search?.focus());
                  }}
                />
              </label>
            </div>
            <Show when={failed()}>
              {(f) => <RetryNote message={catalogNotice(f().message)} onRetry={() => modelList(device(), f().harness.id, true)} />}
            </Show>
            <div class="cmp-models-list" id="cmp-model-list" role="listbox" aria-label="Models" ref={listEl}>
              <Show
                when={rows().length}
                fallback={<div class="cmp-models-note">{loading() ? "Checking available agents…" : "No models match your search."}</div>}
              >
                <Show when={!grouped()}><div class="cmp-models-label" aria-hidden="true">{label()}</div></Show>
                <For each={rows()}>
                  {(row, index) => {
                    const first = () => grouped() && (index() === 0 || (rows()[index() - 1]!.favoriteSection !== row.favoriteSection || (!row.favoriteSection && rows()[index() - 1]!.harness.id !== row.harness.id)));
                    const last = () => index() === rows().length - 1 || (grouped() && (rows()[index() + 1]!.favoriteSection !== row.favoriteSection || (!row.favoriteSection && rows()[index() + 1]!.harness.id !== row.harness.id)));
                    const isDefault = () => isDefaultModel(row.harness.id, row.model.id);
                    return (
                      <>
                        <Show when={first()}>
                          <div class="cmp-models-group" role="presentation">
                            <div class="cmp-models-group-head">
                              <Icon name={row.favoriteSection ? "dither-bookmark" : brandIcon(row.harness.id)} size={14} />
                              <span class="cmp-models-group-name">{row.favoriteSection ? "Favorites" : harnessName(row.harness)}</span>

                            </div>
                          </div>
                        </Show>
                        <div
                          id={`cmp-model-${index()}`}
                          role="option"
                          class="cmp-model-row"
                          data-index={index()}
                          data-gap={last() ? (grouped() ? "group" : "end") : undefined}
                          aria-selected={(isSelected(row)) ? "true" : "false"}
                          aria-disabled={(row.selectedOnly || undefined) == null ? undefined : (row.selectedOnly || undefined) ? "true" : "false"}
                          data-active={(keyboard() && active() === index()) ? "true" : "false"}
                          title={row.selectedOnly ? "Selected in this chat; absent from the current model list" : undefined}
                          onMouseMove={() => {
                            setKeyboard(false);
                            setActive(index());
                          }}
                          onClick={() => pick(row)}
                        >
                          <span class="cmp-model-glyph"><Icon name={brandIcon(row.harness.id)} size={16} /></span>
                          <div class="cmp-model-text">
                            <span class="cmp-model-name">{row.model.label}</span>

                          </div>
                          <Show when={isDefault()}><span class="cmp-model-default">Default</span></Show>
                          <Show when={isSelected(row)}>
                            <Icon name="check" size={14} class="cmp-model-check" />
                          </Show>
                          <Show when={index() < 9}>
                            <span class="cmp-kbd" aria-hidden="true">{index() + 1}</span>
                          </Show>
                        </div>
                      </>
                    );
                  }}
                </For>
              </Show>
            </div>

          </div>
        </Show>
      </Show>
      </div><Show when={rows()[active()]}>{row => <ModelDetail row={row()} />}</Show>
    </div>
  );
}

function catalogNotice(message: string) {
  return message.trim() ? `Couldn't load models: ${message}` : "Couldn't load models";
}

function RetryNote(props: { message: string; onRetry: () => void }) {
  return (
    <div class="cmp-models-retry" role="alert">
      <Icon name="alert-circle" size={14} />
      <span class="cmp-models-retry-text">{props.message}</span>
      <button type="button" class="cmp-text-action" onClick={() => props.onRetry()}>Retry</button>
    </div>
  );
}

function NoAgents(props: { close: () => void }) {
  return (
    <div class="cmp-models-empty">
      <Icon name="terminal" size={20} />
      <strong>No agents available</strong>
      <span>Enable an installed agent in Settings → Providers, or install an agent CLI.</span>
      <button
        type="button"
        class="cmp-text-action"
        onClick={() => {
          props.close();
          navigate({ kind: "settings", section: "providers" });
        }}
      >
        Open settings
      </button>
    </div>
  );
}

/** A two-choice option that reads as a switch (on/off, true/false, fast, or a context size), as desktop's model_option_switch_values. */
function switchValues(option: ModelOption): { on: string; off: string } | null {
  if (option.choices.length !== 2) return null;
  const [a, b] = option.choices.map((c) => c.id) as [string, string];
  const has = (value: string) => [a, b].some((c) => c.toLowerCase() === value);
  const boolean = (has("on") && has("off")) || (has("true") && has("false"));
  if (!boolean && !has("fast") && !option.id.toLowerCase().includes("context")) return null;
  const enabled = (value: string) => ["on", "true", "fast"].includes(value.toLowerCase());
  const size = (value: string) => {
    const match = /^([\d.]+)([km])$/i.exec(value);
    return match ? Number(match[1]) * (match[2]!.toLowerCase() === "m" ? 1e6 : 1e3) : NaN;
  };
  return enabled(a) || (!enabled(b) && size(a) > size(b)) ? { on: a, off: b } : { on: b, off: a };
}

interface Group {
  option: ModelOption;
  selected: string;
  choices: { id: string; label: string; isDefault: boolean }[];
  toggle: { on: string; off: string } | null;
}

/** The selected model's other settings; effort and fast mode have their own controls in the footer. */
function Settings(props: { resolved: Resolved; change: typeof apply }) {
  const r = () => props.resolved;
  const groups = createMemo<Group[]>(() => {
    const model = r().model;
    if (!model) return [];
    const effort = !r().ladder.length ? effortOption(model)?.id : undefined;
    const fast = model.options?.find((o) => fastModeValues(o))?.id;
    return (model.options ?? [])
      .filter((option) => option.choices.length && option.id !== effort && option.id !== fast && option.id !== contextOf(model, r().options)?.option.id)
      .map((option) => ({
        option,
        selected: r().options[option.id] ?? option.defaultChoice,
        choices: option.choices.map((c) => ({ id: c.id, label: c.label, isDefault: c.id === option.defaultChoice })),
        toggle: switchValues(option),
      }));
  });
  const saving = () => !!selectedChat() && !!catalog.saving[selectedChat()!.id];
  const choose = (group: Group, value: string) => props.change({ option: { id: group.option.id, value, isDefault: value === group.option.defaultChoice } });
  const valueLabel = (group: Group) => group.choices.find((c) => c.id === group.selected)?.label ?? "Agent default";
  return (
    <Show when={groups().length}>
      <div class="cmp-models-tray" aria-busy={(saving()) ? "true" : "false"}>
        {/* By position, so a revalidated catalog updates rows in place instead of remounting an open menu. */}
        <For keyed={false} each={groups()}>
          {(group) => (
            <Show
              when={group().toggle}
              fallback={
                <DropdownMenu placement="right-end" gutter={8} modal={false}>
                  <DropdownMenu.Trigger class="cmp-setting-row" aria-label={`${group().option.label}: ${valueLabel(group())}`}>
                    <span class="cmp-setting-label">{group().option.label}</span>
                    <span class="cmp-setting-value">{valueLabel(group())}</span>
                    <Icon name="alt-arrow-right" size={12} class="cmp-setting-chevron" />
                  </DropdownMenu.Trigger>
                  <DropdownMenu.Portal>
                    <DropdownMenu.Content class="cmp-popover cmp-menu cmp-setting-menu">
                      <DropdownMenu.RadioGroup value={group().selected} onChange={(value) => choose(group(), value)}>
                        <For each={group().choices}>
                          {(choice) => (
                            <DropdownMenu.RadioItem value={choice.id} class="cmp-menu-row cmp-setting-choice" closeOnSelect>
                              <span class="cmp-setting-label">{choice.label}</span>
                              <Show when={choice.isDefault}><span class="cmp-setting-default">Default</span></Show>
                              <span class="cmp-menu-check">
                                <DropdownMenu.ItemIndicator><Icon name="check" size={14} /></DropdownMenu.ItemIndicator>
                              </span>
                            </DropdownMenu.RadioItem>
                          )}
                        </For>
                      </DropdownMenu.RadioGroup>
                    </DropdownMenu.Content>
                  </DropdownMenu.Portal>
                </DropdownMenu>
              }
            >
              {(values) => (
                <button
                  type="button"
                  role="switch"
                  class="cmp-setting-row"
                  aria-label={group().option.label}
                  aria-checked={(group().selected === values().on) ? "true" : "false"}
                  onClick={() => choose(group(), group().selected === values().on ? values().off : values().on)}
                >
                  <span class="cmp-setting-label">{group().option.label}</span>
                  <span class="cmp-setting-value">{valueLabel(group())}</span>
                  <span class="toggle" data-on={(group().selected === values().on) ? "true" : "false"} aria-hidden="true" />
                </button>
              )}
            </Show>
          )}
        </For>
      </div>
    </Show>
  );
}

/** Desktop's text tooltip, with line breaks kept. */
function Tip(props: { content: string; children: JSX.Element }) {
  return (
    <KTooltip openDelay={400} gutter={6} placement="top">
      <KTooltip.Trigger as="div" class="cmp-tip-anchor">{props.children}</KTooltip.Trigger>
      <KTooltip.Portal>
        <KTooltip.Content class="tooltip cmp-tip">{props.content}</KTooltip.Content>
      </KTooltip.Portal>
    </KTooltip>
  );
}

/** Desktop model_detail.rs: one compact card follows the hovered or keyboard row. */
function contextOf(model: Model, options: Record<string,string>) {
  for (const option of model.options ?? []) {
    if (!option.id.toLowerCase().includes("context")) continue;
    const values = switchValues(option);
    if (!values) continue;
    const count = (value: string) => { const match = /^([\d.]+)([km])$/i.exec(value); return match ? Number(match[1]) * (match[2]!.toLowerCase() === "m" ? 1e6 : 1e3) : 0; };
    if (count(values.on) < 1e6 || count(values.off) >= 1e6) continue;
    return {option, ...values, selected: options[option.id] ?? option.defaultChoice};
  }
  return null;
}
function ModelDetail(props: { row: Row }) {
  const resolved = createMemo<Resolved>(() => {
    const current = resolve();
    if (current.harness === props.row.harness.id && current.modelId === props.row.model.id) return current;
    const memory = defaults();
    const key = `${props.row.harness.id}|${props.row.model.id}`;
    const ladder = props.row.model.reasoningLevels?.length ? props.row.model.reasoningLevels : props.row.harness.reasoningLevels ?? [];
    return {...current,harness:props.row.harness.id,harnessInfo:props.row.harness,model:props.row.model,modelId:props.row.model.id,modelLabel:props.row.model.label,ladder,reasoning:clampReasoning(memory.reasoning[key] ?? memory.reasoning[props.row.harness.id],ladder),options:memory.options[key] ?? {}};
  });
  const change: typeof apply = value => { const current=resolve(); apply({...value,...(current.harness !== props.row.harness.id ? {harness:props.row.harness.id} : {}),...(current.harness !== props.row.harness.id || current.modelId !== props.row.model.id ? {model:props.row.model.id} : {})}); };
  const effort = createMemo(() => effortOf(resolved(), change));
  const fast = createMemo(() => fastOf(resolved(), change));
  const context = createMemo(() => contextOf(props.row.model,resolved().options));
  const favorite = () => defaults().favorites.includes(`${props.row.harness.id}|${props.row.model.id}`);
  const toggleFavorite = () => { const key=`${props.row.harness.id}|${props.row.model.id}`; updateDefaults(draft => { draft.favorites = draft.favorites.includes(key) ? draft.favorites.filter(item=>item!==key) : [...draft.favorites,key]; }); };
  const toggleDefault = () => {
    if (!isDefaultModel(props.row.harness.id,props.row.model.id)) return makeDefault(props.row);
    const current = resolve();
    if (!selectedChat()) setCatalog(draft => { Object.assign(draft.draft,{harness:current.harness,model:current.modelId}); });
    setView(draft => { draft.preferences.defaultModel=null; });
    const harness = props.row.harness.id;
    updateDefaults(draft => { delete draft.harness; delete draft.models[harness]; });
  };
  return <aside class="cmp-model-detail" aria-label="Model details">
    <header class="cmp-detail-header"><Icon name={brandIcon(props.row.harness.id)} size={16} /><strong>{props.row.model.label}</strong><span>{harnessName(props.row.harness)}</span></header>
    <div class="cmp-detail-body">
      <Show when={effort()}>{e => <><div class="cmp-detail-reasoning"><span>Reasoning</span><span>{e().labels[e().selected]}</span></div><EffortSlider effort={e()} /></>}</Show>
      <div class="cmp-detail-controls">
        <Show when={fast()}>{f => <Tip content={f().on ? "Fast mode · On" : "Fast mode · Off"}><button type="button" class="cmp-detail-toggle" aria-label="Fast mode" aria-pressed={f().on ? "true":"false"} onClick={()=>f().flip()}><Icon name="dither-bolt" size={16} /></button></Tip>}</Show>
        <Tip content={favorite() ? "Remove from favorites" : "Add to favorites"}><button type="button" class="cmp-detail-toggle" aria-label="Favorite" aria-pressed={favorite()?"true":"false"} onClick={toggleFavorite}><Icon name="dither-bookmark" size={16} /></button></Tip>
        <Tip content={isDefaultModel(props.row.harness.id,props.row.model.id) ? "Default model · New chats start here" : "Set as default"}><button type="button" class="cmp-detail-toggle" aria-label="Default model" aria-pressed={isDefaultModel(props.row.harness.id,props.row.model.id)?"true":"false"} onClick={toggleDefault}><Icon name="dither-flag" size={16} /></button></Tip>
        <Show when={context()}>{c => <div class="cmp-detail-context" role="radiogroup" aria-label="Context window" onKeyDown={event => { const value = ({ArrowLeft:c().off,ArrowRight:c().on,Home:c().off,End:c().on} as Record<string,string>)[event.key]; if (value) { event.preventDefault(); event.stopPropagation(); change({option:{id:c().option.id,value,isDefault:value===c().option.defaultChoice}}); } }}><For each={[c().off,c().on]}>{value=><button type="button" role="radio" aria-checked={c().selected===value?"true":"false"} onClick={()=>change({option:{id:c().option.id,value,isDefault:value===c().option.defaultChoice}})}>{c().option.choices.find(choice=>choice.id===value)?.label ?? value.toUpperCase()}</button>}</For></div>}</Show>
      </div>
      <Settings resolved={resolved()} change={change} />
    </div>
  </aside>;
}

const THUMB_INSET = 12;

function EffortSlider(props: { effort: Effort }) {
  let el: HTMLDivElement | undefined;
  const [drag, setDrag] = createSignal<number | null>(null);
  const count = () => props.effort.labels.length;
  const last = () => Math.max(1, count() - 1);
  const fraction = () => drag() ?? props.effort.selected / last();
  const current = () => props.effort.labels[props.effort.selected] ?? "Default";
  const fractionAt = (x: number) => {
    const box = el!.getBoundingClientRect();
    return Math.min(1, Math.max(0, (x - box.left - THUMB_INSET) / Math.max(1, box.width - THUMB_INSET * 2)));
  };
  const pickAt = (x: number) => {
    const f = fractionAt(x);
    if (drag() !== null) setDrag(count() > 1 ? f : 0);
    props.effort.pick(Math.round(f * (count() - 1)));
  };
  const onKeyDown = (event: KeyboardEvent) => {
    const at = props.effort.selected;
    const end = count() - 1;
    const next = { ArrowLeft: at - 1, ArrowDown: at - 1, ArrowRight: at + 1, ArrowUp: at + 1, Home: 0, End: end }[event.key];
    if (next === undefined) return;
    event.preventDefault();
    event.stopPropagation();
    props.effort.pick(Math.min(end, Math.max(0, next)));
  };
  return (
    <Tip content={current()}>
      <div
        ref={el}
        class="cmp-effort"
        role="slider"
        tabindex={0}
        aria-label={`Reasoning effort: ${current()}`}
        aria-valuemin={0}
        aria-valuemax={count() - 1}
        aria-valuenow={props.effort.selected}
        aria-valuetext={current()}
        data-dragging={(drag() !== null) ? "true" : "false"}
        style={{ "--f": String(fraction()) }}
        onKeyDown={onKeyDown}
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          event.preventDefault();
          el!.focus();
          el!.setPointerCapture(event.pointerId);
          setDrag(fractionAt(event.clientX));
          pickAt(event.clientX);
        }}
        onPointerMove={(event) => drag() !== null && pickAt(event.clientX)}
        onPointerUp={() => setDrag(null)}
        onPointerCancel={() => setDrag(null)}
      >
        <span class="cmp-effort-track" />
        <span class="cmp-effort-fill" />
        <span class="cmp-effort-stops">
          <For keyed={false} each={props.effort.labels}>{() => <span />}</For>
        </span>
        <span class="cmp-effort-rail">
          <span class="cmp-effort-thumb" />
        </span>
      </div>
    </Tip>
  );
}
