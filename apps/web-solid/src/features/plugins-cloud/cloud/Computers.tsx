// Cloud → Your computers: the computers enrolled to run commands from cloud chats.
// Mirrors crates/ui/src/settings/cloud_chat_computers.rs. Bridge enrollments only.
import { rpc } from "@/core/connection";
import { createEffect, createSignal, For, Show } from "solid-js";
import { errorText, owner, ownerKey } from "../account";
import { Action, ErrorStrip, Group, Row } from "../ui";

type Computer = {
  deviceId: string;
  name: string;
  platform: string;
  alwaysAllowed: boolean;
  connected: boolean;
};

const platforms: Record<string, string> = { macos: "macOS", linux: "Linux", windows: "Windows" };

export function Computers() {
  const [computers, setComputers] = createSignal<Computer[]>([]);
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  async function run(action = "list", deviceId?: string) {
    const current = owner();
    if (!current || current.organizationId) return;
    setBusy(true);
    setError("");
    try {
      if (action !== "list")
        await rpc.call("CloudComputerControl", { expectedAccountId: current.accountId, action, deviceId });
      const rows = await rpc.call<Computer[]>("CloudComputerControl", {
        expectedAccountId: current.accountId,
        action: "list",
      });
      if (ownerKey(owner()) === ownerKey(current)) setComputers(rows);
    } catch (e) {
      setError(errorText(e, "Couldn't load your computers."));
    } finally {
      setBusy(false);
    }
  }
  createEffect(
    () => ownerKey(owner()),
    () => {
      setComputers([]);
      void run();
    },
  );
  return (
    <Show when={owner() && !owner()?.organizationId}>
      <Group
        title="Your computers"
        help="Cloud chats can run commands on these computers. Each command asks you first. Turn this on from the desktop app on each computer."
      >
        <For each={computers()}>
          {(computer) => (
            <Row
              title={computer.name}
              description={`${platforms[computer.platform] ?? computer.platform} · ${computer.connected ? "Online" : "Offline"}${computer.alwaysAllowed ? " · Always allowed" : ""}`}
              action={
                <>
                  <Show when={computer.alwaysAllowed}>
                    <Action disabled={busy()} onClick={() => void run("revoke", computer.deviceId)}>
                      Revoke always
                    </Action>
                  </Show>
                  <Action disabled={busy()} onClick={() => void run("forget", computer.deviceId)}>
                    Remove
                  </Action>
                </>
              }
            />
          )}
        </For>
        <Show when={computers().length === 0 && !busy()}>
          <Row title="No computers yet" description="Allow cloud chats from Settings in the desktop app." />
        </Show>
      </Group>
      <Show when={error()}>{(text) => <ErrorStrip onDismiss={() => setError("")}>{text()}</ErrorStrip>}</Show>
    </Show>
  );
}
