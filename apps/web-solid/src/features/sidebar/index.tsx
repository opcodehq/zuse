// The left column (crates/ui shell.rs render_chat_sidebar): the title strip, global actions, the list
// heading, the project filter, search, the session list and the footer.
import "./sidebar.css";
import { Popover } from "@kobalte/core/popover";
import { createMemo, createSignal, For, type JSX, Show } from "solid-js";
import { formatKeys } from "@/core/keys";
import { state } from "@/core/store";
import { setSidebarVisible, view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { Tooltip } from "@/ui/primitives";
import { signedIn } from "./account";
import { newChat, newProject, openPlugins } from "./actions";
import { Dialogs } from "./Dialogs";
import { Footer } from "./Footer";
import { List } from "./List";
import { EntriesMenu, viewEntries } from "./menus";
import { projectKey, projectList, projectOffline, projectRemotes, representative, spaceName } from "./model";
import { now } from "./util";
import { query, searchOpen, setQuery, setSearchOpen } from "./overlay";
import { option, organization, setOption } from "./prefs";
import { ProjectAvatar } from "./Row";

function HeadButton(props: { label: string; icon: string; active?: boolean; onClick: () => void }) {
  return (
    <Tooltip content={props.label}>
      <button
        type="button"
        class="sb-hbtn"
        aria-label={props.label}
        aria-pressed={props.active === undefined ? undefined : props.active}
        data-active={props.active ? "true" : undefined}
        onClick={() => props.onClick()}
      >
        <Icon name={props.icon} size={14} />
      </button>
    </Tooltip>
  );
}

function Nav() {
  const row = (label: string, icon: string, onClick: () => void, current?: () => boolean) => (
    <button type="button" class="sb-nav-row" aria-current={current?.() ? "page" : undefined} onClick={onClick}>
      <Icon name={icon} size={14} />
      <span>{label}</span>
    </button>
  );
  return (
    <nav class="sb-nav" aria-label="Main">
      {row("New chat", "dither-chat", () => newChat())}
      {row("New project", "dither-folder", newProject, () => view.route.kind === "newProject")}
      <Show when={signedIn()}>{row("Plugins", "dither-plug", openPlugins, () => view.route.kind === "plugins")}</Show>
    </nav>
  );
}

/** The project picker behind the filter chip and the Threads heading's folder button. */
function FilterMenu(props: { trigger: (triggerProps: Record<string, unknown>) => JSX.Element; placement: "bottom-start" | "bottom-end" }) {
  const [open, setOpen] = createSignal(false);
  const [text, setText] = createSignal("");
  const [active, setActive] = createSignal(0);
  const filter = () => option("spaceFilter");
  const filterKey = () => {
    const id = filter();
    const space = id ? state.spaces[id] : undefined;
    return space ? projectKey(space) : null;
  };
  const rows = createMemo(() => {
    const needle = text().trim().toLowerCase();
    const projects = projectList().filter((p) => !needle || p.name.toLowerCase().includes(needle));
    return [...(needle ? [] : [{ key: "", spaceId: null as string | null, name: "All projects" }]), ...projects, { key: "new", spaceId: null, name: "New project" }];
  });
  const choose = (row: { key: string; spaceId: string | null }) => {
    setOpen(false);
    if (row.key === "new") return newProject();
    setOption("spaceFilter", row.spaceId);
  };
  return (
    <Popover
      open={open()}
      onOpenChange={(next) => {
        setOpen(next);
        if (next) {
          setText("");
          setActive(0);
        }
      }}
      placement={props.placement}
      gutter={4}
    >
      <Popover.Trigger as={(p: Record<string, unknown>) => props.trigger(p)} />
      <Popover.Portal>
        <Popover.Content class="menu sb-filter-menu" aria-label="Filter by project">
          <input
            class="field sb-filter-search"
            placeholder="Search projects"
            aria-label="Search projects"
            aria-controls="sb-filter-list"
            aria-activedescendant={`sb-filter-${active()}`}
            value={text()}
            autofocus
            onInput={(e) => {
              setText(e.currentTarget.value);
              setActive(0);
            }}
            onKeyDown={(e) => {
              const count = rows().length;
              if (e.key === "ArrowDown") setActive((active() + 1) % count);
              else if (e.key === "ArrowUp") setActive((active() + count - 1) % count);
              else if (e.key === "Enter") {
                const row = rows()[active()];
                if (row) choose(row);
              } else return;
              e.preventDefault();
              document.getElementById(`sb-filter-${active()}`)?.scrollIntoView({ block: "nearest" });
            }}
          />
          <div class="sb-filter-list" id="sb-filter-list" role="listbox" aria-label="Projects">
            <For each={rows()}>
              {(row, index) => {
                const selected = () => (row.key === "" ? !filter() : row.key !== "new" && row.key === filterKey());
                const space = () => (row.spaceId ? state.spaces[row.spaceId] : undefined);
                return (
                  <>
                    <Show when={row.key === "new"}>
                      <div class="menu-sep" />
                    </Show>
                    <div
                      id={`sb-filter-${index()}`}
                      class="menu-item"
                      role="option"
                      aria-selected={selected()}
                      data-highlighted={active() === index() ? "" : undefined}
                      onPointerMove={() => setActive(index())}
                      onClick={() => choose(row)}
                    >
                      <Show
                        when={space()}
                        fallback={<Icon name={row.key === "new" ? "plus" : "folder"} size={14} />}
                      >
                        {(s) => <ProjectAvatar name={row.name} remotes={projectRemotes(s())} />}
                      </Show>
                      <span style={{ flex: 1, "min-width": 0, overflow: "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }}>{row.name}</span>
                      <Show when={selected()}>
                        <span class="sb-menu-check"><Icon name="check" size={14} /></span>
                      </Show>
                    </div>
                  </>
                );
              }}
            </For>
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover>
  );
}

function Heading() {
  const org = () => organization();
  const label = () => (org() === "projects" ? "Projects" : org() === "threads" ? "Threads" : "Devices");
  const toggleSearch = () => {
    const next = !searchOpen();
    setSearchOpen(next);
    if (!next) setQuery("");
  };
  return (
    <div class="sb-heading">
      <span class="sb-heading-label" id="sb-heading-label">{label()}</span>
      <HeadButton label="Search threads" icon="magnifer" active={searchOpen()} onClick={toggleSearch} />
      <Show when={org() === "threads" && option("showProjectFilter")}>
        <FilterMenu
          placement="bottom-end"
          trigger={(p) => (
            <button type="button" {...p} class="sb-hbtn" aria-label="Filter by project" title="Filter by project" data-active={option("spaceFilter") ? "true" : undefined}>
              <Icon name="folder" size={14} />
            </button>
          )}
        />
      </Show>
      <EntriesMenu
        label="View options"
        entries={viewEntries}
        trigger={(p) => (
          <button type="button" {...p} class="sb-hbtn" title="View options">
            <Icon name="more-horizontal" size={14} />
          </button>
        )}
      />
      <Show when={org() === "projects"}>
        <HeadButton label="New project" icon="folder-with-files" onClick={newProject} />
      </Show>
      <HeadButton label="New chat" icon="plus" onClick={() => newChat()} />
    </div>
  );
}

function FilterChip() {
  const space = () => {
    const id = option("spaceFilter");
    return id ? state.spaces[id] : undefined;
  };
  const offline = () => {
    const s = space();
    return !!s && projectOffline(s, now());
  };
  return (
    <Show when={option("showProjectFilter") && option("spaceFilter")}>
      <div class="sb-chiprow">
        <div class="sb-chip">
          <FilterMenu
            placement="bottom-start"
            trigger={(p) => (
              <button type="button" {...p} class="sb-chip-open" aria-label={`Project filter: ${space() ? spaceName(representative(space()!)) : "All projects"}`}>
                <Show when={space()} fallback={<Icon name="folder" size={14} />}>
                  {(s) => <ProjectAvatar name={spaceName(representative(s()))} remotes={projectRemotes(s())} />}
                </Show>
                <span style={{ "min-width": 0, overflow: "hidden", "white-space": "nowrap", "text-overflow": "ellipsis" }}>
                  {space() ? spaceName(representative(space()!)) : "All projects"}
                </span>
                <Show when={offline()}>
                  <span style={{ color: "color-mix(in srgb, var(--warning) 80%, transparent)", display: "inline-flex" }} title="No host is reachable">
                    <Icon name="wifi-off" size={12} />
                  </span>
                </Show>
              </button>
            )}
          />
          <button type="button" class="sb-chip-x" aria-label="Clear project filter" title="Clear filter" onClick={() => setOption("spaceFilter", null)}>
            <Icon name="close" size={12} />
          </button>
        </div>
      </div>
    </Show>
  );
}

function Search() {
  return (
    <Show when={searchOpen()}>
      <div class="sb-search" role="search">
        <Icon name="magnifer" size={14} style={{ color: "var(--text-faint)" }} />
        <input
          ref={(el) => queueMicrotask(() => el.focus())}
          type="search"
          placeholder="Search title, project, branch"
          aria-label="Search threads"
          value={query()}
          onInput={(e) => setQuery(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              setQuery("");
              setSearchOpen(false);
            } else if (e.key === "ArrowDown") {
              e.preventDefault();
              document.querySelector<HTMLElement>(".sb-scroll .sb-focus")?.focus();
            }
          }}
        />
        <button
          type="button"
          class="sb-chip-x"
          aria-label="Close search"
          onClick={() => {
            setQuery("");
            setSearchOpen(false);
          }}
        >
          <Icon name="close" size={12} />
        </button>
      </div>
    </Show>
  );
}

/** Desktop's sidebar title strip (shell/workspace_layout.rs workspace_sidebar_header): the app name and
 *  the hide control, 36px tall. */
function TitleStrip() {
  return (
    <div class="sb-title">
      <span class="sb-title-text">Zuse (Beta)</span>
      <Tooltip content={`Hide projects panel · ${formatKeys("mod+b")}`}>
        <button type="button" class="sb-fbtn" aria-label="Hide projects panel" onClick={() => setSidebarVisible(false)}>
          <Icon name="split-columns" size={14} />
        </button>
      </Tooltip>
    </div>
  );
}

export function Sidebar() {
  return (
    <div class="sb">
      <TitleStrip />
      <Nav />
      <Heading />
      <FilterChip />
      <Search />
      <List />
      <Footer />
      <Dialogs />
    </div>
  );
}
