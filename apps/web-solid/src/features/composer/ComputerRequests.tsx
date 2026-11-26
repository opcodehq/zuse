import { createEffect, createSignal, For, onCleanup, Show } from "solid-js";
import { rpc } from "@/core/connection";
import { state } from "@/core/store";
import { readCloudTarget } from "@/features/connection/cloud";
import { Action as Btn, ErrorStrip } from "@/features/plugins-cloud/ui";

type Computer = { deviceId: string; name: string; platform: string; connected: boolean };
type Run = {
  id: string;
  computer: string;
  command: string;
  state: string;
  result: { status: string; output: string };
};
type Preview = {
  id: string;
  computerName: string;
  role?: string;
  command: string;
  cwd: string;
  shell: string;
  osUser: string;
};
export function ComputerRequests(props: { chatId: string }) {
  const target = readCloudTarget();
  const [open, setOpen] = createSignal(false);
  const [computers, setComputers] = createSignal<Computer[]>([]);
  const [pending, setPending] = createSignal<Preview[]>([]);
  const [runs, setRuns] = createSignal<Run[]>([]);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  async function call<T>(action: string, extra: Record<string, unknown> = {}) {
    if (!target) throw new Error("Open a cloud chat.");
    return rpc.call<T>("CloudComputerControl", {
      expectedAccountId: target.accountId,
      workspaceId: target.workspaceId,
      chatId: props.chatId,
      action,
      ...extra,
    });
  }
  // Requests are watched whether or not the panel is open, so a new one
  // shows its approval at once.
  createEffect(
    () => [props.chatId, state.epoch] as const,
    () => {
      // Another chat (or session): never show the previous one's requests.
      setComputers([]);
      setPending([]);
      setRuns([]);
      setError("");
      if (!target) return;
      const chat = props.chatId,
        epoch = state.epoch;
      let active = true;
      const current = () =>
        active && chat === props.chatId && epoch === state.epoch;
      async function load() {
        // Each list refreshes on its own; a failed one keeps what was shown,
        // so a running command never loses its Stop.
        const [devices, rows, results] = await Promise.allSettled([
          call<Computer[]>("list"),
          call<Preview[]>("pending"),
          call<Run[]>("results"),
        ]);
        if (!current()) return;
        if (devices.status === "fulfilled") setComputers(devices.value);
        if (rows.status === "fulfilled") setPending(rows.value);
        if (results.status === "fulfilled") setRuns(results.value);
        const failed = [devices, rows, results].find((r) => r.status === "rejected");
        setError(failed && failed.status === "rejected" ? String(failed.reason) : "");
      }
      void load();
      const timer = setInterval(() => void load(), 5000);
      onCleanup(() => {
        active = false;
        clearInterval(timer);
      });
    },
  );
  async function act(action: string, extra: Record<string, unknown>) {
    const chat = props.chatId,
      epoch = state.epoch;
    setBusy(true);
    setError("");
    try {
      await call(action, extra);
      const [rows, results] = await Promise.all([
        call<Preview[]>("pending"),
        call<Run[]>("results"),
      ]);
      if (chat === props.chatId && epoch === state.epoch) {
        setPending(rows);
        setRuns(results);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Show when={target}>
      <div>
        {/* Waiting commands always show, like a permission prompt. */}
        <div style={{ "max-height": "40vh", "overflow-y": "auto" }}>
          <For each={pending()}>
            {(request) => (
              <div>
                <p>Run this command on your computer?</p>
                <p>
                  {request.computerName} · {request.osUser} · {request.shell}
                </p>
                <pre style={{ "max-height": "12em", "overflow-y": "auto" }}>
                  {request.cwd}
                  {"\n"}$ {request.command}
                </pre>
                <For
                  each={[
                    ["deny", "Deny"],
                    ["session", "Allow for this chat"],
                    ["always", "Allow always"],
                    ["once", "Allow once"],
                  ]}
                >
                  {([decision, label]) => (
                    <Btn
                      disabled={busy()}
                      primary={decision === "once"}
                      onClick={() =>
                        void act("decide", { requestId: request.id, decision })
                      }
                    >
                      {label}
                    </Btn>
                  )}
                </For>
              </div>
            )}
          </For>
        </div>
        <Btn onClick={() => setOpen(!open())}>
          {open() ? "Hide computers" : "Computers"}
        </Btn>
        <Show when={open()}>
          <For each={computers()}>
            {(computer) => (
              <p>
                {computer.name} · {computer.platform} · {computer.connected ? "Online" : "Offline"}
              </p>
            )}
          </For>
          {/* One bounded region, so long output never pushes the editor
              or a Stop button out of view. */}
          <div style={{ "max-height": "40vh", "overflow-y": "auto" }}>
            <For each={runs().filter((run) => run.state !== "pending")}>
              {(run) => (
                <div>
                  <p>
                    {run.computer} · {run.result.status}
                  </p>
                  {/* Approved but not yet started can still be withdrawn. */}
                  <Show when={run.state === "running" || run.state === "approved"}>
                    <Btn
                      disabled={busy()}
                      onClick={() => void act("cancel", { requestId: run.id })}
                    >
                      Stop
                    </Btn>
                  </Show>
                  <pre style={{ "max-height": "12em", "overflow-y": "auto" }}>
                    {run.command}
                    {"\n"}
                    {run.result.output}
                  </pre>
                </div>
              )}
            </For>
          </div>
        </Show>
        <Show when={error()}>
          {(message) => <ErrorStrip>{message()}</ErrorStrip>}
        </Show>
      </div>
    </Show>
  );
}
