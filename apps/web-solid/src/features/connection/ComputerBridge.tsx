import { createEffect, createSignal, For, Show } from "solid-js";
import { state } from "@/core/store";
import { rpc } from "@/core/connection";
import { accountUser } from "./remote";
import { Btn, ErrorStrip, Frame } from "./ui";

type Computer = {
  deviceId: string;
  name: string;
  platform: string;
  alwaysAllowed: boolean;
  connected: boolean;
};

export function ComputerBridge() {
  const [computers, setComputers] = createSignal<Computer[]>([]);
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  createEffect(
    () => [accountUser()?.id, state.epoch] as const,
    () => {
      setComputers([]);
      setError("");
    },
  );
  async function refresh(action = "list", deviceId?: string) {
    setBusy(true);
    setError("");
    try {
      const expectedAccountId = accountUser()?.id;
      if (!expectedAccountId)
        throw new Error("Sign in to manage cloud chat computers.");
      if (action !== "list")
        await rpc.call("CloudComputerControl", {
          expectedAccountId,
          action,
          deviceId,
        });
      const rows = await rpc.call<Computer[]>("CloudComputerControl", {
        expectedAccountId,
        action: "list",
      });
      if (accountUser()?.id === expectedAccountId) setComputers(rows);
    } catch (e) {
      setComputers([]);
      setError(e instanceof Error ? e.message : "Could not load computers.");
    } finally {
      setBusy(false);
    }
  }
  return (
    <Frame
      title="Cloud chat computers"
      help="Computers enrolled to offer tasks to your private cloud chats."
    >
      <Btn disabled={busy()} onClick={() => void refresh()}>
        Load computers
      </Btn>
      <For each={computers()}>
        {(computer) => (
          <div class="cx-stack">
            <span>
              {computer.name} · {computer.platform} · {computer.connected ? "Online" : "Offline"}
            </span>
            <Show when={computer.alwaysAllowed}>
              <Btn
                disabled={busy()}
                onClick={() => void refresh("revoke", computer.deviceId)}
              >
                Revoke always allow
              </Btn>
            </Show>
            <Btn
              disabled={busy()}
              onClick={() => void refresh("forget", computer.deviceId)}
            >
              Remove computer
            </Btn>
          </div>
        )}
      </For>
      <Show when={error()}>{(text) => <ErrorStrip message={text()} />}</Show>
    </Frame>
  );
}
