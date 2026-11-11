// A virtualized unified diff: file headers, notices, hunks and numbered lines.
// Only the visible rows exist in the DOM, so a large patch costs what fits on screen.
import { createVirtualizer } from "@tanstack/solid-virtual";
import { createMemo, createSignal, For, Show } from "solid-js";
import { Icon } from "@/ui/Icon";
import { IconButton } from "@/ui/primitives";
import { diffRows, type DiffRow, type FileDiff } from "./diff";
import { DiffStat, FileIcon, statusTone } from "./parts";
import { basename, dirname } from "./util";

const FILE_ROW = 34;
const LINE_ROW = 20;

export function DiffView(props: { files: FileDiff[]; wrap?: boolean; onOpenFile?: (path: string) => void; label: string }) {
  const [collapsed, setCollapsed] = createSignal<ReadonlySet<string>>(new Set());
  const rows = createMemo(() => diffRows(props.files, collapsed()));
  let scroller!: HTMLDivElement;

  const virtualizer = createVirtualizer({
    get count() {
      return rows().length;
    },
    getScrollElement: () => scroller,
    estimateSize: (index) => (rows()[index]?.kind === "file" ? FILE_ROW : LINE_ROW),
    overscan: 12,
    getItemKey: (index) => {
      const row = rows()[index];
      return row?.kind === "file" ? `f:${props.files[row.file]?.path}` : index;
    },
  });

  const toggle = (path: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  const allCollapsed = () => props.files.length > 0 && props.files.every((f) => collapsed().has(f.path));

  return (
    <div class="ws-diff">
      <div class="ws-diff-tools">
        <span class="ws-muted ws-grow">{props.label}</span>
        <IconButton
          icon="fold-vertical"
          active={allCollapsed()}
          label={allCollapsed() ? "Expand all files" : "Collapse all files"}
          size="sm"
          iconSize={14}
          onClick={() => setCollapsed(allCollapsed() ? new Set<string>() : new Set(props.files.map((f) => f.path)))}
        />
      </div>
      <div class="ws-diff-scroll" ref={scroller} data-wrap={props.wrap ? "true" : undefined} tabIndex={0} aria-label={props.label}>
        <div class="ws-diff-sizer" style={{ height: `${virtualizer.getTotalSize()}px` }}>
          <For each={virtualizer.getVirtualItems()}>
            {(item) => {
              const row = () => rows()[item.index];
              return (
                <Show when={row()}>
                  {(r) => (
                    <div
                      class="ws-diff-row"
                      data-index={item.index}
                      ref={(el) => queueMicrotask(() => el.isConnected && virtualizer.measureElement(el))}
                      style={{ transform: `translateY(${item.start}px)` }}
                    >
                      <Row row={r()} files={props.files} collapsed={collapsed()} onToggle={toggle} onOpenFile={props.onOpenFile} />
                    </div>
                  )}
                </Show>
              );
            }}
          </For>
        </div>
      </div>
    </div>
  );
}

function Row(props: { row: DiffRow; files: FileDiff[]; collapsed: ReadonlySet<string>; onToggle: (path: string) => void; onOpenFile?: (path: string) => void }) {
  const file = () => props.files[props.row.file]!;
  return (
    <>
      <Show when={props.row.kind === "file"}>
        <div class="ws-diff-file">
          <button
            type="button"
            class="ws-diff-file-toggle"
            aria-expanded={!props.collapsed.has(file().path)}
            aria-label={`${file().path}, ${props.collapsed.has(file().path) ? "expand" : "collapse"}`}
            onClick={() => props.onToggle(file().path)}
          >
            <Icon name={props.collapsed.has(file().path) ? "alt-arrow-right" : "alt-arrow-down"} size={11} class="ws-faint" />
            <FileIcon name={basename(file().path)} />
            <span class="ws-diff-file-name" data-tone={statusTone(file().status)}>{basename(file().path)}</span>
            <span class="ws-truncate ws-faint">{dirname(file().path)}</span>
          </button>
          <DiffStat additions={file().additions} deletions={file().deletions} />
          <Show when={props.onOpenFile && file().status !== "deleted"}>
            <IconButton icon="document" label={`Open ${basename(file().path)}`} size="sm" iconSize={14} onClick={() => props.onOpenFile?.(file().path)} />
          </Show>
        </div>
      </Show>
      <Show when={props.row.kind === "notice" && props.row}>
        {(r) => <div class="ws-diff-notice">{(r() as { text: string }).text}</div>}
      </Show>
      <Show when={props.row.kind === "hunk" && props.row}>
        {(r) => <div class="ws-diff-hunk">{(r() as { text: string }).text}</div>}
      </Show>
      <Show when={props.row.kind === "line" && props.row}>
        {(r) => {
          const line = () => (r() as Extract<DiffRow, { kind: "line" }>).line;
          return (
            <div class="ws-diff-line" data-kind={line().kind}>
              <span class="ws-diff-no">{line().oldNo ?? ""}</span>
              <span class="ws-diff-no">{line().newNo ?? ""}</span>
              <span class="ws-diff-mark" aria-hidden="true">{line().kind === "add" ? "+" : line().kind === "del" ? "−" : ""}</span>
              <span class="ws-diff-text">{line().text || " "}</span>
            </div>
          );
        }}
      </Show>
    </>
  );
}
