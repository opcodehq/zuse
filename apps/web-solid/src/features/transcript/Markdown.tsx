// Streamed markdown: settled blocks keep their DOM and only the open tail block re-renders, at most once per frame.
// Lists and tables render per item and cell, so a growing one rebuilds only the part that changed.
import { createEffect, createMemo, createSelector, createSignal, Index, Match, on, onCleanup, Show, Switch, untrack, type Accessor } from "solid-js";
import { Dynamic } from "solid-js/web";
import type { Tokens } from "marked";
import { Icon } from "@/ui/Icon";
import { blockHtml, IncrementalMarkdown, listItemHtml, tableCellHtml } from "./markdown";
import { highlight, paintWindowed, renderTokens, SPAN_LINES } from "./highlight";
import { copyText } from "./clipboard";

/** Tail renders costing more than this stretch the interval between renders. */
const BUDGET_MS = 6;

/** Runs `done` once the frame after now has been styled, laid out and painted. */
function afterPaint(done: () => void) {
  requestAnimationFrame(() => {
    const channel = new MessageChannel();
    channel.port1.onmessage = done;
    channel.port2.postMessage(null);
  });
}

/** Follows `source`, but while live spaces out updates so rendering takes at most a quarter of the time. */
function paced(source: Accessor<string>, live: Accessor<boolean>): Accessor<string> {
  const [value, setValue] = createSignal(untrack(source));
  let timer: ReturnType<typeof setTimeout> | undefined;
  let cost = 0;
  let last = 0;
  let disposed = false;
  const commit = (text: string) => {
    timer = undefined;
    const t0 = performance.now();
    setValue(text);
    // The DOM write is the cheap part; style, layout and paint of a long reply happen in the next frame.
    const script = performance.now() - t0;
    let frameStart = 0;
    requestAnimationFrame(() => (frameStart = performance.now()));
    afterPaint(() => {
      if (disposed) return;
      cost = script + (frameStart ? performance.now() - frameStart : 0);
      last = t0;
    });
  };
  createEffect(
    on(
      [source, live],
      ([text, isLive]) => {
        clearTimeout(timer);
        const wait = isLive && cost > BUDGET_MS ? last + cost * 4 - performance.now() : 0;
        if (wait <= 0) commit(text);
        else timer = setTimeout(() => commit(untrack(source)), wait);
      },
      { defer: true },
    ),
  );
  onCleanup(() => {
    disposed = true;
    clearTimeout(timer);
  });
  return value;
}

export function Markdown(props: { text: string; live: boolean; class?: string }) {
  const doc = new IncrementalMarkdown();
  // eslint-disable-next-line solid/reactivity -- paced reads both accessors inside an effect
  const text = paced(() => props.text, () => props.live);
  // The block list is updated in place, so every update must notify.
  const blocks = createMemo(() => doc.update(text()), undefined, { equals: false });
  // Only the tail block's `settled` flips, so a token never re-runs every code block.
  const isTail = createSelector(() => (props.live ? blocks().length - 1 : -1));
  return (
    <div class={`md ${props.live ? "md-live " : ""}${props.class ?? ""}`}>
      <Index each={blocks()}>
        {(block, ix) => {
          const settled = () => !isTail(ix);
          return (
            // eslint-disable-next-line solid/no-innerhtml -- blockHtml output is DOMPurify-sanitized
            <Switch fallback={<div class="md-block" innerHTML={blockHtml(block().token)} />}>
              <Match when={block().token.type === "code" && (block().token as Tokens.Code)}>
                {(code) => <CodeBlock code={code().text} lang={code().lang} settled={settled()} />}
              </Match>
              <Match when={block().token.type === "list" && (block().token as Tokens.List)}>{(list) => <MdList list={list()} />}</Match>
              <Match when={block().token.type === "table" && (block().token as Tokens.Table)}>{(table) => <MdTable table={table()} />}</Match>
            </Switch>
          );
        }}
      </Index>
    </div>
  );
}

/** Same markup as marked's list renderer. */
function MdList(props: { list: Tokens.List }) {
  const start = () => (props.list.ordered && props.list.start !== 1 && props.list.start !== "" ? props.list.start : undefined);
  return (
    <div class="md-block">
      <Dynamic component={props.list.ordered ? "ol" : "ul"} start={start()}>
        <Index each={props.list.items}>
          {(item) => {
            // Re-lexing makes new tokens every frame; only a changed source re-renders the item.
            const key = createMemo(() => `${props.list.loose ? "L" : "T"}${item().raw}`);
            const html = createMemo(on(key, () => listItemHtml(untrack(item))));
            // eslint-disable-next-line solid/no-innerhtml -- listItemHtml output is DOMPurify-sanitized
            return <li innerHTML={html()} />;
          }}
        </Index>
      </Dynamic>
    </div>
  );
}

/** Same markup as marked's table renderer, wrapped like `blockHtml` wraps tables. */
function MdTable(props: { table: Tokens.Table }) {
  return (
    <div class="md-block">
      <div class="md-table">
        <table>
          <thead>
            <tr>
              <Index each={props.table.header}>{(cell) => <Cell cell={cell()} header />}</Index>
            </tr>
          </thead>
          <Show when={props.table.rows.length}>
            <tbody>
              <Index each={props.table.rows}>
                {(row) => (
                  <tr>
                    <Index each={row()}>{(cell) => <Cell cell={cell()} header={false} />}</Index>
                  </tr>
                )}
              </Index>
            </tbody>
          </Show>
        </table>
      </div>
    </div>
  );
}

function Cell(props: { cell: Tokens.TableCell; header: boolean }) {
  const key = createMemo(() => props.cell.text);
  const html = createMemo(on(key, () => tableCellHtml(untrack(() => props.cell))));
  const align = () => props.cell.align ?? undefined;
  return (
    <Show
      when={props.header}
      // eslint-disable-next-line solid/no-innerhtml -- tableCellHtml output is DOMPurify-sanitized
      fallback={<td align={align()} innerHTML={html()} />}
    >
      {/* eslint-disable-next-line solid/no-innerhtml -- tableCellHtml output is DOMPurify-sanitized */}
      <th align={align()} innerHTML={html()} />
    </Show>
  );
}

export function CodeBlock(props: { code: string; lang?: string; settled: boolean }) {
  let pre!: HTMLElement;
  const [wrap, setWrap] = createSignal(false);
  const [copied, setCopied] = createSignal(false);
  const lang = () => props.lang?.trim().split(/\s+/)[0] || undefined;
  // Plain text first; colours replace it once the grammar has run on the settled block.
  createEffect(
    on([() => props.code, lang, () => props.settled], ([code, language, settled]) => {
      pre.textContent = code;
      if (!settled || !language) return;
      let current = true;
      let unpaint: (() => void) | undefined;
      onCleanup(() => {
        current = false;
        unpaint?.();
      });
      void highlight(code, language).then((tokens) => {
        if (!current || !tokens) return;
        if (code.split("\n", SPAN_LINES + 1).length <= SPAN_LINES) renderTokens(pre, code, tokens);
        else unpaint = paintWindowed(pre, code, tokens);
      });
    }),
  );
  let reset: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(reset));
  const copy = async () => {
    if (!(await copyText(props.code))) return;
    setCopied(true);
    clearTimeout(reset);
    reset = setTimeout(() => setCopied(false), 1500);
  };
  return (
    <div class="code-block">
      <div class="code-head">
        <span class="code-lang">{lang() ?? ""}</span>
        <button
          type="button"
          class="code-action"
          aria-pressed={wrap()}
          aria-label={wrap() ? "Use horizontal scrolling" : "Fit content"}
          title={wrap() ? "Use horizontal scrolling" : "Fit content"}
          onClick={() => setWrap(!wrap())}
        >
          <Icon name="wrap-text" size={13} />
        </button>
        <button type="button" class="code-action" aria-label="Copy code" title="Copy code" onClick={copy}>
          <Icon name={copied() ? "check" : "copy"} size={12} />
          <Show when={copied()}><span>Copied</span></Show>
        </button>
      </div>
      <pre class="code-body" data-wrap={wrap() ? "true" : undefined} tabIndex={0} aria-label={lang() ? `${lang()} code` : "Code"}>
        <code ref={pre} />
      </pre>
    </div>
  );
}
