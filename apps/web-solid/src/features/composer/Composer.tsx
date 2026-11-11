// The chat composer: landing heading, context and queue trays, the editor card and its toolbar.
import { createEffect, createMemo, createSignal, on, onCleanup, onMount, Show } from "solid-js";
import { rpc } from "@/core/connection";
import { registerShortcut } from "@/core/keys";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import { navigate, view } from "@/core/view";
import { openShortcuts } from "@/features/extras/overlay";
import { Icon } from "@/ui/Icon";
import { catalog, draftSpace, resolve, selectedChat, setCatalog, spaceName } from "./catalog";
import { readableDraft } from "./completion";
import { AccessPicker, AddMenu, ContextRing, PlanChip, PluginsButton, SendKey, type SendMode } from "./Controls";
import { draftFor, focusComposer, setDraft } from "./drafts";
import { Editor, type EditorApi } from "./Editor";
import { cycleReasoning, ModelPill, openModelPicker, toggleFastMode } from "./ModelPicker";
import { cancelQueueEdit, commitQueueEdit, queueEdit } from "./queueEdit";
import { draftKey, hasContent, interrupt, runLive, runWorkspaceCommand, sendBlocked, stageFiles, submit } from "./send";
import { ContextRow } from "./Targets";
import { activateFirstQueued, TrayPill, TrayRows } from "./Tray";
import { composer } from "./drafts";
import { ComposerList, composerFlash, composerList, listKeyDown, mac, searchHistory, showFlash, stashDraft, StashButton, stashKeys } from "./Lists";
import "./composer.css";

const NARROW = 440;
const ATTACHMENT_TRAILER = /\n\nAttached images \(local files[^\n]*\):\n[\s\S]*$/;

/** Text of each prompt this chat has sent, oldest first, for Up/Down recall. */
function sentPrompts(): string[] {
  const out: string[] = [];
  for (const entry of state.transcript?.messages ?? []) {
    if (entry.role !== "user") continue;
    const text = entry.parts.flatMap((p) => (p.kind === "text" ? [p.text] : [])).join("\n").replace(ATTACHMENT_TRAILER, "");
    if (text.trim()) out.push(text);
  }
  return out;
}

export function Composer() {
  const landing = () => !view.selectedChatId;
  return (
    <div class="cmp-root" data-landing={landing()}>
      <Show when={selectedChat()?.archived} fallback={<Live />}>
        <ArchivedFooter />
      </Show>
    </div>
  );
}

const startWithoutProject = () => {
  setCatalog("draft", "noProject", true);
  focusComposer();
};

function Landing() {
  const space = () => draftSpace();
  const onboarding = () => state.workspaceSynced && Object.keys(state.spaces).length === 0 && !catalog.draft.noProject;
  return (
    <div class="cmp-landing">
      <Show
        when={!onboarding()}
        fallback={
          <div class="cmp-onboarding">
            <Icon name="zuse-logo" size={48} class="cmp-onboarding-logo" />
            <h1 class="cmp-onboarding-title">Add a project to get started</h1>
            <p class="cmp-landing-sub">A project is a folder on one of your devices.</p>
            <button type="button" class="btn" data-variant="primary" onClick={() => navigate({ kind: "newProject" })}>Add a project</button>
            <button type="button" class="cmp-link" onClick={startWithoutProject}>Start without a project</button>
          </div>
        }
      >
        <h1 class="cmp-heading">{space() ? `What should we build in ${spaceName(space()!)}?` : "What should we build today?"}</h1>
        <Show when={space()}>
          <button type="button" class="cmp-link" onClick={startWithoutProject}>Start without a project</button>
        </Show>
      </Show>
    </div>
  );
}

function Live() {
  let editor: EditorApi | undefined;
  let fileInput: HTMLInputElement | undefined;
  let card: HTMLDivElement | undefined;
  const [narrow, setNarrow] = createSignal(false);
  const [dragging, setDragging] = createSignal(false);
  const key = () => draftKey();
  const r = createMemo(resolve);
  const live = () => runLive();
  const editing = () => !!queueEdit() && queueEdit()!.chatId === view.selectedChatId;
  const mode = (): SendMode => (editing() ? "send" : live() && !hasContent() ? "stop" : live() ? "queue" : "send");
  const sendLabel = () => (editing() ? "Save to queue" : mode() === "queue" ? "Queue message" : "Send message");

  // Up/Down recall of sent prompts while the draft is empty or still a recalled prompt.
  let recall: { index: number; text: string } | null = null;
  createEffect(on(() => view.selectedChatId, () => (recall = null)));

  const send = (steer = false) => {
    // An open stash or search owns Enter; the send key must not send its query.
    if (composerList()) return;
    if (editing()) return void commitQueueEdit();
    recall = null;
    void submit({ steer, openModelPicker });
  };

  let lastEscape = 0;
  const onKeyDown = (event: KeyboardEvent): boolean => {
    const textarea = editor?.textarea();
    const text = draftFor(key()).text;
    if (listKeyDown(event, key())) return true;
    const mod = mac ? event.metaKey : event.ctrlKey;
    if (mod && !event.shiftKey && !event.altKey && event.key.toLowerCase() === "s") {
      event.preventDefault();
      if (!editing()) stashDraft(key());
      return true;
    }
    if (mac ? event.ctrlKey && !event.metaKey && event.code === "KeyR" : event.altKey && !event.ctrlKey && event.code === "KeyR") {
      event.preventDefault();
      if (!editing()) searchHistory(key());
      return true;
    }
    // Option on macOS turns letters into symbols, so the global Alt shortcuts never match there.
    if (mac && event.altKey && !event.metaKey && !event.ctrlKey) {
      const run = { KeyP: openModelPicker, KeyT: cycleReasoning, KeyO: toggleFastMode }[event.code];
      if (run) {
        event.preventDefault();
        run();
        return true;
      }
    }
    if (event.key === "Escape") {
      if (editing()) {
        event.preventDefault();
        cancelQueueEdit();
        return true;
      }
      const now = Date.now();
      const again = now - lastEscape < 600;
      if (live()) {
        event.preventDefault();
        if (again) {
          lastEscape = 0;
          void interrupt();
        } else lastEscape = now;
        return true;
      }
      // Escape twice on an idle draft clears it.
      if (text.trim()) {
        event.preventDefault();
        if (again) {
          lastEscape = 0;
          recall = null;
          setDraft(key(), { text: "", mentions: [], skills: [] });
        } else {
          lastEscape = now;
          showFlash("escape");
        }
        return true;
      }
      return false;
    }
    if (event.key === "?" && !text && !event.isComposing) {
      event.preventDefault();
      openShortcuts();
      return true;
    }
    if ((event.key === "ArrowUp" || event.key === "ArrowDown") && !event.shiftKey && !event.altKey && !event.metaKey && !event.ctrlKey && textarea) {
      const recalling = recall !== null && recall.text === text;
      const caret = textarea.selectionStart;
      const firstLine = !text.slice(0, caret).includes("\n");
      const lastLine = !text.slice(textarea.selectionEnd).includes("\n");
      if (editing() || (text && !recalling)) return false;
      if (event.key === "ArrowUp" ? !firstLine : !lastLine || !recalling) return false;
      const prompts = sentPrompts();
      if (!prompts.length) return false;
      const from = recalling ? recall!.index : prompts.length;
      const index = from + (event.key === "ArrowUp" ? -1 : 1);
      if (index < 0) return true;
      event.preventDefault();
      if (index >= prompts.length) {
        recall = null;
        setDraft(key(), { text: "", mentions: [], skills: [] });
        return true;
      }
      const draft = readableDraft(prompts[index]!);
      recall = { index, text: draft.text };
      setDraft(key(), draft);
      queueMicrotask(() => textarea.setSelectionRange(0, 0));
      return true;
    }
    return false;
  };

  const modifiedSubmit = () => {
    if (hasContent()) send(true);
    else activateFirstQueued();
  };

  onMount(() => {
    const disposers = [
      registerShortcut({ keys: "alt+p", label: "Switch model", inInputs: true, run: openModelPicker }),
      registerShortcut({ keys: "alt+t", label: "Change reasoning effort", inInputs: true, run: cycleReasoning }),
      registerShortcut({ keys: "alt+o", label: "Toggle fast mode", inInputs: true, run: toggleFastMode }),
    ];
    onCleanup(() => disposers.forEach((dispose) => dispose()));
    if (card) {
      const observer = new ResizeObserver(([entry]) => setNarrow((entry?.contentRect.width ?? NARROW) < NARROW));
      observer.observe(card);
      onCleanup(() => observer.disconnect());
    }
  });
  // Opening a chat or the new-chat canvas puts the caret in the composer.
  createEffect(on(() => view.selectedChatId, () => queueMicrotask(() => editor?.focus())));

  const hasFiles = (event: DragEvent) => Array.from(event.dataTransfer?.types ?? []).includes("Files");
  let dragDepth = 0;
  const dropHandlers = {
    onDragEnter: (event: DragEvent) => {
      if (!hasFiles(event)) return;
      event.preventDefault();
      dragDepth++;
      setDragging(true);
    },
    onDragOver: (event: DragEvent) => {
      if (!hasFiles(event)) return;
      event.preventDefault();
      if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
    },
    onDragLeave: () => {
      dragDepth = Math.max(0, dragDepth - 1);
      if (!dragDepth) setDragging(false);
    },
    onDrop: (event: DragEvent) => {
      if (!hasFiles(event)) return;
      event.preventDefault();
      dragDepth = 0;
      setDragging(false);
      const files = Array.from(event.dataTransfer?.files ?? []);
      if (files.length) stageFiles(files, key());
      editor?.focus();
    },
  };

  const blocked = () => {
    const reason = sendBlocked();
    return reason !== null || (editing() && !!queueEdit()?.finishing);
  };

  return (
    <>
      <Show when={!view.selectedChatId}>
        <Landing />
      </Show>
      <div class="cmp-column" {...dropHandlers}>
        <Show
          when={view.selectedChatId}
          fallback={
            <div class="cmp-context">
              <div class="cmp-tray-rows">
                <Show when={r().noAgents}>
                  <TrayPill tone="warning" icon="terminal" title="No agents available" subtitle="Enable an installed agent in Settings → Providers, or install an agent CLI.">
                    <button type="button" class="cmp-text-action" onClick={() => navigate({ kind: "settings", section: "providers" })}>Settings</button>
                  </TrayPill>
                </Show>
                <TrayRows />
              </div>
              <ContextRow />
            </div>
          }
        >
          <div class="cmp-tray">
            <TrayRows />
          </div>
        </Show>
        <div class="cmp-surface">
          <ComposerList draftKey={key()} />
          <Show when={composerFlash() && !composerList()}>
            <div class="cmp-flash-wrap">
              <div class="cmp-popover cmp-flash" role="status">
                <Show when={composerFlash() === "escape"} fallback={<>Draft stashed · <kbd class="cmp-keycap">{stashKeys}</kbd> to view</>}>
                  Press <kbd class="cmp-keycap">Esc</kbd> again to clear
                </Show>
              </div>
            </div>
          </Show>
          <div
            ref={card}
            class="cmp-card"
            data-drop={dragging()}
            data-drop-label="Drop files to attach"
            onMouseDown={(event) => {
              // Padding and the toolbar's empty space belong to the text field.
              const target = event.target as HTMLElement;
              if (target === event.currentTarget || target.classList.contains("cmp-toolbar") || target.classList.contains("cmp-editor")) {
                event.preventDefault();
                editor?.focus();
              }
            }}
          >
            <Editor
              draftKey={key()}
              placeholder={editing() ? "Edit queued message" : composerList()?.kind === "history" ? "Search sent messages…" : "Do anything…"}
              onSubmit={(steer) => send(steer)}
              onModifiedSubmit={modifiedSubmit}
              onKeyDown={onKeyDown}
              onPasteFiles={(files) => stageFiles(files, key())}
              onWorkspaceCommand={(row) => runWorkspaceCommand(row.command, openModelPicker)}
              ref={(api) => (editor = api)}
            />
            <div class="cmp-toolbar">
              <div class="cmp-toolbar-lead">
                <AddMenu onAttach={() => fileInput?.click()} />
                <AccessPicker narrow={narrow()} />
                <PlanChip narrow={narrow()} />
                <PluginsButton />
              </div>
              <div class="cmp-toolbar-trail">
                <StashButton draftKey={key()} />
                <ModelPill />
              </div>
              <div class="cmp-toolbar-end">
                <ContextRing />
                <SendKey
                  mode={mode()}
                  blocked={blocked()}
                  empty={!editing() && !hasContent()}
                  stopping={!!view.selectedChatId && !!composer.interrupting[view.selectedChatId]}
                  label={sendLabel()}
                  onSend={() => send(false)}
                  onStop={() => void interrupt()}
                />
              </div>
            </div>
          </div>
        </div>
        <input
          ref={fileInput}
          type="file"
          accept="image/*"
          multiple
          hidden
          aria-hidden="true"
          tabIndex={-1}
          onChange={(event) => {
            const files = Array.from(event.currentTarget.files ?? []);
            event.currentTarget.value = "";
            if (files.length) stageFiles(files, key());
            editor?.focus();
          }}
        />
      </div>
    </>
  );
}

function ArchivedFooter() {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const unarchive = async () => {
    const chat = selectedChat();
    if (!chat) return;
    setBusy(true);
    setError(null);
    try {
      await rpc.call(M.MUTATE, { op: "setChatArchived", chatId: chat.id, archived: false, targetDeviceId: chat.deviceId });
    } catch (e) {
      setError(`Unarchive failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div class="cmp-archived" role="status">
      <Icon name="archive-minimalistic" size={16} style={{ color: "var(--text-muted)" }} />
      <div class="cmp-archived-text">
        <span>This chat is archived.</span>
        <Show when={error()}><span class="cmp-archived-error">{error()}</span></Show>
      </div>
      <button type="button" aria-label="Unarchive thread" disabled={busy() || state.connection.kind !== "connected"} onClick={() => void unarchive()}>
        {busy() ? "Unarchiving…" : "Unarchive"}
      </button>
    </div>
  );
}
