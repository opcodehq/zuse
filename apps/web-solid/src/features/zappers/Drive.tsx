import { Markdown } from "@/features/transcript/Markdown";
import { FileIcon } from "@/features/workspace/parts";
import { Icon } from "@/ui/Icon";
import { createEffect, createMemo, createSignal, createStore, For, Show } from "solid-js";
import type { Zapper, ZapperAction, ZapperFile } from "@/core/zapper-types";
import { Button, Dialog, IconButton, Menu } from "@/ui/primitives";
import { closePage, panelFor, pageKey, showPage } from "./panel";
import { ancestors, buildTree, filesUnder, foldersUnder, isReserved, latestFiles, parentOf, pathProblem, RESERVED, visibleRows, type TreeNode, type TreeRow } from "./files-model";
import { command, commandState, loadResources, resources } from "./state";

// Per Zapper: which folders are open and which row is selected, kept while the inspector changes pages.
const [trees, setTrees] = createStore<Record<string, { expanded: string[]; selected: string | null; seeded: boolean }>>({});
const treeOf = (id: string) => trees[id] ?? { expanded: [], selected: null, seeded: false };
function updateTree(id: string, change: (tree: { expanded: string[]; selected: string | null; seeded: boolean }) => void) {
  setTrees((draft) => {
    if (!draft[id]) draft[id] = { expanded: [], selected: null, seeded: false };
    change(draft[id]!);
  });
}
const reveal = (id: string, path: string) =>
  updateTree(id, (tree) => {
    tree.selected = path;
    tree.expanded = [...new Set([...tree.expanded, ...ancestors(path)])];
  });

type Draft = { path: string; content: string; version: number; latest?: ZapperFile; error?: string };
type Pending =
  | { kind: "edit"; draft: Draft }
  | { kind: "folder"; path: string; error?: string }
  | { kind: "move"; node: TreeNode; to: string; error?: string }
  | { kind: "delete"; node: TreeNode; error?: string };

const [pending, setPending] = createSignal<Pending | null>(null);

async function run(zapper: Zapper, action: ZapperAction) {
  await command(zapper.id, action);
  await loadResources(zapper);
}
const failure = (error: unknown) => (error instanceof Error ? error.message : String(error));

export function Drive(props: { zapper: Zapper; history: boolean; path?: string; onOpen?: (path: string) => void }) {
  return (
    <>
      <Show when={props.history}>
        <History zapper={props.zapper} />
      </Show>
      <Show when={!props.history && props.path !== undefined}>
        <FilePage zapper={props.zapper} path={props.path ?? ""} />
      </Show>
      <Show when={!props.history && props.path === undefined}>
        <FileTree zapper={props.zapper} onOpen={props.onOpen} />
      </Show>
    </>
  );
}

function FileTree(props: { zapper: Zapper; onOpen?: (path: string) => void }) {
  const folders = () => props.zapper.folders ?? [];
  const nodes = createMemo(() => buildTree(resources.files, folders()));
  const tree = () => treeOf(props.zapper.id);
  const expanded = createMemo(() => new Set(tree().expanded));
  const rows = createMemo(() => visibleRows(nodes(), expanded()));
  const rowAt = createMemo(() => new Map(rows().map((r) => [r.node.path, r])));
  const extra = () => latestFiles(resources.files).filter((f) => !isReserved(f.path)).length + folders().length;
  // Open the top folders that hold something the first time the files arrive.
  createEffect(
    () => !tree().seeded && !resources.loading && resources.files.length > 0 && resources.id === props.zapper.id,
    (ready) => {
      if (!ready) return;
      const open = nodes()
        .filter((n) => n.kind === "folder" && n.children.length)
        .map((n) => n.path);
      updateTree(props.zapper.id, (draft) => {
        draft.seeded = true;
        draft.expanded = [...new Set([...draft.expanded, ...open])];
      });
    },
  );
  const toggle = (path: string) =>
    updateTree(props.zapper.id, (draft) => {
      draft.expanded = draft.expanded.includes(path) ? draft.expanded.filter((p) => p !== path) : [...draft.expanded, path];
    });
  const activate = (row: TreeRow) => {
    updateTree(props.zapper.id, (draft) => {
      draft.selected = row.node.path;
    });
    if (row.node.kind === "folder") toggle(row.node.path);
    else props.onOpen?.(row.node.path);
  };
  const focusRow = (index: number) => {
    const row = rows()[index];
    if (!row) return;
    updateTree(props.zapper.id, (draft) => {
      draft.selected = row.node.path;
    });
    queueMicrotask(() => document.querySelector<HTMLElement>('.zp-tree [data-path="' + CSS.escape(row.node.path) + '"]')?.focus());
  };
  const onKeyDown = (event: KeyboardEvent) => {
    const index = rows().findIndex((row) => row.node.path === tree().selected);
    const row = rows()[index];
    const keys: Record<string, () => void> = {
      ArrowDown: () => focusRow(Math.min(rows().length - 1, index + 1)),
      ArrowUp: () => focusRow(Math.max(0, index - 1)),
      Home: () => focusRow(0),
      End: () => focusRow(rows().length - 1),
      ArrowRight: () => row?.node.kind === "folder" && (row.expanded ? focusRow(index + 1) : toggle(row.node.path)),
      ArrowLeft: () => {
        if (row?.node.kind === "folder" && row.expanded) toggle(row.node.path);
        else if (row && parentOf(row.node.path)) focusRow(rows().findIndex((r) => r.node.path === parentOf(row.node.path)));
      },
      Enter: () => row && activate(row),
    };
    if (!keys[event.key] || (event.target as Element).closest(".zp-tree-more")) return;
    event.preventDefault();
    keys[event.key]!();
  };
  const selectedFolder = () => {
    const path = tree().selected;
    if (!path) return "";
    return nodes().length && rows().find((r) => r.node.path === path)?.node.kind === "folder" ? path : parentOf(path);
  };
  return (
    <div class="zp-files">
      <div class="zp-section-heading">
        <h3>Files</h3>
        <div>
          <IconButton size="sm" icon="document-add" label="New file" title="New file" onClick={() => newFile(selectedFolder())} />
          <IconButton size="sm" icon="folder-add" label="New folder" title="New folder" onClick={() => setPending({ kind: "folder", path: prefix(selectedFolder()) })} />
          <IconButton size="sm" icon="refresh" label="Refresh files" title="Refresh" busy={resources.loading} onClick={() => void loadResources(props.zapper)} />
        </div>
      </div>
      <Show when={resources.error}>
        <div class="zp-error zp-files-error" role="alert">
          <span>{resources.error}</span>
          <Button size="sm" onClick={() => void loadResources(props.zapper)}>Retry</Button>
        </div>
      </Show>
      <Show
        when={resources.files.length || !resources.loading}
        fallback={<p class="zp-muted">Loading files…</p>}
      >
        <div class="zp-tree" role="tree" aria-label="Zapper files" onKeyDown={onKeyDown}>
          <For each={rows().map((r) => r.node.path)}>
            {(path, index) => {
              let last = rowAt().get(path)!;
              const row = () => (last = rowAt().get(path) ?? last);
              const node = () => row().node;
              const folder = () => node().kind === "folder";
              const file = () => (node().kind === "file" ? (node() as Extract<TreeNode, { kind: "file" }>).file : null);
              const selected = () => tree().selected === node().path;
              return (
                <div
                  class="zp-tree-row"
                  role="treeitem"
                  aria-level={row().depth + 1}
                  aria-selected={selected() ? "true" : "false"}
                  aria-expanded={folder() ? (row().expanded ? "true" : "false") : undefined}
                  tabindex={selected() || (!tree().selected && index() === 0) ? 0 : -1}
                  data-path={node().path}
                  data-missing={!folder() && !file() ? "true" : undefined}
                  style={{ "padding-left": 8 + row().depth * 16 + "px" }}
                  onClick={(event) => {
                    if (!(event.target as Element).closest(".zp-tree-more")) activate(row());
                  }}
                >
                  <For each={Array.from({ length: row().depth })}>
                    {(_, level) => <span class="zp-tree-guide" style={{ left: 14 + level() * 16 + "px" }} aria-hidden="true" />}
                  </For>
                  <span class="zp-tree-caret" aria-hidden="true">
                    <Show when={folder()}>
                      <Icon name={row().expanded ? "alt-arrow-down" : "alt-arrow-right"} size={11} />
                    </Show>
                  </span>
                  <FileIcon name={node().name} directory={folder()} open={row().expanded} />
                  <span class="zp-tree-name">{node().name}</span>
                  <Show when={!folder() && !file()}>
                    <span class="zp-tree-hint">not written</span>
                  </Show>
                  <span class="zp-tree-more">
                    <Menu
                      placement="bottom-end"
                      trigger={(attrs) => (
                        <button {...attrs} type="button" class="zp-tree-more-button" aria-label={"Actions for " + node().path} tabindex={-1}>
                          <Icon name="more-horizontal" size={13} />
                        </button>
                      )}
                      items={rowActions(props.zapper, node(), props.onOpen)}
                    />
                  </span>
                </div>
              );
            }}
          </For>
        </div>
        <Show when={!extra()}>
          <p class="zp-muted zp-files-empty">
            Add folders and documents for the coordinator and its threads to open when they need them. Files you add stay out of their prompts until opened.
          </p>
        </Show>
      </Show>
    </div>
  );
}

const prefix = (folder: string) => (folder ? folder + "/" : "");
const newFile = (folder: string) => setPending({ kind: "edit", draft: { path: prefix(folder), content: "", version: 0 } });

function rowActions(zapper: Zapper, node: TreeNode, onOpen?: (path: string) => void) {
  const reserved = isReserved(node.path);
  if (node.kind === "folder")
    return [
      { label: "New file here", icon: "document-add", onSelect: () => newFile(node.path) },
      { label: "New folder here", icon: "folder-add", onSelect: () => setPending({ kind: "folder", path: prefix(node.path) }) },
      { label: "Rename or move…", icon: "pen", onSelect: () => setPending({ kind: "move", node, to: node.path }) },
      { label: "Delete folder…", icon: "trash-bin-minimalistic", danger: true, onSelect: () => setPending({ kind: "delete", node }) },
    ];
  return [
    { label: "Open", icon: "document", onSelect: () => onOpen?.(node.path) },
    { label: "Edit", icon: "pen", onSelect: () => editFile(zapper, node.path) },
    { label: reserved ? "Rename or move (fixed name)" : "Rename or move…", disabled: reserved, onSelect: () => setPending({ kind: "move", node, to: node.path }) },
    { label: reserved ? "Delete (always kept)" : "Delete…", danger: !reserved, disabled: reserved, onSelect: () => setPending({ kind: "delete", node }) },
  ];
}

function editFile(_zapper: Zapper, path: string) {
  const file = latestFiles(resources.files).find((f) => f.path === path);
  setPending({ kind: "edit", draft: { path, content: file?.content ?? "", version: file?.version ?? 0 } });
}

function FilePage(props: { zapper: Zapper; path: string }) {
  const file = () => latestFiles(resources.files).find((f) => f.path === props.path);
  const deleted = () => !file() && resources.files.some((f) => f.path === props.path);
  const node = (): TreeNode => ({ kind: "file", name: props.path.slice(props.path.lastIndexOf("/") + 1), path: props.path, file: file() ?? null });
  const reserved = () => isReserved(props.path);
  return (
    <div class="zp-file-page">
      <div class="zp-file-header">
        <FileIcon name={props.path} />
        <strong class="zp-file-title" title={props.path}>{props.path}</strong>
        <Show when={file()}>{(f) => <small class="zp-muted">v{f().version}</small>}</Show>
        <Show when={file() || reserved()}>
          <Button size="sm" onClick={() => editFile(props.zapper, props.path)}>{file() ? "Edit" : "Write"}</Button>
        </Show>
        <Show when={file() && !reserved()}>
          <Menu
            placement="bottom-end"
            trigger={(attrs) => (
              <IconButton {...attrs} size="sm" icon="more-horizontal" label={"Actions for " + props.path} />
            )}
            items={[
              { label: "Rename or move…", icon: "pen", onSelect: () => setPending({ kind: "move", node: node(), to: props.path }) },
              { label: "Version history", icon: "history", onSelect: () => showPage(props.zapper.id, { kind: "history" }) },
              { label: "Delete…", icon: "trash-bin-minimalistic", danger: true, onSelect: () => setPending({ kind: "delete", node: node() }) },
            ]}
          />
        </Show>
      </div>
      <Show when={RESERVED[props.path]}>{(about) => <p class="zp-muted zp-file-about">{about()}</p>}</Show>
      <Show
        when={file()}
        fallback={
          <p class="zp-muted">
            {resources.loading
              ? "Loading file…"
              : reserved()
                ? "Not written yet."
                : deleted()
                  ? "This file was deleted. Restore an earlier version from Version history."
                  : "This file was moved or no longer exists."}
          </p>
        }
      >
        {(f) => (
          <Show when={f().content.trim()} fallback={<p class="zp-muted">Empty.</p>}>
            <Show when={/\.(md|markdown)$/i.test(f().path)} fallback={<pre class="zp-file-preview">{f().content}</pre>}>
              <Markdown text={f().content} live={false} />
            </Show>
          </Show>
        )}
      </Show>
    </div>
  );
}

function History(props: { zapper: Zapper }) {
  const latest = createMemo(() => {
    const map = new Map<string, ZapperFile>();
    for (const f of resources.files) if ((map.get(f.path)?.version ?? 0) < f.version) map.set(f.path, f);
    return map;
  });
  const [restore, setRestore] = createSignal<ZapperFile | null>(null);
  const older = () =>
    resources.files
      .filter((f) => f.version < (latest().get(f.path)?.version ?? 0))
      .slice()
      .reverse();
  const restoreVersion = async () => {
    const f = restore();
    if (!f) return;
    try {
      await run(props.zapper, { type: "restoreFile", path: f.path, version: f.version, expectedVersion: latest().get(f.path)?.version ?? 0 });
      setRestore(null);
    } catch {
      /* shared alert */
    }
  };
  return (
    <>
      <div class="zp-section-heading">
        <h3>File history</h3>
        <IconButton size="sm" icon="refresh" label="Refresh history" busy={resources.loading} onClick={() => void loadResources(props.zapper)} />
      </div>
      <For each={older()} fallback={<p class="zp-muted">{resources.loading ? "Loading history…" : "No previous versions yet."}</p>}>
        {(file) => (
          <div class="zp-card">
            <div class="zp-section-heading">
              <strong>{file.path}</strong>
              <small>
                v{file.version}
                {file.deleted ? " · deleted" : latest().get(file.path)?.deleted ? " · file deleted since" : ""}
              </small>
            </div>
            <Show when={!file.deleted}>
              <Show when={/\.md$/i.test(file.path)} fallback={<pre class="zp-file-preview">{file.content}</pre>}>
                <Markdown text={file.content} live={false} />
              </Show>
              <Button size="sm" onClick={() => setRestore(file)}>Restore version</Button>
            </Show>
          </div>
        )}
      </For>
      <Dialog
        open={!!restore()}
        onOpenChange={(open) => {
          if (!open) setRestore(null);
        }}
        title="Restore document version?"
        description="This creates a new current version. The existing history is preserved."
        footer={
          <Button disabled={commandState.busy} onClick={() => void restoreVersion()}>
            Restore version
          </Button>
        }
      />
    </>
  );
}

/** The file dialogs, mounted once per inspector so every Drive surface shares them. */
export function FileDialogs(props: { zapper: Zapper }) {
  const close = () => setPending(null);
  const patch = (change: Partial<Pending>) => setPending((p) => (p ? ({ ...p, ...change } as Pending) : p));
  const reopen = (from: string, to: string) => {
    const panel = panelFor(props.zapper.id);
    const open = panel.open.find((page) => pageKey(page) === "file:" + from);
    if (open) {
      closePage(props.zapper.id, open);
      showPage(props.zapper.id, { kind: "file", path: to });
    }
    reveal(props.zapper.id, to);
  };
  const saveFile = async (event: SubmitEvent) => {
    event.preventDefault();
    const p = pending();
    if (p?.kind !== "edit") return;
    const draft = p.draft;
    const path = draft.path.trim();
    const problem = draft.version ? null : pathProblem(path);
    if (problem) return patch({ draft: { ...draft, error: problem } } as Partial<Pending>);
    try {
      await run(props.zapper, { type: "writeFile", path, content: draft.content, expectedVersion: draft.version });
      close();
      reveal(props.zapper.id, path);
    } catch (error) {
      const text = failure(error);
      if (!/version conflict/i.test(text)) return patch({ draft: { ...draft, error: text } } as Partial<Pending>);
      await loadResources(props.zapper);
      const latest = latestFiles(resources.files).find((f) => f.path === path);
      patch({
        draft: {
          ...draft,
          latest,
          version: latest?.version ?? draft.version,
          error: latest
            ? "This file changed while you were editing. Your draft is kept; the latest version (v" + latest.version + ") is below. Saving now replaces it."
            : "This file was deleted while you were editing. Your draft is kept; saving creates it again.",
        },
      } as Partial<Pending>);
    }
  };
  const saveFolder = async (event: SubmitEvent) => {
    event.preventDefault();
    const p = pending();
    if (p?.kind !== "folder") return;
    const path = p.path.trim().replace(/\/+$/, "");
    const problem = pathProblem(path);
    if (problem) return patch({ error: problem });
    try {
      await run(props.zapper, { type: "createFolder", path });
      close();
      reveal(props.zapper.id, path);
    } catch (error) {
      patch({ error: failure(error) });
    }
  };
  const move = async (event: SubmitEvent) => {
    event.preventDefault();
    const p = pending();
    if (p?.kind !== "move") return;
    const to = p.to.trim().replace(/\/+$/, "");
    const problem = to === p.node.path ? "Choose a different name or folder." : pathProblem(to);
    if (problem) return patch({ error: problem });
    try {
      if (p.node.kind === "folder") await run(props.zapper, { type: "moveFolder", path: p.node.path, to, files: filesUnder(resources.files, p.node.path) });
      else await run(props.zapper, { type: "moveFile", path: p.node.path, to, expectedVersion: p.node.file?.version ?? 0 });
      close();
      reopen(p.node.path, to);
    } catch (error) {
      patch({ error: failure(error) });
    }
  };
  const remove = async () => {
    const p = pending();
    if (p?.kind !== "delete") return;
    try {
      if (p.node.kind === "folder") await run(props.zapper, { type: "deleteFolder", path: p.node.path, files: filesUnder(resources.files, p.node.path) });
      else await run(props.zapper, { type: "deleteFile", path: p.node.path, expectedVersion: p.node.file?.version ?? 0 });
      const page = panelFor(props.zapper.id).open.find((page) => pageKey(page) === "file:" + p.node.path);
      if (page) closePage(props.zapper.id, page);
      close();
    } catch (error) {
      patch({ error: failure(error) });
    }
  };
  const scope = createMemo(() => {
    const p = pending();
    if (p?.kind !== "delete" || p.node.kind !== "folder") return null;
    return { files: filesUnder(resources.files, p.node.path), folders: foldersUnder(props.zapper.folders ?? [], p.node.path) };
  });
  return (
    <>
      <Show when={pending()?.kind === "edit" && (pending() as Extract<Pending, { kind: "edit" }>).draft}>
        {(draft) => (
          <Dialog open onOpenChange={(open) => !open && close()} title={draft().version ? "Edit " + draft().path : "New file"} description={RESERVED[draft().path]}>
            <form class="zp-form" onSubmit={saveFile}>
              <Show when={!draft().version && !isReserved(draft().path)}>
                <label>
                  Path
                  <input
                    required
                    autofocus
                    value={draft().path}
                    onInput={(e) => patch({ draft: { ...draft(), path: e.currentTarget.value, error: undefined } } as Partial<Pending>)}
                    placeholder="handoffs/plan.md"
                  />
                </label>
              </Show>
              <label>
                Content
                <textarea rows={18} value={draft().content} onInput={(e) => patch({ draft: { ...draft(), content: e.currentTarget.value } } as Partial<Pending>)} />
              </label>
              <Show when={draft().error}>
                <p role="alert" class="zp-error">{draft().error}</p>
              </Show>
              <Show when={draft().latest}>
                {(latest) => (
                  <details class="zp-file-latest" open>
                    <summary>Latest version (v{latest().version})</summary>
                    <pre class="zp-file-preview">{latest().content}</pre>
                  </details>
                )}
              </Show>
              <div class="zp-form-actions">
                <Button type="button" onClick={close}>Cancel</Button>
                <Button type="submit" variant="primary" disabled={commandState.busy}>Save</Button>
              </div>
            </form>
          </Dialog>
        )}
      </Show>
      <Show when={pending()?.kind === "folder" && (pending() as Extract<Pending, { kind: "folder" }>)}>
        {(p) => (
          <Dialog open onOpenChange={(open) => !open && close()} title="New folder" description="Folders hold documents the coordinator and threads open when they need them.">
            <form class="zp-form" onSubmit={saveFolder}>
              <label>
                Path
                <input required autofocus value={p().path} onInput={(e) => patch({ path: e.currentTarget.value, error: undefined })} placeholder="handoffs/design" />
              </label>
              <Show when={p().error}><p role="alert" class="zp-error">{p().error}</p></Show>
              <div class="zp-form-actions">
                <Button type="button" onClick={close}>Cancel</Button>
                <Button type="submit" variant="primary" disabled={commandState.busy}>Create folder</Button>
              </div>
            </form>
          </Dialog>
        )}
      </Show>
      <Show when={pending()?.kind === "move" && (pending() as Extract<Pending, { kind: "move" }>)}>
        {(p) => (
          <Dialog
            open
            onOpenChange={(open) => !open && close()}
            title={"Rename or move " + p().node.name}
            description={p().node.kind === "folder" ? "Everything inside moves with it. Version history stays with each file's old path." : "Version history stays with the old path."}
          >
            <form class="zp-form" onSubmit={move}>
              <label>
                New path
                <input required autofocus value={p().to} onInput={(e) => patch({ to: e.currentTarget.value, error: undefined })} />
              </label>
              <Show when={p().error}><p role="alert" class="zp-error">{p().error}</p></Show>
              <div class="zp-form-actions">
                <Button type="button" onClick={close}>Cancel</Button>
                <Button type="submit" variant="primary" disabled={commandState.busy}>Move</Button>
              </div>
            </form>
          </Dialog>
        )}
      </Show>
      <Show when={pending()?.kind === "delete" && (pending() as Extract<Pending, { kind: "delete" }>)}>
        {(p) => (
          <Dialog
            open
            onOpenChange={(open) => !open && close()}
            title={p().node.kind === "folder" ? "Delete " + p().node.name + " and everything in it?" : "Delete " + p().node.path + "?"}
            description="Version history keeps every version, so you can restore files later."
            footer={
              <>
                <Button onClick={close}>Cancel</Button>
                <Button variant="danger" disabled={commandState.busy} onClick={() => void remove()}>
                  {scope() ? (scope()!.files.length ? "Delete " + scope()!.files.length + (scope()!.files.length === 1 ? " file" : " files") : "Delete folder") : "Delete file"}
                </Button>
              </>
            }
          >
            <Show when={scope()}>
              {(s) => (
                <div class="zp-delete-scope">
                  <p>
                    {s().files.length ? s().files.length + (s().files.length === 1 ? " file" : " files") : "No files"}
                    {s().folders.length ? " and " + s().folders.length + (s().folders.length === 1 ? " folder" : " folders") : ""} inside {p().node.path}:
                  </p>
                  <ul>
                    <For each={[...s().files.map((f) => f.path), ...s().folders.map((f) => f + "/")].slice(0, 8)}>{(path) => <li>{path.slice(p().node.path.length + 1)}</li>}</For>
                  </ul>
                  <Show when={s().files.length + s().folders.length > 8}>
                    <p class="zp-muted">and {s().files.length + s().folders.length - 8} more</p>
                  </Show>
                </div>
              )}
            </Show>
            <Show when={p().error}><p role="alert" class="zp-error">{p().error}</p></Show>
          </Dialog>
        )}
      </Show>
    </>
  );
}
