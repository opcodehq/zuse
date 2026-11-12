// Toolbar controls: add menu, agent access, plan chip, plugins, context usage, and the send/stop key.
import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { Popover } from "@kobalte/core/popover";
import { createMemo, createSignal, For, Show } from "solid-js";
import { state } from "@/core/store";
import { navigate } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { toast } from "@/ui/primitives";
import { resolvedTheme } from "@/shell/theme";
import { ditherBackground, Glyph, SEND_DITHER } from "./assets";
import { focusComposer } from "./drafts";
import { automaticReviewSupported, catalog, changeConfig, planAvailable, resolve, SANDBOXES, selectedChat, type Sandbox } from "./catalog";

const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));
const apply = (change: Parameters<typeof changeConfig>[0]) =>
  changeConfig(change).catch((error) => toast(`Couldn't change chat settings: ${errorText(error)}`, "danger"));

// Menus that changed a setting hand the caret back to the prompt when they close.
let picked = false;
const markPicked = () => (picked = true);
const closeFocus = (event: Event) => {
  if (!picked) return;
  picked = false;
  event.preventDefault();
  // Focus may already be in a picker the user opened since; leave it there.
  const active = document.activeElement;
  const menu = event.currentTarget instanceof Element ? event.currentTarget : null;
  if (active && active !== document.body && !menu?.contains(active) && !active.closest(".cmp-card")) return;
  focusComposer();
};

/** crates/protocol/src/presentation.rs access_details. */
export const ACCESS: Record<Sandbox, { label: string; description: string; icon: string }> = {
  "workspace-write": { label: "Ask for approval", description: "Asks before restricted actions.", icon: "access-ask" },
  auto: { label: "Approve for me", description: "Reviews actions, asks when needed.", icon: "access-auto" },
  "danger-full-access": { label: "Full access", description: "Runs actions without approval.", icon: "access-full" },
};

export function AddMenu(props: { onAttach: () => void; disabled?: boolean }) {
  const r = createMemo(resolve);
  const planOk = () => planAvailable(r().harness);
  return (
    <DropdownMenu placement="top-start" gutter={8} modal={false}>
      <DropdownMenu.Trigger class="cmp-key" aria-label="Add to message" title="Add to message" disabled={props.disabled}>
        <Icon name="dither-plus" size={14} />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content class="cmp-popover cmp-menu" style={{ width: "260px" }} onCloseAutoFocus={closeFocus}>
          <DropdownMenu.Item class="cmp-menu-row" data-tall="true" onSelect={() => props.onAttach()}>
            <Icon name="plus" size={15} style={{ color: "var(--text-muted)" }} />
            <span class="cmp-menu-text"><span class="cmp-menu-title">Attach images</span></span>
            <span class="cmp-menu-check" />
          </DropdownMenu.Item>
          <DropdownMenu.CheckboxItem
            class="cmp-menu-row"
            data-tall="true"
            closeOnSelect
            checked={r().plan}
            disabled={!r().plan && !planOk()}
            onSelect={markPicked}
            onChange={(checked) => apply({ plan: checked })}
          >
            <Glyph name="access-read-only" size={15} style={{ color: "var(--text-muted)" }} />
            <span class="cmp-menu-text">
              <span class="cmp-menu-title">Plan mode</span>
              <span class="cmp-menu-desc">{planOk() || r().plan ? "Inspects and proposes without changing files." : "Not available for this agent"}</span>
            </span>
            <span class="cmp-menu-check">
              <DropdownMenu.ItemIndicator><Icon name="check" size={14} /></DropdownMenu.ItemIndicator>
            </span>
          </DropdownMenu.CheckboxItem>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu>
  );
}

export function PlanChip(props: { narrow: boolean }) {
  return (
    <Show when={resolve().plan}>
      <button
        type="button"
        class="cmp-chip"
        data-tone="accent"
        data-narrow={props.narrow}
        aria-label="Exit plan mode"
        title="Exit plan mode"
        onClick={() => {
          apply({ plan: false });
          focusComposer();
        }}
      >
        <Glyph name="access-read-only" size={14} />
        <Show when={!props.narrow}>Plan</Show>
      </button>
    </Show>
  );
}

export function AccessPicker(props: { narrow: boolean }) {
  const r = createMemo(resolve);
  const saving = () => !!selectedChat() && !!catalog.saving[selectedChat()!.id];
  const details = () => ACCESS[r().sandbox];
  const unavailable = (level: Sandbox) => (level === "auto" && !automaticReviewSupported(r().harness) ? "Not available for this agent" : null);
  return (
    <DropdownMenu placement="top-start" gutter={8} modal={false}>
      <DropdownMenu.Trigger
        class="cmp-chip"
        data-narrow={props.narrow}
        data-tone={r().sandbox === "danger-full-access" ? "warning" : undefined}
        aria-label={`Permission mode: ${details().label}`}
        title={props.narrow ? details().label : undefined}
        disabled={saving()}
      >
        <Glyph name={details().icon} size={14} />
        <Show when={!props.narrow}>{saving() ? "Saving…" : details().label}</Show>
        <Icon name="alt-arrow-down" size={12} class="cmp-chevron" />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content class="cmp-popover cmp-menu" style={{ width: "260px" }} onCloseAutoFocus={closeFocus}>
          <DropdownMenu.Group>
            <DropdownMenu.GroupLabel class="cmp-menu-heading">Agent access</DropdownMenu.GroupLabel>
            <DropdownMenu.RadioGroup value={r().sandbox} onChange={(value) => apply({ sandbox: value as Sandbox })}>
              <For each={SANDBOXES}>
                {(level) => (
                  <DropdownMenu.RadioItem value={level} class="cmp-menu-row" data-tall="true" closeOnSelect disabled={!!unavailable(level)} onSelect={markPicked}>
                    <Glyph name={ACCESS[level].icon} size={16} style={{ color: r().sandbox === level ? "var(--text)" : "var(--text-muted)" }} />
                    <span class="cmp-menu-text">
                      <span class="cmp-menu-title">{ACCESS[level].label}</span>
                      <span class="cmp-menu-desc">{unavailable(level) ?? ACCESS[level].description}</span>
                    </span>
                    <span class="cmp-menu-check">
                      <DropdownMenu.ItemIndicator><Icon name="check" size={14} /></DropdownMenu.ItemIndicator>
                    </span>
                  </DropdownMenu.RadioItem>
                )}
              </For>
            </DropdownMenu.RadioGroup>
          </DropdownMenu.Group>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu>
  );
}

export function PluginsButton() {
  return (
    <button type="button" class="cmp-key" aria-label="Plugins" title="Plugins" onClick={() => navigate({ kind: "plugins" })}>
      <Icon name="dither-plug" size={14} />
    </button>
  );
}

/** Compact token counts: 999, 241.4K, 1M, 1.2M. */
export function formatTokens(count: number) {
  const compact = (value: number, suffix: string) => {
    const rounded = Math.round(value * 10) / 10;
    return `${Number.isInteger(rounded) ? rounded.toFixed(0) : rounded.toFixed(1)}${suffix}`;
  };
  if (count < 1000) return String(count);
  if (count < 999_950) return compact(count / 1000, "K");
  return compact(count / 1_000_000, "M");
}

/** The selected chat's context occupancy as reported by the host. */
export function contextReading() {
  const usage = state.transcript?.contextUsage as { tokens?: number | null; used?: number | null; window?: number | null } | null | undefined;
  const window = usage?.window;
  if (!usage || !window || window <= 0) return null;
  const tokens = usage.tokens ?? usage.used ?? null;
  if (tokens === null || tokens === undefined) return { header: formatTokens(window), fraction: null, available: null };
  return {
    header: `${formatTokens(tokens)} / ${formatTokens(window)}`,
    fraction: Math.min(1, Math.max(0, tokens / window)),
    available: formatTokens(Math.max(0, window - tokens)),
  };
}

export function ContextRing() {
  const reading = createMemo(contextReading);
  const [open, setOpen] = createSignal(false);
  const high = () => (reading()?.fraction ?? 0) >= 0.9;
  const C = 2 * Math.PI * 5.5;
  return (
    <Show when={reading()}>
      {(r) => (
        <Popover open={open()} onOpenChange={setOpen} placement="top-end" gutter={8}>
          <Popover.Trigger
            class="cmp-key cmp-ring"
            data-high={high()}
            aria-label={`Context: ${r().header}${r().fraction !== null ? `, ${Math.round(r().fraction! * 100)}% used` : ""}`}
          >
            <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
              <circle cx="7" cy="7" r="5.5" fill="none" stroke-width="2" class="cmp-ring-track" />
              <circle
                cx="7"
                cy="7"
                r="5.5"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-dasharray={`${C * (r().fraction ?? 0)} ${C}`}
                transform="rotate(-90 7 7)"
              />
            </svg>
          </Popover.Trigger>
          <Popover.Portal>
            <Popover.Content class="cmp-popover cmp-usage-card">
              <div class="cmp-usage-head">
                <span class="cmp-usage-title">Context</span>
                <span class="cmp-usage-value">{r().header}</span>
              </div>
              <Show when={r().fraction !== null}>
                <div class="cmp-meter" data-high={high()} role="meter" aria-label="Window used" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(r().fraction! * 100)}>
                  <span style={{ width: `${r().fraction! * 100}%` }} />
                </div>
                <div class="cmp-usage-head">
                  <span>Window used</span>
                  <span class="cmp-usage-value">{Math.round(r().fraction! * 100)}%</span>
                </div>
                <div class="cmp-usage-head">
                  <span>Available</span>
                  <span class="cmp-usage-value">{r().available}</span>
                </div>
              </Show>
            </Popover.Content>
          </Popover.Portal>
        </Popover>
      )}
    </Show>
  );
}

export type SendMode = "send" | "queue" | "stop";

export function SendKey(props: { mode: SendMode; blocked: boolean; empty: boolean; stopping: boolean; onSend: () => void; onStop: () => void; label: string }) {
  return (
    <Show
      when={props.mode !== "stop"}
      fallback={
        <button
          type="button"
          class="cmp-stop"
          aria-label="Stop"
          title="Stop"
          disabled={props.stopping}
          style={{ "background-image": ditherBackground(resolvedTheme() === "light" ? [115, 115, 115] : [163, 163, 163], 28, 28, 7) }}
          onClick={() => props.onStop()}
        >
          <span class="cmp-stop-square" />
        </button>
      }
    >
      <button
        type="button"
        class="cmp-send"
        aria-label={props.label}
        title={props.label}
        aria-disabled={props.blocked || props.empty}
        style={{ "background-image": props.blocked || props.empty ? undefined : ditherBackground(SEND_DITHER, 28, 28, 7) }}
        onClick={() => !props.blocked && props.onSend()}
      >
        <Icon name="dither-send" size={14} />
      </button>
    </Show>
  );
}
