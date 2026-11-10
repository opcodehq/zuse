// New-chat destination chips on the context tray: project, computer, and import.
import { Popover } from "@kobalte/core/popover";
import { createMemo, createSignal, For, type JSX, Show } from "solid-js";
import type { Space } from "@/core/protocol";
import { state } from "@/core/store";
import { navigate, setView, view } from "@/core/view";
import { openImportChat } from "@/features/extras/overlay";
import { Icon } from "@/ui/Icon";
import { Glyph } from "./assets";
import { catalog, deviceOnline, draftSpace, setCatalog, spaceName, targetDeviceId } from "./catalog";
import { focusComposer } from "./drafts";

export interface PickOption {
  id: string;
  label: string;
  icon?: string;
  meta?: string;
  selected: boolean;
  onSelect: () => void;
}

/** A tray chip that opens a searchable list. Arrow keys move, Enter picks, Escape closes. */
export function SearchPicker(props: {
  label: string;
  ariaLabel: string;
  icon: string;
  tone?: "warning";
  options: PickOption[];
  search?: string;
  empty: string;
  noMatch: string;
  footer?: { label: string; icon: string; onSelect: () => void };
  /** Rows above the searchable list that stay put while filtering. */
  pinned?: PickOption[];
  disabled?: boolean;
}) {
  const [open, setOpen] = createSignal(false);
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal(0);
  const listId = `cmp-pick-${Math.random().toString(36).slice(2)}`;
  let searchInput: HTMLInputElement | undefined;
  const filtered = createMemo(() => {
    const q = query().trim().toLowerCase();
    const match = (o: PickOption) => !q || o.label.toLowerCase().includes(q);
    return [...(props.pinned ?? []).filter(match), ...props.options.filter(match)];
  });
  let picked = false;
  const choose = (option: PickOption | undefined) => {
    if (!option) return;
    option.onSelect();
    picked = true;
    setOpen(false);
  };
  const onOpenChange = (next: boolean) => {
    setOpen(next);
    if (next) {
      setQuery("");
      setActive(Math.max(0, filtered().findIndex((o) => o.selected)));
    }
  };
  const onKeyDown = (event: KeyboardEvent) => {
    const count = filtered().length;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (count) setActive((i) => (i + (event.key === "ArrowDown" ? 1 : -1) + count) % count);
    } else if (event.key === "Enter") {
      event.preventDefault();
      choose(filtered()[active()]);
    }
  };
  return (
    <Popover open={open()} onOpenChange={onOpenChange} placement="top-start" gutter={6}>
      <Popover.Trigger class="cmp-tray-chip" aria-label={props.ariaLabel} data-tone={props.tone} disabled={props.disabled}>
        <Glyph name={props.icon} size={14} />
        <span class="cmp-truncate">{props.label}</span>
        <Icon name="alt-arrow-down" size={12} class="cmp-chevron" />
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          class="cmp-popover cmp-menu"
          style={{ width: "280px" }}
          onKeyDown={onKeyDown}
          onOpenAutoFocus={(event) => {
            picked = false;
            if (!searchInput) return;
            event.preventDefault();
            searchInput.focus();
          }}
          onCloseAutoFocus={(event) => {
            if (!picked) return;
            event.preventDefault();
            focusComposer();
          }}
        >
          <Show when={props.search}>
            <label class="cmp-menu-search">
              <Icon name="magnifer" size={14} />
              <input
                ref={searchInput}
                value={query()}
                placeholder={props.search}
                aria-label={props.search}
                aria-controls={listId}
                aria-activedescendant={filtered().length ? `${listId}-${active()}` : undefined}
                onInput={(event) => {
                  setQuery(event.currentTarget.value);
                  setActive(0);
                }}
              />
            </label>
          </Show>
          <div class="cmp-menu-scroll" role="listbox" id={listId} aria-label={props.ariaLabel} tabIndex={props.search ? -1 : 0}>
            <For each={filtered()}>
              {(option, index) => (
                <div
                  id={`${listId}-${index()}`}
                  role="option"
                  class="cmp-menu-row"
                  aria-selected={option.selected}
                  data-highlighted={active() === index() ? "" : undefined}
                  onMouseMove={() => setActive(index())}
                  onClick={() => choose(option)}
                >
                  <Show when={option.icon} fallback={<span style={{ width: "14px", "flex-shrink": 0 }} />}>
                    {(name) => <Glyph name={name()} size={14} />}
                  </Show>
                  <span class="cmp-menu-title" style={{ "font-weight": 400, flex: 1 }}>{option.label}</span>
                  <Show when={option.meta}><span class="cmp-menu-meta">{option.meta}</span></Show>
                  <span class="cmp-menu-check">
                    <Show when={option.selected}><Icon name="check" size={14} /></Show>
                  </span>
                </div>
              )}
            </For>
            <Show when={!filtered().some((o) => !props.pinned?.some((p) => p.id === o.id))}>
              <div class="cmp-menu-note">{query().trim() ? props.noMatch : props.empty}</div>
            </Show>
          </div>
          <Show when={props.footer}>
            {(footer) => (
              <>
                <div class="cmp-menu-sep" />
                <button
                  type="button"
                  class="cmp-menu-row"
                  onClick={() => {
                    setOpen(false);
                    footer().onSelect();
                  }}
                >
                  <Icon name={footer().icon} size={14} />
                  <span class="cmp-menu-title" style={{ "font-weight": 400 }}>{footer().label}</span>
                </button>
              </>
            )}
          </Show>
        </Popover.Content>
      </Popover.Portal>
    </Popover>
  );
}

const deviceLabel = (deviceId: string | null | undefined) =>
  !deviceId || deviceId === state.localDeviceId ? "This computer" : state.devices[deviceId]?.name ?? "Unknown computer";

/** One row per project path; the computer chip picks which device's copy runs it. */
function projectGroups(): { path: string; name: string; spaces: Space[] }[] {
  const groups = new Map<string, Space[]>();
  for (const space of Object.values(state.spaces)) groups.set(space.path, [...(groups.get(space.path) ?? []), space]);
  return [...groups.entries()]
    .map(([path, spaces]) => ({ path, spaces, name: spaceName(spaces.find((s) => s.deviceId === state.localDeviceId) ?? spaces[0]!) }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

export function ProjectChip() {
  const current = () => draftSpace();
  const options = createMemo<PickOption[]>(() =>
    projectGroups().map((group) => ({
      id: group.path,
      label: group.name,
      icon: "dither-folder",
      selected: current()?.path === group.path,
      onSelect: () => {
        const space = group.spaces.find((s) => s.deviceId === catalog.draft.deviceId) ?? group.spaces.find((s) => s.deviceId === state.localDeviceId) ?? group.spaces[0]!;
        setView("draftProjectPath", group.path);
        setCatalog("draft", { noProject: false, deviceId: space.deviceId });
      },
    })),
  );
  const none = (): PickOption => ({
    id: "",
    label: "No project",
    selected: !current(),
    onSelect: () => setCatalog("draft", { noProject: true, deviceId: targetDeviceId() ?? undefined }),
  });
  return (
    <SearchPicker
      label={current() ? spaceName(current()!) : "No project"}
      ariaLabel="Project"
      icon="dither-folder"
      options={options()}
      pinned={[none()]}
      search="Search projects…"
      empty="No projects yet."
      noMatch="No projects match."
      footer={{ label: "Add new project…", icon: "plus", onSelect: () => navigate({ kind: "newProject" }) }}
    />
  );
}

export function ComputerChip() {
  const current = () => targetDeviceId();
  const options = createMemo<PickOption[]>(() => {
    const space = draftSpace();
    if (space) {
      return Object.values(state.spaces)
        .filter((s) => s.path === space.path)
        .map((s) => ({
          id: s.id,
          label: deviceLabel(s.deviceId),
          icon: "dither-monitor",
          meta: deviceOnline(s.deviceId) ? undefined : "Offline",
          selected: s.id === space.id,
          onSelect: () => setCatalog("draft", "deviceId", s.deviceId),
        }));
    }
    const ids = [...new Set([state.localDeviceId, ...Object.keys(state.devices)].filter((id): id is string => !!id))];
    return ids.map((id) => ({
      id,
      label: deviceLabel(id),
      icon: "dither-monitor",
      meta: deviceOnline(id) ? undefined : "Offline",
      selected: id === current(),
      onSelect: () => setCatalog("draft", "deviceId", id),
    }));
  });
  // With only one computer to run on there is nothing to choose, so the chip stays out of the way.
  return (
    <Show when={options().length > 1}>
      <SearchPicker
        label={deviceLabel(current())}
        ariaLabel="Computer"
        icon="dither-monitor"
        tone={deviceOnline(current()) ? undefined : "warning"}
        options={options()}
        empty="No computers available."
        noMatch="No computers match."
      />
    </Show>
  );
}

export function ImportChip() {
  return (
    <button type="button" class="cmp-tray-chip" onClick={() => openImportChat()} disabled={state.connection.kind !== "connected"}>
      <Glyph name="chat-import" size={14} />
      <span class="cmp-truncate">Import chat</span>
    </button>
  );
}

/** The new-chat context row: where the chat runs, then what it starts from. The computer chip shows only with a choice. */
export function ContextRow(props: { children?: JSX.Element }) {
  return (
    <div class="cmp-context-row">
      <ProjectChip />
      <ComputerChip />
      <Show when={!view.selectedChatId}>
        <ImportChip />
      </Show>
      <div class="cmp-spacer" />
      {props.children}
    </div>
  );
}
