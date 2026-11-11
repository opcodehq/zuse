// The prompt editor: an auto-growing textarea with `@` file and `/` command completion above the card.
import { createEffect, createMemo, createSignal, For, Match, on, onCleanup, onMount, Show, Switch } from "solid-js";
import { state } from "@/core/store";
import { view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { Glyph } from "./assets";
import { draftSpace, resolve, targetDeviceId } from "./catalog";
import { completionScope, searchFiles, slashCatalog, slashRows, tokenAt, type FileMatch, type SlashRow, type Token } from "./completion";
import { draftFor, setDraft } from "./drafts";
import { completeShortcode, emojiSuggestions, emojiTrigger, type EmojiTrigger } from "./emoji";

export interface EditorApi {
  focus(): void;
  textarea(): HTMLTextAreaElement | undefined;
}

export interface EditorProps {
  draftKey: string;
  placeholder: string;
  disabled?: boolean;
  onSubmit: (steer: boolean) => void;
  onModifiedSubmit: () => void;
  onKeyDown?: (event: KeyboardEvent) => boolean;
  onPasteFiles: (files: File[]) => void;
  onWorkspaceCommand: (row: Extract<SlashRow, { kind: "workspace" }>) => void;
  ref?: (api: EditorApi) => void;
}

type Completion =
  | { kind: "mention"; token: Token; loading: boolean; error?: string; rows: FileMatch[] }
  | { kind: "slash"; token: Token; loading: boolean; error?: string; rows: SlashRow[] };

export function Editor(props: EditorProps) {
  let textarea: HTMLTextAreaElement | undefined;
  let listbox: HTMLDivElement | undefined;
  const [caret, setCaret] = createSignal(0);
  const [completion, setCompletion] = createSignal<Completion | null>(null);
  const [active, setActive] = createSignal(0);
  const [dismissed, setDismissed] = createSignal<string | null>(null);
  const [focused, setFocused] = createSignal(false);
  const text = () => draftFor(props.draftKey).text;
  const listId = `composer-completion-${Math.random().toString(36).slice(2)}`;

  onMount(() => props.ref?.({ focus: () => textarea?.focus(), textarea: () => textarea }));

  const resize = () => {
    if (!textarea) return;
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, 226)}px`;
  };
  createEffect(on(text, () => queueMicrotask(resize)));

  const token = createMemo(() => (focused() ? tokenAt(text(), caret()) : null));
  let seq = 0;
  createEffect(
    on(token, (t) => {
      const id = ++seq;
      if (!t || dismissed() === `${t.start}:${t.kind}`) return setCompletion(null);
      setDismissed(null);
      const scope = completionScope(view.selectedChatId, draftSpace()?.id);
      const target = targetDeviceId();
      if (t.kind === "mention") {
        if (!scope) return setCompletion({ kind: "mention", token: t, loading: false, rows: [] });
        setCompletion((prev) => ({ kind: "mention", token: t, loading: true, rows: prev?.kind === "mention" ? prev.rows : [] }));
        const timer = setTimeout(() => {
          searchFiles(t.query, scope, target).then(
            (rows) => id === seq && setCompletion({ kind: "mention", token: t, loading: false, rows: rows.slice(0, 50) }),
            (error) => id === seq && setCompletion({ kind: "mention", token: t, loading: false, rows: [], error: error instanceof Error ? error.message : "File search failed" }),
          );
        }, 60);
        onCleanup(() => clearTimeout(timer));
      } else {
        const harness = resolve().harness;
        const inChat = !!view.selectedChatId;
        if (!harness) return setCompletion({ kind: "slash", token: t, loading: false, rows: slashRows([], [], inChat, t.query) });
        setCompletion((prev) => ({ kind: "slash", token: t, loading: true, rows: prev?.kind === "slash" ? prev.rows : slashRows([], [], inChat, t.query) }));
        slashCatalog(harness, scope, target).then((result) => {
          if (id !== seq) return;
          setCompletion({ kind: "slash", token: t, loading: false, error: result.error, rows: slashRows(result.commands, result.skills, inChat, t.query) });
        });
      }
      setActive(0);
    }),
  );

  // Emoji suggestions when no `@` or `/` token owns the caret.
  const [emojiActive, setEmojiActive] = createSignal(0);
  const [emojiDismissed, setEmojiDismissed] = createSignal<string | null>(null);
  const trigger = createMemo((): EmojiTrigger | null => {
    if (!focused() || token()) return null;
    const t = emojiTrigger(text(), caret());
    return t && emojiDismissed() !== `${t.start}:${t.query}` ? t : null;
  });
  const emojiRows = createMemo(() => {
    const t = trigger();
    return t ? emojiSuggestions(t.query) : [];
  });
  createEffect(on(emojiRows, () => setEmojiActive(0)));
  const acceptEmoji = (index = emojiActive()) => {
    const t = trigger();
    const row = emojiRows()[index];
    if (!t || !row) return;
    const { next, at } = replaceRange(t.start, t.end, row.emoji);
    setDraft(props.draftKey, { text: next });
    setCaretAfter(at);
  };

  const rowCount = () => completion()?.rows.length ?? 0;
  createEffect(on(active, (index) => listbox?.querySelector(`[data-index="${index}"]`)?.scrollIntoView({ block: "nearest" })));

  const replaceRange = (start: number, end: number, insert: string) => {
    const value = text();
    const next = value.slice(0, start) + insert + value.slice(end);
    const at = start + insert.length;
    return { next, at };
  };

  const accept = (index = active()) => {
    const c = completion();
    if (!c) return;
    const draft = draftFor(props.draftKey);
    if (c.kind === "mention") {
      const row = c.rows[index];
      if (!row) return;
      const token = `@${row.path}${row.isDir && !row.path.endsWith("/") ? "/" : ""}`;
      const { next, at } = replaceRange(c.token.start, c.token.end, `${token} `);
      setDraft(props.draftKey, { text: next, mentions: [...draft.mentions.filter((m) => m.token !== token), { token, path: row.path, isDir: row.isDir }] });
      setCaretAfter(at);
    } else {
      const row = c.rows[index];
      if (!row) return;
      if (row.kind === "workspace") {
        const { next, at } = replaceRange(c.token.start, c.token.end, "");
        setDraft(props.draftKey, { text: next.trimStart() === "" ? "" : next });
        setCaretAfter(Math.min(at, next.length));
        setCompletion(null);
        // After the caret returns, so a picker the command opens keeps focus.
        const run = props.onWorkspaceCommand;
        queueMicrotask(() => run(row));
        return;
      }
      if (row.kind === "skill") {
        const token = `$${row.name}`;
        const { next, at } = replaceRange(c.token.start, c.token.end, `${token} `);
        setDraft(props.draftKey, { text: next, skills: [...draft.skills.filter((s) => s.token !== token), { token, skill: row.skill }] });
        setCaretAfter(at);
      } else {
        const { next, at } = replaceRange(c.token.start, c.token.end, `/${row.name} `);
        setDraft(props.draftKey, { text: next });
        setCaretAfter(at);
      }
    }
    setCompletion(null);
  };

  const setCaretAfter = (at: number) =>
    queueMicrotask(() => {
      if (!textarea) return;
      textarea.focus();
      textarea.setSelectionRange(at, at);
      setCaret(at);
    });

  const syncCaret = () => textarea && setCaret(textarea.selectionStart ?? 0);

  const onKeyDown = (event: KeyboardEvent) => {
    const emojiCount = emojiRows().length;
    if (emojiCount && !event.isComposing) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        setEmojiActive((i) => (i + (event.key === "ArrowDown" ? 1 : -1) + emojiCount) % emojiCount);
        return;
      }
      if ((event.key === "Enter" || event.key === "Tab") && !event.shiftKey) {
        event.preventDefault();
        acceptEmoji();
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        const t = trigger();
        if (t) setEmojiDismissed(`${t.start}:${t.query}`);
        return;
      }
    }
    const c = completion();
    if (c && !event.isComposing) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        if (rowCount()) {
          event.preventDefault();
          setActive((i) => (i + (event.key === "ArrowDown" ? 1 : -1) + rowCount()) % rowCount());
        }
        return;
      }
      if ((event.key === "Enter" || event.key === "Tab") && !event.shiftKey && rowCount()) {
        event.preventDefault();
        accept();
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        setDismissed(`${c.token.start}:${c.kind}`);
        setCompletion(null);
        return;
      }
    }
    if (props.onKeyDown?.(event)) return;
    if (event.key === "Enter" && !event.isComposing && event.keyCode !== 229) {
      const mod = event.metaKey || event.ctrlKey;
      if (mod) {
        event.preventDefault();
        props.onModifiedSubmit();
      } else if (event.altKey) {
        event.preventDefault();
        props.onSubmit(false);
      } else if (!event.shiftKey) {
        event.preventDefault();
        props.onSubmit(false);
      }
    }
  };

  const onPaste = (event: ClipboardEvent) => {
    const files = Array.from(event.clipboardData?.files ?? []);
    if (files.length) {
      event.preventDefault();
      props.onPasteFiles(files);
    }
  };

  return (
    <div class="cmp-editor">
      <textarea
        ref={textarea}
        class="cmp-input"
        rows={1}
        value={text()}
        placeholder={props.placeholder}
        aria-label={props.placeholder}
        aria-autocomplete="list"
        aria-expanded={!!completion() || emojiRows().length > 0}
        aria-controls={emojiRows().length ? `${listId}-emoji` : completion() ? listId : undefined}
        aria-activedescendant={emojiRows().length ? `${listId}-emoji-${emojiActive()}` : completion() && rowCount() ? `${listId}-${active()}` : undefined}
        disabled={props.disabled}
        spellcheck
        onInput={(event) => {
          const input = event.currentTarget;
          const value = input.value;
          const caretAt = input.selectionStart ?? value.length;
          // Typing the closing colon of a known `:name:` swaps in the emoji.
          const done = (event as InputEvent).data === ":" ? completeShortcode(value, caretAt - 1) : null;
          if (done) {
            setDraft(props.draftKey, { text: value.slice(0, done.start) + done.emoji + value.slice(done.end) });
            setCaretAfter(done.start + done.emoji.length);
          } else setDraft(props.draftKey, { text: value });
          syncCaret();
          resize();
        }}
        onKeyDown={onKeyDown}
        onKeyUp={syncCaret}
        onClick={syncCaret}
        onSelect={syncCaret}
        onFocus={() => {
          setFocused(true);
          syncCaret();
        }}
        onBlur={() => setFocused(false)}
        onPaste={onPaste}
      />
      <Show when={emojiRows().length}>
        <div class="cmp-popup cmp-emoji" onMouseDown={(event) => event.preventDefault()}>
          <div class="cmp-popup-list" role="listbox" id={`${listId}-emoji`} aria-label="Emoji">
            <For each={emojiRows()}>
              {(row, index) => (
                <div
                  id={`${listId}-emoji-${index()}`}
                  class="cmp-row cmp-list-row"
                  role="option"
                  aria-selected={emojiActive() === index()}
                  onMouseMove={() => setEmojiActive(index())}
                  onClick={() => acceptEmoji(index())}
                >
                  <span class="cmp-emoji-glyph" aria-hidden="true">{row.emoji}</span>
                  <span class="cmp-emoji-name">:{row.name}:</span>
                </div>
              )}
            </For>
          </div>
        </div>
      </Show>
      <Show when={completion()}>
        {(c) => (
          <div class="cmp-popup" data-kind={c().kind} onMouseDown={(event) => event.preventDefault()}>
            <Switch>
              <Match when={c().loading && c().rows.length === 0}>
                <div class="cmp-skeleton" role="status" aria-label="Loading">
                  <span class="skeleton" /><span class="skeleton" /><span class="skeleton" />
                </div>
              </Match>
              <Match when={c().error && c().rows.length === 0}>
                <div class="cmp-popup-empty" data-tone="danger">{c().error}</div>
              </Match>
              <Match when={c().rows.length === 0}>
                <div class="cmp-popup-empty">
                  {c().kind === "mention"
                    ? c().token.query
                      ? "No matching files"
                      : "No files available"
                    : c().token.query
                      ? "No matching commands or skills"
                      : "No commands or skills available"}
                </div>
              </Match>
              <Match when={c().rows.length > 0}>
                <Show when={c().kind === "mention"}>
                  <div class="cmp-menu-heading">Files</div>
                </Show>
                <div class="cmp-popup-list" role="listbox" id={listId} ref={listbox} aria-label={c().kind === "mention" ? "Files" : "Commands and skills"}>
                  <Show when={c().kind === "mention"} fallback={<SlashList rows={(c() as Extract<Completion, { kind: "slash" }>).rows} active={active()} listId={listId} onHover={setActive} onPick={accept} />}>
                    <For each={(c() as Extract<Completion, { kind: "mention" }>).rows}>
                      {(row, index) => {
                        const slash = row.path.replace(/\/$/, "").lastIndexOf("/");
                        const name = slash >= 0 ? row.path.slice(slash + 1) : row.path;
                        const dir = slash >= 0 ? row.path.slice(0, slash) : "";
                        return (
                          <div
                            id={`${listId}-${index()}`}
                            class="cmp-row"
                            role="option"
                            data-index={index()}
                            aria-selected={active() === index()}
                            onMouseMove={() => setActive(index())}
                            onClick={() => accept(index())}
                          >
                            <Icon name={row.isDir ? "folder" : "document"} size={14} />
                            <span class="cmp-row-name">{name}</span>
                            <span class="cmp-row-detail">{dir}</span>
                          </div>
                        );
                      }}
                    </For>
                  </Show>
                </div>
              </Match>
            </Switch>
          </div>
        )}
      </Show>
    </div>
  );
}

function SlashList(props: { rows: SlashRow[]; active: number; listId: string; onHover: (index: number) => void; onPick: (index: number) => void }) {
  return (
    <For each={props.rows}>
      {(row, index) => (
        <div
          id={`${props.listId}-${index()}`}
          class="cmp-row cmp-slash-row"
          role="option"
          data-index={index()}
          aria-selected={props.active === index()}
          onMouseMove={() => props.onHover(index())}
          onClick={() => props.onPick(index())}
        >
          <Show when={row.kind === "skill"}>
            <Glyph name="skill" size={14} class="cmp-skill-icon" />
          </Show>
          <span class="cmp-slash-name">{row.kind === "skill" ? row.name : `/${row.name}`}</span>
          <span class="cmp-row-detail cmp-slash-desc">{row.kind === "command" && row.hint ? `${row.description} ${row.hint}`.trim() : row.description}</span>
        </div>
      )}
    </For>
  );
}

export const localDeviceName = () => (state.localDeviceId ? state.devices[state.localDeviceId]?.name : undefined);
