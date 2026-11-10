// Tool groups: summary header, activity rail, chips with folding detail (desktop `render_tool_group`).
import { createMemo, createSignal, For, Index, type JSX, Match, Show, Switch } from "solid-js";
import { Icon } from "@/ui/Icon";
import { diffFile, type FileDiff } from "./diff";
import { frameThrottled } from "./frame";
import { Markdown } from "./Markdown";
import {
  agentStatus,
  callBlock,
  callOf,
  chipDiffStats,
  chipLabel,
  fileBadgeName,
  fileIcon,
  filePathOf,
  formatKb,
  FULL_OUTPUT_MAX_LINES,
  groupCollapses,
  harnessIcon,
  isAgentItem,
  outputDetail,
  type OutputDetail,
  protocolGroupSummary,
  type ReasoningPart,
  type TextPart,
  type TodoItem,
  type ToolDiff,
  type ToolDiffStat,
  toolGroupSummary,
  toolIcon,
  type ToolItem,
  type ToolPart,
  turnSummary,
} from "./model";
import { blobState, fetchBlob, isOpen, toggleOpen } from "./resources";

export interface GroupContext {
  harness: () => string | undefined;
}

/** A run of tool calls and thoughts. Ordinary groups fold behind a summary; lone agents and thinking stay open. */
export function ToolGroup(props: { id: string; tools: ToolItem[]; live: boolean; ctx: GroupContext }) {
  const collapses = () => groupCollapses(props.tools);
  const agentGroup = () => props.tools.length > 1 && props.tools.every(isAgentItem);
  const liveAgents = () => props.tools.some((t) => isAgentItem(t) && agentStatus(t.part as ToolPart) === "running");
  const open = () => !collapses() || isOpen(props.id, liveAgents());
  const summary = createMemo(() => toolGroupSummary(props.tools, props.live));
  const description = () =>
    protocolGroupSummary(props.tools.filter((t) => t.kind === "call").map((t) => ({ call: callOf(t.part as ToolPart), isError: !!(t.part as ToolPart).isError })));
  const bodyId = () => `${props.id}-body`;
  return (
    <div class="tool-group">
      <Show when={collapses()}>
        <button
          type="button"
          class="tool-group-head"
          aria-expanded={open()}
          aria-controls={bodyId()}
          title={description()}
          onClick={() => toggleOpen(props.id, liveAgents())}
        >
          <Show when={agentGroup()}>
            <AvatarStack count={props.tools.length} icon={harnessIcon(props.ctx.harness())} />
          </Show>
          <span class="tool-group-title" classList={{ shimmer: summary().running || liveAgents() }}>{summary().text}</span>
        </button>
      </Show>
      <Show when={open()}>
        <div id={bodyId()} class="tool-chips" data-rail={collapses() ? "true" : undefined}>
          <Index each={props.tools}>
            {(tool, ix) => (
              <ToolRow
                groupId={props.id}
                item={tool()}
                index={ix}
                last={ix === props.tools.length - 1}
                rail={collapses()}
                live={props.live}
                ctx={props.ctx}
              />
            )}
          </Index>
        </div>
      </Show>
    </div>
  );
}

/** A settled turn's single fold header ("12 tool calls · 3 messages") and, when open, its work. */
export function FoldHeader(props: { id: string; tools: ToolItem[]; live: boolean; children: JSX.Element }) {
  const open = () => isOpen(props.id);
  const icons = createMemo(() => {
    const seen: string[] = [];
    for (const tool of props.tools) {
      if (tool.kind !== "call") continue;
      const icon = toolIcon(tool);
      if (!seen.includes(icon)) seen.push(icon);
      if (seen.length === 5) break;
    }
    return seen;
  });
  const text = () => (props.live ? toolGroupSummary(props.tools, true).text : turnSummary(props.tools));
  return (
    <div class="tool-fold">
      <button type="button" class="tool-group-head" aria-expanded={open()} onClick={() => toggleOpen(props.id)}>
        <span class="tool-group-title" classList={{ shimmer: props.live && props.tools.some((t) => !(t.part as ToolPart).resolved) }}>{text()}</span>
        <For each={icons()}>{(icon) => <Icon name={icon} size={13} class="tool-fold-icon" />}</For>
      </button>
      <Show when={open()}>
        <div class="tool-fold-body">{props.children}</div>
      </Show>
    </div>
  );
}

function AvatarStack(props: { count: number; icon: string }) {
  return (
    <span class="avatar-stack" aria-hidden="true">
      <For each={Array.from({ length: Math.min(3, props.count) })}>
        {() => <span class="avatar"><Icon name={props.icon} size={12} /></span>}
      </For>
    </span>
  );
}

type Detail =
  | { kind: "output"; detail: OutputDetail }
  | { kind: "diff"; file: FileDiff }
  | { kind: "stats"; stats: ToolDiffStat[] }
  | { kind: "todo"; items: TodoItem[] }
  | { kind: "text"; part: ReasoningPart | TextPart };

function ToolRow(props: {
  groupId: string;
  item: ToolItem;
  index: number;
  last: boolean;
  rail: boolean;
  live: boolean;
  ctx: GroupContext;
}) {
  const key = () => `${props.groupId}#d${props.item.key}`;
  const call = () => (props.item.kind === "call" ? (props.item.part as ToolPart) : null);
  const isAgent = () => isAgentItem(props.item);
  const textPart = () => (props.item.kind === "call" ? null : (props.item.part as ReasoningPart | TextPart));
  const streaming = () => props.live && props.item.kind !== "call" && props.last;
  const text = frameThrottled(() => textPart()?.text ?? "", streaming);
  const [shownRef, setShownRef] = createSignal<string | null>(null);

  const invocation = createMemo(() => {
    const part = call();
    if (!part || isAgent()) return null;
    const c = callOf(part);
    return c.kind === "todo" ? null : callBlock(c);
  });

  const detail = createMemo((): Detail | null => {
    const part = call();
    if (!part) return textPart() ? { kind: "text", part: textPart()! } : null;
    if (isAgent()) return null;
    const c = callOf(part);
    if (c.kind === "todo" && c.items?.length) return { kind: "todo", items: c.items };
    const ref = shownRef();
    const ready = ref ? blobState(ref) : undefined;
    if (ref && ready?.status === "ready") {
      if (ref.endsWith(".diff")) {
        try {
          const diff = JSON.parse(ready.text) as ToolDiff;
          return { kind: "diff", file: diffFile(diff.path, diff.oldText, diff.newText) };
        } catch {
          // Not a diff document after all; show it as text.
        }
      }
      const full = outputDetail(ready.text, FULL_OUTPUT_MAX_LINES);
      if (full) return { kind: "output", detail: full };
    }
    if (part.diff) {
      const diff = part.diff as ToolDiff;
      return { kind: "diff", file: diffFile(diff.path, diff.oldText, diff.newText) };
    }
    const stats = part.diffStats as ToolDiffStat[] | undefined;
    if (stats?.length) return { kind: "stats", stats };
    const output = outputDetail(part.output);
    return output ? { kind: "output", detail: output } : null;
  });

  const affordance = createMemo(() => {
    const part = call();
    // A plan shows its items; its output is only the tool's acknowledgement.
    if (!part || isAgent() || callOf(part).kind === "todo") return null;
    const candidates: [string | undefined, string, number | undefined][] = [
      [part.diffRef, "diff", undefined],
      [part.outputRef, "output", part.outputBytes],
    ];
    for (const [ref, what, bytes] of candidates) {
      if (!ref) continue;
      const blob = blobState(ref);
      if (blob?.status === "ready" && shownRef() === ref) continue;
      const label =
        blob?.status === "loading" ? `Loading full ${what}…`
        : blob?.status === "failed" ? `Couldn't load full ${what} — tap to retry`
        : blob?.status === "ready" ? `Show full ${what}`
        : bytes ? `Show full ${what} (${formatKb(bytes)})` : `Show full ${what}`;
      return { ref, label, loading: blob?.status === "loading" };
    }
    return null;
  });

  const expandable = () => !!(detail() || invocation());
  // A live thought opens by default so its text streams in view.
  const open = () => expandable() && isOpen(key(), props.item.kind === "thought" && streaming());
  const flatThought = () => !props.rail && props.item.kind === "thought";
  const carded = () => props.item.kind === "call" && !isAgent();
  const panelId = () => `${key()}-panel`;

  const showFull = (ref: string) => {
    setShownRef(ref);
    void fetchBlob(ref);
  };

  return (
    <div class="tool-row" data-rail={props.rail ? "true" : undefined} data-first={props.index === 0 ? "true" : undefined} data-last={props.last ? "true" : undefined}>
      <Show when={props.rail}>
        <span class="tool-rail" aria-hidden="true">
          <Icon name={toolIcon(props.item)} size={14} class="tool-rail-icon" classList={{ error: !!call()?.isError }} />
        </span>
      </Show>
      <div class="tool-card">
        <Show
          when={!isAgent()}
          fallback={<SubagentHeader part={call()!} harness={props.ctx.harness()} />}
        >
          <Show
            when={expandable()}
            fallback={<div class="chip-head"><ChipHeader item={props.item} text={text} live={props.live} flat={flatThought()} open={false} trail={false} /></div>}
          >
            <button type="button" class="chip-head" aria-expanded={open()} aria-controls={panelId()} onClick={() => toggleOpen(key(), props.item.kind === "thought" && streaming())}>
              <ChipHeader item={props.item} text={text} live={props.live} flat={flatThought()} open={open()} trail />
            </button>
          </Show>
        </Show>
        <Show when={open()}>
          <div id={panelId()} class="chip-panel" data-carded={carded() ? "true" : undefined} data-flat={flatThought() ? "true" : undefined}>
            <Show when={invocation()}>{(lines) => <OutputLines detail={lines()} />}</Show>
            <Show when={detail()}>
              {(d) => (
                <>
                  <Show when={invocation()}><div class="chip-sep" /></Show>
                  <DetailBody detail={d()} text={text} live={streaming()} />
                </>
              )}
            </Show>
            <Show when={affordance()}>
              {(a) => (
                <button type="button" class="chip-more" disabled={a().loading} onClick={() => showFull(a().ref)}>
                  {a().label}
                </button>
              )}
            </Show>
          </div>
        </Show>
      </div>
    </div>
  );
}

function ChipHeader(props: { item: ToolItem; text: () => string; live: boolean; flat: boolean; open: boolean; trail: boolean }) {
  const part = () => (props.item.kind === "call" ? (props.item.part as ToolPart) : null);
  const label = createMemo(() => chipLabel(props.item, props.text));
  const path = () => {
    const p = part();
    return p ? filePathOf(callOf(p)) : null;
  };
  const codeDetail = () => {
    const p = part();
    const kind = p ? callOf(p).kind : null;
    return kind === "exec" || kind === "mcp";
  };
  const stats = () => {
    const p = part();
    return p ? chipDiffStats(p) : null;
  };
  const pending = () => props.live && (part() ? !part()!.resolved : false);
  return (
    <>
      <Show when={props.flat}><Icon name="tool-thinking" size={14} class="chip-flat-icon" /></Show>
      <span class="chip-label" classList={{ shimmer: pending() }}>{label()[0]}</span>
      <Show when={part()?.isError}><span class="chip-error">Error</span></Show>
      <Switch fallback={<Show when={label()[1]}><span class="chip-detail" classList={{ italic: props.item.kind === "thought" }}>{label()[1]}</span></Show>}>
        <Match when={path()}>
          {(p) => (
            <span class="chip-file" title={p()}>
              <Icon name={fileIcon(p())} size={14} />
              <span class="truncate">{fileBadgeName(p())}</span>
            </span>
          )}
        </Match>
        <Match when={codeDetail() && label()[1]}>
          <span class="chip-code">{label()[1]}</span>
        </Match>
      </Switch>
      <Show when={stats()}>
        {(s) => (
          <span class="chip-stats">
            <Show when={s()[0] > 0}><span class="add">+{s()[0]}</span></Show>
            <Show when={s()[1] > 0}><span class="del">−{s()[1]}</span></Show>
          </span>
        )}
      </Show>
      <Show when={props.trail}>
        <span class="chip-trail" data-open={props.open ? "true" : undefined} aria-hidden="true">
          <Icon name={props.open ? "alt-arrow-down" : "alt-arrow-right"} size={12} />
        </span>
      </Show>
    </>
  );
}

function SubagentHeader(props: { part: ToolPart; harness: string | undefined }) {
  const status = () => agentStatus(props.part);
  const call = () => callOf(props.part);
  const title = () => chipLabel({ key: props.part.id, kind: "call", part: props.part }, () => "")[1] || "Subagent";
  const model = () => {
    const c = call();
    const input = c.kind === "unknown" || c.kind === "mcp" ? (c.input as Record<string, unknown> | undefined) : undefined;
    for (const key of ["model", "modelId", "model_id", "subagent_model"]) {
      const value = input?.[key];
      if (typeof value === "string" && value.trim()) return value.trim();
    }
    return null;
  };
  return (
    <div class="chip-head subagent">
      <span class="subagent-status" data-status={status()}>
        <Switch>
          <Match when={status() === "running"}><span class="subagent-live" role="img" aria-label="Subagent running" /></Match>
          <Match when={status() === "done"}><Icon name="check-circle" size={14} label="Done" /></Match>
          <Match when={status() === "failed"}><Icon name="close-circle" size={14} label="Failed" /></Match>
        </Switch>
      </span>
      <Icon name="tool-agent" size={14} class="subagent-icon" />
      <span class="chip-label strong truncate">{title()}</span>
      <Show when={status() === "running"}><span class="chip-detail">Agent running</span></Show>
      <Show when={model()}><span class="chip-detail">{model()}</span></Show>
    </div>
  );
}

function DetailBody(props: { detail: Detail; text: () => string; live: boolean }) {
  return (
    <Switch>
      <Match when={props.detail.kind === "output" && props.detail}>{(d) => <OutputLines detail={d().detail} />}</Match>
      <Match when={props.detail.kind === "diff" && props.detail}>{(d) => <DiffView file={d().file} />}</Match>
      <Match when={props.detail.kind === "stats" && props.detail}>
        {(d) => (
          <ul class="chip-stat-list">
            <For each={d().stats}>
              {(stat) => (
                <li>
                  <Icon name={fileIcon(stat.path)} size={14} />
                  <span class="truncate path">{stat.path}</span>
                  <span class="add">+{stat.additions}</span>
                  <span class="del">−{stat.deletions}</span>
                </li>
              )}
            </For>
          </ul>
        )}
      </Match>
      <Match when={props.detail.kind === "todo" && props.detail}>
        {(d) => (
          <ul class="todo-list" aria-label="Plan">
            <For each={d().items}>
              {(item) => {
                const status = () => (item.done || item.status === "completed" ? "done" : item.status === "inProgress" || item.status === "in_progress" ? "active" : "pending");
                return (
                  <li data-status={status()}>
                    <span class="todo-mark" aria-label={status() === "done" ? "Completed" : status() === "active" ? "In progress" : "Pending"} role="img" />
                    <span>{item.text}</span>
                  </li>
                );
              }}
            </For>
          </ul>
        )}
      </Match>
      <Match when={props.detail.kind === "text"}>
        <Markdown text={props.text()} live={props.live} class="md-thought" />
      </Match>
    </Switch>
  );
}

function OutputLines(props: { detail: OutputDetail }) {
  return (
    <div class="chip-output">
      <pre tabIndex={0}>{props.detail.lines.join("\n")}</pre>
      <Show when={props.detail.truncatedBy > 0}>
        <div class="chip-more-lines">… {props.detail.truncatedBy} more lines</div>
      </Show>
    </div>
  );
}

export function DiffView(props: { file: FileDiff }) {
  return (
    <div class="diff" role="table" aria-label={`Changes to ${props.file.path}`}>
      <For each={props.file.hunks}>
        {(hunk) => (
          <div role="rowgroup">
            <div class="diff-hunk" role="row">{hunk.header}</div>
            <For each={hunk.lines}>
              {(line) => (
                <div class="diff-line" data-kind={line.kind} role="row">
                  <span class="diff-no">{line.oldNo ?? ""}</span>
                  <span class="diff-no">{line.newNo ?? ""}</span>
                  <span class="diff-sign" aria-label={line.kind === "add" ? "added" : line.kind === "del" ? "removed" : undefined}>
                    {line.kind === "add" ? "+" : line.kind === "del" ? "−" : " "}
                  </span>
                  <span class="diff-text">{line.text}</span>
                </div>
              )}
            </For>
          </div>
        )}
      </For>
      <Show when={props.file.truncatedBy > 0}>
        <div class="chip-more-lines">… {props.file.truncatedBy} more lines</div>
      </Show>
    </div>
  );
}
