import { Markdown } from "@/features/transcript/Markdown";
import { Icon } from "@/ui/Icon";
import { createMemo, createSignal, For, Show } from "solid-js";
import type { Zapper, ZapperFile } from "@/core/zapper-types";
import { Button, Dialog } from "@/ui/primitives";
import { command, commandState, loadResources, resources } from "./state";

export function Drive(props: { zapper: Zapper; history: boolean; path?: string; onOpen?: (path:string)=>void }) {
  const current = createMemo(() => {
    const latest = new Map<string, ZapperFile>();
    for (const f of resources.files)
      if ((latest.get(f.path)?.version ?? 0) < f.version) latest.set(f.path, f);
    return [...latest.values()].sort((a, b) => a.path.localeCompare(b.path));
  });
  const [edit, setEdit] = createSignal<{
    path: string;
    content: string;
    version: number;
  } | null>(null);
  const [restore, setRestore] = createSignal<ZapperFile | null>(null);
  const save = async (event: SubmitEvent) => {
    event.preventDefault();
    const f = edit();
    if (!f) return;
    try {
      await command(props.zapper.id, {
        type: "writeFile",
        path: f.path.trim(),
        content: f.content,
        expectedVersion: f.version,
      });
      setEdit(null);
      await loadResources(props.zapper);
    } catch {
      /* shared alert */
    }
  };
  const restoreVersion = async () => {
    const f = restore();
    if (!f) return;
    try {
      await command(props.zapper.id, {
        type: "restoreFile",
        path: f.path,
        version: f.version,
        expectedVersion: current().find((c) => c.path === f.path)?.version ?? 0,
      });
      setRestore(null);
      await loadResources(props.zapper);
    } catch {
      /* shared alert */
    }
  };
  return (
    <>
      <div class="zp-section-heading">
        <h3>{props.path ?? (props.history ? "File history" : "Drive")}</h3>
        <Show when={!props.history && !props.path}>
          <Button
            size="sm"
            onClick={() => setEdit({ path: "", content: "", version: 0 })}
          >
            New document
          </Button>
        </Show>
        <Button
          size="sm"
          disabled={resources.loading}
          onClick={() => void loadResources(props.zapper)}
        >
          Refresh
        </Button>
      </div>
      <Show when={resources.error}>
        <p class="zp-error" role="alert">
          {resources.error}
        </p>
      </Show>
      <For
        each={
          props.history
            ? resources.files
                .filter(
                  (f) =>
                    f.version <
                    (current().find((c) => c.path === f.path)?.version ?? 0),
                )
                .slice()
                .reverse()
            : current().filter(file => props.path ? file.path === props.path : (file.path !== "INSTRUCTIONS.md" || file.version > 1) && file.content.trim())
        }
        fallback={
          <p class="zp-muted">
            {resources.loading
              ? "Loading documents…"
              : props.history
                ? "No previous versions yet."
                : "No documents yet. Add notes and instructions for your Zapper."}
          </p>
        }
      >
        {(file) => (
          <div class="zp-card">
            <div class="zp-section-heading">
              <Show when={props.onOpen} fallback={<strong>{file.path}</strong>}><button type="button" class="zp-document-link" onClick={()=>props.onOpen?.(file.path)}><Icon name="document" size={15} />{file.path}</button></Show>
              <small>v{file.version}</small>
            </div>
            <Show when={props.path || props.history}><Show when={/\.md$/i.test(file.path)} fallback={<pre class="zp-file-preview">{file.content}</pre>}><Markdown text={file.content} live={false} /></Show></Show>
            <Show when={!props.onOpen}><Button size="sm" onClick={()=>props.history ? setRestore(file) : setEdit({...file})}>{props.history ? "Restore version" : "Edit document"}</Button></Show>
          </div>
        )}
      </For>
      <Show when={edit()}>
        {(file) => (
          <Dialog
            open
            onOpenChange={(open) => {
              if (!open) setEdit(null);
            }}
            title={file().version ? "Edit document" : "New document"}
          >
            <form class="zp-form" onSubmit={save}>
              <label>
                Path
                <input
                  required
                  disabled={!!edit()?.version}
                  value={edit()?.path ?? ""}
                  onInput={(e) =>
                    setEdit((f) => f && { ...f, path: e.currentTarget.value })
                  }
                  placeholder="notes/plan.md"
                />
              </label>
              <label>
                Content
                <textarea
                  rows={18}
                  value={edit()?.content ?? ""}
                  onInput={(e) =>
                    setEdit(
                      (f) => f && { ...f, content: e.currentTarget.value },
                    )
                  }
                />
              </label>
              <Show when={commandState.error}>
                <p role="alert" class="zp-error">
                  {commandState.error}
                </p>
              </Show>
              <Button
                type="submit"
                variant="primary"
                disabled={commandState.busy}
              >
                Save document
              </Button>
            </form>
          </Dialog>
        )}
      </Show>
      <Dialog
        open={!!restore()}
        onOpenChange={(open) => {
          if (!open) setRestore(null);
        }}
        title="Restore document version?"
        description="This creates a new current version. The existing history is preserved."
        footer={
          <Button
            disabled={commandState.busy}
            onClick={() => void restoreVersion()}
          >
            Restore version
          </Button>
        }
      />
    </>
  );
}
