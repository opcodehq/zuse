// Engine-owned preferences on the General page: thread naming and the worktree location.
import { onConnected, rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { childPath, parentPath } from "@/core/paths";
import { createStore } from "solid-js";
import { message, type TitleSettings, uncertain, type WorktreeStatus } from "./data";

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
  setGeneral(draft => { Object.assign(draft, { naming: null, worktrees: null, folders: null, namingError: null, worktreesError: null, foldersError: null }); });
});

export async function loadNaming() {
  if (general.namingLoading || general.namingSaving) return;
  setGeneral(draft => { Object.assign(draft, { namingLoading: true }); });
  try {
    ((value: { naming: TitleSettings; namingError: null; }) => setGeneral(draft => { Object.assign(draft, value); }))({ naming: await rpc.call<TitleSettings>(M.GET_TITLE_SETTINGS), namingError: null });
  } catch (error) {
    setGeneral(draft => { Object.assign(draft, { namingError: message(error) }); });
  } finally {
    setGeneral(draft => { Object.assign(draft, { namingLoading: false }); });
  }
}

export async function saveNaming(harness: string | null, model: string | null) {
  if (general.namingSaving) return;
  setGeneral(draft => { Object.assign(draft, { namingSaving: true, namingError: null }); });
  try {
    const settings = { harness, model: model && model.trim() && model !== "default" ? model : null };
    ((value: { naming: TitleSettings; }) => setGeneral(draft => { Object.assign(draft, value); }))({ naming: await rpc.call<TitleSettings>(M.SET_TITLE_SETTINGS, settings) });
  } catch (error) {
    setGeneral(draft => {
      Object.assign(draft, {
        namingError: uncertain(error)
          ? "The naming preference may have saved before disconnection. It will be read after reconnecting; the change will not be repeated automatically."
          : message(error),
      });
    });
    setGeneral(draft => { Object.assign(draft, { namingSaving: false }); });
    void loadNaming();
    return;
  }
  setGeneral(draft => { Object.assign(draft, { namingSaving: false }); });
}

export async function loadWorktrees() {
  if (general.worktreesLoading || general.worktreesSaving) return;
  setGeneral(draft => { Object.assign(draft, { worktreesLoading: true }); });
  try {
    ((value: { worktrees: WorktreeStatus; worktreesError: null; }) => setGeneral(draft => { Object.assign(draft, value); }))({ worktrees: await rpc.call<WorktreeStatus>(M.GET_WORKTREE_SETTINGS), worktreesError: null });
  } catch (error) {
    setGeneral(draft => { Object.assign(draft, { worktreesError: message(error) }); });
  } finally {
    setGeneral(draft => { Object.assign(draft, { worktreesLoading: false }); });
  }
}

export async function saveWorktrees(useCustomDirectory: boolean, customDirectory: string | null) {
  if (general.worktreesSaving) return false;
  setGeneral(draft => { Object.assign(draft, { worktreesSaving: true, worktreesError: null }); });
  try {
    ((value: { worktrees: WorktreeStatus; }) => setGeneral(draft => { Object.assign(draft, value); }))({ worktrees: await rpc.call<WorktreeStatus>(M.SET_WORKTREE_SETTINGS, { useCustomDirectory, customDirectory }) });
    return true;
  } catch (error) {
    setGeneral(draft => {
      Object.assign(draft, {
        worktreesError: uncertain(error)
          ? "The save may have completed before disconnection. The current location will be read after reconnecting. Your entered path is kept."
          : message(error),
      });
    });
    return false;
  } finally {
    setGeneral(draft => { Object.assign(draft, { worktreesSaving: false }); });
  }
}

let folderRequest = 0;
/** `climb` opens the nearest existing ancestor when the starting folder doesn't exist yet (the default often doesn't). */
export async function browseFolders(path?: string, climb = false) {
  const request = ++folderRequest;
  setGeneral(draft => { Object.assign(draft, { foldersLoading: true, foldersError: null }); });
  let target = path;
  try {
    for (;;) {
      try {
        const listing = await rpc.call<FolderListing>(M.LIST_FOLDERS, target ? { path: target } : {});
        if (request === folderRequest) setGeneral(draft => { Object.assign(draft, { folders: listing }); });
        return;
      } catch (error) {
        const parent = climb && target ? parentPath(target) : null;
        if (!parent || request !== folderRequest) throw error;
        target = parent;
      }
    }
  } catch (error) {
    if (request === folderRequest) setGeneral(draft => { Object.assign(draft, { foldersError: message(error) }); });
  } finally {
    if (request === folderRequest) setGeneral(draft => { Object.assign(draft, { foldersLoading: false }); });
  }
}

export function closeFolders() {
  folderRequest++;
  setGeneral(draft => { Object.assign(draft, { folders: null, foldersLoading: false, foldersError: null }); });
}

export { childPath as joinPath,parentPath };
