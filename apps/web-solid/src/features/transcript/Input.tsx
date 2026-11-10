// The docked answer surface for the agent's open request: a permission card, a question tray or a secret prompt.
// Desktop shows these in place of the composer; here they dock at the transcript's foot, above the composer.
import { createEffect, createMemo, createSignal, For, on, onCleanup, Show } from "solid-js";
import { createStore } from "solid-js/store";
import type { MessagePart } from "@/core/protocol";
import { state } from "@/core/store";
import { Icon } from "@/ui/Icon";
import { isAnswered, respondInput, submitSecret } from "./actions";
import { Markdown } from "./Markdown";

type InputPart = Extract<MessagePart, { kind: "input" }>;

type Action = { kind: "fileWrite"; path: string } | { kind: "command"; command: string } | { kind: "network"; url: string } | { kind: "other"; tool: string; summary: string };

interface Question {
  id: string;
  header: string;
  question: string;
  options: string[];
  multiSelect?: boolean;
  prefill?: string | null;
  multiline?: boolean;
  approvalPrompt?: { persistentAllowed?: boolean; action: Action; allowLabel: string; denyLabel: string } | null;
  approval?: { tool: string; input: unknown; allowLabel: string; denyLabel: string } | null;
}

const ALLOW_CHAT = "Always allow in this chat";
const ALLOW_PROJECT = "Always allow in project";
const SECRET = "private-secret:";

/** The newest unresolved request in the open chat's transcript. */
export function pendingInput(): InputPart | null {
  const messages = state.transcript?.messages ?? [];
  for (let i = messages.length - 1; i >= Math.max(0, messages.length - 12); i--) {
    const parts = messages[i]!.parts;
    for (let j = parts.length - 1; j >= 0; j--) {
      const part = parts[j]!;
      if (part.kind === "input" && !part.resolved && !isAnswered(part.requestId)) return part;
    }
  }
  return null;
}

export function InputDock(props: { chatId: string }) {
  const part = createMemo(pendingInput);
  return (
    <Show when={part()} keyed>
      {(p) => <InputPanel chatId={props.chatId} part={p} />}
    </Show>
  );
}

function InputPanel(props: { chatId: string; part: InputPart }) {
  const questions = () => props.part.questions as Question[];
  const [page, setPage] = createSignal(0);
  const [picked, setPicked] = createStore<number[][]>(questions().map(() => []));
  const [typed, setTyped] = createStore<string[]>(questions().map((q) => q.prefill ?? ""));
  const current = () => questions()[page()];
  const finish = () => {
    const answers = questions().map((q, ix) => {
      const text = q.multiline ? typed[ix] ?? "" : (typed[ix] ?? "").trim();
      return { questionId: q.id, labels: text || q.multiline ? [text] : (picked[ix] ?? []).map((p) => q.options[p]!).filter(Boolean) };
    });
    void respondInput(props.chatId, props.part.requestId, answers);
  };
  const advance = () => (page() + 1 < questions().length ? setPage(page() + 1) : finish());
  const answerWith = (label: string) => {
    const q = current();
    if (!q) return;
    const ix = q.options.indexOf(label);
    if (ix < 0) return;
    setPicked(page(), [ix]);
    setTyped(page(), "");
    advance();
  };
  return (
    <div class="t-dock">
      <Show when={current()}>
        {(q) => (
          <Show
            when={!q().approvalPrompt}
            fallback={<Permission question={q()} remaining={questions().length - page()} onAnswer={answerWith} />}
          >
            <Show
              when={!q().id.startsWith(SECRET)}
              fallback={<Secret question={q()} onSubmit={(value, decline) => void submitSecret(props.chatId, props.part.requestId, q().id.slice(SECRET.length), value, decline)} />}
            >
              <QuestionTray
                question={q()}
                page={page()}
                count={questions().length}
                picked={picked[page()] ?? []}
                typed={typed[page()] ?? ""}
                onPick={(ix) => {
                  if (q().multiSelect) {
                    const list = picked[page()] ?? [];
                    setPicked(page(), list.includes(ix) ? list.filter((p) => p !== ix) : [...list, ix]);
                  } else {
                    setPicked(page(), [ix]);
                    if (!typed[page()]?.trim() && !q().multiline) advance();
                  }
                }}
                onType={(text) => setTyped(page(), text)}
                onBack={() => setPage(Math.max(0, page() - 1))}
                onSubmit={advance}
              />
            </Show>
          </Show>
        )}
      </Show>
    </div>
  );
}

function headline(question: Question): [string, string] {
  const action = question.approvalPrompt!.action;
  switch (action.kind) {
    case "fileWrite": return ["Write file?", action.path];
    case "command": return ["Run shell command?", action.command];
    case "network": return ["Make network request?", action.url];
    default: return [question.header, question.question];
  }
}

const isPlanTool = (tool: string | undefined) => !!tool && /exitplanmode|exit_plan/i.test(tool);

function Permission(props: { question: Question; remaining: number; onAnswer: (label: string) => void }) {
  let card!: HTMLDivElement;
  const prompt = () => props.question.approvalPrompt!;
  const text = () => (plan() ? ["Approve plan?", ""] as const : headline(props.question));
  const has = (label: string) => !!prompt().persistentAllowed && props.question.options.includes(label);
  // A plan approval carries the plan; show it rendered so the user approves what they read. The wire keeps
  // only the provider's preview (`…Allow this action?\n\n{json}`), so read the plan out of that.
  const plan = () => {
    const action = prompt().action;
    const tool = props.question.approval?.tool ?? (action.kind === "other" ? action.tool : undefined);
    if (!isPlanTool(tool)) return null;
    const input = props.question.approval?.input as { plan?: unknown; input?: { plan?: unknown } } | undefined;
    const direct = input?.plan ?? input?.input?.plan;
    if (typeof direct === "string") return direct;
    const preview = props.question.question.slice(props.question.question.indexOf("\n\n") + 2);
    try {
      const parsed = JSON.parse(preview) as { plan?: unknown };
      return typeof parsed.plan === "string" ? parsed.plan : null;
    } catch {
      return null;
    }
  };
  // Desktop permission_prompt: take focus only from the composer (or nowhere), never from the terminal, an editor or settings.
  createEffect(
    on(
      () => props.question.id,
      () => {
        const focused = document.activeElement;
        if (focused && card.contains(focused)) return;
        if (!focused || focused === document.body || focused.closest(".cmp-input")) card.focus();
      },
    ),
  );
  // Answered: the card unmounts with focus inside it, so hand focus back to the composer instead of the page.
  onCleanup(() => {
    if (!card.contains(document.activeElement)) return;
    queueMicrotask(() => {
      if (document.activeElement && document.activeElement !== document.body) return;
      document.querySelector<HTMLElement>(".cmp-input")?.focus();
    });
  });
  const onKey = (event: KeyboardEvent) => {
    if (event.key === "Escape") props.onAnswer(prompt().denyLabel);
    else if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) props.onAnswer(prompt().allowLabel);
    else return;
    event.preventDefault();
    event.stopPropagation();
  };
  return (
    <div ref={card} class="t-permission" role="group" aria-label={text()[0]} tabIndex={-1} onKeyDown={onKey}>
      <div class="t-permission-head">
        <span class="t-permission-title">{text()[0]}</span>
        <Show when={props.remaining > 1}><span class="t-badge">+{props.remaining - 1} more</span></Show>
      </div>
      <Show when={plan()} fallback={<div class="t-permission-detail" tabIndex={0}>{text()[1]}</div>}>
        {(markdown) => (
          <div class="t-permission-plan" tabIndex={0} aria-label="Plan">
            <Markdown text={markdown()} live={false} />
          </div>
        )}
      </Show>
      <Show when={!has(ALLOW_CHAT) && !has(ALLOW_PROJECT)}>
        <div class="t-permission-note">Only “Allow once” is available for this request.</div>
      </Show>
      <div class="t-permission-actions">
        <button type="button" class="t-ghost" aria-label="Deny this action" title="Escape" onClick={() => props.onAnswer(prompt().denyLabel)}>Deny</button>
        <button type="button" class="t-ghost" disabled={!has(ALLOW_CHAT)} onClick={() => props.onAnswer(ALLOW_CHAT)}>Allow for this chat</button>
        <button type="button" class="t-ghost" aria-label="Always allow in this project" disabled={!has(ALLOW_PROJECT)} onClick={() => props.onAnswer(ALLOW_PROJECT)}>Always allow</button>
        <button type="button" class="t-primary" title="⌘ / Ctrl + Enter" onClick={() => props.onAnswer(prompt().allowLabel)}>Allow once</button>
      </div>
    </div>
  );
}

function QuestionTray(props: {
  question: Question;
  page: number;
  count: number;
  picked: number[];
  typed: string;
  onPick: (ix: number) => void;
  onType: (text: string) => void;
  onBack: () => void;
  onSubmit: () => void;
}) {
  let editor!: HTMLTextAreaElement;
  createEffect(on(() => props.page, () => editor.focus()));
  const canSubmit = () => !!props.typed.trim() || props.picked.length > 0 || !!props.question.multiline;
  const last = () => props.page + 1 === props.count;
  const onKey = (event: KeyboardEvent) => {
    if (event.key === "Enter" && !event.shiftKey && !props.question.multiline) {
      event.preventDefault();
      if (canSubmit()) props.onSubmit();
    }
  };
  // Desktop `on_wizard_key`: a bare digit picks while the answer is empty, Escape steps back a page.
  const onPanelKey = (event: KeyboardEvent) => {
    const inInput = event.target === editor;
    const empty = !props.typed;
    const modified = event.metaKey || event.ctrlKey || event.altKey;
    if (/^[1-9]$/.test(event.key) && !modified && (!inInput || empty)) {
      if (Number(event.key) > props.question.options.length) return;
      event.preventDefault();
      props.onPick(Number(event.key) - 1);
    } else if (event.key === "Escape" && (!inInput || empty)) {
      event.preventDefault();
      props.onBack();
    }
  };
  const titleId = () => `question-${props.question.id}`;
  return (
    <div class="t-question" role="group" aria-labelledby={titleId()} onKeyDown={onPanelKey}>
      <div class="t-question-tray">
        <div class="t-question-head">
          <div id={titleId()} class="t-question-text">{props.question.question}</div>
          <Show when={props.count > 1}>
            <div class="t-question-pager">
              <button type="button" class="t-ghost t-icon" aria-label="Previous question" disabled={props.page === 0} onClick={() => props.onBack()}>
                <Icon name="alt-arrow-left" size={12} />
              </button>
              <span>{props.page + 1} of {props.count}</span>
            </div>
          </Show>
        </div>
        <Show when={props.question.multiSelect}>
          <div class="t-question-hint">Choose all that apply</div>
        </Show>
        <Show when={props.question.options.length}>
          <div class="t-question-options" role={props.question.multiSelect ? "group" : "radiogroup"} aria-labelledby={titleId()}>
            <For each={props.question.options}>
              {(option, ix) => {
                const on = () => props.picked.includes(ix());
                return (
                  <button
                    type="button"
                    class="t-option"
                    role={props.question.multiSelect ? "checkbox" : "radio"}
                    aria-checked={on()}
                    data-picked={on() ? "true" : undefined}
                    onClick={() => props.onPick(ix())}
                  >
                    <span class="t-option-key" aria-hidden="true">{on() ? "✓" : ix() + 1}</span>
                    <span class="t-option-label">{option}</span>
                  </button>
                );
              }}
            </For>
          </div>
        </Show>
      </div>
      <div class="t-question-composer">
        <textarea
          ref={editor}
          class="t-question-input"
          rows={props.question.multiline ? 4 : 2}
          placeholder="Type an answer…"
          aria-label="Your answer"
          value={props.typed}
          onInput={(e) => props.onType(e.currentTarget.value)}
          onKeyDown={onKey}
        />
        <div class="t-question-foot">
          <span>Type an answer or choose an option above</span>
          <button type="button" class="t-primary" disabled={!canSubmit()} onClick={() => props.onSubmit()}>
            {last() ? "Submit answer" : "Continue"}
          </button>
        </div>
      </div>
    </div>
  );
}

function Secret(props: { question: Question; onSubmit: (value: string, decline: boolean) => void }) {
  const [value, setValue] = createSignal("");
  let input!: HTMLInputElement;
  createEffect(on(() => props.question.id, () => input.focus()));
  return (
    <form
      class="t-question"
      aria-label={props.question.header || "Secret"}
      onSubmit={(e) => {
        e.preventDefault();
        if (value()) props.onSubmit(value(), false);
      }}
    >
      <div class="t-question-tray">
        <div class="t-question-head">
          <div class="t-question-text">{props.question.question}</div>
        </div>
      </div>
      <div class="t-question-composer">
        <input ref={input} class="t-question-input" type="password" autocomplete="off" placeholder="Enter secret…" aria-label="Secret value" value={value()} onInput={(e) => setValue(e.currentTarget.value)} />
        <div class="t-question-foot">
          <span>Private · used once · kept out of chat</span>
          <button type="button" class="t-ghost" onClick={() => props.onSubmit("", true)}>Decline</button>
          <button type="submit" class="t-primary" disabled={!value()}>Approve and use</button>
        </div>
      </div>
    </form>
  );
}
