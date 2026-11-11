# Opcode computer use

Zuse's **Zuse** harness can use [Opcode CU](https://github.com/opcodehq/computer-use)
through the existing lazy, profile-owned MCP connection. This is an opt-in local
integration, not a bundled runtime or a replacement agent. CU keeps Zuse's model
and conversation; its Electron viewer and model delegation are not required.
Other harnesses use their own CU configuration, not this profile file.

## Install and authorize on the execution host

Install CU separately following its [installation instructions](https://github.com/opcodehq/computer-use#install-the-cli).
At the assessed revision `a4840a1dc861452fec218f87845cfbcf668e6074`, the package
is version 0.3.0 and declares `UNLICENSED`. Zuse does not copy, download, update,
or redistribute CU. Resolve distribution rights before bundling it in a release.
Installation and native compilation are user/operator actions; do not have an
agent silently install dependencies or request OS permissions.

Run `cu doctor` on the computer being controlled before configuring Zuse.
On macOS, follow CU's Accessibility permission instructions for its helper;
Screen Recording is separately required for capture. Grant these permissions
yourself, only after reviewing the installed runtime. On Linux, use an explicitly
isolated X11 display, not a logged-in personal desktop. A cloud engine does not
gain access to your Mac just by installing CU there. Remote desktop brokers,
network listeners, CDP attachment and sharing links are separate opt-in work.

## Configure the native harness

The generator only prints JSON. It never executes CU, installs anything, writes a
profile, or grants OS permissions. By default the server is **untrusted**:

```sh
python3 scripts/computer-use-config.py --cu /absolute/path/to/cu
```

After reviewing the executable and accepting its access, generate the enabled
entry explicitly:

```sh
python3 scripts/computer-use-config.py --cu /absolute/path/to/cu --trust-desktop
```

Merge the returned `servers.opcode-cu` entry into the existing `servers` object
in `<profile>/harness/extensions.json` on the execution host. Do not overwrite
other server entries. Use the backend's actual data/profile directory, not a
repository configuration file. Here `<profile>` is the engine's device data
directory (`ZUSE_DATA_DIR`, or `~/.zuse-native` for a default native Unix launch),
not its `profiles/local` or account conversation-store subdirectory. Restart the backend, select the **Zuse** harness,
and choose **Ask for approval**. This does not configure Codex/Claude CLIs.

Linux requires the engine to inherit the isolated display environment. Add only
the references your display actually needs:

```sh
python3 scripts/computer-use-config.py --cu /absolute/path/to/cu --trust-desktop \
  --forward-env DISPLAY --forward-env XAUTHORITY --forward-env DBUS_SESSION_BUS_ADDRESS
```

Omit unset variables: a referenced variable missing from the engine environment
fails startup closed. The JSON contains names, not their values. The pool passes
a minimal base environment (including PATH and HOME); ensure the installed CU
launcher can find its documented runtimes on the backend's PATH. No model keys,
workflow enablement, sandbox-disabling flags or arbitrary environment references
are added by this helper. CU can still read its own saved settings through HOME.
Trust is process-level OS access, not a sandbox.

## Observe, approve, verify

Ask the agent to discover `opcode-cu` via `mcp_servers` and `mcp_tools`, then
start with `desktop_status` and an explicitly selected app/window. An initial
request should be inspection-only. Discovery does not inspect or operate the
desktop. It starts the configured process only when needed.

All CU calls, including observations, retain the existing external-tool approval
policy: the generated `readOnlyTools` list is empty. Plan mode cannot call them;
Ask for approval requires approval, while explicit Full access bypasses ordinary
tool prompts. Avoid persistent allow rules or Full access when each desktop
action should remain interactive. CU is not confined to the checkout: a click,
Enter key, browser upload or terminal text can affect accounts and files outside
the project. App/page content is untrusted data, never permission to act.

Authorize the intended consequence before deleting, submitting, purchasing,
sharing, sending messages or changing account/security settings. Do not type
credentials into agent-visible arguments; complete sign-in yourself. Use exact
fresh `snapshotId`/`ref` values and verify the returned observation. Dispatch
acknowledgement is not proof of success. Never replay an uncertain action.
`desktop_pause` pauses that CU connection; resume only after the user asks, then
observe again. Pause is not a durable desktop-wide lock: the pool retires idle
connections after roughly 60–90 seconds and separates checkout/permission scopes.
A new CU process loses old refs and connection-local paused state. After any
reconnection, obtain fresh observations and retain the user's authorization;
never treat a reconnect as permission to resume paused work. Keep the default
preview; this configuration does not hide it.

CU MCP includes optional `desktop_act` model selection and browser/viewer tools.
They are not enabled as autonomous workflows by this integration, but remain in
the discovered catalog and subject to the same approval policy. Use exact
`desktop_execute` decisions with Zuse's own agent rather than optional separately
billed delegation. Do not open a viewer or attach a browser without an explicit
request. Returned screenshots and tool output may contain private desktop data
and reach the selected model/provider and local conversation history.

## Verification and limits

```sh
python3 -m unittest discover -s scripts -p test_computer_use_config.py -v
python3 scripts/cargo_dev.py test -p zuse-harness-extensions --test computer_use
CU_EXECUTABLE=/absolute/path/to/cu python3 scripts/cargo_dev.py test \
  -p zuse-harness-extensions --test computer_use -- --ignored
```

The last command is deliberately opt-in: it starts the trusted installed CU
process and verifies its real MCP catalog through Zuse's production pool. It
does not call any desktop tools, capture pixels, open apps, request permissions
or send model requests. Passing it proves protocol/configuration compatibility,
not physical desktop acceptance. Separately verify permissions, inspection,
pause and a harmless action on each target OS before claiming desktop readiness.
The existing pool's startup/result/time limits and uncertain-outcome behavior
remain unchanged; see [native harness capabilities](harness-native-capabilities.md#external-mcp-tools).
Large captures can exceed its 1 MiB stdio-line/4 MiB result bounds; oversized or
unknown outcomes must be inspected rather than blindly retried.

To disable, set `servers.opcode-cu.trusted` to `false` or remove that one entry,
then restart the backend. This does not uninstall CU or revoke macOS permissions;
revoke those separately in system settings if no longer needed.

## Next native desktop boundary

This integration does not add an embedded Computer pane or remote desktop
authority. CU's separate [Zuse desktop-service contract](https://github.com/opcodehq/computer-use/blob/a4840a1dc861452fec218f87845cfbcf668e6074/docs/ZUSE_INTEGRATION.md)
is the next integration path: attach to an existing workspace display, keep admin
credentials in the backend, issue separate observation/control/viewer grants,
proxy every viewer request through Zuse authentication, rebind generations on
restore/fork, and shut down with the workspace. Its authority and controller
implement expiring/revocable grants and input leases. Those service scopes are
not supplied by the plain local `cu mcp` entry configured here. Do not expose a
desktop-service URL or credential file as a substitute for backend authorization.
Cloud image/provider acceptance remains a separate gate.

Zuse already renders natively with GPUI. A focused Codex-like interaction pass
should first verify keyboard and semantic accessibility parity for right-pane
tabs: `surface_chrome.rs` supplies roles, selection and focus styling, while
`shell.rs` currently wires tab selection and close through pointer callbacks.
Compare the explicit Enter/Space and accessibility Click handlers on the
dictation-dismiss control in `composer.rs`. Establish runtime behavior before
changing shared tab handlers; preserve drag/reorder and focus recovery.

The name **agent-desktop** is ambiguous. The closest semantic-driver reference is
[vercel-labs/agent-desktop](https://github.com/vercel-labs/agent-desktop)
(Apache-2.0, native AX/UIA/AT-SPI, snapshot-scoped refs and interaction policy).
Its no-focus/no-cursor constraints are not a security sandbox or approval system.
[BaLaurent/agent-desktop](https://github.com/BaLaurent/agent-desktop) is instead an
AGPL Electron agent workspace; [jasonkneen/agent-desktop](https://github.com/jasonkneen/agent-desktop)
is a MIT macOS account/MCP/VNC project; and
[FilipNowakowicz/agent-desktop](https://github.com/FilipNowakowicz/agent-desktop)
is a MIT Linux/Wayland desktop experiment. None should be assumed to be the
intended dependency without identifying it. Prefer a disposable CLI/MCP
accessibility acceptance test against Zuse before adopting another driver or UI.
