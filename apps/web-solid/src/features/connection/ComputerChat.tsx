import { createEffect, createSignal, For, onCleanup, Show } from "solid-js";
import { rpc } from "@/core/connection";
import { state } from "@/core/store";
import { readCloudTarget } from "./cloud";
import { Btn, ErrorStrip } from "./ui";

type Computer = { deviceId: string; name: string; platform: string; connected: boolean };
type Run = {
  id: string;
  computer: string;
  command: string;
  result: { status: string; output: string };
};
type Preview = {
  id: string;
  computerName: string;
  command: string;
  cwd: string;
  shell: string;
  osUser: string;
};
export function ComputerChat(props: { chatId: string }) {
  const target = readCloudTarget();
  const [open, setOpen] = createSignal(false);
  const [computers, setComputers] = createSignal<Computer[]>([]);
  const [attached, setAttached] = createSignal<string[]>([]);
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
  createEffect(
    () => [open(), props.chatId, state.epoch] as const,
    () => {
      if (!open() || !target) return;
      const chat = props.chatId,
        epoch = state.epoch;
      let active = true;
      const current = () =>
        active && chat === props.chatId && epoch === state.epoch;
      async function load() {
        try {
          const [devices, ids, rows, results] = await Promise.all([
            call<Computer[]>("list"),
            call<string[]>("attached"),
            call<Preview[]>("pending"),
            call<Run[]>("results"),
          ]);
          if (current()) {
            setComputers(devices);
            setAttached(ids);
            setPending(rows);
            setRuns(results);
          }
        } catch (e) {
          if (current()) {
            setComputers([]);
            setAttached([]);
            setPending([]);
            setRuns([]);
            setError(String(e));
          }
        }
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
      const [ids, rows, results] = await Promise.all([
        call<string[]>("attached"),
        call<Preview[]>("pending"),
        call<Run[]>("results"),
      ]);
      if (chat === props.chatId && epoch === state.epoch) {
        setAttached(ids);
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
        <Btn onClick={() => setOpen(!open())}>
          {open() ? "Hide computers" : "Computers"}
        </Btn>
        <Show when={open()}>
          <For each={computers()}>
            {(computer) => (
              <Btn
                disabled={busy()}
                onClick={() =>
                  void act(
                    attached().includes(computer.deviceId)
                      ? "detach"
                      : "attach",
                    { deviceId: computer.deviceId },
                  )
                }
              >
                {attached().includes(computer.deviceId) ? "Detach" : "Attach"}{" "}
                {computer.name} · {computer.platform} · {computer.connected ? "Online" : "Offline"}
              </Btn>
            )}
          </For>
          <For each={pending()}>
            {(request) => (
              <div>
                <p>
                  {request.computerName} · {request.osUser} · {request.shell}
                </p>
                <pre>
                  {request.cwd}
                  {"\n"}
                  {request.command}
                </pre>
                <p>Runs with this OS user's access to files and credentials.</p>
                <For
                  each={[
                    ["once", "Approve once"],
                    ["session", "Approve for this session"],
                    ["always", "Approve always"],
                    ["deny", "Deny"],
                  ]}
                >
                  {([decision, label]) => (
                    <Btn
                      disabled={busy()}
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
          <For each={runs()}>
            {(run) => (
              <div>
                <p>
                  {run.computer} · {run.result.status}
                </p>
                <pre>
                  {run.command}
                  {"\n"}
                  {run.result.output}
                </pre>
                <Show when={run.result.status === "Running"}>
                  <Btn
                    disabled={busy()}
                    onClick={() => void act("cancel", { requestId: run.id })}
                  >
                    Stop
                  </Btn>
                </Show>
              </div>
            )}
          </For>
          <Show when={error()}>
            {(message) => <ErrorStrip message={message()} />}
          </Show>
        </Show>
      </div>
    </Show>
  );
}
