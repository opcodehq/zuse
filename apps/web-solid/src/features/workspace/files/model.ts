// File tree and open-file state per chat. The engine owns the files; this is the projection
// the Files surface reads, plus the save/conflict/reconnect protocol from crates/viewport/src/workspace.rs.
import { batch } from "solid-js";
import { createStore, produce, reconcile } from "solid-js/store";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { RpcError } from "@/core/rpc";
import { state } from "@/core/store";
import { openPanel, errorText } from "../store";
import type { WorkspaceDirectoryPage, WorkspaceEntry, WorkspaceFileChanges, WorkspaceFileText, WriteOutcome } from "../types";

export interface DirState {
  entries: WorkspaceEntry[];
  loading: boolean;
  error: string | null;
  nextCursor: string | null;
  truncated: boolean;
}

export type Conflict =
  | { kind: "changed" | "deleted" | "replaced" | "notRegularFile"; message: string }
  | { kind: "external"; message: string };

export interface OpenFile {
  path: string;
  /** The last read or saved snapshot; edits are compared against its text. */
  file: WorkspaceFileText | null;
  loading: boolean;
  error: string | null;
  saving: boolean;
  /** Set while a reconnect or uncertain save is being verified against the engine. */
  verifying: boolean;
  conflict: Conflict | null;
  /** The engine reported the file changed while it had unsaved edits. */
  dirty: boolean;
  /** Unsaved editor text, kept here so edits survive the editor unmounting (chat switch, hidden panel). */
  draftText?: string | null;
}

interface ChatFiles {
  dirs: Record<string, DirState>;
  expanded: Record<string, boolean>;
  selected: string | null;
  checkoutId: string | null;
  open: OpenFile | null;
}

const [files, setFiles] = createStore<Record<string, ChatFiles>>({});
export { files };

/** A file another surface asked the Files panel to open. */
const [pending, setPending] = createStore<{ path: string | null; seq: number }>({ path: null, seq: 0 });
export { pending };

export function openFileInPanel(path: string) {
  setPending({ path, seq: pending.seq + 1 });
  openPanel("files");
}
export function takePending() {
  const path = pending.path;
  setPending("path", null);
  return path;
}

function chatFiles(chatId: string): ChatFiles {
  if (!files[chatId]) setFiles(chatId, { dirs: {}, expanded: {}, selected: null, checkoutId: null, open: null });
  return files[chatId]!;
}

function target(chatId: string) {
  return { chatId, targetDeviceId: state.chats[chatId]?.deviceId ?? null };
}

/** List one directory, replacing its entries, or appending the next page. */
export function loadDirectory(chatId: string, directory: string, more = false) {
  const current = chatFiles(chatId).dirs[directory];
  if (current?.loading) return;
  const cursor = more ? current?.nextCursor : null;
  setFiles(chatId, "dirs", directory, (prev) => ({ entries: prev?.entries ?? [], nextCursor: prev?.nextCursor ?? null, truncated: prev?.truncated ?? false, loading: true, error: null }));
  rpc
    .call<WorkspaceDirectoryPage>(M.LIST_WORKSPACE_DIRECTORY, { ...target(chatId), directory, ...(cursor ? { cursor } : {}) })
    .then(
      (page) =>
        batch(() => {
          if (page.checkoutId) setFiles(chatId, "checkoutId", page.checkoutId);
          const entries = cursor ? [...(files[chatId]?.dirs[directory]?.entries ?? []), ...page.entries] : page.entries;
          setFiles(chatId, "dirs", directory, reconcile({ entries: sortEntries(entries), loading: false, error: null, nextCursor: page.nextCursor ?? null, truncated: page.truncated }, { key: "path" }));
        }),
      (error) => setFiles(chatId, "dirs", directory, (prev) => ({ ...prev!, loading: false, error: errorText(error, "Couldn't list this folder.") })),
    );
}

function sortEntries(entries: WorkspaceEntry[]) {
  return [...entries].sort((a, b) => {
    const dirA = a.kind === "directory" ? 0 : 1;
    const dirB = b.kind === "directory" ? 0 : 1;
    return dirA - dirB || a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
  });
}

export function toggleDirectory(chatId: string, path: string, open?: boolean) {
  const next = open ?? !chatFiles(chatId).expanded[path];
  setFiles(chatId, "expanded", path, next);
  if (next && !files[chatId]!.dirs[path]) loadDirectory(chatId, path);
}

export function selectPath(chatId: string, path: string | null) {
  chatFiles(chatId);
  setFiles(chatId, "selected", path);
}

/** Expand every ancestor of `path` so it shows in the tree. */
export function reveal(chatId: string, path: string) {
  const parts = path.split("/");
  for (let i = 1; i < parts.length; i++) toggleDirectory(chatId, parts.slice(0, i).join("/"), true);
  selectPath(chatId, path);
}

/** Apply a WatchWorkspaceFiles frame: reload affected folders already listed. */
export function applyFileChanges(chatId: string, frame: WorkspaceFileChanges, isDirty: () => boolean) {
  const tree = files[chatId];
  if (!tree) return;
  const reload = new Set<string>();
  if (frame.resyncRequired) {
    for (const dir of Object.keys(tree.dirs)) reload.add(dir);
  }
  for (const change of frame.changes) {
    for (const path of [change.path, change.oldPath].filter((p): p is string => !!p)) {
      const parent = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
      if (tree.dirs[parent]) reload.add(parent);
    }
  }
  for (const dir of reload) loadDirectory(chatId, dir);
  const open = tree.open;
  if (!open?.file || open.saving) return;
  const touched = frame.resyncRequired || frame.changes.some((c) => c.path === open.path || c.oldPath === open.path);
  if (!touched) return;
  if (isDirty()) {
    // Confirm the engine copy really changed before warning; our own save also emits a change.
    verifyOpenFile(chatId, isDirty, "external");
  } else readFile(chatId, open.path);
}

/** Read a file into the editor. Refuses while a save or verification is in flight. */
export function readFile(chatId: string, path: string) {
  const tree = chatFiles(chatId);
  if (tree.open?.saving || tree.open?.verifying) return;
  setFiles(chatId, "open", {
    path,
    file: tree.open?.path === path ? tree.open.file : null,
    loading: true,
    error: null,
    saving: false,
    verifying: false,
    conflict: null,
    dirty: false,
  });
  setFiles(chatId, "selected", path);
  rpc.call<WorkspaceFileText>(M.READ_WORKSPACE_FILE, { ...target(chatId), path }).then(
    (file) => {
      if (files[chatId]?.open?.path !== path) return;
      setFiles(chatId, "open", (prev) => ({ ...prev!, file, loading: false, error: null }));
    },
    (error) => {
      if (files[chatId]?.open?.path !== path) return;
      setFiles(chatId, "open", (prev) => ({ ...prev!, loading: false, error: errorText(error, "Couldn't open this file.") }));
    },
  );
}

/** Record the editor's unsaved text for the open file, or `null` once it matches the snapshot. */
export function setFileDraft(chatId: string, path: string, text: string | null) {
  const open = files[chatId]?.open;
  if (open?.path !== path || (open.draftText ?? null) === text) return;
  setFiles(chatId, "open", "draftText", text);
}

export const hasUnsavedFiles = () => Object.values(files).some((tree) => tree.open?.draftText != null);

window.addEventListener("beforeunload", (event) => {
  if (hasUnsavedFiles()) event.preventDefault();
});

export function closeFile(chatId: string) {
  setFiles(chatId, "open", null);
}

export function canSave(open: OpenFile | null | undefined) {
  return !!open?.file && !open.saving && !open.verifying && !open.file.readOnlyReason && !open.file.truncated && typeof open.file.text === "string" && !!open.file.contentHash;
}

/** Save the editor text against the snapshot it was read from. Conflicts never overwrite silently. */
export function saveFile(chatId: string, text: string, force = false): Promise<void> {
  const open = files[chatId]?.open;
  if (!open?.file || !canSave(open)) return Promise.resolve();
  if (open.conflict && !force) return Promise.resolve();
  const file = open.file;
  const path = open.path;
  setFiles(chatId, "open", (prev) => ({ ...prev!, saving: true, error: null }));
  return rpc
    .call<WriteOutcome>(M.WRITE_WORKSPACE_FILE, {
      ...target(chatId),
      path,
      text,
      expectedCheckoutId: file.checkoutId,
      expectedContentHash: file.contentHash,
      encoding: file.encoding === "utf8Bom" ? "utf8Bom" : "utf8",
      lineEnding: file.lineEnding === "crlf" ? "crlf" : "lf",
    })
    .then(
      (outcome) => {
        if (files[chatId]?.open?.path !== path) return;
        if (outcome.status === "written") {
          setFiles(chatId, "open", (prev) => ({
            ...prev!,
            saving: false,
            conflict: null,
            file: { ...file, text, contentHash: outcome.file.contentHash, size: outcome.file.size, modifiedAt: outcome.file.modifiedAt ?? file.modifiedAt },
          }));
        } else {
          setFiles(chatId, "open", (prev) => ({ ...prev!, saving: false, conflict: { kind: outcome.reason, message: conflictMessage(outcome.reason) } }));
        }
      },
      (error) => {
        if (files[chatId]?.open?.path !== path) return;
        setFiles(chatId, "open", (prev) => ({ ...prev!, saving: false }));
        if (error instanceof RpcError && error.outcomeUnknown) {
          // The write may have landed. Read back and compare instead of resending.
          setFiles(chatId, "open", "error", "The connection dropped during the save. Checking whether it was written…");
          pendingVerify.set(chatId, text);
        } else setFiles(chatId, "open", "error", errorText(error, "Couldn't save this file."));
      },
    );
}

function conflictMessage(reason: string) {
  switch (reason) {
    case "deleted":
      return "This file was deleted on disk after you opened it.";
    case "replaced":
      return "This file was replaced on disk after you opened it.";
    case "notRegularFile":
      return "This path is no longer a regular file.";
    default:
      return "This file changed on disk after you opened it.";
  }
}

/** Texts from saves whose receipts were lost, checked after the next connection. */
const pendingVerify = new Map<string, string>();

/**
 * Re-read the open file and compare with what we hold. Used after reconnects, uncertain saves and
 * external changes: an unchanged hash keeps the edits, a matching text confirms a lost save receipt.
 */
export function verifyOpenFile(chatId: string, isDirty: () => boolean, reason: "reconnect" | "external" = "reconnect") {
  const open = files[chatId]?.open;
  if (!open?.file || open.verifying) return;
  const path = open.path;
  const held = open.file;
  const saved = pendingVerify.get(chatId);
  setFiles(chatId, "open", "verifying", true);
  rpc.call<WorkspaceFileText>(M.READ_WORKSPACE_FILE, { ...target(chatId), path }).then(
    (fresh) => {
      if (files[chatId]?.open?.path !== path) return;
      pendingVerify.delete(chatId);
      setFiles(
        chatId,
        "open",
        produce((o) => {
          if (!o) return;
          o.verifying = false;
          if (saved !== undefined) {
            o.error = null;
            if (fresh.text === saved) {
              o.file = fresh;
              return;
            }
            o.error = "The save was not written. Your edits are still here; save again.";
          }
          if (fresh.contentHash === held.contentHash && fresh.checkoutId === held.checkoutId) return;
          if (!isDirty()) {
            o.file = fresh;
            return;
          }
          o.conflict = {
            kind: reason === "external" ? "external" : "changed",
            message: reason === "external" ? "This file changed on disk while you were editing it." : "This file changed on disk while the connection was down.",
          };
        }),
      );
    },
    (error) => {
      if (files[chatId]?.open?.path !== path) return;
      const text = errorText(error);
      setFiles(chatId, "open", produce((o) => {
        if (!o) return;
        o.verifying = false;
        if (/not found|no such file|does not exist/i.test(text)) o.conflict = { kind: "deleted", message: conflictMessage("deleted") };
        else o.error = `Couldn't verify the file: ${text}`;
      }));
    },
  );
}

/** Take the engine's copy for a conflicted file, keeping the user's text for an explicit overwrite. */
export function refreshBaseline(chatId: string): Promise<WorkspaceFileText | null> {
  const open = files[chatId]?.open;
  if (!open) return Promise.resolve(null);
  const path = open.path;
  return rpc.call<WorkspaceFileText>(M.READ_WORKSPACE_FILE, { ...target(chatId), path }).then(
    (fresh) => {
      if (files[chatId]?.open?.path !== path) return null;
      setFiles(chatId, "open", (prev) => ({ ...prev!, file: fresh, conflict: null, error: null }));
      return fresh;
    },
    (error) => {
      setFiles(chatId, "open", "error", errorText(error));
      return null;
    },
  );
}

export function resetChatFiles(chatId: string) {
  setFiles(chatId, reconcile({ dirs: {}, expanded: {}, selected: null, checkoutId: null, open: files[chatId]?.open ?? null }));
}
