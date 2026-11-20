import { Show } from "solid-js";
import { Button } from "@/ui/primitives";
import { commandState, dismissError, retryCommand } from "./state";
export function CommandAlert() {
  return (
    <Show when={commandState.error}>
      <div class="zp-alert" role="alert">
        {commandState.error}
        <Show when={commandState.uncertain}>
          <Button
            disabled={commandState.busy}
            onClick={() => void retryCommand().catch(() => {})}
          >
            Retry action
          </Button>
        </Show>
        <Button onClick={dismissError}>Dismiss</Button>
      </div>
    </Show>
  );
}
