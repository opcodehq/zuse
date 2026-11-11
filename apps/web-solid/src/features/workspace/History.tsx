// Git history: a virtualized commit list with the lane graph, refs, search and paging.
// Rows, graph metrics and colours follow crates/ui/src/history.rs; a commit opens its own diff in Changes.
import { createVirtualizer } from "@tanstack/solid-virtual";
import { batch, createEffect, createMemo, createSignal, For, Index, on, onCleanup, Show } from "solid-js";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import { Icon } from "@/ui/Icon";
import { Button, IconButton, Menu, Spinner } from "@/ui/primitives";
import { reviewCommit } from "./Changes";
import { layoutGraph, type GraphRow } from "./graph";
import { Banner, Empty } from "./parts";
import { readErrorText, type Target } from "./store";
import type { GitHistoryCommit, GitHistoryPage, GitHistoryRef } from "./types";
import { copyText, timeAgo } from "./util";

const PAGE_SIZE = 100;
const ROW_HEIGHT = 36;
const LANE = 12;
const NODE_RADIUS = 3;
const HEAD_RING = 2;
const STROKE = 1.5;
const GRAPH_PAD = 8;
const GRAPH_MAX = 120;
const SEARCH_DEBOUNCE = 70;

interface PageMeta {
  headSha: string | null;
  nextCursor: number | null;
  totalCount: number | null;
  comparison: GitHistoryPage["comparison"];
}

export function History(props: { target: Target; active: boolean }) {
  const [commits, setCommits] = createSignal<GitHistoryCommit[]>([]);
  const [meta, setMeta] = createSignal<PageMeta | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [query, setQuery] = createSignal("");
  const [applied, setApplied] = createSignal("");
  const [selected, setSelected] = createSignal<string | null>(null);
  let seq = 0;
  let scroller!: HTMLDivElement;

  const connected = () => state.connection.kind === "connected";
  const graph = createMemo(() => layoutGraph(commits(), meta()?.headSha));
  const graphWidth = () => Math.min(GRAPH_MAX, Math.max(1, graph().maxLaneCount) * LANE + GRAPH_PAD);

  const load = (append: boolean) => {
    const cwd = props.target.cwd;
    if (!cwd) return;
    if (!connected()) {
      setError("Reconnect to load history.");
      return;
    }
    const cursor = append ? meta()?.nextCursor : 0;
    if (append && (cursor === null || cursor === undefined || loading())) return;
    const id = ++seq;
    const search = applied();
    setLoading(true);
    setError(null);
    const params: Record<string, unknown> = { cwd, chatId: props.target.chatId, cursor, limit: PAGE_SIZE, targetDeviceId: props.target.deviceId };
    if (search) params.query = search;
    rpc
      .call<GitHistoryPage>(search ? M.SEARCH_GIT_HISTORY : M.LIST_GIT_HISTORY, params)
      .then(
        (page) => {
          if (id !== seq) return;
          const previous = meta();
          if (append && previous && previous.headSha !== (page.headSha ?? null)) {
            setError("History changed while loading. Showing the refreshed history.");
            queueMicrotask(() => load(false));
            return;
          }
          batch(() => {
            const seen = new Set(append ? commits().map((c) => c.sha) : []);
            const fresh = page.commits.filter((c) => !seen.has(c.sha) && (seen.add(c.sha), true));
            setCommits(append ? [...commits(), ...fresh] : fresh);
            setMeta({
              headSha: page.headSha ?? null,
              nextCursor: page.nextCursor ?? null,
              totalCount: page.totalCount ?? (append ? previous?.totalCount ?? null : null),
              comparison: page.comparison ?? (append ? previous?.comparison : null) ?? null,
            });
          });
        },
        (e) => id === seq && setError(readErrorText(e, "Couldn't read Git history.", true)),
      )
      .finally(() => id === seq && setLoading(false));
  };

  // Reload on open, on a new search, and when the connection returns.
  createEffect(
    on([() => props.active, applied, () => state.epoch, connected, () => props.target.cwd], ([active, , , live]) => {
      if (active && live) load(false);
    }),
  );

  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  createEffect(
    on(
      query,
      (q) => {
        clearTimeout(searchTimer);
        searchTimer = setTimeout(() => setApplied(q.trim()), SEARCH_DEBOUNCE);
      },
      { defer: true },
    ),
  );
  onCleanup(() => clearTimeout(searchTimer));
  // A new search starts from its first match, not wherever the old list was scrolled.
  createEffect(on(applied, () => scroller?.scrollTo({ top: 0 }), { defer: true }));

  const virtualizer = createVirtualizer({
    get count() {
      return commits().length;
    },
    getScrollElement: () => scroller,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
    getItemKey: (index) => commits()[index]?.sha ?? index,
  });

  // Page in as the list nears its end.
  createEffect(() => {
    const items = virtualizer.getVirtualItems();
    const last = items[items.length - 1];
    if (last && last.index >= commits().length - 20 && meta()?.nextCursor != null && !loading()) load(true);
  });

  const open = (commit: GitHistoryCommit) => {
    setSelected(commit.sha);
    reviewCommit(props.target.chatId, commit.sha, commit.subject);
  };

  const onKeyDown = (event: KeyboardEvent) => {
    const list = commits();
    if (!list.length) return;
    const index = list.findIndex((c) => c.sha === selected());
    let next = index;
    if (event.key === "ArrowDown") next = Math.min(list.length - 1, index + 1);
    else if (event.key === "ArrowUp") next = Math.max(0, index - 1);
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = list.length - 1;
    else if (event.key === "Enter" && index >= 0) {
      event.preventDefault();
      open(list[index]!);
      return;
    } else return;
    event.preventDefault();
    const commit = list[next];
    if (!commit) return;
    setSelected(commit.sha);
    virtualizer.scrollToIndex(next, { align: "auto" });
    queueMicrotask(() => scroller.querySelector<HTMLElement>(`[data-sha="${commit.sha}"]`)?.focus());
  };

  const count = () => meta()?.totalCount ?? commits().length;

  return (
    <div class="ws-surface-col ws-history">
      <div class="ws-panel-header">
        <h2 class="ws-panel-title">{props.target.archived ? "Saved history" : "History"}</h2>
        <Show when={meta()}>
          <span class="ws-faint ws-small">{count().toLocaleString()} {count() === 1 ? "commit" : "commits"}</span>
        </Show>
        <div class="ws-grow" />
        <div class="ws-search ws-history-search">
          <Icon name="magnifer" size={14} class="ws-faint" />
          <input
            class="ws-search-input"
            type="search"
            placeholder="Search commits"
            aria-label="Search commits"
            value={query()}
            onInput={(e) => setQuery(e.currentTarget.value)}
            onKeyDown={(e) => e.key === "Escape" && setQuery("")}
          />
        </div>
        <Show when={loading()} fallback={<IconButton icon="refresh" label="Refresh history" size="sm" iconSize={14} onClick={() => load(false)} />}>
          <span class="ws-icon-slot"><Spinner label="Loading history" /></span>
        </Show>
      </div>
      <Show when={meta()?.comparison}>
        {(c) => (
          <div class="ws-history-compare ws-small ws-muted" title={`Compared with ${c().base}`}>
            <span aria-label={`${c().ahead} ahead`}>↑{c().ahead}</span>
            <span aria-label={`${c().behind} behind`}>↓{c().behind}</span>
            <span class="ws-truncate">· {c().base}</span>
          </div>
        )}
      </Show>
      <Show when={error()}>
        <Banner tone="danger" actions={<Button size="sm" variant="ghost" onClick={() => load(false)}>Retry</Button>}>{error()}</Banner>
      </Show>
      <Show when={props.target.cwd} fallback={<Empty icon="history" title="No checkout" detail="Open a project or thread to see its history." />}>
        <Show when={commits().length || loading()} fallback={<Show when={meta()}><Empty icon="history" title={applied() ? "No matching commits" : "No commits to show"} /></Show>}>
          <div class="ws-history-list" ref={scroller} role="listbox" aria-label="Commits" tabIndex={-1} onKeyDown={onKeyDown}>
            <div class="ws-tree-sizer" style={{ height: `${virtualizer.getTotalSize()}px` }}>
              <For each={virtualizer.getVirtualItems()}>
                {(item) => {
                  const commit = () => commits()[item.index];
                  const row = () => graph().rows[item.index];
                  return (
                    <Show when={commit()}>
                      {(c) => (
                        <div class="ws-tree-slot" style={{ transform: `translateY(${item.start}px)`, height: `${ROW_HEIGHT}px` }}>
                          <CommitRow
                            commit={c()}
                            row={row()}
                            width={graphWidth()}
                            selected={selected() === c().sha}
                            focusable={selected() === c().sha || (!selected() && item.index === 0)}
                            onOpen={() => open(c())}
                          />
                        </div>
                      )}
                    </Show>
                  );
                }}
              </For>
            </div>
            <Show when={loading() && !commits().length}>
              <div class="ws-empty"><Spinner label="Loading history" /></div>
            </Show>
          </div>
        </Show>
      </Show>
    </div>
  );
}

function CommitRow(props: { commit: GitHistoryCommit; row: GraphRow | undefined; width: number; selected: boolean; focusable: boolean; onOpen: () => void }) {
  const refs = () => props.commit.refs.slice(0, 3);
  const extra = () => props.commit.refs.length - refs().length;
  const short = () => props.commit.sha.slice(0, 7);
  return (
    <div
      class="ws-commit"
      role="option"
      aria-selected={props.selected}
      tabIndex={props.focusable ? 0 : -1}
      data-sha={props.commit.sha}
      aria-label={`${props.commit.subject}, ${props.commit.authorName}, ${timeAgo(props.commit.authoredAt)}`}
      onClick={() => props.onOpen()}
      title={`${props.commit.subject}\n${props.commit.authorName} <${props.commit.authorEmail}>\n${new Date(props.commit.authoredAt).toLocaleString()}\n${props.commit.sha}`}
    >
      <GraphCell row={props.row} width={props.width} />
      <div class="ws-commit-main">
        <div class="ws-commit-line">
          <span class="ws-commit-subject ws-truncate" data-head={props.row?.isHead ? "true" : undefined}>{props.commit.subject || short()}</span>
          <For each={refs()}>{(ref) => <RefBadge reference={ref} />}</For>
          <Show when={extra() > 0}>
            <span class="ws-ref" data-kind="more">+{extra()}</span>
          </Show>
        </div>
        <div class="ws-commit-meta ws-faint">
          <span class="ws-truncate">{props.commit.authorName}</span>
          <span>· {timeAgo(props.commit.authoredAt)}</span>
        </div>
      </div>
      <span class="ws-commit-sha ws-mono ws-faint">{short()}</span>
      <div class="ws-commit-actions" onClick={(e) => e.stopPropagation()}>
        <Menu
          placement="bottom-end"
          trigger={(p) => (
            <button {...p} type="button" class="icon-btn" data-size="sm" aria-label={`Actions for ${short()}`} title="Commit actions" tabIndex={-1}>
              <Icon name="more-horizontal" size={14} />
            </button>
          )}
          items={[
            { label: "View changes", icon: "git-compare", onSelect: () => props.onOpen() },
            { label: "Copy commit SHA", icon: "copy", onSelect: () => void copyText(props.commit.sha, "Commit SHA copied") },
            { label: "Copy subject", icon: "copy", onSelect: () => void copyText(props.commit.subject, "Subject copied") },
          ]}
        />
      </div>
    </div>
  );
}

function RefBadge(props: { reference: GitHistoryRef }) {
  const icon = () => (props.reference.kind === "tag" ? "tag" : props.reference.kind === "remote" ? "cloud" : "git-branch");
  const kind = () => (props.reference.kind === "tag" ? "Tag" : props.reference.kind === "remote" ? "Remote branch" : "Branch");
  return (
    <span class="ws-ref" data-kind={props.reference.kind} title={`${kind()}: ${props.reference.label}`}>
      <Icon name={icon()} size={10} />
      <span class="ws-truncate">{props.reference.label}</span>
    </span>
  );
}

const laneX = (lane: number) => GRAPH_PAD / 2 + LANE / 2 + lane * LANE;

function GraphCell(props: { row: GraphRow | undefined; width: number }) {
  const mid = ROW_HEIGHT / 2;
  // A lane change bends over the row's half; straight lanes stay straight.
  const path = (x1: number, y1: number, x2: number, y2: number) => {
    if (x1 === x2) return `M${x1} ${y1}L${x2} ${y2}`;
    const k = (y2 - y1) / 2;
    return `M${x1} ${y1}C${x1} ${y1 + k} ${x2} ${y2 - k} ${x2} ${y2}`;
  };
  return (
    <svg class="ws-graph" width={props.width} height={ROW_HEIGHT} viewBox={`0 0 ${props.width} ${ROW_HEIGHT}`} aria-hidden="true">
      <Show when={props.row}>
        {(row) => (
          <>
            <Index each={row().segments}>
              {(segment) => {
                const d = () => {
                  const s = segment();
                  if (s.shape === "through") return path(laneX(s.fromLane), 0, laneX(s.toLane), ROW_HEIGHT);
                  if (s.shape === "incoming") return path(laneX(s.fromLane), 0, laneX(s.toLane), mid);
                  return path(laneX(s.fromLane), mid, laneX(s.toLane), ROW_HEIGHT);
                };
                return <path d={d()} fill="none" stroke-width={STROKE} stroke-linecap="round" class="ws-graph-line" data-color={segment().colorId % 6} />;
              }}
            </Index>
            <Show when={row().isHead}>
              <circle cx={laneX(row().nodeLane)} cy={mid} r={NODE_RADIUS + HEAD_RING} fill="none" stroke-width={STROKE} class="ws-graph-line" data-color={row().nodeColorId % 6} />
            </Show>
            <circle cx={laneX(row().nodeLane)} cy={mid} r={NODE_RADIUS} class="ws-graph-node" data-color={row().nodeColorId % 6} />
          </>
        )}
      </Show>
    </svg>
  );
}
