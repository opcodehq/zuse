// The virtualized session list: Pinned, custom sections, then project, device or flat rows.
// Every item has an exact height from the model, so nothing is measured.
import { ContextMenu } from "@kobalte/core/context-menu";
import { createVirtualizer, defaultRangeExtractor } from "@tanstack/solid-virtual";
import { createEffect, createMemo, createSignal, For, type JSX, Match, on, Show, Switch, untrack } from "solid-js";
import { state } from "@/core/store";
import { view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { newChat } from "./actions";
import { dropHandlers } from "./drag";
import { EntriesMenu, MenuEntries, projectEntries, sectionEntries } from "./menus";
import { type Item, layout, projectRemotes, representative, spaceName } from "./model";
import { dragging, query } from "./overlay";
import { changeSection } from "./pins";
import { option, setOption, toggleCollapsed } from "./prefs";
import { ProjectAvatar, Row } from "./Row";
import { now } from "./util";

type ItemOf<K extends Item["kind"]> = Extract<Item, { kind: K }>;

function Chevron(props: { open: boolean }) {
  return (
    <span class="sb-chev" data-open={props.open ? "true" : "false"}>
      <Icon name="alt-arrow-down" size={12} />
    </span>
  );
}

function Count(props: { count: number; show: boolean }) {
  return (
    <Show when={props.show}>
      <span class="sb-count" aria-label={`${props.count} sessions`}>{props.count}</span>
    </Show>
  );
}

function PinnedHeader(props: { item: ItemOf<"pinned"> }) {
  return (
    <div class="sb-head" {...dropHandlers(() => ({ kind: "pinned" }))}>
      <button
        type="button"
        class="sb-head-toggle sb-focus"
        aria-expanded={props.item.open}
        onClick={() => setOption("pinnedOpen", !props.item.open)}
      >
        <span class="sb-head-label">Pinned</span>
        <Count count={props.item.count} show={!props.item.open} />
        <span style={{ flex: 1 }} />
        <Chevron open={props.item.open} />
      </button>
    </div>
  );
}

function SectionHeader(props: { item: ItemOf<"section"> }) {
  const section = () => props.item.section;
  return (
    <div class="sb-head" {...dropHandlers(() => ({ kind: "section", id: section().id }))}>
      <button
        type="button"
        class="sb-head-toggle sb-focus"
        aria-expanded={!section().collapsed}
        onClick={() => changeSection({ action: "collapse", id: section().id, collapsed: !section().collapsed })}
      >
        <span class="sb-head-label">{section().name}</span>
        <Count count={props.item.count} show={section().collapsed} />
        <span style={{ flex: 1 }} />
      </button>
      <span class="sb-head-actions">
        <EntriesMenu
          label="Section options"
          entries={() => sectionEntries(section().id)}
          trigger={(p) => (
            <button type="button" {...p} class="sb-act" title="Section options">
              <Icon name="more-horizontal" size={14} />
            </button>
          )}
        />
        <Chevron open={!section().collapsed} />
      </span>
    </div>
  );
}

function GroupHeader(props: { item: ItemOf<"group"> }) {
  const space = () => (props.item.spaceId ? state.spaces[props.item.spaceId] : undefined);
  const showIcon = () => props.item.project && option("showProjectIcon") && !!space();
  const toggle = () => toggleCollapsed(props.item.groupKey);
  const header = (rest: Record<string, unknown>): JSX.Element => (
    <div {...rest} class="sb-head" data-project={props.item.project ? "true" : undefined} {...dropHandlers(() => ({ kind: "regular" }))}>
      <button type="button" class="sb-head-toggle sb-focus" aria-expanded={props.item.open} onClick={toggle}>
        <Show
          when={props.item.project}
          fallback={
            <>
              <span class="sb-head-label">{props.item.label}</span>
              <Count count={props.item.count} show={!props.item.open} />
              <span style={{ flex: 1 }} />
              <Chevron open={props.item.open} />
            </>
          }
        >
          <span class="sb-lead" data-icon={showIcon() ? "true" : undefined}>
            <Show when={showIcon() && space()}>
              {(s) => (
                <span class="sb-lead-icon">
                  <ProjectAvatar name={spaceName(representative(s()))} remotes={projectRemotes(s())} />
                </span>
              )}
            </Show>
            <span class="sb-lead-chev">
              <Chevron open={props.item.open} />
            </span>
          </span>
          <span class="sb-head-label">{props.item.label}</span>
          <Count count={props.item.count} show={!props.item.open} />
          <span style={{ flex: 1 }} />
        </Show>
      </button>
      <Show when={props.item.project && props.item.spaceId}>
        {(spaceId) => (
          <span class="sb-head-actions">
            <EntriesMenu
              label={`Project settings for ${props.item.label}`}
              entries={() => projectEntries(spaceId())}
              placement="bottom-start"
              trigger={(p) => (
                <button type="button" {...p} class="sb-act" title="Project settings">
                  <Icon name="settings" size={14} />
                </button>
              )}
            />
            <button
              type="button"
              class="sb-act"
              title="New chat in project"
              aria-label={`New chat in ${props.item.label}`}
              onClick={() => newChat(props.item.groupKey)}
            >
              <Icon name="pen-new-square" size={14} />
            </button>
          </span>
        )}
      </Show>
    </div>
  );
  return (
    <Show when={props.item.project && props.item.spaceId} fallback={header({})}>
      {(spaceId) => (
        <ContextMenu modal={false}>
          <ContextMenu.Trigger as={(p: Record<string, unknown>) => header(p)} />
          <ContextMenu.Portal>
            <ContextMenu.Content class="menu" aria-label={`Actions for ${props.item.label}`}>
              <MenuEntries entries={untrack(() => projectEntries(spaceId()))} />
            </ContextMenu.Content>
          </ContextMenu.Portal>
        </ContextMenu>
      )}
    </Show>
  );
}

function Note(props: { item: ItemOf<"note"> }) {
  const zone = () => props.item.zone;
  return (
    <Show when={zone()} fallback={<div class="sb-note">{props.item.text}</div>}>
      {(z) => (
        <div
          class="sb-note"
          data-drop={dragging() ? "true" : undefined}
          {...dropHandlers(() => {
            const current = z();
            return current.kind === "section" ? { kind: "section", id: current.id } : current.kind === "pinned" ? { kind: "pinned" } : { kind: "regular" };
          })}
        >
          {props.item.text}
        </div>
      )}
    </Show>
  );
}

function ItemView(props: { item: Item; search: boolean }) {
  return (
    <div style={{ "padding-top": `${props.item.top}px`, height: `${props.item.size}px` }}>
      <Switch>
        <Match when={props.item.kind === "row" && props.item}>{(item) => <Row item={item()} search={props.search} />}</Match>
        <Match when={props.item.kind === "group" && props.item}>{(item) => <GroupHeader item={item()} />}</Match>
        <Match when={props.item.kind === "section" && props.item}>{(item) => <SectionHeader item={item()} />}</Match>
        <Match when={props.item.kind === "pinned" && props.item}>{(item) => <PinnedHeader item={item()} />}</Match>
        <Match when={props.item.kind === "note" && props.item}>{(item) => <Note item={item()} />}</Match>
      </Switch>
    </div>
  );
}

export function List() {
  let scroller!: HTMLDivElement;
  const [atTop, setAtTop] = createSignal(true);
  const model = createMemo(() => layout({ at: now(), query: query(), dragging: !!dragging() }));
  const items = () => model().items;
  const byKey = createMemo(() => new Map(items().map((item) => [item.key, item])));
  const virtualizer = createVirtualizer({
    get count() {
      return items().length;
    },
    getScrollElement: () => scroller,
    estimateSize: (index) => items()[index]?.size ?? 28,
    // A fresh key function per layout is what makes the virtualizer re-measure: a reorder or a
    // height change at the same count would otherwise keep stale keys and offsets.
    get getItemKey() {
      const staticList = items();
      return (index: number) => staticList[index]?.key ?? index;
    },
    overscan: 12,
    // The dragged row stays mounted: removing a drag source cancels the browser's drag.
    rangeExtractor: (range) => {
      const indexes = defaultRangeExtractor(range);
      const id = dragging();
      if (!id) return indexes;
      const at = items().findIndex((item) => item.kind === "row" && item.chatId === id);
      return at < 0 || indexes.includes(at) ? indexes : [...indexes, at].sort((a, b) => a - b);
    },
    // Vertical padding lives here, not in CSS, so scrolled-to rows land exactly; the scroll
    // padding keeps them clear of the edge fades. The top matches desktop's SIDEBAR_LIST_PAD_TOP.
    paddingStart: 4,
    paddingEnd: 16,
    scrollPaddingStart: 12,
    scrollPaddingEnd: 24,
  });

  // Keyed by item key, so a row keeps its DOM node while items above it come and go.
  // Virtual items carry their own key: right after the list changes they can lag a tick, and
  // reading keys by index then would unmount rows that only moved.
  const placement = createMemo(() => {
    const map = new Map<string, { index: number; start: number }>();
    for (const virtual of virtualizer.getVirtualItems()) {
      if (typeof virtual.key === "string") map.set(virtual.key, { index: virtual.index, start: virtual.start });
    }
    return map;
  });
  const visibleKeys = createMemo(() => [...placement().keys()], undefined, { equals: (a, b) => a.length === b.length && a.every((k, i) => k === b[i]) });

  // Keep the open chat in view when something other than a click selects it.
  createEffect(
    on(
      () => {
        const id = view.selectedChatId;
        const open = id ? state.chats[id] : undefined;
        return open?.threadId ?? id;
      },
      (id) => {
        if (!id) return;
        const index = items().findIndex((item) => item.kind === "row" && item.chatId === id);
        if (index >= 0) virtualizer.scrollToIndex(index, { align: "auto" });
      },
    ),
  );
  createEffect(on(query, () => scroller && (scroller.scrollTop = 0), { defer: true }));

  /** Up/Down move between rows and headings; virtualized targets are scrolled in first. */
  const onKeyDown = (event: KeyboardEvent) => {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    const target = event.target as HTMLElement;
    if (!target.classList.contains("sb-focus")) return;
    const host = target.closest<HTMLElement>("[data-index]");
    if (!host) return;
    event.preventDefault();
    const list = items();
    const focusable = (i: number) => list[i] && list[i]!.kind !== "note";
    let index = Number(host.dataset.index);
    const step = event.key === "ArrowDown" || event.key === "Home" ? 1 : -1;
    if (event.key === "Home") index = -1;
    if (event.key === "End") index = list.length;
    do index += step;
    while (index >= 0 && index < list.length && !focusable(index));
    if (index < 0 || index >= list.length) return;
    virtualizer.scrollToIndex(index, { align: "auto" });
    requestAnimationFrame(() => scroller.querySelector<HTMLElement>(`[data-index="${index}"] .sb-focus`)?.focus());
  };

  return (
    <div
      ref={scroller}
      class="sb-scroll"
      data-top={atTop() ? "true" : undefined}
      role="navigation"
      aria-label={model().mode === "search" ? "Search results" : "Threads"}
      onScroll={() => setAtTop(scroller.scrollTop <= 0)}
      onKeyDown={onKeyDown}
    >
      <div style={{ height: `${virtualizer.getTotalSize()}px`, position: "relative" }}>
        <For each={visibleKeys()}>
          {(key) => {
            const place = () => placement().get(key);
            return (
              <div class="sb-vitem" data-index={place()?.index} style={{ transform: `translateY(${place()?.start ?? 0}px)` }}>
                <Show when={byKey().get(key)}>{(item) => <ItemView item={item()} search={model().mode === "search"} />}</Show>
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
}

