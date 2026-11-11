// The Actions section of the session info card and its editor (crates/ui/src/shell/session_info.rs, actions_ui.rs).
import { createMemo, createSignal, For, Show } from "solid-js";
import { Icon } from "@/ui/Icon";
import { Button, Dialog } from "@/ui/primitives";
import {
  ACTION_ICONS,
  actionGlyph,
  actionsContext,
  actionsStatus,
  canRun,
  closeEditor,
  deleteEditorAction,
  editor,
  openEditor,
  retryActions,
  runAction,
  saveEditor,
  snapshotOf,
  updateEditor,
} from "./actions";

/**
 * The selected chat's project actions: a header with the count, a row per action (click runs it,
 * the gear edits it), imports from zuse.json, then Add action. Renders nothing outside a project
 * or when the host does not support actions.
 */
export function ProjectActions(props: {
  onRun?: () => void;
  /** Desktop's environment summary starts the section folded and remembers the fold while the app runs. */
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  const context = createMemo(() => actionsContext());
  const status = () => actionsStatus(context());
  const visible = () => !!context() && status()?.state !== "unsupported";
  const snapshot = () => snapshotOf(status());
  const actions = () => snapshot()?.actions ?? [];
  const [open, setOpen] = createSignal(props.defaultOpen ?? true);
  return (
    <Show when={visible()}>
      <section class="pj-actions" aria-label="Project actions">
        <button
          type="button"
          class="pj-actions-head"
          aria-expanded={open()}
          aria-label={actions().length ? `Actions, ${actions().length}` : "Actions"}
          onClick={() => {
            setOpen((v) => !v);
            props.onOpenChange?.(open());
          }}
        >
          <Icon name="action-play" size={16} class="pj-glyph" />
          <span>Actions</span>
          <span class="pj-actions-trail">
            <Show when={actions().length}>
              <span class="pj-actions-count">{actions().length}</span>
            </Show>
            <Icon name="alt-arrow-right" size={12} class="pj-actions-chevron" />
          </span>
        </button>
        <Show when={open()}>
          <Show when={status()?.state === "loading" || !status()}>
            <div class="pj-action-note">Loading actions…</div>
          </Show>
          <Show when={status()?.state === "unavailable" && (status() as { message: string }).message}>
            {(message) => (
              <>
                <div class="pj-action-note" role="alert">
                  {message()}
                </div>
                <button type="button" class="pj-action-link" onClick={() => retryActions()}>
                  Retry
                </button>
              </>
            )}
          </Show>
          <Show when={status()?.state !== "loading" && status()}>
            <For each={actions()}>
              {(action) => (
                <div class="pj-action-row">
                  <button
                    type="button"
                    class="pj-action-run"
                    disabled={!canRun(status())}
                    aria-label={`Run ${action.name}`}
                    title={action.command}
                    onClick={() => {
                      props.onRun?.();
                      void runAction(action);
                    }}
                  >
                    <Icon name={actionGlyph(action.icon)} size={16} class="pj-glyph" />
                    <span class="pj-action-name">{action.runOnWorktreeCreate ? `${action.name} (setup)` : action.name}</span>
                  </button>
                  <button
                    type="button"
                    class="pj-action-edit"
                    aria-label={`Edit action ${action.name}`}
                    title="Edit action"
                    onClick={() => openEditor(action)}
                  >
                    <Icon name="settings-minimalistic" size={14} />
                  </button>
                </div>
              )}
            </For>
            <For each={snapshot()?.importableActions ?? []}>
              {(draft) => (
                <div class="pj-action-row">
                  <button type="button" class="pj-action-run" aria-label={`Import ${draft.name}`} onClick={() => openEditor(undefined, draft)}>
                    <Icon name={actionGlyph(draft.icon)} size={16} class="pj-glyph" />
                    <span class="pj-action-name">{draft.name}</span>
                    <span class="pj-action-import">Import</span>
                  </button>
                </div>
              )}
            </For>
            <Show when={snapshot()?.projectFileIssue}>{(issue) => <div class="pj-action-note">{issue()}</div>}</Show>
            <button type="button" class="pj-action-link" onClick={() => openEditor()}>
              Add action
            </button>
          </Show>
        </Show>
      </section>
      <ActionEditor />
    </Show>
  );
}

/** Add, edit and delete one action. Mounted with the section; also usable on its own. */
export function ActionEditor() {
  const current = () => editor();
  return (
    <>
      <Dialog
        open={!!current() && !current()!.confirmDelete}
        onOpenChange={(open) => !open && !current()?.saving && closeEditor()}
        title={current()?.actionId ? "Edit action" : "Add action"}
      >
        <Show when={current()}>
          {(draft) => (
            <form
              class="pj-editor"
              onSubmit={(event) => {
                event.preventDefault();
                void saveEditor();
              }}
            >
              <label class="pj-field-label" for="pj-action-name">
                Name
              </label>
              <input
                id="pj-action-name"
                class="pj-field"
                value={draft().name}
                placeholder="Action name"
                autofocus
                disabled={draft().saving}
                onInput={(e) => updateEditor({ name: e.currentTarget.value, error: null })}
              />
              <label class="pj-field-label" for="pj-action-command">
                Command
              </label>
              <textarea
                id="pj-action-command"
                class="pj-field pj-field-command"
                value={draft().command}
                placeholder="Command"
                spellcheck={false}
                disabled={draft().saving}
                onInput={(e) => updateEditor({ command: e.currentTarget.value, error: null })}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                    e.preventDefault();
                    void saveEditor();
                  }
                }}
              />
              <span class="pj-field-label" id="pj-action-icon-label">
                Icon
              </span>
              <div class="pj-icons" role="radiogroup" aria-labelledby="pj-action-icon-label">
                <For each={ACTION_ICONS}>
                  {(choice) => (
                    <button
                      type="button"
                      role="radio"
                      class="pj-icon-choice"
                      aria-checked={draft().icon === choice.icon}
                      aria-label={choice.label}
                      title={choice.label}
                      onClick={() => updateEditor({ icon: choice.icon })}
                    >
                      <Icon name={actionGlyph(choice.icon)} size={16} />
                    </button>
                  )}
                </For>
              </div>
              <button
                type="button"
                role="checkbox"
                class="pj-check"
                aria-checked={draft().runOnWorktreeCreate}
                onClick={() => updateEditor({ runOnWorktreeCreate: !draft().runOnWorktreeCreate })}
              >
                <span class="pj-check-box">
                  <Show when={draft().runOnWorktreeCreate}>
                    <Icon name="check" size={12} />
                  </Show>
                </span>
                Run automatically on worktree creation
              </button>
              <Show when={draft().error}>
                <div class="pj-editor-error" role="alert">
                  {draft().error}
                </div>
              </Show>
              <div class="pj-editor-footer">
                <div>
                  <Show when={draft().actionId}>
                    <Button variant="danger" disabled={draft().saving} onClick={() => updateEditor({ confirmDelete: true })}>
                      Delete action
                    </Button>
                  </Show>
                </div>
                <div>
                  <Button variant="ghost" onClick={() => closeEditor()}>
                    Cancel
                  </Button>
                  <Button type="submit" variant="primary" disabled={draft().saving}>
                    {draft().saving ? "Saving…" : "Save action"}
                  </Button>
                </div>
              </div>
            </form>
          )}
        </Show>
      </Dialog>
      <Dialog
        open={!!current()?.confirmDelete}
        onOpenChange={(open) => !open && !current()?.saving && updateEditor({ confirmDelete: false })}
        title="Delete action?"
        description={`“${current()?.name.trim() ?? ""}” will be permanently deleted.`}
        footer={
          <>
            <Button variant="ghost" onClick={() => updateEditor({ confirmDelete: false })}>
              Cancel
            </Button>
            <Button variant="danger" disabled={current()?.saving} onClick={() => void deleteEditorAction()}>
              {current()?.saving ? "Deleting…" : "Delete"}
            </Button>
          </>
        }
      />
    </>
  );
}
