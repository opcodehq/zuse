// Compact remote access building blocks, matching crates/viewport remote_access_style.
import { Dialog as KDialog } from "@kobalte/core/dialog";
import { Tooltip as KTooltip } from "@kobalte/core/tooltip";
import { type JSX, Show, splitProps } from "solid-js";
import { Icon } from "@/ui/Icon";
import wifi from "@repo/crates/icons/assets/free/wifi.svg?raw";
import link from "@repo/crates/icons/assets/free/link.svg?raw";

// Two glyphs desktop takes from crates/icons; the shared set has the rest.
const extra: Record<string, string> = { wifi, link };

export function Glyph(props: { name: string; size?: number }) {
  const size = () => `${props.size ?? 16}px`;
  return (
    <Show when={extra[props.name]} fallback={<Icon name={props.name} size={props.size ?? 16} />}>
      {(svg) => (
        <span
          aria-hidden="true"
          style={{ display: "inline-flex", width: size(), height: size(), "flex-shrink": 0 }}
          // eslint-disable-next-line solid/no-innerhtml -- trusted SVGs bundled from the repository
          innerHTML={svg().replace("<svg ", '<svg width="100%" height="100%" ')}
        />
      )}
    </Show>
  );
}

type ButtonProps = JSX.ButtonHTMLAttributes<HTMLButtonElement> & {
  size?: "md" | "sm" | "method" | "add" | "lg";
  tone?: "default" | "danger" | "solid" | "quiet";
};

/** Disabled buttons stay focusable, as on desktop, so focus is never dropped while an action runs. */
export function Btn(props: ButtonProps) {
  const [local, rest] = splitProps(props, ["size", "tone", "class", "children", "disabled", "onClick"]);
  const click = (event: MouseEvent & { currentTarget: HTMLButtonElement; target: Element }) => {
    if (local.disabled) return event.preventDefault();
    const handler = local.onClick;
    if (typeof handler === "function") handler(event);
    else if (handler) handler[0](handler[1], event);
  };
  return (
    <button
      type="button"
      {...rest}
      aria-disabled={local.disabled ? "true" : rest["aria-disabled"]}
      onClick={click}
      class={`cx-btn ${local.class ?? ""}`}
      data-size={local.size ?? "md"}
      data-tone={local.tone ?? "default"}
    >
      {local.children}
    </button>
  );
}

export function Help(props: { title: string; text: string }) {
  return (
    <KTooltip openDelay={300} gutter={6}>
      <KTooltip.Trigger as="button" type="button" class="cx-help" aria-label={`About ${props.title}`}>
        <Icon name="info-circle" size={14} />
      </KTooltip.Trigger>
      <KTooltip.Portal>
        <KTooltip.Content class="tooltip">{props.text}</KTooltip.Content>
      </KTooltip.Portal>
    </KTooltip>
  );
}

export function Frame(props: { title: string; help: string; action?: JSX.Element; first?: boolean; children: JSX.Element }) {
  return (
    <section class="cx-frame" aria-label={props.title}>
      <div class="cx-frame-head" data-first={props.first ? "true" : undefined}>
        <div class="cx-frame-title">
          <h3>{props.title}</h3>
          <Help title={props.title} text={props.help} />
        </div>
        {props.action}
      </div>
      <div class="cx-card">{props.children}</div>
    </section>
  );
}

export function Details(props: { title: string; text?: string; tone?: "success" }) {
  return (
    <div class="cx-details">
      <div class="cx-details-title">{props.title}</div>
      <Show when={props.text}>
        <div class="cx-details-text" data-tone={props.tone}>{props.text}</div>
      </Show>
    </div>
  );
}

export function MethodRow(props: { glyph: string; title: string; text: string; children?: JSX.Element }) {
  return (
    <div class="cx-row" data-size="method">
      <span class="cx-tile"><Glyph name={props.glyph} /></span>
      <Details title={props.title} text={props.text} />
      {props.children}
    </div>
  );
}

export function ErrorStrip(props: { message: string }) {
  return (
    <div class="cx-error" role="alert">
      <Icon name="danger-triangle" size={16} />
      <span>{props.message}</span>
    </div>
  );
}

/** A focused dialog with desktop's header, body and footer bands. */
export function RemoteDialog(props: {
  open: boolean;
  title: string;
  description: string;
  onClose: () => void;
  busy?: boolean;
  children?: JSX.Element;
  footer: JSX.Element;
}) {
  // These dialogs have no Kobalte trigger, so focus goes back to whatever opened them.
  let opener: HTMLElement | null = null;
  const remember = () => {
    opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  };
  const restore = (event: Event) => {
    if (!opener?.isConnected) return;
    event.preventDefault();
    opener.focus();
    opener = null;
  };
  return (
    <KDialog open={props.open} onOpenChange={(open) => !open && !props.busy && props.onClose()}>
      <KDialog.Portal>
        <KDialog.Overlay class="cx-dialog-overlay" />
        <KDialog.Content class="cx cx-dialog" onOpenAutoFocus={remember} onCloseAutoFocus={restore}>
          <div class="cx-dialog-head">
            <KDialog.Title class="cx-dialog-title">{props.title}</KDialog.Title>
            <KDialog.Description class="cx-dialog-description">{props.description}</KDialog.Description>
          </div>
          <Show when={props.children}>
            <div class="cx-dialog-body">{props.children}</div>
          </Show>
          <div class="cx-dialog-footer">{props.footer}</div>
        </KDialog.Content>
      </KDialog.Portal>
    </KDialog>
  );
}

export { writeClipboard as copyText } from "@/ui/clipboard";
