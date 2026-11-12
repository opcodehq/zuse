// Settings → Devices: remote access to this computer, connect links, other computers, connected
// and account devices, direct and relay routes, and the device registry. Mirrors crates/ui
// settings/devices.rs, pairing.rs and managed_access.rs; browser-only rows come from the web client.
import { createEffect, createSignal, For, on, onCleanup, Show } from "solid-js";
import { M } from "@/core/methods";
import { rpc } from "@/core/connection";
import { state } from "@/core/store";
import type { Device } from "@/core/protocol";
import { Icon } from "@/ui/Icon";
import { Toggle, toast } from "@/ui/primitives";
import { openAccountComputer, returnToController } from "./account";
import { forget, forgetCurrent, linkFor, openInThisTab, savedComputers } from "./computers";
import { addComputer } from "./add";
import { accountUser, act, clearInvitation, createInvitation, refresh, remote, setRemote, type Peer } from "./remote";
import { AccountSignIn } from "./signin";
import { area, routeLine } from "./status";
import { Btn, copyText, Details, ErrorStrip, Frame, Glyph, MethodRow, RemoteDialog } from "./ui";

type DialogKind = "network" | "invitation" | "computer" | "setup" | "unlink";

const str = (value: unknown) => (typeof value === "string" ? value : undefined);
const nowSeconds = () => Math.floor(Date.now() / 1000);

export function DevicesSettings() {
  const [dialog, setDialog] = createSignal<DialogKind | null>(null);
  const [advanced, setAdvanced] = createSignal(false);
  createEffect(on(() => [state.epoch, state.connection.kind === "connected", accountUser()?.id] as const, () => void refresh()));
  const busy = () => remote.busy !== null;
  const open = (kind: DialogKind) => {
    if (busy()) return;
    setRemote("error", null);
    setDialog(kind);
  };
  const close = () => {
    if (busy()) return;
    setDialog(null);
    clearInvitation();
  };
  // A new connection may reach a different engine state; desktop drops open remote dialogs too.
  createEffect(on(() => state.epoch, () => setDialog(null), { defer: true }));

  return (
    <div class="cx cx-page">
      <header class="cx-head">
        <h1 class="cx-title">Remote access</h1>
        <p class="cx-subtitle">Use this computer from your phone, a browser, or another computer.</p>
      </header>
      <div class="cx-stack">
        <Frame title="Remote access to this computer" help="Choose how other Zuse apps and browsers can reach this computer." first>
          <ServeMethod open={open} />
          <NetworkMethod open={open} />
        </Frame>
        <ConnectCard open={open} />
        <ComputersCard open={open} />
        <PeersCard />
        <AccountDevicesCard />
        <Frame title="Direct connections" help="Let paired devices reach this computer over your local network or Tailscale. Zuse uses whichever route is fastest and stays end-to-end encrypted.">
          <DirectRows />
        </Frame>
        <div class="cx-more">
          <Btn aria-expanded={advanced()} onClick={() => setAdvanced(!advanced())}>
            {advanced() ? "Hide advanced" : "Advanced connection settings"}
          </Btn>
        </div>
        <Show when={advanced()}>
          <RelayCard />
        </Show>
        <BrowserCards />
        <Show when={dialog() === null && (remote.error ?? remote.loadError)}>{(text) => <ErrorStrip message={text()} />}</Show>
        <Registry />
      </div>
      <NetworkDialog open={dialog() === "network"} onClose={close} />
      <InvitationDialog open={dialog() === "invitation"} onClose={close} />
      <ComputerDialog open={dialog() === "computer"} onClose={close} />
      <ServeDialog kind={dialog() === "setup" ? "setup" : dialog() === "unlink" ? "unlink" : null} onClose={close} />
    </div>
  );
}

/* Access methods */

function ServeMethod(props: { open: (kind: DialogKind) => void }) {
  const host = () => remote.host ?? {};
  const configured = () => host().configured === true;
  const text = () => {
    if (configured() && host().tunnelRunning === true) {
      const endpoint = (host().endpoint ?? {}) as Record<string, unknown>;
      return `Available anywhere · ${str(endpoint.httpBaseUrl) ?? "Zuse account"}`;
    }
    if (configured() && host().accountMatches !== true) return "Sign in to the linked account.";
    if (configured()) return "Reconnecting to your account…";
    return "Access this computer from your signed-in devices.";
  };
  return (
    <MethodRow glyph="remote-server" title="Zuse Serve" text={text()}>
      <Btn size="method" aria-disabled={remote.busy !== null} onClick={() => props.open(configured() ? "unlink" : "setup")}>
        {remote.busy === M.LINK_MANAGED_HOST || remote.busy === M.UNLINK_MANAGED_HOST ? "Updating…" : configured() ? "Turn off" : "Set up"}
      </Btn>
    </MethodRow>
  );
}

function NetworkMethod(props: { open: (kind: DialogKind) => void }) {
  const running = () => remote.network?.running === true;
  const managed = () => remote.network?.managed === true;
  const available = () => remote.networkKnown && state.connection.kind === "connected";
  const busy = () => remote.busy === M.START_BROWSER_ACCESS || remote.busy === M.STOP_BROWSER_ACCESS;
  const text = () =>
    running()
      ? `Available on this network · ${str(remote.network?.origin) ?? "Running"}`
      : !available()
        ? "Local network access is unavailable. Refresh to check again."
        : "Connect from another device on the same Wi-Fi.";
  // A listener started from the command line is not this page's to stop.
  const disabled = () => !available() || remote.busy !== null || (running() && !managed());
  return (
    <MethodRow glyph="wifi" title="Local network" text={text()}>
      <Btn size="method" disabled={disabled()} onClick={() => props.open("network")}>
        {busy() ? "Updating…" : running() ? "Turn off" : "Turn on"}
      </Btn>
    </MethodRow>
  );
}

function ConnectCard(props: { open: (kind: DialogKind) => void }) {
  const ready = () => remote.network?.running === true || remote.relay?.running === true || remote.direct?.running === true || remote.host?.tunnelRunning === true;
  return (
    <Frame title="Connect another device" help="Create a temporary link that authorizes another Zuse app or browser to use this computer.">
      <div class="cx-row" data-size="method">
        <span class="cx-tile"><Glyph name="link" /></span>
        <Details title="Create a connect link" text={ready() ? "Authorize another device with a link that lasts ten minutes." : "Set up one of the connections above first."} />
        <Btn size="sm" disabled={!ready() || remote.busy !== null} onClick={() => props.open("invitation")}>Create link</Btn>
      </div>
    </Frame>
  );
}

/* Other computers: account environments, plus computers saved through the account. */

function ComputersCard(props: { open: (kind: DialogKind) => void }) {
  const rows = () => {
    const live = remote.environments;
    const ids = new Set(live.map((row) => str(row.environmentId)));
    return [...live, ...remote.saved.filter((row) => !ids.has(str(row.environmentId)))];
  };
  const supports = (row: Record<string, unknown>) => {
    const features = ((row.capabilities ?? {}) as { features?: unknown }).features;
    return row.wireProtocolVersion === 1 && Array.isArray(features) && features.includes("zuse-rpc-json-v1");
  };
  return (
    <Frame
      title="Other computers"
      help="Projects and chats from connected computers stay in the same sidebar."
      action={
        <Btn size="add" onClick={() => props.open("computer")}>
          <Icon name="plus" size={14} />
          Add computer
        </Btn>
      }
    >
      <Show
        when={rows().length > 0}
        fallback={
          <div class="cx-row cx-empty-computers">
            <span class="cx-tile" data-size="lg"><Icon name="monitor" size={16} /></span>
            <Details title="No other computers yet" text="Signed-in computers appear automatically, or you can add one." />
          </div>
        }
      >
        <For each={rows()}>
          {(computer) => {
            const id = () => str(computer.environmentId) ?? "";
            const label = () => str(computer.label) ?? "Linked computer";
            const current = () => id() === state.localDeviceId;
            const compatible = () => supports(computer);
            const remembered = () => remote.saved.some((row) => row.environmentId === computer.environmentId);
            const text = () => (current() ? "This computer" : compatible() ? (str(computer.serviceState) ?? "Availability checked on connect") : "Update Zuse on this computer to connect.");
            const connect = () => {
              const subject = accountUser()?.id;
              if (!subject) return;
              try {
                openAccountComputer({ environmentId: id(), expectedSubject: subject, label: label() });
              } catch (error) {
                setRemote("error", error instanceof Error ? error.message : "Could not open computer tab");
              }
            };
            return (
              <div class="cx-row">
                <span class="cx-glyph"><Icon name="monitor" size={16} /></span>
                <Details title={label()} text={text()} />
                <div class="cx-actions" data-gap="8">
                  <Show when={!current() && compatible()}>
                    <Btn aria-label={`Connect to ${label()}`} onClick={connect}>Connect</Btn>
                  </Show>
                  <Show when={remembered()}>
                    <Btn
                      aria-label={`Forget saved ${label()}`}
                      disabled={remote.busy !== null}
                      onClick={() => void forgetSaved(id())}
                    >
                      Forget saved
                    </Btn>
                  </Show>
                </div>
              </div>
            );
          }}
        </For>
      </Show>
    </Frame>
  );
}

async function forgetSaved(environmentId: string) {
  const reply = await act<{ computers?: Record<string, unknown>[] }>(M.FORGET_ACCOUNT_ROUTE, { environmentId, expectedSubject: accountUser()?.id ?? "" });
  if (reply?.computers) setRemote("saved", reply.computers);
}

/* Devices that paired with this computer. */

function peerStatus(peer: Peer) {
  if (peer.pairedAt !== null && peer.pairedAt !== undefined) return "Connected";
  if (peer.expiresAt !== null && peer.expiresAt !== undefined && peer.expiresAt <= nowSeconds()) return "Invitation expired";
  return "Invitation pending";
}

function PeersCard() {
  return (
    <Frame
      title="Connected devices"
      help="Manage devices that have connected to this computer."
      action={<Btn disabled={remote.loading} onClick={() => void refresh()}>Refresh</Btn>}
    >
      <Show when={remote.peers.length > 0} fallback={<div class="cx-empty">No other devices are connected yet.</div>}>
        <For each={remote.peers}>
          {(peer) => (
            <div class="cx-row">
              <span class="cx-glyph"><Icon name="smartphone" size={16} /></span>
              <Details title={peer.label} text={peerStatus(peer)} />
              <Btn size="sm" tone="danger" aria-label={`Revoke ${peer.label}`} disabled={remote.busy !== null} onClick={() => void act(M.REVOKE_PAIRING, { pairId: peer.id })}>
                Revoke
              </Btn>
            </div>
          )}
        </For>
      </Show>
    </Frame>
  );
}

function AccountDevicesCard() {
  return (
    <Show when={accountUser()}>
      <Frame title="Account devices" help="Manage devices authorized through your account.">
        <Show when={remote.accountDevices.length > 0} fallback={<div class="cx-empty">No account devices connected yet.</div>}>
          <For each={remote.accountDevices}>
            {(device) => (
              <div class="cx-row">
                <Details title={str(device.label) ?? "Account device"} text={str(device.platform) ?? "Device"} />
                <Btn
                  size="sm"
                  tone="danger"
                  aria-label={`Revoke ${str(device.label) ?? "account device"}`}
                  disabled={remote.busy !== null}
                  onClick={() => void act(M.REVOKE_ACCOUNT_DEVICE, { clientId: str(device.clientId) ?? "" })}
                >
                  Revoke
                </Btn>
              </div>
            )}
          </For>
        </Show>
      </Frame>
    </Show>
  );
}

/* Direct and relay routes. */

function routeSummary(status: Record<string, unknown> | null) {
  const routes = Array.isArray(status?.routes) ? (status.routes as { kind?: string }[]) : [];
  const parts = [routes.some((r) => r.kind === "lan") && "Local network", routes.some((r) => r.kind === "tailnet") && "Tailscale"].filter(Boolean);
  return parts.length ? `On · ${parts.join(" and ")}` : "On · waiting for a network";
}

function DirectRows() {
  const running = () => remote.direct?.running === true;
  const serve = () => remote.direct?.tailscaleServe === true;
  return (
    <>
      <Show when={routeLine()}>
        {(line) => (
          <div class="cx-row">
            <Details title={area.mode === "account" ? "This tab" : "This tab"} text={line()} />
          </div>
        )}
      </Show>
      <Show when={remote.direct} fallback={<div class="cx-empty">This computer does not offer direct connections.</div>}>
        <div class="cx-row">
          <Details title="Local network and Tailscale" text={running() ? routeSummary(remote.direct) : "Off"} />
          <Toggle
            checked={running()}
            label="Allow local network and Tailscale connections"
            onChange={() => void (running() ? act(M.STOP_DIRECT_ACCESS) : act(M.START_DIRECT_ACCESS, { tailscaleServe: serve() }))}
          />
        </div>
        <Show when={running() && remote.direct?.tailscaleAvailable === true}>
          <div class="cx-row">
            <Details title="Tailscale HTTPS" text="Also publish on your tailnet with a trusted certificate, so browsers can use it." />
            <Toggle checked={serve()} label="Publish over Tailscale HTTPS" onChange={() => void act(M.START_DIRECT_ACCESS, { tailscaleServe: !serve() })} />
          </div>
        </Show>
        <Show when={str(remote.direct?.error)}>{(error) => <div class="cx-row"><span class="cx-note" style={{ color: "var(--danger)" }}>{error()}</span></div>}</Show>
      </Show>
    </>
  );
}

function RelayCard() {
  const [origin, setOrigin] = createSignal("");
  const [credential, setCredential] = createSignal("");
  const running = () => remote.relay?.running === true;
  const auto = () => remote.relay?.autoStart === true;
  const change = async () => {
    if (running()) return void (await act(M.STOP_RELAY_ACCESS));
    if (remote.relay?.configured === true && !origin().trim() && !credential().trim()) return void (await act(M.RESUME_RELAY_ACCESS));
    const reply = await act(M.START_RELAY_ACCESS, { origin: origin().trim(), hostTokenFile: credential().trim() });
    if (reply) {
      setOrigin("");
      setCredential("");
    }
  };
  return (
    <Frame title="Relay access" help="Configure an outbound relay for connections across networks.">
      <Show when={remote.relay} fallback={<div class="cx-empty">This computer does not offer relay access.</div>}>
        <div class="cx-row">
          <Details title="Relay" text={running() ? (str(remote.relay?.origin) ?? "Running") : "Stopped"} />
          <Btn disabled={remote.busy !== null} onClick={() => void change()}>
            {remote.busy && [M.START_RELAY_ACCESS, M.STOP_RELAY_ACCESS, M.RESUME_RELAY_ACCESS].includes(remote.busy as never) ? "Working…" : running() ? "Stop relay" : "Start relay"}
          </Btn>
        </div>
        <div class="cx-row">
          <Details title="Start when Zuse opens" text="Start relay once to save its configuration." />
          <Toggle checked={auto()} label="Start relay when Zuse opens" onChange={() => void act(M.SET_RELAY_AUTO_START, { enabled: !auto() })} />
        </div>
        <Show when={!running()}>
          <div class="cx-row" data-wrap="true">
            <Details title="Relay URL" text="Leave both fields blank to resume the saved relay." />
            <input class="cx-field" type="url" placeholder="https://relay.example.com" aria-label="Relay URL" value={origin()} onInput={(e) => setOrigin(e.currentTarget.value)} />
          </div>
          <div class="cx-row" data-wrap="true">
            <Details title="Host credential" text="Private token file on this computer." />
            <input class="cx-field" placeholder="Private host credential file on this computer" aria-label="Host credential file" value={credential()} onInput={(e) => setCredential(e.currentTarget.value)} />
          </div>
        </Show>
        <Show when={str(remote.relay?.error)}>{(error) => <div class="cx-row"><span class="cx-note" style={{ color: "var(--danger)" }}>{error()}</span></div>}</Show>
      </Show>
    </Frame>
  );
}

/* Rows that only exist in a browser: account tabs, saved computers, and this tab's pairing. */

function BrowserCards() {
  return (
    <>
      <Show when={area.mode === "account"}>
        <Frame title="Account connection" help="This tab runs on the selected computer.">
          <div class="cx-row">
            <Details title="Return to controller" text="Save local edits before returning to your controller." />
            <Btn onClick={() => returnToController()}>Return</Btn>
          </div>
        </Frame>
      </Show>
      <Show when={savedComputers().length > 0}>
        <Frame title="Computers in this browser" help="Connections saved here reopen without a new link until you forget or revoke them.">
          <For each={savedComputers()}>
            {(computer) => (
              <div class="cx-row">
                <span class="cx-glyph"><Icon name="monitor" size={16} /></span>
                <Details title={computer.origin} text={computer.current ? "Used by this tab" : "Saved"} />
                {/* The computer in use is forgotten from "This browser", which also reloads the tab. */}
                <Show when={!computer.current}>
                  <div class="cx-actions" data-gap="8">
                    <Btn
                      size="sm"
                      aria-label={`Open ${computer.origin}`}
                      onClick={() => {
                        const link = linkFor(computer.id);
                        if (link) openInThisTab(link);
                      }}
                    >
                      Open
                    </Btn>
                    <Btn size="sm" aria-label={`Forget ${computer.origin}`} onClick={() => forget(computer.id)}>Forget</Btn>
                  </div>
                </Show>
              </div>
            )}
          </For>
        </Frame>
      </Show>
      <Show when={area.mode === "paired"}>
        <Frame title="This browser" help="Manage the access saved in this tab.">
          <div class="cx-row">
            <Details title="Forget paired access" text="This reloads the tab. Save local drafts first." />
            <Btn onClick={() => forgetCurrent()}>Forget pairing</Btn>
          </div>
        </Frame>
      </Show>
    </>
  );
}

/* Device registry, from settings/devices.rs. */

const ONLINE_WINDOW_SECONDS = 70;

function platformLabel(platform: string) {
  return ({ macos: "macOS", darwin: "macOS", linux: "Linux", windows: "Windows", web: "Web", ios: "iOS", android: "Android" } as Record<string, string>)[platform] ?? platform;
}

function lastSeen(at: number | null, now: number) {
  if (at === null) return "never seen";
  const seconds = Math.floor((now - at) / 1000);
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86_400)}d ago`;
}

function seenAt(device: Device): number | null {
  const raw = (device as unknown as { lastSeenAt?: string | null }).lastSeenAt;
  if (!raw) return null;
  const at = Date.parse(raw);
  return Number.isNaN(at) ? null : at;
}

function Registry() {
  const [now, setNow] = createSignal(Date.now());
  const timer = setInterval(() => setNow(Date.now()), 30_000);
  onCleanup(() => clearInterval(timer));
  const devices = () => Object.values(state.devices);
  const local = () => devices().filter((device) => device.id === state.localDeviceId);
  const others = () => devices().filter((device) => device.id !== state.localDeviceId);
  const [renaming, setRenaming] = createSignal<Device | null>(null);
  const [copied, setCopied] = createSignal<string | null>(null);
  const copy = async (id: string) => {
    if (!(await copyText(id))) return toast("Could not copy the device ID", "danger");
    setCopied(id);
    setTimeout(() => setCopied((value) => (value === id ? null : value)), 1500);
  };
  const Row = (props: { device: Device }) => {
    const isLocal = () => props.device.id === state.localDeviceId;
    const at = () => seenAt(props.device);
    const online = () => {
      const seen = at();
      return seen !== null && (now() - seen) / 1000 <= ONLINE_WINDOW_SECONDS;
    };
    const version = () => (props.device as unknown as { version?: string | null }).version;
    return (
      <div class="cx-row">
        <div class="cx-details" style={{ "min-width": "160px" }}>
          <div class="cx-row-title">{props.device.name}</div>
          <div class="cx-meta">
            <span>{platformLabel((props.device as unknown as { platform?: string }).platform ?? "")}</span>
            <Show when={version()}>{(v) => <span>v{v()}</span>}</Show>
            <Show when={!isLocal()}>
              <Show when={online()} fallback={<span>Last seen {lastSeen(at(), now())}</span>}>
                <span data-tone="success">Online</span>
              </Show>
            </Show>
          </div>
        </div>
        <div class="cx-actions">
          <Btn tone="quiet" aria-label={`Copy device ID ${props.device.id}`} onClick={() => void copy(props.device.id)}>
            {copied() === props.device.id ? "Copied" : "Copy ID"}
          </Btn>
          <Btn aria-label={`Rename ${props.device.name}`} disabled={state.connection.kind !== "connected"} onClick={() => setRenaming(props.device)}>Rename</Btn>
        </div>
      </div>
    );
  };
  return (
    <div class="cx-registry">
      <Show when={local().length > 0}>
        <div class="cx-group">
          <div class="cx-group-title">This computer</div>
          <div class="cx-card"><For each={local()}>{(device) => <Row device={device} />}</For></div>
        </div>
      </Show>
      <div class="cx-group">
        <div class="cx-group-title">Other devices</div>
        <div class="cx-card">
          <Show when={others().length > 0} fallback={<div class="cx-row"><span class="cx-note">Sign in on another device to see it here.</span></div>}>
            <For each={others()}>{(device) => <Row device={device} />}</For>
          </Show>
        </div>
      </div>
      <RenameDialog device={renaming()} onClose={() => setRenaming(null)} />
    </div>
  );
}

function RenameDialog(props: { device: Device | null; onClose: () => void }) {
  const [name, setName] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  createEffect(on(() => props.device, (device) => {
    setName(device?.name ?? "");
    setError(null);
  }));
  const save = async (event?: Event) => {
    event?.preventDefault();
    const device = props.device;
    const next = name().trim();
    if (!device || !next || busy()) return;
    setBusy(true);
    try {
      await rpc.call(M.MUTATE, { op: "renameDevice", deviceId: device.id, name: next });
      props.onClose();
    } catch (failure) {
      setError(`Rename failed: ${failure instanceof Error ? failure.message : "unknown error"}`);
    } finally {
      setBusy(false);
    }
  };
  return (
    <RemoteDialog
      open={props.device !== null}
      title="Rename device"
      description="The new name appears on every device in this workspace."
      busy={busy()}
      onClose={props.onClose}
      footer={
        <>
          <Btn onClick={props.onClose} disabled={busy()}>Cancel</Btn>
          <Btn tone="solid" disabled={busy() || !name().trim()} onClick={() => void save()}>Rename</Btn>
        </>
      }
    >
      <form onSubmit={save}>
        <input class="cx-field" aria-label="Device name" placeholder="Device name" value={name()} onInput={(e) => setName(e.currentTarget.value)} autofocus />
      </form>
      <Show when={error()}>{(text) => <ErrorStrip message={text()} />}</Show>
    </RemoteDialog>
  );
}

/* Dialogs */

function NetworkDialog(props: { open: boolean; onClose: () => void }) {
  const running = () => remote.network?.running === true;
  const [listen, setListen] = createSignal("127.0.0.1:27656");
  const [origin, setOrigin] = createSignal("");
  const [cert, setCert] = createSignal("");
  const [key, setKey] = createSignal("");
  const optional = (value: string) => value.trim() || null;
  const confirm = async () => {
    const reply = running()
      ? await act(M.STOP_BROWSER_ACCESS)
      : await act(M.START_BROWSER_ACCESS, { listen: listen().trim(), publicOrigin: optional(origin()), tlsCert: optional(cert()), tlsKey: optional(key()) });
    if (reply) props.onClose();
  };
  return (
    <RemoteDialog
      open={props.open}
      busy={remote.busy !== null}
      onClose={props.onClose}
      title={running() ? "Turn off local network access?" : "Turn on local network access?"}
      description={running() ? "Devices using this connection will disconnect. You can turn it on again at any time." : "Choose the address and optional HTTPS configuration for this computer."}
      footer={
        <>
          <Btn onClick={props.onClose} disabled={remote.busy !== null}>Cancel</Btn>
          <Btn tone="solid" disabled={remote.busy !== null || state.connection.kind !== "connected"} onClick={() => void confirm()}>
            {remote.busy ? "Updating…" : running() ? "Turn off" : "Turn on"}
          </Btn>
        </>
      }
    >
      <Show when={!running()}>
        <label class="cx-label">
          <Details title="Listen address" text="Use localhost for this computer or a LAN address with trusted HTTPS." />
          <input class="cx-field" value={listen()} onInput={(e) => setListen(e.currentTarget.value)} placeholder="127.0.0.1:27656" />
        </label>
        <label class="cx-label">
          <Details title="Public URL" text="HTTPS URL used by your other devices. Optional for localhost." />
          <input class="cx-field" type="url" value={origin()} onInput={(e) => setOrigin(e.currentTarget.value)} placeholder="https://your-computer.example:27656" />
        </label>
        <div class="cx-label">
          <Details title="HTTPS certificate and key" text="File paths on this computer. Leave blank for a local connection." />
          <input class="cx-field" aria-label="Certificate file path" value={cert()} onInput={(e) => setCert(e.currentTarget.value)} placeholder="Certificate file path" />
          <input class="cx-field" aria-label="Private key file path" value={key()} onInput={(e) => setKey(e.currentTarget.value)} placeholder="Private key file path" />
        </div>
      </Show>
      <Show when={remote.error}>{(text) => <ErrorStrip message={text()} />}</Show>
    </RemoteDialog>
  );
}

function InvitationDialog(props: { open: boolean; onClose: () => void }) {
  const [name, setName] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  createEffect(on(() => props.open, (open) => open && (setName(""), setError(null))));
  const create = async (event?: Event) => {
    event?.preventDefault();
    if (!name().trim()) return setError("Enter a name for the device you want to pair.");
    setError(null);
    await createInvitation(name().trim());
  };
  const copy = async (text: string) => toast((await copyText(text)) ? "Link copied" : "Could not copy the link", "default");
  return (
    <RemoteDialog
      open={props.open}
      busy={remote.busy !== null}
      onClose={props.onClose}
      title="Connect another device"
      description="Open the private link on the device you want to connect. Unused invitations expire in ten minutes."
      footer={<Btn onClick={props.onClose} disabled={remote.busy !== null}>Done</Btn>}
    >
      <form class="cx-label" onSubmit={create}>
        <Details title="Device name" text="A name you recognize in Connected devices." />
        <input class="cx-field" aria-label="Device name" placeholder="Device name" value={name()} onInput={(e) => setName(e.currentTarget.value)} autofocus />
        <div>
          <Btn type="submit" disabled={remote.busy !== null}>{remote.busy === M.CREATE_PAIRING ? "Creating…" : "Create link"}</Btn>
        </div>
      </form>
      <Show when={remote.invitation}>
        {(invitation) => (
          <>
            <Show when={invitation().browserLink}>
              {(link) => (
                <div class="cx-label">
                  <Details
                    title={invitation().appLink ? "Browser link" : "Private connection link"}
                    text={
                      invitation().selfSigned
                        ? "Open this link in a web browser. It may warn about this computer's private certificate the first time."
                        : "Share this link only with the device you want to connect."
                    }
                  />
                  <div class="cx-actions" data-gap="8">
                    <Btn onClick={() => void copy(link())}>{invitation().appLink ? "Copy browser link" : "Copy link"}</Btn>
                    <a class="cx-btn" data-size="md" data-tone="default" href={link()} target="_blank" rel="noopener noreferrer">Open link</a>
                  </div>
                </div>
              )}
            </Show>
            <Show when={invitation().appLink}>
              {(link) => (
                <div class="cx-label">
                  <Details title="Zuse app link" text="Use this link in the Zuse desktop or mobile app. Share it only with the device you want to connect." />
                  <div><Btn onClick={() => void copy(link())}>Copy app link</Btn></div>
                </div>
              )}
            </Show>
          </>
        )}
      </Show>
      <Show when={error() ?? remote.error}>{(text) => <ErrorStrip message={text()} />}</Show>
    </RemoteDialog>
  );
}

function ComputerDialog(props: { open: boolean; onClose: () => void }) {
  const [value, setValue] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  createEffect(on(() => props.open, (open) => open && (setValue(""), setError(null))));
  const connect = async (event?: Event) => {
    event?.preventDefault();
    if (busy()) return;
    if (!value().trim()) return setError("Enter a valid private connection link from the other computer.");
    setBusy(true);
    setError(null);
    try {
      await addComputer(value());
      props.onClose();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : "Enter a valid private connection link from the other computer.");
    } finally {
      setBusy(false);
    }
  };
  return (
    <RemoteDialog
      open={props.open}
      busy={busy()}
      onClose={props.onClose}
      title="Add computer"
      description="Paste a connection link from the other computer to open its workspace."
      footer={
        <>
          <Btn onClick={props.onClose} disabled={busy()}>Cancel</Btn>
          <Btn tone="solid" disabled={busy()} onClick={() => void connect()}>{busy() ? "Connecting…" : "Connect"}</Btn>
        </>
      }
    >
      <form onSubmit={connect}>
        <input
          class="cx-field"
          type="url"
          aria-label="Private connection link"
          aria-invalid={error() ? "true" : undefined}
          placeholder="Paste a private connection link"
          autocomplete="off"
          spellcheck={false}
          value={value()}
          onInput={(e) => setValue(e.currentTarget.value)}
          autofocus
        />
      </form>
      <Show when={error()}>{(text) => <ErrorStrip message={text()} />}</Show>
    </RemoteDialog>
  );
}

function ServeDialog(props: { kind: "setup" | "unlink" | null; onClose: () => void }) {
  const [label, setLabel] = createSignal("");
  const signedIn = () => !!accountUser();
  const confirm = async () => {
    const reply = props.kind === "setup" ? await act(M.LINK_MANAGED_HOST, { label: label().trim() }) : await act(M.UNLINK_MANAGED_HOST);
    if (reply) props.onClose();
  };
  const enabled = () => remote.busy === null && state.connection.kind === "connected" && (props.kind !== "setup" || signedIn());
  return (
    <RemoteDialog
      open={props.kind !== null}
      busy={remote.busy !== null}
      onClose={props.onClose}
      title={props.kind === "unlink" ? "Turn off Zuse Serve?" : "Set up Zuse Serve"}
      description={
        props.kind === "unlink"
          ? "This computer will no longer be available through your account. Local chats stay on this computer."
          : "Link this computer to your Zuse account to access it from your other devices."
      }
      footer={
        <>
          <Btn onClick={props.onClose} disabled={remote.busy !== null}>Cancel</Btn>
          <Btn tone="solid" disabled={!enabled()} onClick={() => void confirm()}>
            {remote.busy ? "Updating…" : props.kind === "unlink" ? "Turn off" : "Link computer"}
          </Btn>
        </>
      }
    >
      <Show when={props.kind === "setup"}>
        <AccountSignIn />
        <Show when={signedIn()}>
          <label class="cx-label">
            <Details title="Computer name" text="A name you recognize on your other devices." />
            <input class="cx-field" placeholder="Computer name" value={label()} onInput={(e) => setLabel(e.currentTarget.value)} />
          </label>
        </Show>
      </Show>
      <Show when={remote.error ?? str(remote.host?.error)}>{(text) => <ErrorStrip message={text()} />}</Show>
    </RemoteDialog>
  );
}
