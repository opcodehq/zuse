import { omit } from "solid-js";
// Shared controls. Menus and dialogs use Kobalte for focus management and keyboard behaviour.
// Shared controls. Menus and dialogs use Kobalte for focus management and keyboard behaviour.
import { Dialog as KDialog } from "@kobalte/core/dialog";
import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { Tooltip as KTooltip } from "@kobalte/core/tooltip";
import { type JSX } from "@solidjs/web";
import { createSignal, For, Show } from "solid-js";
import { Icon } from "./Icon";
import { GlyphSpinner } from "./loaders";

type ButtonProps = JSX.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "default" | "primary" | "ghost" | "danger" | "outline";
  size?: "md" | "sm";
  icon?: string;
};

export function Button(props: ButtonProps) {
  const rest = omit(props, "variant", "size", "icon", "children", "class");
  return (
    <button
      type="button"
      {...rest}
      class={`btn ${props.class ?? ""}`}
      data-variant={props.variant ?? "default"}
      data-size={props.size ?? "md"}
    >
      <Show when={props.icon}>{(name) => <Icon name={name()} size={14} />}</Show>
      {props.children}
    </button>
  );
}

export function IconButton(
  props: JSX.ButtonHTMLAttributes<HTMLButtonElement> & { icon: string; label: string; size?: "md" | "sm"; active?: boolean; iconSize?: number; busy?: boolean },
) {
  const rest = omit(props, "icon", "label", "size", "active", "iconSize", "class", "busy");
  return (
    <button
      type="button"
      {...rest}
      class={`icon-btn ${props.class ?? ""}`}
      aria-label={props.label}
      title={props.label}
      data-size={props.size ?? "md"}
      data-active={props.active ? "true" : undefined}
      aria-busy={props.busy ? "true" : undefined}
    >
      {/* Busy swaps the icon for the activity glyph, as desktop's refresh buttons do. */}
      <Show when={props.busy} fallback={<Icon name={props.icon} size={props.iconSize ?? (props.size === "sm" ? 14 : 16)} />}>
        <GlyphSpinner cell={1.75} decorative />
      </Show>
    </button>
  );
}

export function TextField(props: JSX.InputHTMLAttributes<HTMLInputElement>) {
  const rest = omit(props, "class");
  return <input {...rest} class={`field ${props.class ?? ""}`} />;
}

export function Toggle(props: { checked: boolean; label: string; onChange: (value: boolean) => void; disabled?: boolean }) {
  return (
    <button
      type="button"
      role="switch"
      class="toggle"
      aria-checked={(props.checked) ? "true" : "false"}
      aria-label={props.label}
      disabled={props.disabled}
      onClick={() => props.onChange(!props.checked)}
    />
  );
}

/** The default loader: desktop's dithered activity glyph. See `ui/loaders.tsx` for the ring and terminal variants. */
export function Spinner(props: Parameters<typeof GlyphSpinner>[0]) {
  return <GlyphSpinner {...props} />;
}

export interface MenuItem {
  label: string;
  icon?: string;
  danger?: boolean;
  disabled?: boolean;
  onSelect?: () => void;
  separator?: boolean;
  /** Right-aligned secondary text, such as a shortcut. */
  hint?: string;
  /** Opens a submenu instead of selecting. */
  children?: MenuItem[];
}

function MenuItems(props: { items: MenuItem[] }) {
  return (
    <For each={props.items}>
      {(item) =>
        item.separator ? (
          <DropdownMenu.Separator class="menu-sep" />
        ) : item.children ? (
          <DropdownMenu.Sub gutter={2}>
            <DropdownMenu.SubTrigger class="menu-item" disabled={item.disabled}>
              <Show when={item.icon}>{(name) => <Icon name={name()} size={14} />}</Show>
              {item.label}
              <Icon name="alt-arrow-right" size={12} class="menu-hint" />
            </DropdownMenu.SubTrigger>
            <DropdownMenu.Portal>
              <DropdownMenu.SubContent class="menu">
                <MenuItems items={item.children} />
              </DropdownMenu.SubContent>
            </DropdownMenu.Portal>
          </DropdownMenu.Sub>
        ) : (
          <DropdownMenu.Item class="menu-item" data-danger={item.danger ? "true" : undefined} disabled={item.disabled} onSelect={() => item.onSelect?.()}>
            <Show when={item.icon}>{(name) => <Icon name={name()} size={14} />}</Show>
            {item.label}
            <Show when={item.hint}><span class="menu-hint">{item.hint}</span></Show>
          </DropdownMenu.Item>
        )
      }
    </For>
  );
}

/** A dropdown menu attached to any trigger element. */
export function Menu(props: { trigger: (triggerProps: Record<string, unknown>) => JSX.Element; items: MenuItem[]; placement?: "bottom-start" | "bottom-end" | "top-start" | "top-end" | "right-start" }) {
  return (
    // Modal menus lock page scroll, and Kobalte can then loop forever hunting for a scroll parent inside virtualized lists.
    <DropdownMenu placement={props.placement ?? "bottom-start"} gutter={4} modal={false}>
      <DropdownMenu.Trigger as={(triggerProps: Record<string, unknown>) => props.trigger(triggerProps)} />
      <DropdownMenu.Portal>
        <DropdownMenu.Content class="menu">
          <MenuItems items={props.items} />
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu>
  );
}

export function Dialog(props: { open: boolean; onOpenChange: (open: boolean) => void; title: string; description?: string; children?: JSX.Element; footer?: JSX.Element }) {
  return (
    <KDialog open={props.open} onOpenChange={props.onOpenChange}>
      <KDialog.Portal>
        <KDialog.Overlay class="dialog-overlay" />
        <KDialog.Content class="dialog" onKeyDown={event => { if (event.key === "Escape" && !event.defaultPrevented && (event.target as Element).closest('[role="dialog"]') === event.currentTarget) { event.preventDefault(); event.stopPropagation(); props.onOpenChange(false); } }}>
          <div class="dialog-header"><KDialog.Title class="dialog-title">{props.title}</KDialog.Title><KDialog.CloseButton class="dialog-close" aria-label="Close dialog"><Icon name="close" size={14} /></KDialog.CloseButton></div>
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

export function Tooltip(props: { content: string; children: JSX.Element }) {
  return (
    <KTooltip openDelay={400} gutter={6}>
      <KTooltip.Trigger as="span" style={{ display: "inline-flex" }}>{props.children}</KTooltip.Trigger>
      <KTooltip.Portal>
        <KTooltip.Content class="tooltip">{props.content}</KTooltip.Content>
      </KTooltip.Portal>
    </KTooltip>
  );
}

export function Tabs<T extends string>(props: { value: T; items: { value: T; label: string }[]; onChange: (value: T) => void; label: string }) {
  return (
    <div class="tabs" role="tablist" aria-label={props.label}>
      <For each={props.items}>
        {(item) => (
          <button type="button" class="tab" role="tab" aria-selected={(props.value === item.value) ? "true" : "false"} onClick={() => props.onChange(item.value)}>
            {item.label}
          </button>
        )}
      </For>
    </div>
  );
}

interface Toast {
  id: number;
  message: string;
  tone: "default" | "danger";
}
const [toasts, setToasts] = createSignal<Toast[]>([]);
let toastId = 0;
// Desktop's toaster: the countdown pauses while the pointer or focus is on the stack and while the window is inactive.
const countdowns = new Map<number, { left: number; started: number; timer?: ReturnType<typeof setTimeout> }>();
let toastHover = false;
let toastStack: HTMLElement | undefined;
const toastsPaused = () =>
  toastHover || !!toastStack?.contains(document.activeElement) || document.hidden || !document.hasFocus();

export function dismissToast(id: number) {
  clearTimeout(countdowns.get(id)?.timer);
  countdowns.delete(id);
  setToasts((list) => list.filter((t) => t.id !== id));
  // A removed, focused close button never reports focusout.
  queueMicrotask(runCountdowns);
}

function runCountdowns() {
  const paused = toastsPaused();
  const now = Date.now();
  for (const [id, countdown] of countdowns) {
    if (paused && countdown.timer) {
      clearTimeout(countdown.timer);
      countdown.timer = undefined;
      countdown.left -= now - countdown.started;
    } else if (!paused && !countdown.timer) {
      countdown.started = now;
      countdown.timer = setTimeout(() => dismissToast(id), Math.max(0, countdown.left));
    }
  }
}

if (typeof window !== "undefined") {
  for (const event of ["focus", "blur"]) window.addEventListener(event, runCountdowns);
  document.addEventListener("visibilitychange", runCountdowns);
}

export function toast(message: string, tone: Toast["tone"] = "default") {
  const id = ++toastId;
  for (const old of toasts().slice(0, -3)) dismissToast(old.id);
  setToasts((list) => [...list.slice(-3), { id, message, tone }]);
  countdowns.set(id, { left: tone === "danger" ? 8000 : 4000, started: Date.now() });
  runCountdowns();
}

function hoverToasts(hover: boolean) {
  toastHover = hover;
  runCountdowns();
}

export function ToastRegion() {
  return (
    <section
      ref={toastStack}
      class="toast-region"
      aria-label="Notifications"
      onPointerEnter={() => hoverToasts(true)}
      onPointerLeave={() => hoverToasts(false)}
      onFocusIn={runCountdowns}
      onFocusOut={() => queueMicrotask(runCountdowns)}
    >
      <For each={toasts()}>
        {(t) => (
          <div class="toast" data-tone={t.tone} role={t.tone === "danger" ? "alert" : "status"}>
            <Icon name={t.tone === "danger" ? "danger-triangle" : "info-circle"} size={14} />
            <span style={{ flex: 1 }}>{t.message}</span>
            <button type="button" class="toast-close" aria-label="Dismiss notification" title="Dismiss" onClick={() => dismissToast(t.id)}>
              <Icon name="close" size={12} />
            </button>
          </div>
        )}
      </For>
    </section>
  );
}
