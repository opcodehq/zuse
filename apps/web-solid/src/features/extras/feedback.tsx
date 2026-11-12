// Shared states for any surface: empty, loading and failed. Content sits on the surface, no cards.
import { writeClipboard } from "@/ui/clipboard";
import { createSignal, For, type JSX, onCleanup, onMount, Show } from "solid-js";
import { Icon } from "@/ui/Icon";
import { Button, toast } from "@/ui/primitives";
import { errorText } from "./util";

/** Centered title and hint, as desktop's palette and panel empties (`palette_empty`). */
export function EmptyState(props: { title: string; hint?: string; icon?: string; children?: JSX.Element }) {
  return (
    <div class="x-empty">
      <Show when={props.icon}>{(name) => <Icon name={name()} size={20} class="x-empty-icon" />}</Show>
      <span class="x-empty-title">{props.title}</span>
      <Show when={props.hint}><span class="x-empty-hint">{props.hint}</span></Show>
      <Show when={props.children}><div class="x-empty-actions">{props.children}</div></Show>
    </div>
  );
}

/** Placeholder rows while a list loads. Hidden from assistive tech; the label is announced once. */
export function Skeleton(props: { rows?: number; label?: string; variant?: "line" | "row" }) {
  const rows = () => Array.from({ length: Math.max(1, Math.min(12, props.rows ?? 3)) }, (_, i) => i);
  return (
    <div class="x-skeleton" role="status" aria-label={props.label ?? "Loading"} data-variant={props.variant ?? "row"}>
      <For each={rows()}>
        {(i) => (
          <div class="x-skeleton-row" aria-hidden="true" style={{ "--i": i }}>
            <Show when={(props.variant ?? "row") === "row"}><span class="x-skeleton-glyph skeleton" /></Show>
            {/* Desktop's width ladder (pull_request_skeleton WIDTHS), so rows read as text, not slabs. */}
            <span class="x-skeleton-bar skeleton" style={{ width: `${[62, 48, 71, 55, 39, 66][i % 6]}%` }} />
          </div>
        )}
      </For>
    </div>
  );
}

function details(error: unknown) {
  if (error instanceof Error) {
    const head = error.name && error.name !== "Error" ? `${error.name}: ${error.message}` : error.message;
    const stack = error.stack ?? "";
    // V8 stacks already open with the name and message.
    return stack.includes(error.message) ? stack : [head, stack].filter(Boolean).join("\n\n");
  }
  try {
    return typeof error === "string" ? error : JSON.stringify(error, null, 2);
  } catch {
    return String(error);
  }
}

/**
 * A view that failed to render. The heading takes focus so screen readers land on it,
 * and the details are copyable because failure text is meant to be pasted.
 */
export function ErrorScreen(props: { error: unknown; reset?: () => void; title?: string }) {
  let heading: HTMLHeadingElement | undefined;
  const [copied, setCopied] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(timer));
  onMount(() => heading?.focus());
  const message = () => errorText(props.error, "An unexpected error stopped this view.");
  const copy = async () => {
    if (!(await writeClipboard(details(props.error)))) return toast("Couldn't copy the error details.", "danger");
    setCopied(true);
    clearTimeout(timer);
    timer = setTimeout(() => setCopied(false), 1500);
  };
  return (
    <div class="x-error-screen" role="alert" aria-labelledby="x-error-title" aria-describedby="x-error-message">
      <span class="x-error-glyph" aria-hidden="true"><Icon name="danger-triangle" size={16} /></span>
      <h1 ref={heading} id="x-error-title" class="x-error-title" tabIndex={-1}>{props.title ?? "Something went wrong"}</h1>
      <p id="x-error-message" class="x-error-message">{message()}</p>
      <div class="x-error-actions">
        <Show when={props.reset}>{(reset) => <Button variant="primary" onClick={() => reset()()}>Try again</Button>}</Show>
        <Button icon="refresh" onClick={() => location.reload()}>Reload</Button>
        <Button variant="ghost" icon={copied() ? "check-circle" : "copy"} onClick={() => void copy()}>{copied() ? "Copied" : "Copy details"}</Button>
      </div>
      <details class="x-error-details">
        <summary>Details</summary>
        <pre>{details(props.error)}</pre>
      </details>
    </div>
  );
}

const IGNORED = [/ResizeObserver loop/i, /^Script error\.?$/];
const GAP_MS = 5000;

/** Uncaught errors from event handlers and promises never reach an error boundary; say so instead of failing silently. */
export function watchUncaughtErrors() {
  let last = "";
  let at = 0;
  const report = (error: unknown) => {
    const message = errorText(error, "");
    if (!message || IGNORED.some((pattern) => pattern.test(message))) return;
    const now = Date.now();
    if (message === last && now - at < GAP_MS * 6) return;
    if (now - at < GAP_MS) return;
    last = message;
    at = now;
    toast(`Something went wrong: ${message}`, "danger");
  };
  const onError = (event: ErrorEvent) => report(event.error ?? event.message);
  const onRejection = (event: PromiseRejectionEvent) => report(event.reason);
  window.addEventListener("error", onError);
  window.addEventListener("unhandledrejection", onRejection);
  return () => {
    window.removeEventListener("error", onError);
    window.removeEventListener("unhandledrejection", onRejection);
  };
}
