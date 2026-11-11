// Editing a queued message in the composer under the host's edit lease.
import { createEffect, createRoot, createSignal, on } from "solid-js";
import { rpc } from "@/core/connection";
import { chatRequest } from "@/core/chat";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import { view } from "@/core/view";
import { draftFor, fail, holdPersistedDraft, setDraft, snapshotDraft, type Draft } from "./drafts";

export interface QueuedMessage {
  id: string;
  text: string;
  attachments?: string[];
  holdForTurnEnd?: boolean;
  issuedBy?: string;
  issuedAt?: number;
  deliveryGate?: { kind: "editing" | "reviewRequired" | string; ownerDeviceId?: string; previousLeaseId?: string } | null;
}

interface Lease { id: string; baseTextHash: string; attachments: string[] }
interface Edit {
  chatId: string;
  id: string;
  lease?: Lease;
  previous?: Draft;
  finishing: boolean;
}

const [edit, setEdit] = createSignal<Edit | null>(null);
export { edit as queueEdit };

const instance = crypto.randomUUID();
// Per tab, so a reload hands back the lease it held instead of leaving the row locked until it expires.
const LEASE = "zuse.web.queueEdit.v1";
interface HeldLease { chatId: string; id: string; leaseId: string; hash: string }
function rememberLease(lease: HeldLease | null) {
  try {
    if (lease) sessionStorage.setItem(LEASE, JSON.stringify(lease));
    else sessionStorage.removeItem(LEASE);
  } catch {
    // Without storage the lease simply expires on the host.
  }
}
function heldLease(): HeldLease | null {
  try {
    const raw = sessionStorage.getItem(LEASE);
    return raw ? (JSON.parse(raw) as HeldLease) : null;
  } catch {
    return null;
  }
}
let renewTimer: ReturnType<typeof setInterval> | undefined;
const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** The queued text without the internal attachment trailer the host rebuilds on promotion. */
export function queueText(text: string) {
  return text.replace(/\n\nAttached images \(local files[^\n]*\):\n[\s\S]*$/, "");
}

export function queueTitle(row: QueuedMessage) {
  const text = queueText(row.text).replace(/\[([^\]]*)\]\(zuse-[a-z]+:[^)]*\)/g, "$1").split(/\s+/).join(" ").trim();
  const count = row.attachments?.length ?? 0;
  if (text) return text;
  return count ? `(${count} file${count === 1 ? "" : "s"})` : "(empty)";
}

export async function beginQueueEdit(chatId: string, row: QueuedMessage) {
  if (edit()) return;
  // Snapshot before the request: the draft may change (or the chat be left) while it is in flight.
  const pending: Edit = { chatId, id: row.id, previous: snapshotDraft(chatId), finishing: false };
  setEdit(pending);
  try {
    const reply = await rpc.call<Record<string, unknown>>(
      M.BEGIN_QUEUED_MESSAGE_EDIT,
      chatRequest(chatId, { id: row.id, editorDeviceId: state.localDeviceId ?? "web", editorInstanceId: instance }),
    );
    const leaseId = typeof reply?.leaseId === "string" ? reply.leaseId : "";
    const hash = typeof reply?.baseTextHash === "string" ? reply.baseTextHash : "";
    const acquired = reply?.outcome === "acquired" && !!leaseId && !!hash;
    if (edit() !== pending) {
      // Abandoned while waiting (the user left the chat): hand the lease back and leave the draft alone.
      if (acquired) void release(chatId, row.id, leaseId, hash);
      return;
    }
    if (!acquired) {
      setEdit(null);
      return fail(
        chatId,
        reply?.outcome === "locked" ? "That queued message is being edited on another device" : reply?.outcome === "missing" ? "That queued message is no longer available" : "The chat host returned an invalid edit lease",
      );
    }
    holdPersistedDraft(chatId, pending.previous ?? { text: "", mentions: [], skills: [] });
    rememberLease({ chatId, id: row.id, leaseId, hash });
    setDraft(chatId, { text: queueText(typeof reply.text === "string" ? reply.text : row.text), mentions: [], skills: [] });
    setEdit({ ...pending, lease: { id: leaseId, baseTextHash: hash, attachments: Array.isArray(reply.attachments) ? (reply.attachments as string[]) : [] } });
    clearInterval(renewTimer);
    renewTimer = setInterval(() => void renew(), 20_000);
  } catch (error) {
    if (edit() !== pending) return;
    setEdit(null);
    fail(chatId, `Connect to the chat host to edit this message: ${errorText(error)}`);
  }
}

function release(chatId: string, id: string, leaseId: string, expectedTextHash: string) {
  return rpc.call(M.FINISH_QUEUED_MESSAGE_EDIT, chatRequest(chatId, { id, leaseId, action: "cancel", expectedTextHash })).catch(() => {
    // The lease expires on the host if this is lost.
  });
}

async function renew() {
  const current = edit();
  if (!current?.lease || current.finishing) return;
  try {
    const reply = await rpc.call<Record<string, unknown>>(M.RENEW_QUEUED_MESSAGE_EDIT, chatRequest(current.chatId, { id: current.id, leaseId: current.lease.id }));
    if (reply?.outcome !== "renewed") fail(current.chatId, "The edit lease was lost; your text is still in the editor");
  } catch {
    fail(current.chatId, "Couldn't reach the chat host; your edit is still in the editor");
  }
}

async function finish(action: "commit" | "cancel" | "discard") {
  const current = edit();
  if (!current?.lease || current.finishing) return false;
  setEdit({ ...current, finishing: true });
  const text = draftFor(current.chatId).text;
  try {
    const reply = await rpc.call<Record<string, unknown>>(
      M.FINISH_QUEUED_MESSAGE_EDIT,
      chatRequest(current.chatId, {
        id: current.id,
        leaseId: current.lease.id,
        action,
        expectedTextHash: current.lease.baseTextHash,
        ...(action === "commit" ? { text, attachments: current.lease.attachments } : {}),
      }),
    );
    const expected = action === "cancel" ? "cancelled" : action === "discard" ? "discarded" : "committed";
    if (reply?.outcome !== expected) {
      setEdit({ ...current, finishing: false });
      fail(
        current.chatId,
        reply?.outcome === "conflict" ? "This message changed on another device; your edit was kept locally" : reply?.outcome === "missing" || reply?.outcome === "lost" ? "The queued message was removed; your edit was kept locally" : "The chat host did not confirm this queue edit",
      );
      return true;
    }
    leave();
  } catch (error) {
    setEdit({ ...current, finishing: false });
    fail(current.chatId, `Couldn't reach the chat host; your edit is still in the editor (${errorText(error)})`);
  }
  return true;
}

function leave() {
  const current = edit();
  clearInterval(renewTimer);
  // Without a lease the composer never showed the queued text, so the draft is still the user's own.
  if (current?.lease) {
    rememberLease(null);
    holdPersistedDraft(current.chatId, null);
  }
  if (current?.lease) setDraft(current.chatId, current.previous ?? { text: "", mentions: [], skills: [] });
  setEdit(null);
}

/** Enter while editing saves the row (an emptied row is discarded). */
export function commitQueueEdit() {
  const current = edit();
  if (!current) return false;
  void finish(draftFor(current.chatId).text.trim() || current.lease?.attachments.length ? "commit" : "discard");
  return true;
}

export function cancelQueueEdit() {
  const current = edit();
  if (!current) return false;
  if (!current.lease) leave();
  else void finish("cancel");
  return true;
}

createRoot(() => {
  // An edit belongs to one visible row: leaving the chat cancels it.
  createEffect(on(() => view.selectedChatId, (chatId) => {
    const current = edit();
    if (current && current.chatId !== chatId) cancelQueueEdit();
  }, { defer: true }));
  // Another device removed the row: keep both drafts in the composer.
  createEffect(() => {
    const current = edit();
    if (!current?.lease || current.finishing) return;
    const rows = (state.queue[current.chatId] ?? []) as QueuedMessage[];
    if (rows.some((row) => row.id === current.id)) return;
    clearInterval(renewTimer);
    rememberLease(null);
    holdPersistedDraft(current.chatId, null);
    const text = [current.previous?.text ?? "", draftFor(current.chatId).text].filter((t) => t.trim()).join("\n\n");
    setEdit(null);
    setDraft(current.chatId, { text, mentions: current.previous?.mentions ?? [], skills: current.previous?.skills ?? [] });
    fail(current.chatId, "The queued message was removed; your edit remains in the composer");
  });
});

createRoot(() => {
  // A reload mid-edit: the draft in storage is already the user's own; release the old lease once its chat is known.
  const stale = heldLease();
  if (!stale) return;
  let pending = true;
  createEffect(() => {
    if (!pending || state.connection.kind !== "connected" || !state.chats[stale.chatId]) return;
    pending = false;
    if (heldLease()?.leaseId === stale.leaseId) rememberLease(null);
    void release(stale.chatId, stale.id, stale.leaseId, stale.hash);
  });
});
