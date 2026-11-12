// Project creation progress: a trailing badge in project lists and the composer-style notice row.
import { createSignal, Show } from "solid-js";
import { Icon } from "@/ui/Icon";
import { DitherRing } from "@/ui/loaders";
import { cancelCreation, canRetry, creationForSpace, describeCreation, retryCreation } from "./creations";
import { errorText } from "./hosts";
import type { ProjectCreation } from "./types";

/** Progress while running, the failure otherwise; nothing once ready. With `action`, the Cancel or Retry control. */
export function CreationBadge(props: { op: ProjectCreation; action?: boolean }) {
  const running = () => props.op.status === "running";
  const failed = () => !!props.op.error || !!props.op.publishError;
  const label = () =>
    running()
      ? props.op.percent != null
        ? `${props.op.stage} · ${props.op.percent}%`
        : props.op.stage
      : props.op.publishError
        ? "Publishing failed"
        : props.op.status === "cancelled"
          ? "Cancelled"
          : props.op.status === "interrupted"
            ? "Interrupted"
            : "Failed";
  // A cancellation is the user's choice, not a failure; keep it quiet.
  const tone = () => (running() || props.op.status === "cancelled" ? undefined : "danger");
  return (
    <Show when={running() || failed()}>
      <Show
        when={props.action}
        fallback={
          <span class="pj-badge" data-tone={tone()} title={props.op.error ?? props.op.publishError ?? undefined}>
            <Show when={running()} fallback={<Icon name={tone() ? "danger-triangle" : "info-circle"} size={13} />}>
              <DitherRing size={12} tone="accent" label="Setting up" />
            </Show>
            <span class="pj-badge-text">{label()}</span>
          </span>
        }
      >
        <CreationAction op={props.op} class="pj-row-action" />
      </Show>
    </Show>
  );
}

/** Cancel while running, Retry after a failure, cancellation, interruption or a publishing error. */
export function CreationAction(props: { op: ProjectCreation; class?: string; onError?: (message: string) => void }) {
  const [pending, setPending] = createSignal(false);
  const running = () => props.op.status === "running";
  const show = () => running() || canRetry(props.op);
  const run = async () => {
    if (pending()) return;
    setPending(true);
    try {
      if (running()) await cancelCreation(props.op);
      else await retryCreation(props.op);
    } catch (error) {
      props.onError?.(errorText(error, running() ? "Could not cancel setup." : "Could not retry setup."));
    } finally {
      setPending(false);
    }
  };
  return (
    <Show when={show()}>
      <button
        type="button"
        class={`pj-text-btn ${props.class ?? ""}`}
        disabled={pending()}
        aria-label={`${running() ? "Cancel setup of" : "Retry setup of"} ${props.op.request.name}`}
        onClick={() => void run()}
      >
        {running() ? "Cancel" : "Retry"}
      </button>
    </Show>
  );
}

/**
 * The selected project's creation state as one row: what is happening and the next action.
 * Meant for the composer tray; renders nothing when the project has no pending creation.
 */
export function ProjectCreationNotice(props: { spaceId: string | null | undefined }) {
  const op = () => (props.spaceId ? creationForSpace(props.spaceId) : undefined);
  const [error, setError] = createSignal<string | null>(null);
  return (
    <Show when={op()}>
      {(current) => (
        <Show when={describeCreation(current())}>
          {(info) => (
            <div class="pj-notice" data-tone={info().tone} role="status" aria-live="polite">
              <Show
                when={current().status === "running"}
                fallback={<Icon name={info().tone === "danger" ? "danger-triangle" : "info-circle"} size={14} class="pj-notice-glyph" />}
              >
                <DitherRing size={12} tone="accent" label="Setting up" />
              </Show>
              <div class="pj-notice-text">
                <span class="pj-notice-title">{info().title}</span>
                <Show when={error() ?? info().detail}>
                  <span class="pj-notice-detail">{error() ?? info().detail}</span>
                </Show>
              </div>
              <CreationAction op={current()} onError={setError} />
            </div>
          )}
        </Show>
      )}
    </Show>
  );
}
