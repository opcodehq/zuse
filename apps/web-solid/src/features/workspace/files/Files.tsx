// The Files surface: a lazily listed, virtualized tree with Git decorations and search, beside a
// CodeMirror editor that saves against the read snapshot. Mirrors crates/ui/src/files.
import { createVirtualizer } from "@tanstack/solid-virtual";
import { batch, createEffect, createMemo, createSignal, For, Match, on, onCleanup, onMount, Show, Switch } from "solid-js";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import { narrowLayout, view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { imageMime, imageUrl, releaseImageUrl } from "@/ui/imageUrl";
import { Button, Dialog, IconButton, Spinner } from "@/ui/primitives";
import { Banner, Empty, FileIcon } from "../parts";
import { keyedWatch, liveSubscribe, readErrorText, type Target } from "../store";
import type { CheckoutGitStatus, GitFileState, WorkspaceEntry, WorkspaceFileChanges, WorkspaceFileSearchMatch, WorkspaceFileText } from "../types";
import { basename, copyText, decodeBase64Bytes, dirname } from "../util";
import { createEditor, type EditorHandle } from "./editor";
import {
  applyFileChanges,
  canSave,
  closeFile,
  files,
  loadDirectory,
  pending,
  readFile,
  refreshBaseline,
  reveal,
  saveFile,
  selectPath,
  setFileDraft,
  takePending,
  toggleDirectory,
  verifyOpenFile,
} from "./model";

const ROW = 27;
const INDENT = 14;

type Row =
  | { kind: "entry"; entry: WorkspaceEntry; depth: number }
  | { kind: "status"; directory: string; depth: number; text: string; tone?: "danger" }
  | { kind: "more"; directory: string; depth: number };

type Decoration = "added" | "modified" | "deleted" | "conflict";

function decoration(index: GitFileState, worktree: GitFileState): Decoration | null {
  const states = [worktree, index];
  if (states.includes("unmerged")) return "conflict";
  if (states.includes("deleted")) return "deleted";
  if (states.includes("untracked") || states.includes("added")) return "added";
  if (states.some((s) => s !== "unchanged")) return "modified";
  return null;
}

export function Files(props: { target: Target }) {
  // eslint-disable-next-line solid/reactivity -- RightPanel remounts this per chat
  const chatId = props.target.chatId;
  const tree = () => files[chatId];
  const [query, setQuery] = createSignal("");
  const [searchOpen, setSearchOpen] = createSignal(false);
  const searchShown = () => searchOpen() || query() !== "";
  let searchInput: HTMLInputElement | undefined;
  /** The magnifier opens the row and focuses the field; with an empty field it closes it again. */
  const toggleSearch = () => {
    if (searchShown() && !query()) {
      setSearchOpen(false);
      return;
    }
    setSearchOpen(true);
    queueMicrotask(() => searchInput?.focus());
  };
  const closeSearch = () => {
    setQuery("");
    setSearchOpen(false);
    queueMicrotask(() => document.querySelector<HTMLElement>(`.ws-surface[data-chat="${CSS.escape(chatId)}"] .ws-tree`)?.focus());
  };
  const [results, setResults] = createSignal<WorkspaceFileSearchMatch[] | null>(null);
  const [searchError, setSearchError] = createSignal<string | null>(null);
  const [git, setGit] = createSignal<CheckoutGitStatus | null>(null);
  const [dirty, setDirty] = createSignal(files[chatId]?.open?.draftText != null);
  let editor: EditorHandle | undefined;
  let lastBaseline: string | null = null;

  const connected = () => state.connection.kind === "connected";
  const isDirty = () => dirty();

  // Root listing, and a fresh listing after every reconnect.
  keyedWatch(
    () => [state.epoch, connected()] as const,
    ([, live]) => {
      if (!live) return;
      const listed = Object.keys(files[chatId]?.dirs ?? {});
      if (!listed.length) loadDirectory(chatId, "");
      else for (const dir of listed) loadDirectory(chatId, dir);
      const open = files[chatId]?.open;
      if (open?.file) verifyOpenFile(chatId, isDirty);
      else if (open && !open.file) readFile(chatId, open.path);
    },
  );

  keyedWatch(
    () => [state.epoch, connected()] as const,
    ([, live]) =>
      live ? liveSubscribe<WorkspaceFileChanges>(M.WATCH_WORKSPACE_FILES, { chatId, targetDeviceId: props.target.deviceId }, (frame) => applyFileChanges(chatId, frame, isDirty)) : undefined,
  );

  keyedWatch(
    () => [state.epoch, connected()] as const,
    ([, live]) =>
      live
        ? liveSubscribe<{ status: CheckoutGitStatus | null }>(M.WATCH_WORKSPACE_GIT_STATUS, { chatId, targetDeviceId: props.target.deviceId }, (frame) => setGit(frame?.status ?? null), () => setGit(null))
        : undefined,
  );

  const decorations = createMemo(() => {
    const map = new Map<string, Decoration>();
    const dirs = new Set<string>();
    for (const file of git()?.files ?? []) {
      const kind = decoration(file.index, file.worktree);
      if (!kind) continue;
      map.set(file.path, kind);
      let dir = dirname(file.path);
      while (dir) {
        dirs.add(dir);
        dir = dirname(dir);
      }
    }
    return { map, dirs };
  });

  // Search on the host's index, debounced.
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let searchSeq = 0;
  createEffect(
    on(query, (q) => {
      clearTimeout(searchTimer);
      if (!q.trim()) {
        setResults(null);
        setSearchError(null);
        return;
      }
      searchTimer = setTimeout(() => {
        const seq = ++searchSeq;
        rpc.call<WorkspaceFileSearchMatch[] | { matches: WorkspaceFileSearchMatch[] }>(M.SEARCH_WORKSPACE_FILES, { chatId, targetDeviceId: props.target.deviceId, query: q.trim(), limit: 200 }).then(
          (value) => {
            if (seq !== searchSeq) return;
            setResults(Array.isArray(value) ? value : value?.matches ?? []);
            setSearchError(null);
          },
          (e) => seq === searchSeq && setSearchError(readErrorText(e, "Search failed.")),
        );
      }, 150);
    }, { defer: true }),
  );
  onCleanup(() => clearTimeout(searchTimer));

  const rows = createMemo<Row[]>(() => {
    const t = tree();
    if (!t) return [];
    const out: Row[] = [];
    const walk = (directory: string, depth: number) => {
      const dir = t.dirs[directory];
      if (!dir) return;
      for (const entry of dir.entries) {
        out.push({ kind: "entry", entry, depth });
        if (entry.kind === "directory" && t.expanded[entry.path]) {
          const child = t.dirs[entry.path];
          if (!child || (child.loading && !child.entries.length)) out.push({ kind: "status", directory: entry.path, depth: depth + 1, text: "Loading…" });
          else if (child.error) out.push({ kind: "status", directory: entry.path, depth: depth + 1, text: child.error, tone: "danger" });
          else if (!child.entries.length) out.push({ kind: "status", directory: entry.path, depth: depth + 1, text: "Empty folder" });
          walk(entry.path, depth + 1);
        }
      }
      if (dir.nextCursor) out.push({ kind: "more", directory, depth });
    };
    walk("", 0);
    return out;
  });

  const openEntry = (entry: WorkspaceEntry) => {
    if (entry.kind === "directory") toggleDirectory(chatId, entry.path);
    else openPath(entry.path);
  };

  const [discardAsk, setDiscardAsk] = createSignal<(() => void) | null>(null);
  /** Run `action` now, or after the user agrees to drop unsaved edits. */
  const guard = (action: () => void) => (dirty() ? setDiscardAsk(() => action) : action());

  const openPath = (path: string) => {
    if (tree()?.open?.path === path) return;
    guard(() => {
      setDirty(false);
      readFile(chatId, path);
    });
  };
  const close = () =>
    guard(() =>
      batch(() => {
        setDirty(false);
        closeFile(chatId);
      }),
    );

  // Other surfaces (Changes, History) ask for a file through the pending slot.
  createEffect(
    on(
      () => pending.seq,
      () => {
        const path = takePending();
        if (!path) return;
        reveal(chatId, path);
        openPath(path);
      },
    ),
  );

  const open = () => tree()?.open ?? null;
  const editable = () => canSave(open()) || !!open()?.saving || !!open()?.verifying;

  /** Compare the editor with the snapshot and keep any unsaved text in the store. */
  const syncDirty = () => {
    const current = open();
    if (!editor || !current) return;
    const text = editor.text();
    const changed = typeof current.file?.text === "string" && text !== current.file.text;
    setDirty(changed);
    setFileDraft(chatId, current.path, changed ? text : null);
  };

  // Load read snapshots into the editor; a baseline change only replaces clean text.
  createEffect(
    on(
      () => [open()?.path, open()?.file?.text, open()?.file?.contentHash] as const,
      ([path, text]) => {
        if (!editor || !path || typeof text !== "string") return;
        const current = editor.text();
        if (lastBaseline === null || current === lastBaseline || !dirty()) {
          if (current !== text) editor.setText(text);
          editor.setLanguage(path);
        }
        lastBaseline = text;
        syncDirty();
      },
    ),
  );
  const [editorReady, setEditorReady] = createSignal(0);
  createEffect(() => {
    const readOnly = !editable() || !!open()?.conflict;
    if (editorReady()) editor?.setReadOnly(readOnly);
  });

  const save = () => {
    if (!editor || !dirty() || !canSave(open())) return;
    void saveFile(chatId, editor.text());
  };
  const reload = () => {
    const path = open()?.path;
    if (!path) return;
    setDirty(false);
    lastBaseline = null;
    readFile(chatId, path);
  };
  const overwrite = () => {
    if (!editor) return;
    const text = editor.text();
    void refreshBaseline(chatId).then((fresh) => fresh && saveFile(chatId, text, true));
  };

  // Desktop opens files as their own tabs over the explorer, never beside it. Show the tree and the editor side by side
  // only when the panel is wide enough for both (an evenly split 1080p window); otherwise the editor replaces the tree.
  const narrow = () => narrowLayout() || view.preferences.rightPanelWidth < 760;

  return (
    <div class="ws-files" data-narrow={narrow() ? "true" : undefined} data-open={open() ? "true" : undefined}>
      <div class="ws-files-tree">
        {/* Desktop's explorer header (crates/ui/src/files/mod.rs render_explorer_header): a quiet label and 24px
            buttons that flow straight into the tree, no rules. Search opens a row beneath it. */}
        <div class="ws-files-head">
          <div class="ws-files-bar">
            <span class="ws-files-label ws-truncate">{tree()?.dirs[""]?.nextCursor ? "Files (partial)" : "Files"}</span>
            <IconButton
              icon="refresh"
              label="Refresh files"
              size="sm"
              iconSize={15}
              onClick={() => {
                for (const dir of Object.keys(tree()?.dirs ?? { "": null })) loadDirectory(chatId, dir);
              }}
            />
            <IconButton icon="magnifer" label="Search files" size="sm" iconSize={15} active={searchShown()} aria-expanded={searchShown()} onClick={toggleSearch} />
          </div>
          <Show when={searchShown()}>
            <div class="ws-files-search">
              <label class="ws-search">
                <Icon name="magnifer" size={12} class="ws-faint" />
                <input
                  ref={searchInput}
                  class="ws-search-input"
                  type="search"
                  placeholder="Search files"
                  aria-label="Search files"
                  value={query()}
                  onInput={(e) => setQuery(e.currentTarget.value)}
                  onKeyDown={(e) => {
                    if (e.key !== "Escape") return;
                    e.preventDefault();
                    closeSearch();
                  }}
                />
              </label>
            </div>
          </Show>
        </div>
        <Show
          when={results() === null}
          fallback={<SearchResults results={results() ?? []} error={searchError()} onOpen={(m) => (m.kind === "directory" ? (setQuery(""), reveal(chatId, m.path), toggleDirectory(chatId, m.path, true)) : openPath(m.path))} />}
        >
          <Show when={tree()?.dirs[""]?.error && !tree()?.dirs[""]?.entries.length}>
            <Banner tone="danger" actions={<Button size="sm" variant="ghost" onClick={() => loadDirectory(chatId, "")}>Retry</Button>}>{tree()?.dirs[""]?.error}</Banner>
          </Show>
          <Show when={tree()?.dirs[""]} fallback={<div class="ws-empty"><Spinner label="Loading files" /></div>}>
            <Tree
              rows={rows()}
              selected={tree()?.selected ?? null}
              expanded={tree()?.expanded ?? {}}
              decorations={decorations()}
              onActivate={openEntry}
              onSelect={(path) => selectPath(chatId, path)}
              onMore={(dir) => loadDirectory(chatId, dir, true)}
              onCollapse={(path) => toggleDirectory(chatId, path, false)}
              onExpand={(path) => toggleDirectory(chatId, path, true)}
            />
          </Show>
        </Show>
      </div>
      <Show when={open()}>
        {(file) => (
          <div class="ws-editor-pane">
            <div class="ws-panel-header ws-editor-head">
              <Show when={narrow()}>
                <IconButton icon="alt-arrow-left" label="Back to files" size="sm" iconSize={14} onClick={close} />
              </Show>
              <FileIcon name={basename(file().path)} />
              <span class="ws-editor-name" title={file().path}>
                {basename(file().path)}
                <Show when={dirty()}>
                  <span class="ws-dirty" role="img" aria-label="Unsaved changes" />
                </Show>
              </span>
              <span class="ws-truncate ws-faint">{dirname(file().path)}</span>
              <div class="ws-grow" />
              <Show when={file().saving || file().verifying || file().loading}>
                <Spinner label={file().saving ? "Saving" : file().verifying ? "Verifying" : "Loading"} />
              </Show>
              <IconButton icon="copy" label="Copy path" size="sm" iconSize={14} onClick={() => copyText(file().path, "Path copied")} />
              <Button size="sm" variant={dirty() && !file().conflict ? "primary" : "ghost"} icon="floppy-disk" disabled={!dirty() || !canSave(file()) || !!file().conflict} onClick={save} title="Save (Ctrl+S)">
                {file().saving ? "Saving…" : "Save"}
              </Button>
              <Show when={!narrow()}>
                <IconButton
                  icon="close"
                  label="Close file"
                  size="sm"
                  iconSize={14}
                  onClick={close}
                />
              </Show>
            </div>
            <Show when={file().conflict}>
              {(conflict) => (
                <Banner
                  tone="warning"
                  actions={
                    <Show
                      when={conflict().kind !== "deleted" && conflict().kind !== "notRegularFile"}
                      fallback={
                        <>
                          <Button size="sm" variant="ghost" onClick={() => void copyText(editor?.text() ?? "", "Edits copied")}>Copy edits</Button>
                          <Button size="sm" variant="outline" onClick={close}>Close</Button>
                        </>
                      }
                    >
                      <Button size="sm" variant="ghost" onClick={reload}>Reload</Button>
                      <Button size="sm" variant="outline" onClick={overwrite}>Overwrite</Button>
                    </Show>
                  }
                >
                  {conflict().message}{" "}
                  {conflict().kind === "deleted" || conflict().kind === "notRegularFile"
                    ? "It can't be saved here; copy your edits before closing it."
                    : "Reload discards your edits; Overwrite saves yours over it."}
                </Banner>
              )}
            </Show>
            <Show when={file().error}>
              <Banner tone="danger">{file().error}</Banner>
            </Show>
            <Show when={!connected() && dirty()}>
              <Banner tone="warning">Disconnected. Your edits are kept and will be checked against the file when the connection returns.</Banner>
            </Show>
            <Show when={(file().file?.readOnlyReason || file().file?.truncated) && !(file().file?.readOnlyReason === "binary" && IMAGE.test(file().path))}>
              <Banner>{readOnlyText(file().file?.readOnlyReason ?? (file().file?.truncated ? "tooLarge" : null))}</Banner>
            </Show>
            <Show when={file().file && typeof file().file?.text !== "string" ? file().file : null}>
              {(binary) => (
                <Show
                  when={IMAGE.test(binary().path)}
                  fallback={<Empty icon="document" title="No preview" detail={readOnlyText(binary().readOnlyReason ?? null)} />}
                >
                  <ImagePreview target={props.target} file={binary()} />
                </Show>
              )}
            </Show>
            <div
              class="ws-editor"
              hidden={!!file().file && typeof file().file?.text !== "string"}
              ref={(el) =>
                onMount(() => {
                  editor = createEditor(el, { label: `Editor, ${basename(file().path)}`, onChange: syncDirty, onSave: save });
                  lastBaseline = null;
                  const text = open()?.file?.text;
                  if (typeof text === "string") {
                    // Edits made before the editor last unmounted come back on top of the same snapshot.
                    const draft = open()?.draftText;
                    editor.setText(typeof draft === "string" ? draft : text);
                    editor.setLanguage(file().path);
                    lastBaseline = text;
                    syncDirty();
                  }
                  editor.setReadOnly(!editable());
                  setEditorReady((n) => n + 1);
                  onCleanup(() => {
                    editor?.destroy();
                    editor = undefined;
                    setEditorReady(0);
                  });
                })
              }
            />
          </div>
        )}
      </Show>
      <Dialog
        open={!!discardAsk()}
        onOpenChange={(next) => !next && setDiscardAsk(null)}
        title="Discard unsaved changes?"
        description={`Your edits to ${basename(open()?.path ?? "this file")} haven't been saved.`}
        footer={
          <>
            <Button variant="ghost" onClick={() => setDiscardAsk(null)}>Keep editing</Button>
            <Button
              variant="danger"
              onClick={() => {
                const action = discardAsk();
                setDiscardAsk(null);
                action?.();
              }}
            >
              Discard
            </Button>
          </>
        }
      />
    </div>
  );
}

const IMAGE = /\.(png|jpe?g|gif|webp|avif|bmp|ico|svg)$/i;
const IMAGE_LIMIT = 25 * 1024 * 1024;

/** Binary images stream in chunks from the host; `imageUrl` keeps active content (SVG) off this origin. */
function ImagePreview(props: { target: Target; file: WorkspaceFileText }) {
  const [url, setUrl] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  let cancelled = false;
  onCleanup(() => {
    cancelled = true;
    releaseImageUrl(url());
  });
  createEffect(
    on(
      () => [props.file.path, props.file.contentHash] as const,
      async ([path, hash]) => {
        setError(null);
        const parts: Uint8Array[] = [];
        let offset = 0;
        let mime = "";
        try {
          for (;;) {
            const chunk = await rpc.call<{ mimeType: string; data: string; nextOffset: number; done: boolean; size: number }>(M.READ_WORKSPACE_IMAGE, {
              chatId: props.target.chatId,
              targetDeviceId: props.target.deviceId,
              path,
              expectedCheckoutId: props.file.checkoutId,
              expectedContentHash: hash ?? null,
              offset,
            });
            if (cancelled) return;
            if (chunk.size > IMAGE_LIMIT) throw new Error("This image is too large to preview here.");
            mime = chunk.mimeType || mime;
            parts.push(decodeBase64Bytes(chunk.data));
            if (chunk.done || chunk.nextOffset <= offset) break;
            offset = chunk.nextOffset;
          }
          const next = await imageUrl(parts as BlobPart[], imageMime(path, mime));
          if (cancelled) return releaseImageUrl(next);
          const previous = url();
          setUrl(next);
          releaseImageUrl(previous);
        } catch (e) {
          if (!cancelled) setError(readErrorText(e, "Couldn't load this image."));
        }
      },
    ),
  );
  return (
    <div class="ws-image-preview">
      <Show when={url()} fallback={<Show when={error()} fallback={<Spinner label="Loading image" />}><Empty icon="document" title="No preview" detail={error() ?? undefined} /></Show>}>
        {(src) => <img src={src()} alt={basename(props.file.path)} />}
      </Show>
    </div>
  );
}

function readOnlyText(reason: string | null) {
  switch (reason) {
    case "archived":
      return "Saved checkpoint content. Restore the thread before editing.";
    case "binary":
      return "Binary file. Its contents can't be shown as text.";
    case "unsupportedEncoding":
      return "This file isn't UTF-8 text, so it opens read-only.";
    case "mixedLineEndings":
      return "This file mixes line endings, so it opens read-only.";
    case "symlink":
      return "Symbolic links open read-only.";
    case "tooLarge":
      return "This file is too large to edit here, so it opens read-only.";
    case "permissionDenied":
      return "You don't have permission to write this file.";
    case "notRegularFile":
      return "This path isn't a regular file.";
    case "outsideWorkspace":
      return "This file is outside the workspace, so it opens read-only.";
    default:
      return "This file opens read-only.";
  }
}

function Tree(props: {
  rows: Row[];
  selected: string | null;
  expanded: Record<string, boolean>;
  decorations: { map: Map<string, Decoration>; dirs: Set<string> };
  onActivate: (entry: WorkspaceEntry) => void;
  onSelect: (path: string) => void;
  onMore: (directory: string) => void;
  onCollapse: (path: string) => void;
  onExpand: (path: string) => void;
}) {
  let scroller!: HTMLDivElement;
  const virtualizer = createVirtualizer({
    get count() {
      return props.rows.length;
    },
    getScrollElement: () => scroller,
    estimateSize: () => ROW,
    overscan: 16,
  });
  const entryIndex = (path: string | null) => props.rows.findIndex((r) => r.kind === "entry" && r.entry.path === path);

  const focusRow = (index: number) => {
    const row = props.rows[index];
    if (!row || row.kind !== "entry") return;
    props.onSelect(row.entry.path);
    virtualizer.scrollToIndex(index, { align: "auto" });
    queueMicrotask(() => scroller.querySelector<HTMLElement>(`[data-path="${CSS.escape(row.entry.path)}"]`)?.focus());
  };

  const onKeyDown = (event: KeyboardEvent) => {
    const index = entryIndex(props.selected);
    const row = props.rows[index];
    const entries = props.rows.map((r, i) => (r.kind === "entry" ? i : -1)).filter((i) => i >= 0);
    const at = entries.indexOf(index);
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        focusRow(entries[Math.min(entries.length - 1, at + 1)] ?? entries[0] ?? -1);
        break;
      case "ArrowUp":
        event.preventDefault();
        focusRow(entries[Math.max(0, at - 1)] ?? entries[0] ?? -1);
        break;
      case "Home":
        event.preventDefault();
        focusRow(entries[0] ?? -1);
        break;
      case "End":
        event.preventDefault();
        focusRow(entries[entries.length - 1] ?? -1);
        break;
      case "ArrowRight":
        if (row?.kind === "entry" && row.entry.kind === "directory") {
          event.preventDefault();
          if (!props.expanded[row.entry.path]) props.onExpand(row.entry.path);
          else focusRow(index + 1);
        }
        break;
      case "ArrowLeft":
        if (row?.kind === "entry") {
          event.preventDefault();
          if (row.entry.kind === "directory" && props.expanded[row.entry.path]) props.onCollapse(row.entry.path);
          else {
            const parent = dirname(row.entry.path);
            if (parent) focusRow(entryIndex(parent));
          }
        }
        break;
      case "Enter":
      case " ":
        if (row?.kind === "entry") {
          event.preventDefault();
          props.onActivate(row.entry);
        }
        break;
    }
  };

  return (
    <div class="ws-tree" ref={scroller} role="tree" aria-label="Files" onKeyDown={onKeyDown}>
      <div class="ws-tree-sizer" style={{ height: `${virtualizer.getTotalSize()}px` }}>
        <For each={virtualizer.getVirtualItems()}>
          {(item) => {
            const row = () => props.rows[item.index];
            return (
              <div class="ws-tree-slot" style={{ transform: `translateY(${item.start}px)`, height: `${ROW}px` }}>
                {/* Virtual items keep identity while the rows under them shift, so every field reads the row live. */}
                <Switch>
                  <Match when={row()?.kind === "more" && (row() as Extract<Row, { kind: "more" }>)}>
                    {(value) => (
                      <button type="button" class="ws-tree-row ws-muted" style={{ "padding-left": `${8 + value().depth * INDENT + 20}px` }} onClick={() => props.onMore(value().directory)}>
                        Load more…
                      </button>
                    )}
                  </Match>
                  <Match when={row()?.kind === "status" && (row() as Extract<Row, { kind: "status" }>)}>
                    {(value) => (
                      <div class="ws-tree-row ws-faint" data-tone={value().tone} style={{ "padding-left": `${8 + value().depth * INDENT + 20}px` }}>
                        {value().text}
                      </div>
                    )}
                  </Match>
                  <Match when={row()?.kind === "entry" && (row() as Extract<Row, { kind: "entry" }>)}>
                    {(value) => {
                      const entry = () => value().entry;
                      const directory = () => entry().kind === "directory";
                      const deco = () => (directory() ? (props.decorations.dirs.has(entry().path) ? "dir" : null) : props.decorations.map.get(entry().path) ?? null);
                      return (
                        <div
                          class="ws-tree-row"
                          role="treeitem"
                          aria-level={value().depth + 1}
                          aria-selected={props.selected === entry().path}
                          aria-expanded={directory() ? !!props.expanded[entry().path] : undefined}
                          tabIndex={props.selected === entry().path || (!props.selected && item.index === 0) ? 0 : -1}
                          data-path={entry().path}
                          data-ignored={entry().ignored ? "true" : undefined}
                          data-deco={deco() ?? undefined}
                          style={{ "padding-left": `${8 + value().depth * INDENT}px` }}
                          onClick={() => {
                            props.onSelect(entry().path);
                            props.onActivate(entry());
                          }}
                        >
                          <span class="ws-tree-caret">
                            <Show when={directory()}>
                              <Icon name={props.expanded[entry().path] ? "alt-arrow-down" : "alt-arrow-right"} size={11} />
                            </Show>
                          </span>
                          <FileIcon name={entry().name} directory={directory()} open={!!props.expanded[entry().path]} />
                          <span class="ws-truncate ws-tree-name">{entry().name}</span>
                          <Show when={deco() && deco() !== "dir"}>
                            <span class="ws-tree-badge">{deco() === "added" ? "A" : deco() === "deleted" ? "D" : deco() === "conflict" ? "!" : "M"}</span>
                          </Show>
                          <Show when={deco() === "dir"}>
                            <span class="ws-tree-dot" aria-hidden="true" />
                          </Show>
                        </div>
                      );
                    }}
                  </Match>
                </Switch>
              </div>
            );
          }}
        </For>
      </div>
      <Show when={!props.rows.length}>
        <div class="ws-empty ws-muted">This folder is empty.</div>
      </Show>
    </div>
  );
}

function SearchResults(props: { results: WorkspaceFileSearchMatch[]; error: string | null; onOpen: (match: WorkspaceFileSearchMatch) => void }) {
  return (
    <div class="ws-search-results" role="listbox" aria-label="Search results">
      <Show when={props.error}>
        <Banner tone="danger">{props.error}</Banner>
      </Show>
      <For each={props.results} fallback={<Show when={!props.error}><div class="ws-empty ws-muted">No matching files</div></Show>}>
        {(match) => (
          <button type="button" role="option" class="ws-tree-row" onClick={() => props.onOpen(match)}>
            <FileIcon name={match.name} directory={match.kind === "directory"} />
            <span class="ws-tree-name">{match.name}</span>
            <span class="ws-truncate ws-faint">{dirname(match.path)}</span>
          </button>
        )}
      </For>
    </div>
  );
}
