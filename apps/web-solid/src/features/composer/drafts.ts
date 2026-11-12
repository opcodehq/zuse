// Composer-local state: drafts per chat (persisted), staged attachments, failures and in-flight work.
import { batch } from "solid-js";
import { createStore, unwrap } from "solid-js/store";
import { view } from "@/core/view";
import { releaseImageUrl } from "@/ui/imageUrl";
import type { Skill } from "./completion";

/** A completed `@` pick, kept so the visible `@path` becomes a canonical file link at send. */
export interface Mention { token: string; path: string; isDir: boolean }
export interface SkillPick { token: string; skill: Skill }

export interface Draft {
  text: string;
  mentions: Mention[];
  skills: SkillPick[];
}

export interface Attachment {
  id: string;
  name: string;
  mime: string;
  size: number;
  file: File;
  preview?: string;
  /** Committed on the connected engine. */
  uploadId?: string;
  path?: string;
  error?: string;
}

const DRAFTS = "zuse.web.composer.drafts.v1";
const emptyDraft = (): Draft => ({ text: "", mentions: [], skills: [] });

function loadDrafts(): Record<string, Draft> {
  try {
    const raw = localStorage.getItem(DRAFTS);
    return raw ? (JSON.parse(raw) as Record<string, Draft>) : {};
  } catch {
    return {};
  }
}

interface ComposerState {
  drafts: Record<string, Draft>;
  attachments: Record<string, Attachment[]>;
  failure: { key: string; message: string } | null;
  interrupting: Record<string, boolean>;
  sending: boolean;
  dismissedErrors: Record<string, string>;
}

const [composer, setComposer] = createStore<ComposerState>({
  drafts: loadDrafts(),
  attachments: {},
  failure: null,
  interrupting: {},
  sending: false,
  dismissedErrors: {},
});
export { composer, setComposer };

// Tabs share one storage key, so each tab writes only the drafts it changed and merges the others' changes.
const touched = new Set<string>();
// A queue edit borrows the composer: storage keeps the user's own draft so a reload never turns queued text into a plain draft.
const held = new Map<string, Draft>();
let saveTimer: ReturnType<typeof setTimeout> | undefined;
function persist(key: string) {
  touched.add(key);
  clearTimeout(saveTimer);
  saveTimer = setTimeout(flush, 250);
}
function flush() {
  clearTimeout(saveTimer);
  if (!touched.size) return;
  const stored = loadDrafts();
  for (const key of touched) {
    const draft = held.get(key) ?? composer.drafts[key];
    if (draft?.text.trim()) stored[key] = unwrap(draft);
    else delete stored[key];
  }
  touched.clear();
  try {
    localStorage.setItem(DRAFTS, JSON.stringify(stored));
  } catch {
    // Private windows may refuse storage; drafts then last for the tab.
  }
}
window.addEventListener("pagehide", flush);
window.addEventListener("storage", (event) => {
  if (event.key !== DRAFTS) return;
  let incoming: Record<string, Draft> = {};
  try {
    incoming = event.newValue ? (JSON.parse(event.newValue) as Record<string, Draft>) : {};
  } catch {
    return;
  }
  const editing = view.selectedChatId ?? "";
  batch(() => {
    for (const key of new Set([...Object.keys(incoming), ...Object.keys(composer.drafts)])) {
      if (key === editing || touched.has(key)) continue;
      const next = incoming[key];
      if (next) {
        if (JSON.stringify(unwrap(composer.drafts[key])) !== JSON.stringify(next)) setComposer("drafts", key, next);
      } else if (composer.drafts[key]?.text.trim()) setComposer("drafts", key, emptyDraft());
    }
  });
});

export const draftFor = (key: string): Draft => composer.drafts[key] ?? emptyDraft();
/** A detached copy: store setters merge into the live draft, so anything kept across a clear must copy. */
export const snapshotDraft = (key: string): Draft => structuredClone(unwrap(draftFor(key)));

export function setDraft(key: string, draft: Partial<Draft>) {
  setComposer("drafts", key, (current) => ({ ...(current ?? emptyDraft()), ...draft }));
  persist(key);
}

/** Persist `draft` for `key` instead of the live composer text until called with null. */
export function holdPersistedDraft(key: string, draft: Draft | null) {
  if (draft) held.set(key, structuredClone(draft));
  else held.delete(key);
  persist(key);
  flush();
}

export function clearDraft(key: string) {
  setComposer("drafts", key, emptyDraft());
  persist(key);
}

export const attachmentsFor = (key: string) => composer.attachments[key] ?? [];

export function addAttachment(key: string, attachment: Attachment) {
  setComposer("attachments", key, (list) => [...(list ?? []), attachment]);
}
export function updateAttachment(key: string, id: string, patch: Partial<Attachment>) {
  const index = attachmentsFor(key).findIndex((a) => a.id === id);
  if (index >= 0) setComposer("attachments", key, index, patch);
}
export function removeAttachment(key: string, id: string) {
  const row = attachmentsFor(key).find((a) => a.id === id);
  releaseImageUrl(row?.preview);
  setComposer("attachments", key, (list) => (list ?? []).filter((a) => a.id !== id));
}
export function takeAttachments(key: string): Attachment[] {
  const list = [...unwrap(attachmentsFor(key))];
  setComposer("attachments", key, []);
  return list;
}
export function restoreAttachments(key: string, list: Attachment[]) {
  setComposer("attachments", key, (current) => [...list, ...(current ?? []).filter((a) => !list.some((b) => b.id === a.id))]);
}

export function fail(key: string, message: string) {
  setComposer("failure", { key, message });
}
export function clearFailure() {
  setComposer("failure", null);
}

/** Put the caret back in the prompt after a tray, menu or landing choice. */
export const focusComposer = () => queueMicrotask(() => document.querySelector<HTMLTextAreaElement>(".cmp-input")?.focus());
