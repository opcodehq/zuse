// Settings building blocks, matching crates/ui/src/settings/widgets.rs: framed groups, bordered cards, compact rows.
import { writeClipboard } from "@/ui/clipboard";
import { Dialog as KDialog } from "@kobalte/core/dialog";
import { Select as KSelect } from "@kobalte/core/select";
import { For, type JSX, Show, splitProps } from "solid-js";
import { Icon } from "@/ui/Icon";
import { GlyphSpinner } from "@/ui/loaders";

/** Page title with one muted line and a hairline underneath. */
export function SectionTitle(props: { title: string; description: string; count?: number; action?: JSX.Element }) {
  return (
    <header class="st-title">
      <div class="st-title-text">
        <h1>
          {props.title}
          <Show when={props.count !== undefined}>
            <span class="st-title-count">{props.count}</span>
          </Show>
        </h1>
        <p>{props.description}</p>
      </div>
      <Show when={props.action}>{props.action}</Show>
    </header>
  );
}

/** A muted frame with a compact header, holding one card. */
export function Group(props: { title: string; description?: string; action?: JSX.Element; children: JSX.Element; id?: string }) {
  return (
    <section class="st-group" aria-labelledby={props.id ? `${props.id}-title` : undefined}>
      <div class="st-group-head" data-described={props.description ? "true" : undefined}>
        <div class="st-group-text">
          <h2 class="st-row-title" id={props.id ? `${props.id}-title` : undefined}>
            {props.title}
          </h2>
          <Show when={props.description}>
            <p class="st-desc">{props.description}</p>
          </Show>
        </div>
        <Show when={props.action}>
          <div class="st-group-action">{props.action}</div>
        </Show>
      </div>
      {props.children}
    </section>
  );
}

export function Card(props: JSX.HTMLAttributes<HTMLDivElement>) {
  const [local, rest] = splitProps(props, ["class"]);
  return <div {...rest} class={`st-card ${local.class ?? ""}`} />;
}

/** Title and description on the left, the control on the right. */
export function Row(props: { title: JSX.Element; description?: JSX.Element; children?: JSX.Element; top?: boolean; class?: string; titleId?: string }) {
  return (
    <div class={`st-row ${props.class ?? ""}`} data-top={props.top ? "true" : undefined}>
      <div class="st-row-text">
        <div class="st-row-title" id={props.titleId}>
          {props.title}
        </div>
        <Show when={props.description}>
          <div class="st-desc">{props.description}</div>
        </Show>
      </div>
      <Show when={props.children}>
        <div class="st-row-control">{props.children}</div>
      </Show>
    </div>
  );
}

/** Desktop's 32 by 20 switch. */
export function Switch(props: { checked: boolean; label: string; onChange: (value: boolean) => void; disabled?: boolean; description?: string }) {
  return (
    <button
      type="button"
      role="switch"
      class="toggle"
      aria-checked={props.checked}
      aria-label={props.label}
      aria-description={props.description}
      disabled={props.disabled}
      onClick={() => props.onChange(!props.checked)}
    />
  );
}

export interface Option<T extends string> {
  value: T;
  label: string;
  detail?: string;
  disabled?: boolean;
}

/** Desktop's select trigger and checked menu, with listbox semantics from Kobalte. */
export function Select<T extends string>(props: {
  value: T | undefined;
  options: Option<T>[];
  onChange: (value: T) => void;
  label: string;
  width?: number;
  disabled?: boolean;
  placeholder?: string;
  mono?: boolean;
}) {
  const selected = () => props.options.find((o) => o.value === props.value);
  return (
    <KSelect<Option<T>>
      options={props.options}
      optionValue="value"
      optionTextValue="label"
      optionDisabled="disabled"
      value={selected() ?? null}
      onChange={(option) => option && option.value !== props.value && props.onChange(option.value)}
      disabled={props.disabled}
      placeholder={<span class="st-muted">{props.placeholder ?? ""}</span>}
      gutter={4}
      sameWidth={false}
      placement="bottom-end"
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
      <KSelect.Trigger
        class="st-select"
        aria-label={props.label}
        data-mono={props.mono ? "true" : undefined}
        style={{ width: props.width ? `${props.width}px` : undefined }}
      >
        <KSelect.Value<Option<T>> class="st-select-value">{(s) => s.selectedOption()?.label ?? props.placeholder}</KSelect.Value>
        <KSelect.Icon class="st-select-icon">
          <Icon name="alt-arrow-down" size={14} />
        </KSelect.Icon>
      </KSelect.Trigger>
      <KSelect.Portal>
        <KSelect.Content class="st-select-menu">
          <KSelect.Listbox class="st-select-list" />
        </KSelect.Content>
      </KSelect.Portal>
    </KSelect>
  );
}

/** Two to four mutually exclusive choices shown at once. */
export function Segmented<T extends string>(props: { value: T; options: Option<T>[]; onChange: (value: T) => void; label: string; size?: "sm" }) {
  return (
    <div class="st-segmented" role="radiogroup" aria-label={props.label} data-size={props.size}>
      <For each={props.options}>
        {(option) => (
          <button
            type="button"
            role="radio"
            aria-checked={props.value === option.value}
            disabled={option.disabled}
            class="st-segment"
            onClick={() => props.onChange(option.value)}
            onKeyDown={(event) => {
              const index = props.options.findIndex((o) => o.value === props.value);
              const step = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 0;
              if (!step) return;
              event.preventDefault();
              const next = props.options[(index + step + props.options.length) % props.options.length]!;
              props.onChange(next.value);
              const buttons = (event.currentTarget.parentElement?.querySelectorAll("button") ?? []) as NodeListOf<HTMLButtonElement>;
              buttons[props.options.indexOf(next)]?.focus();
            }}
            tabIndex={props.value === option.value ? 0 : -1}
          >
            {option.label}
          </button>
        )}
      </For>
    </div>
  );
}

export function ActionButton(props: JSX.ButtonHTMLAttributes<HTMLButtonElement> & { tone?: "outlined" | "quiet" | "filled" | "solid" | "danger"; icon?: string }) {
  const [local, rest] = splitProps(props, ["tone", "icon", "children", "class"]);
  return (
    <button type="button" {...rest} class={`st-action ${local.class ?? ""}`} data-tone={local.tone ?? "outlined"}>
      <Show when={local.icon}>{(name) => <Icon name={name()} size={14} />}</Show>
      {local.children}
    </button>
  );
}

export function ErrorStrip(props: { children: JSX.Element; action?: JSX.Element }) {
  return (
    <div class="st-error" role="alert">
      <Icon name="danger-triangle" size={16} />
      <div class="st-error-text">{props.children}</div>
      <Show when={props.action}>{props.action}</Show>
    </div>
  );
}

export function WarningStrip(props: { children: JSX.Element }) {
  return (
    <div class="st-warning" role="status">
      <Icon name="info-circle" size={14} />
      <div>{props.children}</div>
    </div>
  );
}

export function Badge(props: { children: JSX.Element; tone?: "active" }) {
  return (
    <span class="st-badge" data-tone={props.tone}>
      {props.children}
    </span>
  );
}

/** Quiet meta fragments joined by dots. */
export function Meta(props: { parts: (JSX.Element | string | null | undefined | false)[] }) {
  const parts = () => props.parts.filter((p) => p !== null && p !== undefined && p !== false && p !== "");
  return (
    <div class="st-meta">
      <For each={parts()}>
        {(part, index) => (
          <>
            <Show when={index() > 0}>
              <span class="st-meta-dot" aria-hidden="true">
                ·
              </span>
            </Show>
            <span class="st-meta-part">{part}</span>
          </>
        )}
      </For>
    </div>
  );
}

export function CopyField(props: { value: string; label: string }) {
  return (
    <div class="st-copy">
      <code>{props.value}</code>
      <button
        type="button"
        class="st-icon-action"
        aria-label={props.label}
        title={props.label}
        onClick={() => void writeClipboard(props.value)}
      >
        <Icon name="copy" size={12} />
      </button>
    </div>
  );
}

export function Mark(props: { name: string; tint?: string; size?: number; dim?: boolean }) {
  return (
    <span class="st-mark" data-dim={props.dim ? "true" : undefined} style={{ color: props.tint }}>
      <Icon name={props.name} size={props.size ?? 16} />
    </span>
  );
}

/** A modal over Settings; wide for lists and forms. */
export function SettingsDialog(props: {
  open: boolean;
  onClose: () => void;
  title: string;
  description?: string;
  wide?: boolean;
  children?: JSX.Element;
  footer?: JSX.Element;
}) {
  return (
    <KDialog open={props.open} onOpenChange={(open) => !open && props.onClose()}>
      <KDialog.Portal>
        <KDialog.Overlay class="dialog-overlay" />
        <KDialog.Content class={`dialog ${props.wide ? "st-wide-dialog" : ""}`}>
          <KDialog.Title class="dialog-title">{props.title}</KDialog.Title>
          <Show when={props.description}>
            <KDialog.Description class="dialog-description">{props.description}</KDialog.Description>
          </Show>
          {props.children}
          <Show when={props.footer}>
            <div class="dialog-footer">{props.footer}</div>
          </Show>
        </KDialog.Content>
      </KDialog.Portal>
    </KDialog>
  );
}

/** Desktop's mono activity glyph (`mini_mono_spinner`, 1.5 px cells) beside a label while something works. */
export function InlineSpinner(props: { label: string }) {
  return <GlyphSpinner label={props.label} cell={1.5} tone="muted" class="st-inline-spinner" />;
}

/** Arrow keys move a radio group's selection and focus, as native radios do. */
export function radioKeys<T>(event: KeyboardEvent, values: readonly T[], current: T, choose: (value: T) => void) {
  const step = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 0;
  if (!step || !values.length) return;
  event.preventDefault();
  const index = Math.max(0, values.indexOf(current));
  const next = (index + step + values.length) % values.length;
  choose(values[next]!);
  const group = (event.currentTarget as HTMLElement).closest('[role="radiogroup"]');
  (group?.querySelectorAll<HTMLElement>('[role="radio"]')[next])?.focus();
}
