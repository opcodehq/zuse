// The command-palette surface the project flows share with desktop's Cmd+K (command_palette.rs `cmd`).
import { createVirtualizer } from "@tanstack/solid-virtual";
import { createEffect, For, type JSX, on, Show, splitProps } from "solid-js";
import { Icon } from "@/ui/Icon";

const mac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

export type Key = "up" | "down" | "left" | "enter" | "mod" | (string & {});

export function KeyCaps(props: { keys: Key[] }) {
  return (
    <span class="pj-keys">
      <For each={props.keys}>
        {(key) => (
          <kbd class="pj-cap">
            {key === "up" ? (
              <Icon name="arrow-up" size={12} />
            ) : key === "down" ? (
              <Icon name="arrow-down" size={12} />
            ) : key === "left" ? (
              <Icon name="arrow-left" size={12} />
            ) : key === "enter" ? (
              <Icon name="return" size={12} />
            ) : key === "mod" ? (
              mac ? "⌘" : "Ctrl"
            ) : (
              key
            )}
          </kbd>
        )}
      </For>
    </span>
  );
}

export function KeyHint(props: { keys: Key[]; label: string }) {
  return (
    <span class="pj-hint">
      <KeyCaps keys={props.keys} />
      <span>{props.label}</span>
    </span>
  );
}

export function Highlight(props: { text: string; query: string }) {
  const parts = () => {
    const query = props.query.trim().toLowerCase();
    const at = query ? props.text.toLowerCase().indexOf(query) : -1;
    if (at < 0) return [props.text, "", ""] as const;
    return [props.text.slice(0, at), props.text.slice(at, at + query.length), props.text.slice(at + query.length)] as const;
  };
  return (
    <>
      {parts()[0]}
      <Show when={parts()[1]}>
        <span class="pj-hl">{parts()[1]}</span>
      </Show>
      {parts()[2]}
    </>
  );
}

export function GroupLabel(props: { children: JSX.Element; status?: string | null }) {
  return (
    <div class="pj-group" role="presentation">
      {props.children}
      <Show when={props.status}>
        <span class="pj-group-status">{props.status}</span>
      </Show>
    </div>
  );
}

/** A palette result row; the input keeps focus, so rows are reached with arrows and clicks. */
export function Row(
  props: JSX.ButtonHTMLAttributes<HTMLButtonElement> & { highlighted?: boolean; glyph?: string; children: JSX.Element },
) {
  const [local, rest] = splitProps(props, ["highlighted", "glyph", "children", "class"]);
  return (
    <button
      type="button"
      role="option"
      tabIndex={-1}
      aria-selected={local.highlighted ? "true" : "false"}
      {...rest}
      class={`pj-row ${local.class ?? ""}`}
      data-highlighted={local.highlighted ? "true" : undefined}
      // The input keeps focus; a row press must not steal it.
      onMouseDown={(event) => event.preventDefault()}
    >
      <Show when={local.glyph}>{(name) => <Icon name={name()} size={16} class="pj-glyph" />}</Show>
      {local.children}
    </button>
  );
}

export function Radio(props: { on: boolean }) {
  return <span class="pj-radio" data-on={props.on ? "true" : undefined} aria-hidden="true" />;
}

/**
 * A virtualized palette list. `items` are rendered by `children`; `active` scrolls the
 * row at that item index into view.
 */
export function VirtualList<T>(props: {
  id: string;
  label: string;
  items: readonly T[];
  estimate: (item: T) => number;
  itemKey: (item: T, index: number) => string;
  active?: number;
  class?: string;
  children: (item: T, index: number) => JSX.Element;
  footer?: JSX.Element;
  role?: JSX.HTMLAttributes<HTMLDivElement>["role"];
  pad?: number;
}) {
  let scroller: HTMLDivElement | undefined;
  const virtualizer = createVirtualizer({
    get count() {
      return props.items.length;
    },
    getScrollElement: () => scroller ?? null,
    estimateSize: (index) => props.estimate(props.items[index]!),
    getItemKey: (index) => props.itemKey(props.items[index]!, index),
    overscan: 10,
    get paddingStart() {
      return props.pad ?? 8;
    },
    get paddingEnd() {
      return props.pad ?? 8;
    },
    scrollPaddingStart: 8,
    scrollPaddingEnd: 8,
    // Measuring inside the observer callback resizes the list in the same frame and trips the loop guard.
    useAnimationFrameWithResizeObserver: true,
  });
  createEffect(
    on(
      () => [props.active, props.items.length] as const,
      ([active, count]) => {
        if (active !== undefined && active >= 0 && active < count) virtualizer.scrollToIndex(active, { align: "auto" });
      },
    ),
  );
  return (
    <div ref={scroller} id={props.id} class={`pj-list ${props.class ?? ""}`} role={props.role ?? "listbox"} aria-label={props.label}>
      <div style={{ height: `${virtualizer.getTotalSize()}px`, position: "relative" }}>
        <For each={virtualizer.getVirtualItems()}>
          {(row) => (
            <div
              data-index={row.index}
              ref={(el) => queueMicrotask(() => el.isConnected && virtualizer.measureElement(el))}
              class="pj-vrow"
              style={{ transform: `translateY(${row.start}px)` }}
            >
              <Show when={props.items[row.index]} keyed>
                {(item) => props.children(item, row.index)}
              </Show>
            </div>
          )}
        </For>
      </div>
      {props.footer}
    </div>
  );
}
