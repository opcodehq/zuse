// Engine-owned preferences on the General page: thread naming and the worktree location.
import { createStore } from "solid-js/store";
import { M } from "@/core/methods";
import { onConnected, rpc } from "@/core/connection";
import { message, type TitleSettings, uncertain, type WorktreeStatus } from "./data";
import { childPath, parentPath } from "@/core/paths";

interface FolderListing {
  path: string;
  entries: { name: string; isDir: boolean; isRepo: boolean }[];
  truncated?: boolean;
}

interface GeneralState {
  naming: TitleSettings | null;
  namingLoading: boolean;
  namingSaving: boolean;
  namingError: string | null;
  worktrees: WorktreeStatus | null;
  worktreesLoading: boolean;
  worktreesSaving: boolean;
  worktreesError: string | null;
  folders: FolderListing | null;
  foldersLoading: boolean;
  foldersError: string | null;
}

export const [general, setGeneral] = createStore<GeneralState>({
  naming: null,
  namingLoading: false,
  namingSaving: false,
  namingError: null,
  worktrees: null,
  worktreesLoading: false,
  worktreesSaving: false,
  worktreesError: null,
  folders: null,
  foldersLoading: false,
  foldersError: null,
});

onConnected(() => {
  setGeneral({ naming: null, worktrees: null, folders: null, namingError: null, worktreesError: null, foldersError: null });
});

export async function loadNaming() {
  if (general.namingLoading || general.namingSaving) return;
  setGeneral({ namingLoading: true });
  try {
    setGeneral({ naming: await rpc.call<TitleSettings>(M.GET_TITLE_SETTINGS), namingError: null });
  } catch (error) {
    setGeneral({ namingError: message(error) });
  } finally {
    setGeneral({ namingLoading: false });
  }
}

export async function saveNaming(harness: string | null, model: string | null) {
  if (general.namingSaving) return;
  setGeneral({ namingSaving: true, namingError: null });
  try {
    const settings = { harness, model: model && model.trim() && model !== "default" ? model : null };
    setGeneral({ naming: await rpc.call<TitleSettings>(M.SET_TITLE_SETTINGS, settings) });
  } catch (error) {
    setGeneral({
      namingError: uncertain(error)
        ? "The naming preference may have saved before disconnection. It will be read after reconnecting; the change will not be repeated automatically."
        : message(error),
    });
    setGeneral({ namingSaving: false });
    void loadNaming();
    return;
  }
  setGeneral({ namingSaving: false });
}

export async function loadWorktrees() {
  if (general.worktreesLoading || general.worktreesSaving) return;
  setGeneral({ worktreesLoading: true });
  try {
    setGeneral({ worktrees: await rpc.call<WorktreeStatus>(M.GET_WORKTREE_SETTINGS), worktreesError: null });
  } catch (error) {
    setGeneral({ worktreesError: message(error) });
  } finally {
    setGeneral({ worktreesLoading: false });
  }
}

export async function saveWorktrees(useCustomDirectory: boolean, customDirectory: string | null) {
  if (general.worktreesSaving) return false;
  setGeneral({ worktreesSaving: true, worktreesError: null });
  try {
    setGeneral({ worktrees: await rpc.call<WorktreeStatus>(M.SET_WORKTREE_SETTINGS, { useCustomDirectory, customDirectory }) });
    return true;
  } catch (error) {
    setGeneral({
      worktreesError: uncertain(error)
        ? "The save may have completed before disconnection. The current location will be read after reconnecting. Your entered path is kept."
        : message(error),
    });
    return false;
  } finally {
    setGeneral({ worktreesSaving: false });
  }
}

let folderRequest = 0;
/** `climb` opens the nearest existing ancestor when the starting folder doesn't exist yet (the default often doesn't). */
export async function browseFolders(path?: string, climb = false) {
  const request = ++folderRequest;
  setGeneral({ foldersLoading: true, foldersError: null });
  let target = path;
  try {
    for (;;) {
      try {
        const listing = await rpc.call<FolderListing>(M.LIST_FOLDERS, target ? { path: target } : {});
        if (request === folderRequest) setGeneral({ folders: listing });
        return;
      } catch (error) {
        const parent = climb && target ? parentPath(target) : null;
        if (!parent || request !== folderRequest) throw error;
        target = parent;
      }
    }
  } catch (error) {
    if (request === folderRequest) setGeneral({ foldersError: message(error) });
  } finally {
    if (request === folderRequest) setGeneral({ foldersLoading: false });
  }
}

export function closeFolders() {
  folderRequest++;
  setGeneral({ folders: null, foldersLoading: false, foldersError: null });
}

export { childPath as joinPath, parentPath };
