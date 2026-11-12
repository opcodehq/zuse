// Settings-frame building blocks shared by Plugins and Cloud (desktop settings_style and cloud_settings/style).
import { createSignal, For, type JSX, Show, splitProps } from "solid-js";
import { Icon } from "@/ui/Icon";
import { GlyphSpinner } from "@/ui/loaders";
import { Tooltip } from "@/ui/primitives";
import { logoUrl } from "./plugins/catalog";

export function Logo(props: { domain: string; name: string; size: number; radius?: number }) {
  const [failed, setFailed] = createSignal(false);
  const url = () => (failed() ? null : logoUrl(props.domain));
  return (
    <span
      class="pc-logo"
      aria-hidden="true"
      style={{ width: `${props.size}px`, height: `${props.size}px`, "border-radius": `${props.radius ?? 10}px` }}
    >
      <Show when={url()} fallback={<span>{(props.name.trim()[0] ?? "?").toUpperCase()}</span>}>
        {(src) => <img src={src()} alt="" loading="lazy" decoding="async" onError={() => setFailed(true)} />}
      </Show>
    </span>
  );
}

export function SearchField(props: { value: string; onInput: (value: string) => void; placeholder: string; label: string; width?: number }) {
  return (
    <label class="pc-search" style={{ width: props.width ? `${props.width}px` : undefined }}>
      <Icon name="magnifer" size={14} />
      <input
        type="search"
        value={props.value}
        placeholder={props.placeholder}
        aria-label={props.label}
        onInput={(event) => props.onInput(event.currentTarget.value)}
      />
    </label>
  );
}

export interface SegmentItem<T extends string> {
  value: T;
  label: string;
  count?: number;
}

/** Desktop segmented tabs: 28px track, selected segment raised. Arrow keys move the selection. */
export function Segmented<T extends string>(props: { items: SegmentItem<T>[]; value: T; onChange: (value: T) => void; label: string; class?: string; disabled?: boolean }) {
  const move = (event: KeyboardEvent, index: number) => {
    const count = props.items.length;
    const next =
      event.key === "ArrowRight" ? (index + 1) % count : event.key === "ArrowLeft" ? (index + count - 1) % count : event.key === "Home" ? 0 : event.key === "End" ? count - 1 : -1;
    if (next < 0) return;
    event.preventDefault();
    const item = props.items[next];
    if (!item) return;
    props.onChange(item.value);
    const list = (event.currentTarget as HTMLElement).parentElement;
    (list?.children[next] as HTMLElement | undefined)?.focus();
  };
  return (
    <div class={`pc-segmented ${props.class ?? ""}`} role="tablist" aria-label={props.label}>
      <For each={props.items}>
        {(item, index) => (
          <button
            type="button"
            role="tab"
            class="pc-segment"
            aria-selected={props.value === item.value}
            tabIndex={props.value === item.value ? 0 : -1}
            disabled={props.disabled}
            onClick={() => props.onChange(item.value)}
            onKeyDown={(event) => move(event, index())}
          >
            {item.label}
            <Show when={item.count}>{(count) => <span class="pc-segment-count">{count()}</span>}</Show>
          </button>
        )}
      </For>
    </div>
  );
}

export function Group(props: { title: string; help?: string; action?: JSX.Element; children: JSX.Element; id?: string }) {
  return (
    <section class="pc-frame" aria-label={props.title} id={props.id}>
      <header class="pc-frame-head">
        <div class="pc-frame-title">
          <h3>{props.title}</h3>
          <Show when={props.help}>
            {(help) => (
              <Tooltip content={help()}>
                <button type="button" class="pc-help" aria-label={`About ${props.title}: ${help()}`}>
                  <Icon name="info-circle" size={12} />
                </button>
              </Tooltip>
            )}
          </Show>
        </div>
        <Show when={props.action}>
          <div class="pc-frame-action">{props.action}</div>
        </Show>
      </header>
      <div class="pc-card">{props.children}</div>
    </section>
  );
}

export function Row(props: { title: JSX.Element; description?: JSX.Element; action?: JSX.Element; children?: JSX.Element; class?: string }) {
  return (
    <div class={`pc-row ${props.class ?? ""}`}>
      <div class="pc-row-main">
        <div class="pc-row-copy">
          <div class="pc-row-title">{props.title}</div>
          <Show when={props.description}>
            <div class="pc-row-description">{props.description}</div>
          </Show>
        </div>
        <Show when={props.action}>
          <div class="pc-row-action">{props.action}</div>
        </Show>
      </div>
      {props.children}
    </div>
  );
}

export type BadgeTone = "ok" | "warn" | "outline" | "error";
export function Badge(props: { tone?: BadgeTone; children: JSX.Element; icon?: string }) {
  return (
    <span class="pc-badge" data-tone={props.tone ?? "ok"}>
      <Show when={props.icon}>{(name) => <Icon name={name()} size={10} />}</Show>
      {props.children}
    </span>
  );
}

type ActionProps = JSX.ButtonHTMLAttributes<HTMLButtonElement> & { primary?: boolean; busy?: boolean; icon?: string };
/** Cloud's compact 28px action. Busy keeps the label's width and shows a spinner over it. */
export function Action(props: ActionProps) {
  const [local, rest] = splitProps(props, ["primary", "busy", "icon", "children", "class", "disabled"]);
  return (
    <button
      type="button"
      {...rest}
      class={`pc-action ${local.class ?? ""}`}
      data-primary={local.primary ? "true" : undefined}
      data-busy={local.busy ? "true" : undefined}
      disabled={local.disabled || local.busy}
      aria-busy={local.busy ? "true" : undefined}
    >
      <span class="pc-action-label">
        <Show when={local.icon}>{(name) => <Icon name={name()} size={14} />}</Show>
        {local.children}
      </span>
      <Show when={local.busy}>
        <GlyphSpinner cell={1.75} tone="current" decorative class="pc-action-spinner" />
      </Show>
    </button>
  );
}

export function ErrorStrip(props: { children: JSX.Element; onDismiss?: () => void }) {
  return (
    <div class="pc-error" role="alert">
      <Icon name="danger-triangle" size={16} class="pc-error-icon" />
      <span>{props.children}</span>
      <Show when={props.onDismiss}>
        <button type="button" class="pc-error-dismiss" aria-label="Dismiss" onClick={() => props.onDismiss?.()}>
          <Icon name="close" size={12} />
        </button>
      </Show>
    </div>
  );
}

export function Note(props: { children: JSX.Element; error?: boolean }) {
  return (
    <div class="pc-note" data-error={props.error ? "true" : undefined} role={props.error ? "alert" : "status"}>
      {props.children}
    </div>
  );
}

export function PageTitle(props: { title: string; subtitle?: string; children?: JSX.Element }) {
  return (
    <div class="pc-title">
      <div class="pc-title-copy">
        <h1>{props.title}</h1>
        <Show when={props.subtitle}>
          <p>{props.subtitle}</p>
        </Show>
      </div>
      {props.children}
    </div>
  );
}

export function formatDate(ms: number | null | undefined, withTime = false) {
  if (!ms || !Number.isFinite(ms)) return null;
  return new Date(ms).toLocaleString(undefined, withTime ? { month: "short", day: "numeric", year: "numeric", hour: "2-digit", minute: "2-digit" } : { month: "short", day: "numeric", year: "numeric" });
}
