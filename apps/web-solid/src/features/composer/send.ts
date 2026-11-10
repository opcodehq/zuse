// Send, queue, steer and interrupt, plus attachment staging. User actions become engine requests.
import { createEffect, createRoot, on } from "solid-js";
import { rpc } from "@/core/connection";
import { chatRequest } from "@/core/chat";
import { M } from "@/core/methods";
import { filePreviewUrl } from "@/ui/imageUrl";
import { RpcError } from "@/core/rpc";
import { state } from "@/core/store";
import { navigate, openChat, setView, view } from "@/core/view";
import { canonicalText, workspaceCommandFor, type WorkspaceCommand } from "./completion";
import { catalog, chatConfig, draftSpace, resolve, selectedChat, setCatalog, targetDeviceId } from "./catalog";
import {
  addAttachment, attachmentsFor, clearDraft, clearFailure, composer, draftFor, fail, restoreAttachments, setComposer, setDraft,
  snapshotDraft, takeAttachments, updateAttachment, type Attachment, type Draft,
} from "./drafts";

export const MAX_ATTACHMENT = 32 * 1024 * 1024;
const CHUNK = 45_000;
const WINDOW = 8;

/** The draft slot this composer edits: the chat, or "" for the new-chat canvas. */
export const draftKey = () => view.selectedChatId ?? "";

const uuid = () => crypto.randomUUID();
const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));
const isLocal = (deviceId: string | null | undefined) => !deviceId || deviceId === state.localDeviceId;

export function runLive(chatId: string | null = view.selectedChatId) {
  if (!chatId) return false;
  const status = state.sessions[chatId]?.status;
  return status === "working" || status === "awaitingInput";
}

export function hasContent(key = draftKey()) {
  return draftFor(key).text.trim().length > 0 || attachmentsFor(key).length > 0;
}

export const uploading = (key = draftKey()) => attachmentsFor(key).some((a) => !a.path && !a.error);

function toBase64(bytes: Uint8Array) {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
}

/** Stream a file to an engine's uploads folder; returns its committed absolute path there. */
async function upload(file: File, uploadId: string, targetDeviceId?: string | null) {
  const target = targetDeviceId && !isLocal(targetDeviceId) ? { targetDeviceId } : {};
  const total = Math.max(1, Math.ceil(file.size / CHUNK));
  let next = 0;
  const worker = async () => {
    while (next < total) {
      const seq = next++;
      const bytes = new Uint8Array(await file.slice(seq * CHUNK, (seq + 1) * CHUNK).arrayBuffer());
      await rpc.call(M.UPLOAD_CHUNK, { uploadId, seq, data: toBase64(bytes), ...target });
    }
  };
  await Promise.all(Array.from({ length: Math.min(WINDOW, total) }, worker));
  const result = await rpc.call<{ path?: string }>(M.UPLOAD_COMMIT, { uploadId, fileName: file.name, ...target });
  if (!result?.path) throw new Error("Attachment did not finish uploading");
  return result.path;
}

/** Stage files from the picker, a paste or a drop. Bytes land on the connected engine right away. */
export function stageFiles(files: File[], key = draftKey()) {
  for (const file of files) {
    const id = uuid();
    const image = file.type.startsWith("image/");
    const row: Attachment = { id, name: file.name || (image ? "image.png" : "file"), mime: file.type, size: file.size, file, preview: filePreviewUrl(file) };
    if (file.size > MAX_ATTACHMENT) row.error = "Attachments must be 32 MB or smaller.";
    addAttachment(key, row);
    if (row.error) continue;
    const uploadId = uuid().replace(/-/g, "");
    updateAttachment(key, id, { uploadId });
    upload(new File([file], row.name, { type: file.type }), uploadId).then(
      (path) => updateAttachment(key, id, { path }),
      (error) => updateAttachment(key, id, { error: errorText(error) || "Couldn't upload the attachment." }),
    );
  }
}

const IMAGE = /\.(png|jpe?g|gif|webp|svg|bmp|tiff?)$/i;
function withAttachments(text: string, paths: string[]) {
  if (!paths.length) return text;
  const body = text.trim() ? text : paths.every((p) => IMAGE.test(p)) ? "See the attached image(s)." : "See the attached file(s).";
  return `${body}\n\nAttached images (local files — open them to view):\n${paths.map((p) => `- ${p}`).join("\n")}`;
}
const attachmentOnlyText = (paths: string[]) => (paths.every((p) => IMAGE.test(p)) ? "See the attached image(s)." : "See the attached file(s).");

function scratchCwd(chatId: string) {
  const id = chatId.replace(/[^a-z0-9]/gi, "").toLowerCase().slice(-8);
  const date = new Date().toISOString().slice(0, 10);
  return `~/.zuse/scratch/${date}${id ? `-${id}` : ""}`;
}

function supportsScratch(deviceId: string | null) {
  const device = deviceId ? state.devices[deviceId] : undefined;
  const caps = device?.capabilities;
  return !Array.isArray(caps) || caps.includes("projectless-scratch-v1");
}

/** Why the composer cannot send right now, if it cannot. */
export function sendBlocked(): string | null {
  if (state.connection.kind !== "connected") return "Engine not connected";
  if (composer.sending) return "Sending…";
  if (uploading()) return "Uploading image…";
  const chat = selectedChat();
  if (view.selectedChatId && !chat) return "This chat was deleted or is still loading.";
  if (chat?.archived) return "Unarchive this thread before sending another message.";
  if (chat && catalog.saving[chat.id]) return "Saving chat settings…";
  if (!chat && resolve().noAgents) return "No agents available";
  return null;
}

export function runWorkspaceCommand(command: WorkspaceCommand, openModelPicker: () => void) {
  switch (command) {
    case "model": return openModelPicker();
    case "new": return openChat(null);
    case "settings": return navigate({ kind: "settings" });
    case "diff": return setView("rightPanel", "changes");
    case "files": return setView("rightPanel", "files");
    case "terminal": return setView("rightPanel", "terminal");
    case "stop": return void interrupt();
  }
}

/** Enter, the send key, or Alt+Enter. `steer` delivers a busy-chat message into the live turn. */
export async function submit(options: { steer?: boolean; openModelPicker: () => void }) {
  const key = draftKey();
  const draft = snapshotDraft(key);
  const command = workspaceCommandFor(draft.text, !!view.selectedChatId);
  if (command && attachmentsFor(key).length === 0) {
    clearDraft(key);
    return runWorkspaceCommand(command, options.openModelPicker);
  }
  if (!hasContent(key)) return;
  const blocked = sendBlocked();
  if (blocked) {
    if (blocked !== "Sending…" && blocked !== "Uploading image…") fail(key, blocked);
    return;
  }
  if (attachmentsFor(key).some((a) => a.error)) return fail(key, "Remove the attachments that failed to upload before sending.");
  if (runLive()) return queueMessage(key, draft, !!options.steer);
  return run(key, draft);
}

async function queueMessage(key: string, draft: Draft, steer: boolean) {
  const chat = selectedChat();
  if (!chat) return;
  const files = takeAttachments(key);
  clearDraft(key);
  clearFailure();
  setComposer("sending", true);
  try {
    const paths = isLocal(chat.deviceId) ? files.map((f) => f.path!) : await Promise.all(files.map((f) => upload(f.file, uuid().replace(/-/g, ""), chat.deviceId)));
    const body = canonicalText(draft);
    const text = body.trim() ? body : attachmentOnlyText(paths);
    const reply = await rpc.call<{ id?: string }>(M.QUEUE_MESSAGE, chatRequest(chat.id, { text, attachments: paths, holdForTurnEnd: true }));
    if (steer && paths.length === 0 && reply?.id) await queueAction("steer", chat.id, reply.id);
  } catch (error) {
    setDraft(key, draft);
    restoreAttachments(key, files);
    fail(key, error instanceof RpcError && error.outcomeUnknown ? error.message : `Send failed: ${errorText(error)}`);
  } finally {
    setComposer("sending", false);
  }
}

async function run(key: string, draft: Draft) {
  const existing = selectedChat();
  // Never turn a send meant for a vanished chat into a new project-less chat.
  if (view.selectedChatId && !existing) return fail(key, "This chat was deleted or is still loading.");
  const r = resolve();
  const config = chatConfig(r);
  if (!config) return fail(key, "Set up an agent in Settings first.");
  const space = existing ? undefined : draftSpace();
  const device = existing?.deviceId ?? targetDeviceId() ?? state.localDeviceId;
  const chatId = existing?.id ?? uuid();
  const messageId = uuid();
  const files = takeAttachments(key);
  clearDraft(key);
  clearFailure();
  setComposer("sending", true);
  let created = false;
  try {
    // Remote hosts receive staged bytes from the local engine after the command is queued.
    const remote = !isLocal(device);
    const paths = files.map((f) => (remote ? `pending://${f.uploadId}/${f.name}` : f.path!));
    const transfers = remote ? files.map((f) => ({ uploadId: f.uploadId, fileName: f.name })) : [];
    let cwd = existing?.cwd ?? ".";
    if (!existing) {
      cwd = space?.path ?? (supportsScratch(device) ? scratchCwd(chatId) : "~");
      await rpc.call(M.MUTATE, {
        op: "createChat",
        chatId,
        ...(space ? { spaceId: space.id } : { deviceId: device, cwd }),
        config,
      });
      created = true;
      setCatalog("draft", { harness: undefined, model: undefined, reasoning: undefined, plan: false });
      openChat(chatId);
    }
    const prompt = withAttachments(canonicalText(draft), paths);
    await rpc.call(M.QUEUE_COMMAND, {
      chatId,
      targetDeviceId: device,
      command: {
        kind: "run",
        messageId,
        request: {
          prompt,
          harness: config.harness,
          model: config.model,
          reasoning: config.reasoning,
          modelOptions: config.modelOptions,
          cwd,
          sandbox: config.sandbox,
          plan: config.plan,
          autoApprove: config.sandbox === "danger-full-access",
          resume: null,
          attachments: paths,
        },
      },
      ...(transfers.length ? { transfers } : {}),
    });
  } catch (error) {
    const unknown = error instanceof RpcError && error.outcomeUnknown;
    if (created && !unknown) {
      void rpc.call(M.MUTATE, { op: "deleteChat", chatId }).catch(() => {});
      if (view.selectedChatId === chatId) openChat(null);
    }
    // A created chat whose first message may have landed is the screen being shown: park the text there, beside the warning.
    const restoreKey = created ? (unknown ? chatId : "") : key;
    setDraft(restoreKey, draft);
    restoreAttachments(restoreKey, files);
    fail(restoreKey, unknown ? errorText(error) : `Send failed: ${errorText(error)}`);
  } finally {
    setComposer("sending", false);
  }
}

createRoot(() => {
  // A stop request lasts until its run ends; the next run gets a fresh Stop key.
  createEffect(() => {
    for (const [chatId, pending] of Object.entries(composer.interrupting)) if (pending && !runLive(chatId)) setComposer("interrupting", chatId, false);
  });
  // The open chat was deleted elsewhere and core moved to the new-chat canvas: carry its draft along.
  createEffect(
    on(
      () => view.selectedChatId,
      (next, prev) => {
        if (next !== null || !prev || state.chats[prev] || !state.workspaceSynced) return;
        if (!hasContent(prev) || hasContent("")) return;
        setDraft("", snapshotDraft(prev));
        restoreAttachments("", takeAttachments(prev));
        clearDraft(prev);
      },
    ),
  );
  // "Engine not connected" is stale the moment the engine is back.
  createEffect(() => {
    if (state.connection.kind === "connected" && composer.failure?.message === "Engine not connected") clearFailure();
  });
});

export async function interrupt(chatId: string | null = view.selectedChatId) {
  if (!chatId || composer.interrupting[chatId]) return;
  setComposer("interrupting", chatId, true);
  try {
    await rpc.call(M.QUEUE_COMMAND, chatRequest(chatId, { command: { kind: "interrupt" } }));
  } catch (error) {
    setComposer("interrupting", chatId, false);
    fail(chatId, `Stop failed: ${errorText(error)}`);
  }
}

export type QueueAction = "remove" | "steer" | "sendNow" | "move";
const QUEUE_METHODS: Record<QueueAction, string> = {
  remove: M.REMOVE_QUEUED_MESSAGE,
  steer: M.STEER_QUEUED_MESSAGE_NOW,
  sendNow: M.SEND_QUEUED_MESSAGE_NOW,
  move: M.MOVE_QUEUED_MESSAGE,
};
const QUEUE_FAILURES: Record<QueueAction, string> = {
  remove: "Couldn't remove the message",
  steer: "Couldn't send that message",
  sendNow: "Couldn't send that message",
  move: "Couldn't reorder the queue",
};

export async function queueAction(action: QueueAction, chatId: string, id: string, toIndex?: number) {
  try {
    const reply = await rpc.call<Record<string, unknown>>(QUEUE_METHODS[action], chatRequest(chatId, { id, ...(toIndex === undefined ? {} : { toIndex }) }));
    const confirmed = reply?.[action === "move" ? "changed" : action === "remove" ? "removed" : "sent"];
    if (confirmed === false && action !== "move") fail(chatId, action === "remove" ? "That message had already left the queue" : QUEUE_FAILURES[action]);
  } catch (error) {
    fail(chatId, `${QUEUE_FAILURES[action]}: ${errorText(error)}`);
  }
}

/** Put the text of the most recent prompt back for another attempt. */
export function lastPrompt(): string | null {
  const messages = state.transcript?.messages ?? [];
  for (let i = messages.length - 1; i >= 0; i--) {
    const entry = messages[i]!;
    if (entry.role !== "user") continue;
    const text = entry.parts.flatMap((p) => (p.kind === "text" ? [p.text] : [])).join("\n");
    return text.trim() ? text : null;
  }
  return null;
}

export async function retry(prompt: string) {
  const key = draftKey();
  setDraft(key, { text: prompt.replace(/\n\nAttached images \(local files[^\n]*\):\n[\s\S]*$/, ""), mentions: [], skills: [] });
  await submit({ openModelPicker: () => {} });
}
