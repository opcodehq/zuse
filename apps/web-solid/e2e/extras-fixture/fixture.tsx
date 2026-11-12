// Test-only page: the shared states and toasts that no engine data reaches on demand (a crashed view,
// an empty list, loading rows). Mounts the real components and styles. Never bundled into the app.
import { render } from "solid-js/web";
import { createSignal, Show } from "solid-js";
import "@/ui/tokens.css";
import "@/ui/primitives.css";
import "@/shell/shell.css";
import "@/features/extras/extras.css";
import { EmptyState, ErrorScreen, Skeleton } from "@/features/extras/feedback";
import { ToastRegion, toast } from "@/ui/primitives";

document.documentElement.dataset.theme = new URLSearchParams(location.search).get("theme") ?? "dark";

const failure = new TypeError("Cannot read properties of undefined (reading 'title')");
const [resets, setResets] = createSignal(0);
const [crashed, setCrashed] = createSignal(true);
(window as unknown as { __fx: unknown }).__fx = { toast, resets, crash: () => setCrashed(true) };

render(
  () => (
    <div style={{ display: "grid", "grid-template-columns": "1fr 1fr", height: "100vh", background: "var(--bg)" }}>
      <div style={{ "border-right": "1px solid var(--border)" }}>
        <Show when={crashed()} fallback={<p style={{ padding: "24px" }}>Recovered view</p>}>
          <ErrorScreen error={failure} reset={() => (setResets((n) => n + 1), setCrashed(false))} />
        </Show>
      </div>
      <div style={{ display: "flex", "flex-direction": "column", gap: "24px", padding: "24px" }}>
        <EmptyState title="No results" hint="Try a command, chat title, project, or device." />
        <EmptyState icon="history" title="No recent chats found" hint="Conversations with a project folder on this computer appear here." />
        <Skeleton rows={4} label="Finding chats" />
        <Skeleton rows={3} variant="line" label="Loading" />
      </div>
      <ToastRegion />
    </div>
  ),
  document.getElementById("root")!,
);
