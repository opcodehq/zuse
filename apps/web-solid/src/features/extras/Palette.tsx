// Global search over commands, threads and the selected chat's workspace, grouped in sections.
// Mirrors crates/ui/src/shell/command_palette.rs: tabs narrow to one kind, rows keep identity across refreshes.
import { batch, createEffect, createMemo, createSignal, For, Index, type JSX, Match, on, onCleanup, onMount, Show, Switch } from "solid-js";
import { createStore } from "solid-js/store";
import { rpc } from "@/core/connection";
import { chatRequest } from "@/core/chat";
import { formatKeys, listShortcuts } from "@/core/keys";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import { navigate, openChat, setView, view } from "@/core/view";
import { resolvedTheme } from "@/shell/theme";
import { FileIcon } from "@/features/workspace/parts";
import { Icon } from "@/ui/Icon";
import { TerminalSpinner } from "@/ui/loaders";
import { archiveSelectedChat, archivableChat, openWorkspacePath } from "./actions";
import { EmptyState } from "./feedback";
import { Highlight } from "./Highlight";
import { closeOverlay, openOverlay, type PaletteTab, paletteTab } from "./overlay";
import { endVoice, startVoice, voiceCall } from "./voice";
import {
  activityAt,
  byteRangesToChars,
  chatLocation,
  chatTitle,
  effectiveIndicator,
  isTopLevel,
  matchRanges,
  spaceName,
  splitPath,
  timeAgo,
} from "./util";

const ALL_TAB_LIMIT = 5;
const THREADS_TAB_LIMIT = 50;
const FILES_TAB_LIMIT = 50;
const CONTENT_TAB_LIMIT = 100;
const CONTENT_MATCHES_PER_FILE = 3;
const MIN_WORKSPACE_QUERY_CHARS = 2;
const CONTENT_DEBOUNCE = 80;
const INDEX_RETRY = 500;
const INDEX_RETRY_MAX = 2000;

type Tab = PaletteTab;
const TABS: { id: Tab; label: string }[] = [
  { id: "all", label: "All" },
  { id: "threads", label: "Threads" },
  { id: "commands", label: "Commands" },
  { id: "files", label: "Files" },
];

type SectionId = "recentFiles" | "quickActions" | "recentThreads" | "threads" | "commands" | "files" | "inFiles";
const SECTION_TITLES: Record<SectionId, string> = {
  recentFiles: "Recent Files",
  quickActions: "Quick Actions",
  recentThreads: "Recent Threads",
  threads: "Threads",
  commands: "Commands",
  files: "Files",
  inFiles: "In Files",
};

interface Command {
  id: string;
  label: string;
  icon: string;
  /** Matches a registered shortcut by label, so a rebind shows immediately. */
  shortcutLabel?: string;
  /** Theme switches keep the palette open so the row flips to its next state. */
  stayOpen?: boolean;
  run: () => void;
}

interface FileMatch {
  path: string;
  name: string;
  kind: "file" | "directory" | "symlink";
}
interface ContentMatch {
  path: string;
  line: number;
  column: number;
  preview: string;
  ranges: [number, number][];
}

type Row =
  | { key: string; kind: "command"; command: Command }
  | { key: string; kind: "chat"; id: string }
  | { key: string; kind: "file"; path: string; isDir: boolean }
  | { key: string; kind: "content"; match: ContentMatch };

interface Section {
  id: SectionId;
  rows: Row[];
  status: string | null;
}

interface Query<T> {
  loading: boolean;
  indexing: boolean;
  error: string | null;
  results: T[];
}
const idle = <T,>(): Query<T> => ({ loading: false, indexing: false, error: null, results: [] });

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function searchError(error: unknown, what: "file" | "content") {
  if (!rpc.connected) return "The session's device is unreachable";
  const message = error instanceof Error ? error.message.toLowerCase() : "";
  if (message.includes("unknown method") || message.includes("unsupported"))
    return `The session's device runs an older zuse — update it to search its ${what === "content" ? "file contents" : "files"}`;
  return "Search failed";
}

/** Desktop's thread_search: every word must hit a field; title outranks metadata; word starts earn half again. */
function scoreThread(fields: { title: string; project: string; branch: string; device: string; archived: boolean }, terms: string[]) {
  if (!terms.length) return null;
  const haystacks: [string, number][] = [
    [fields.title.toLowerCase(), 100],
    [fields.project.toLowerCase(), 40],
    [fields.branch.toLowerCase(), 30],
    [fields.device.toLowerCase(), 20],
  ];
  let total = 0;
  for (const term of terms) {
    let best: number | null = null;
    for (const [haystack, weight] of haystacks) {
      for (let at = haystack.indexOf(term); at >= 0; at = haystack.indexOf(term, at + 1)) {
        const startsWord = at === 0 || !/[\p{L}\p{N}]/u.test(haystack[at - 1]!);
        const score = startsWord ? weight + weight / 2 : weight;
        if (best === null || score > best) best = score;
        if (startsWord) break;
      }
    }
    if (best === null) return null;
    total += best;
  }
  return fields.archived ? Math.floor(total / 2) : total;
}

export function Palette() {
  const [query, setQuery] = createSignal("");
  const [tab, setTab] = createSignal<Tab>(paletteTab());
  const [activeKey, setActiveKey] = createSignal<string | null>(null);
  const [activeIndex, setActiveIndex] = createSignal(0);
  const [files, setFiles] = createStore<Query<FileMatch>>(idle());
  const [content, setContent] = createStore<Query<ContentMatch>>(idle());
  let input!: HTMLInputElement;
  let list!: HTMLDivElement;
  let disposed = false;
  let filesGeneration = 0;
  let contentGeneration = 0;
  let filesReadyFor: string | null = null;
  let enterDown = false;
  onCleanup(() => {
    disposed = true;
    filesGeneration++;
    contentGeneration++;
  });

  const trimmed = () => query().trim().toLowerCase();
  const isDark = () => resolvedTheme() === "dark";

  const commands = createMemo<Command[]>(() => {
    const live = voiceCall();
    const list: Command[] = [
      { id: "new-chat", label: "New chat", icon: "pen-new-square", run: () => openChat(null) },
      { id: "new-project", label: "New project", icon: "folder", shortcutLabel: "Open project", run: () => navigate({ kind: "newProject" }) },
      { id: "settings", label: "Open settings", icon: "settings", shortcutLabel: "Settings", run: () => navigate({ kind: "settings" }) },
      {
        id: "theme",
        label: isDark() ? "Switch to light theme" : "Switch to dark theme",
        icon: isDark() ? "sun" : "moon",
        stayOpen: true,
        run: () => setView("preferences", "theme", isDark() ? "light" : "dark"),
      },
    ];
    if (view.preferences.theme !== "system")
      list.push({ id: "theme-system", label: "Switch to system theme", icon: "monitor", stayOpen: true, run: () => setView("preferences", "theme", "system") });
    if (archivableChat())
      list.push({ id: "archive", label: "Archive thread", icon: "archive-minimalistic", run: () => void archiveSelectedChat() });
    list.push(
      { id: "import", label: "Import chat", icon: "history", run: () => openOverlay("import") },
      { id: "shortcuts", label: "Keyboard shortcuts", icon: "keyboard", shortcutLabel: "Show keyboard shortcuts", run: () => openOverlay("shortcuts") },
      live
        ? { id: "voice", label: "End voice call", icon: "phone-hang-up", run: () => endVoice() }
        : { id: "voice", label: "Start voice call", icon: "microphone", run: () => startVoice() },
    );
    const words = trimmed().split(/\s+/).filter(Boolean);
    return list.filter((command) => words.every((word) => command.label.toLowerCase().includes(word)));
  });

  const recentThreads = (limit: number) =>
    Object.values(state.chats)
      .filter((chat) => isTopLevel(chat) && !chat.archived && chat.id !== view.selectedChatId)
      .sort((a, b) => activityAt(b) - activityAt(a))
      .slice(0, limit)
      .map((chat) => chat.id);

  const threadMatches = (limit: number) => {
    const terms = trimmed().split(/\s+/).filter(Boolean);
    const scored: [string, number, number][] = [];
    // Global history ignores the sidebar's filters; archived threads stay searchable.
    for (const chat of Object.values(state.chats)) {
      if (!isTopLevel(chat)) continue;
      const score = scoreThread(
        {
          title: chatTitle(chat),
          project: spaceName(chat) ?? "",
          branch: chat.branch ?? "",
          device: state.devices[chat.deviceId]?.name ?? "",
          archived: chat.archived,
        },
        terms,
      );
      if (score !== null) scored.push([chat.id, score, activityAt(chat)]);
    }
    scored.sort((a, b) => b[1] - a[1] || b[2] - a[2]);
    return scored.slice(0, limit).map(([id]) => id);
  };

  // Rows keep identity per key, so refreshed results never remount what is already shown.
  const rowCache = new Map<string, Row>();
  const same = (a: Row, b: Row) => {
    if (a.kind === "command" && b.kind === "command") return a.command.label === b.command.label && a.command.icon === b.command.icon;
    if (a.kind === "content" && b.kind === "content") return a.match.preview === b.match.preview && JSON.stringify(a.match.ranges) === JSON.stringify(b.match.ranges);
    return a.kind === b.kind;
  };
  const keep = (row: Row): Row => {
    const old = rowCache.get(row.key);
    if (old && same(old, row)) return old;
    rowCache.set(row.key, row);
    return row;
  };
  const progress = (q: Query<unknown>) => (q.indexing ? "Indexing…" : q.loading ? "Searching…" : null);
  const status = (q: Query<unknown>) => progress(q) ?? q.error;

  const sections = createMemo<Section[]>(() => {
    const empty = trimmed() === "";
    const section = (id: SectionId, rows: Row[], sectionStatus: string | null = null): Section => ({ id, rows: rows.map(keep), status: sectionStatus });
    const commandSection = (id: SectionId) => section(id, commands().map((command) => ({ key: `command:${command.id}`, kind: "command", command })));
    const chats = (id: SectionId, ids: string[]) => section(id, ids.map((chatId) => ({ key: `chat:${chatId}`, kind: "chat", id: chatId })));
    const fileRows = (limit: number, skipDirs: boolean) =>
      files.results
        .filter((found) => !skipDirs || found.kind !== "directory")
        .slice(0, limit)
        .map((found): Row => ({ key: `file:${found.path}`, kind: "file", path: found.path, isDir: found.kind === "directory" }));
    const recentFiles = (limit: number) => section("recentFiles", fileRows(limit, true), progress(files));
    const fileSection = (limit: number) => section("files", fileRows(limit, false), status(files));
    const inFiles = (limit: number) =>
      section(
        "inFiles",
        content.results.slice(0, limit).map((match): Row => ({ key: `content:${match.path}:${match.line}:${match.column}`, kind: "content", match })),
        status(content),
      );
    const out: Section[] = (() => {
      switch (tab()) {
        case "all":
          return empty
            ? [recentFiles(ALL_TAB_LIMIT), commandSection("quickActions"), chats("recentThreads", recentThreads(ALL_TAB_LIMIT))]
            : [chats("threads", threadMatches(ALL_TAB_LIMIT)), commandSection("commands"), fileSection(ALL_TAB_LIMIT), inFiles(ALL_TAB_LIMIT)];
        case "threads":
          return [empty ? chats("recentThreads", recentThreads(THREADS_TAB_LIMIT)) : chats("threads", threadMatches(THREADS_TAB_LIMIT))];
        case "commands":
          return [commandSection(empty ? "quickActions" : "commands")];
        case "files":
          return empty ? [recentFiles(FILES_TAB_LIMIT)] : [fileSection(FILES_TAB_LIMIT), inFiles(CONTENT_TAB_LIMIT)];
      }
    })();
    return out.filter((s) => s.rows.length || s.status);
  });

  const flat = createMemo(() => sections().flatMap((s) => s.rows));
  const active = createMemo(() => {
    const rows = flat();
    const key = activeKey();
    const found = key ? rows.findIndex((row) => row.key === key) : -1;
    return found >= 0 ? found : Math.min(activeIndex(), Math.max(0, rows.length - 1));
  });
  const activeRow = () => flat()[active()];
  const rowId = (row: Row) => `x-palette-row-${encodeURIComponent(row.key).replace(/%/g, "_")}`;

  const resetActive = () => {
    batch(() => {
      setActiveKey(null);
      setActiveIndex(0);
    });
    if (list) list.scrollTop = 0;
  };

  function setActive(index: number, scroll: boolean) {
    const rows = flat();
    batch(() => {
      setActiveIndex(index);
      setActiveKey(rows[index]?.key ?? null);
    });
    if (!scroll) return;
    if (index === 0) {
      list.scrollTop = 0;
      return;
    }
    const row = rows[index];
    const element = row ? document.getElementById(rowId(row)) : null;
    // A section's first row brings its header along.
    const header = element?.previousElementSibling?.classList.contains("x-section-head") ? element.previousElementSibling : null;
    (header ?? element)?.scrollIntoView({ block: "nearest" });
    element?.scrollIntoView({ block: "nearest" });
  }

  function searchWorkspace() {
    const chatId = view.route.kind === "chat" ? view.selectedChatId : null;
    const text = query().trim();
    filesGeneration++;
    contentGeneration++;
    // An empty query lists the workspace's recently changed files; one character is too broad.
    if (!chatId || !state.chats[chatId] || !(text === "" || [...text].length >= MIN_WORKSPACE_QUERY_CHARS)) {
      setFiles(idle());
      setContent(idle());
      return;
    }
    void searchFiles(chatId, text, filesGeneration);
    if (text === "") setContent(idle());
    else void searchContent(chatId, text, contentGeneration);
  }

  async function searchFiles(chatId: string, text: string, generation: number) {
    setFiles({ loading: true });
    let probe = text !== "" && filesReadyFor !== chatId;
    for (;;) {
      let indexing = false;
      try {
        if (probe) {
          // Readiness comes first: a scan finishing after a partial answer still earns one final search.
          indexing = await rpc
            .call<{ state?: string }>(M.WARM_WORKSPACE_SEARCH, chatRequest(chatId))
            .then((warm) => warm?.state === "building", () => false);
        }
        const found = await rpc.call<FileMatch[]>(
          M.SEARCH_WORKSPACE_FILES,
          chatRequest(chatId, { query: text, includeIgnored: false, limit: FILES_TAB_LIMIT }),
        );
        if (disposed || generation !== filesGeneration) return;
        if (probe && !indexing) {
          filesReadyFor = chatId;
          probe = false;
        }
        setFiles({ loading: false, indexing, error: null, results: Array.isArray(found) ? found : [] });
      } catch (error) {
        if (disposed || generation !== filesGeneration) return;
        setFiles({ loading: false, indexing: false, error: searchError(error, "file"), results: [] });
        return;
      }
      if (!indexing) return;
      await sleep(INDEX_RETRY);
      if (disposed || generation !== filesGeneration) return;
    }
  }

  async function searchContent(chatId: string, text: string, generation: number) {
    setContent({ loading: true });
    await sleep(CONTENT_DEBOUNCE);
    // Without a content index every refresh is a full grep, so retries back off.
    for (let delay = INDEX_RETRY; ; delay = Math.min(delay * 2, INDEX_RETRY_MAX)) {
      if (disposed || generation !== contentGeneration) return;
      try {
        const found = await rpc.call<{ matches?: ContentMatch[]; indexing?: boolean }>(
          M.SEARCH_WORKSPACE_CONTENT,
          chatRequest(chatId, { query: text, limit: CONTENT_TAB_LIMIT, perFileLimit: CONTENT_MATCHES_PER_FILE }),
        );
        if (disposed || generation !== contentGeneration) return;
        setContent({ loading: false, indexing: !!found?.indexing, error: null, results: found?.matches ?? [] });
        if (!found?.indexing) return;
      } catch (error) {
        if (disposed || generation !== contentGeneration) return;
        setContent({ loading: false, indexing: false, error: searchError(error, "content"), results: [] });
        return;
      }
      await sleep(delay);
    }
  }

  onMount(() => {
    input.focus();
    searchWorkspace();
  });
  createEffect(on(query, () => {
    resetActive();
    searchWorkspace();
  }, { defer: true }));

  function selectTab(next: Tab) {
    if (next === tab()) return;
    setTab(next);
    resetActive();
  }
  // Mod+P while the palette is open on another tab switches it in place.
  createEffect(on(paletteTab, (next) => selectTab(next), { defer: true }));

  function activate(row: Row | undefined) {
    if (!row) return;
    if (row.kind === "command" && row.command.stayOpen) {
      row.command.run();
      return;
    }
    // Commands that open another overlay replace the palette rather than restore focus under it.
    closeOverlay(!(row.kind === "command" && (row.command.id === "import" || row.command.id === "shortcuts")));
    switch (row.kind) {
      case "command":
        row.command.run();
        break;
      case "chat":
        openChat(row.id);
        break;
      case "file":
        openWorkspacePath(row.path);
        break;
      case "content":
        // The Files panel has no line positioning yet; the file opens at its top.
        openWorkspacePath(row.match.path);
        break;
    }
  }

  const mod = (event: KeyboardEvent) => (/Mac|iPhone|iPad/.test(navigator.platform) ? event.metaKey : event.ctrlKey);

  function onKeyDown(event: KeyboardEvent) {
    const count = flat().length;
    if (mod(event) && /^[1-4]$/.test(event.key)) {
      selectTab(TABS[Number(event.key) - 1]!.id);
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      if (count) setActive(event.key === "ArrowDown" ? (active() + 1) % count : (active() + count - 1) % count, true);
    } else if (event.key === "Enter") {
      // Held keys repeat; one press opens one row.
      if (enterDown || event.repeat || event.isComposing) return event.preventDefault();
      enterDown = true;
      activate(activeRow());
    } else if (event.key === "Tab") {
      const index = TABS.findIndex((t) => t.id === tab());
      selectTab(TABS[(index + (event.shiftKey ? TABS.length - 1 : 1)) % TABS.length]!.id);
    } else if (event.key === "Escape") {
      closeOverlay();
    } else {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
  }

  const shortcutFor = (command: Command) => {
    if (!command.shortcutLabel) return null;
    const found = listShortcuts().find((s) => s.label === command.shortcutLabel);
    return found ? formatKeys(found.keys) : null;
  };

  return (
    <div class="x-scrim" onPointerDown={(e) => e.target === e.currentTarget && closeOverlay()}>
      <div
        class="x-palette"
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        onKeyDown={onKeyDown}
        onKeyUp={(e) => e.key === "Enter" && (enterDown = false)}
      >
        <div class="x-palette-head">
          <span class="x-palette-glyph"><Icon name="magnifer" size={16} /></span>
          <input
            ref={input}
            class="x-palette-input"
            type="text"
            role="combobox"
            aria-expanded="true"
            aria-controls="x-palette-list"
            aria-autocomplete="list"
            aria-activedescendant={activeRow() ? rowId(activeRow()!) : undefined}
            aria-label="Search threads, commands and files"
            placeholder="Search threads, commands and files…"
            spellcheck={false}
            autocomplete="off"
            value={query()}
            onInput={(e) => setQuery(e.currentTarget.value)}
          />
          <kbd class="x-kbd">{formatKeys("mod+k")}</kbd>
        </div>
        <div class="x-palette-panel">
          <div class="x-palette-tabs" role="tablist" aria-label="Result kinds">
            <For each={TABS}>
              {(item, index) => (
                <button
                  type="button"
                  class="x-palette-tab"
                  role="tab"
                  tabIndex={-1}
                  aria-selected={tab() === item.id}
                  aria-controls="x-palette-list"
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => {
                    selectTab(item.id);
                    input.focus();
                  }}
                >
                  {item.label}
                  <kbd class="x-kbd">{formatKeys(`mod+${index() + 1}`)}</kbd>
                </button>
              )}
            </For>
          </div>
          <div ref={list} id="x-palette-list" class="x-palette-list" role="listbox" aria-label="Results">
            <Show
              when={sections().length}
              fallback={<EmptyState title="No results" hint="Try a command, chat title, project, or device." />}
            >
              <Index each={sections()}>
                {(section, sectionIndex) => (
                  <div role="group" aria-label={SECTION_TITLES[section().id]} class="x-section">
                    <Show when={sectionIndex > 0}><div class="x-section-rule" role="presentation" /></Show>
                    <div class="x-section-head" role="presentation">
                      <span>{SECTION_TITLES[section().id]}</span>
                      <Show when={section().status}><span class="x-section-status">{section().status}</span></Show>
                    </div>
                    <For each={section().rows}>
                      {(row) => (
                        <div
                          id={rowId(row)}
                          class="x-row"
                          role="option"
                          aria-selected={activeRow() === row}
                          onMouseDown={(e) => e.preventDefault()}
                          onMouseMove={() => activeRow() !== row && setActive(flat().indexOf(row), false)}
                          onClick={() => activate(row)}
                        >
                          <PaletteRow row={row} query={trimmed()} shortcut={row.kind === "command" ? shortcutFor(row.command) : null} />
                        </div>
                      )}
                    </For>
                  </div>
                )}
              </Index>
            </Show>
          </div>
        </div>
        <div class="x-palette-foot" aria-hidden="true">
          <KeyHint keys={["arrow-up", "arrow-down"]} label="Navigate" />
          <KeyHint keys={["return"]} label="Open" />
          <KeyHint keys={["Esc"]} label="Close" />
        </div>
        <div class="x-sr-only" role="status" aria-live="polite">
          {flat().length ? `${flat().length} results` : "No results"}
        </div>
      </div>
    </div>
  );
}

function PaletteRow(props: { row: Row; query: string; shortcut: string | null }): JSX.Element {
  return (
    <Switch>
      <Match when={props.row.kind === "command" && props.row}>
        {(row) => (
          <>
            <Icon name={row().command.icon} size={16} class="x-row-icon" />
            <span class="x-row-label"><Highlight text={row().command.label} ranges={matchRanges(row().command.label, props.query)} /></span>
            <Show when={props.shortcut}><kbd class="x-kbd">{props.shortcut}</kbd></Show>
          </>
        )}
      </Match>
      <Match when={props.row.kind === "chat" && props.row}>{(row) => <ChatRow id={row().id} query={props.query} />}</Match>
      <Match when={props.row.kind === "file" && props.row}>
        {(row) => {
          const parts = () => splitPath(row().path);
          return (
            <>
              <FileIcon name={parts()[0]} directory={row().isDir} size={16} />
              <span class="x-row-name"><Highlight text={parts()[0]} ranges={matchRanges(parts()[0], props.query)} /></span>
              <Show when={parts()[1]}><span class="x-row-detail">{parts()[1]}</span></Show>
            </>
          );
        }}
      </Match>
      <Match when={props.row.kind === "content" && props.row}>
        {(row) => (
          <>
            <FileIcon name={splitPath(row().match.path)[0]} size={16} />
            <span class="x-row-path">{`${row().match.path}:${row().match.line}`}</span>
            <span class="x-row-code">
              <Highlight text={row().match.preview} ranges={byteRangesToChars(row().match.preview, row().match.ranges)} />
            </span>
          </>
        )}
      </Match>
    </Switch>
  );
}

function ChatRow(props: { id: string; query: string }) {
  const chat = () => state.chats[props.id];
  const indicator = () => effectiveIndicator(props.id);
  return (
    <Show when={chat()}>
      {(c) => (
        <>
          <Show
            when={indicator() === "working"}
            fallback={<span class="x-status" data-state={indicator()} role="img" aria-label={indicator() === "none" ? "Idle" : indicator() === "awaitingInput" ? "Waiting on your input" : "Run failed"} />}
          >
            <TerminalSpinner size={11} label="Working" class="x-status-working" />
          </Show>
          <span class="x-row-main">
            <span class="x-row-title"><Highlight text={chatTitle(c())} ranges={matchRanges(chatTitle(c()), props.query)} /></span>
            <span class="x-row-sub">
              {[chatLocation(c()), c().branch?.trim()].filter(Boolean).join(" · ")}
              <Show when={c().archived}><span class="x-badge">Archived</span></Show>
            </span>
          </span>
          <span class="x-row-meta">{timeAgo(activityAt(c()))}</span>
        </>
      )}
    </Show>
  );
}

function KeyHint(props: { keys: string[]; label: string }) {
  return (
    <span class="x-hint">
      <For each={props.keys}>
        {(key) => <span class="x-cap">{key.includes("-") || key === "return" ? <Icon name={key} size={12} /> : key}</span>}
      </For>
      {props.label}
    </span>
  );
}
