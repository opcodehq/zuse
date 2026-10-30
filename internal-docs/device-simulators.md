# Device simulator pane

Open **Simulator** from the chat's right-pane launcher or `+` menu. The centered
onboarding checks Node.js, npm, and the execution host's device tools. Choose
**Allow agent control**, then **Turn on devices**. This explicitly authorizes
Zuse to install the pinned helpers in its private tool directory.

The pane shows host-reported progress through checking requirements, installing
Device Hub, starting it, finding devices, and optionally installing agent tools.
**Cancel setup** withdraws this chat's request. Failures show the failing step,
the host's error, **Try again**, and **Turn off**. An agent-only install/failure also offers
**Continue without agent tools**, keeping the ready hub and manual viewing. A missing Node.js installation
has a download action. Checking and opening the onboarding do not install tools
or boot a device. Completed helper installations are reused on retry/restart.

## Managed helpers

| Helper | Version | Installed entry |
| --- | --- | --- |
| `expo-device-hub` | `0.15.3` | `dist/server/cli.mjs` |
| `agent-device` | `0.21.23` | `bin/agent-device.mjs` |

These are the exact versions in the original app's simulator toolchain. The
installer requires Node.js 22.12 or newer, npm, and at least one usable platform:
Xcode simulator tools on macOS, or Android Platform-Tools plus Emulator. An empty
device list does not block helper setup; create a runtime/device in the host tools.

Each environment stores helpers beneath
`<profile store>/simulator-tools/<package>/<version>/`. npm installs into a private
staging directory with `--save-exact`, `--no-audit`, and `--no-fund`. The installer
checks the pinned manifest version, entry file, and completion marker before
publishing the directory atomically. It verifies the agent CLI before marking its
install complete. npm runs from the staging directory, independently of project
npm configuration. No global npm installation or administrator access is required.

The enabled chats share one host setup job. One chat cannot cancel another chat's
hub. Agent installation is separate: revoking the last agent consent cancels that
installation while manual viewing can continue. Cancel, disable, chat deletion,
and engine shutdown invalidate in-flight work before it can publish a late
ready state. Completed caches remain; partial staging directories are removed.
Owned helpers, npm commands, and agent CLI commands run in isolated process groups
and have bounded lifetimes/output. Cancelling an owned process group also stops
its descendants. Hiding the pane suspends UI polling and frame delivery; a setup
that the user explicitly requested can continue on the host.

## Design

```text
SimulatorPane → device-addressed engine RPC → Simulators
  State.setup ← DeviceSetup (setup.rs)
                 ├─ read-only Node/npm/platform checks
                 ├─ staged pinned npm installation
                 ├─ supervised loopback Device Hub + health checks
                 └─ optional pinned agent installation
  open/close   → Device Hub boot/shutdown + native readiness/identity checks
  manual input → iOS hub HID socket / Android adb
  capture      → xcrun simctl / adb → bounded progressive PNG frames
  agent input  → Agents (agents.rs)
                  └─ owned per-chat/device daemon + fixed-target CLI
MCP device_* / native zuse_device_* → same owning engine and consent policy
```

The engine checks chat membership and execution-host ownership on every request.
Agent tools take chat/host identity from their MCP origin; they cannot configure
consent. Read-only (plan) chats cannot use agent open/close/input operations.
Device support and agent consent default off per chat and reset with the engine;
the completed package cache persists. Configure accepts partial updates, so
changing consent preserves the current enabled state.

The agent helper is used for agent input. Each chat/device has a separate private
state directory, empty managed configuration, and supervised daemon. Its
registration must match the owned PID and supply a valid loopback port/token.
The CLI receives fixed platform, UDID/emulator serial, session, config, and state
flags. Ambient `AGENT_DEVICE_*` configuration is removed; caller text remains a
positional argument. Closing a session or revoking consent cancels the CLI,
daemon startup, and owned descendants, including XCTest workers. The first iOS
agent interaction can prepare an XCTest runner and take several minutes.

The picker uses compact runtime groups and a bottom agent-consent footer. The
viewer has one 36px icon toolbar, a proportionally fitted rounded screen, and a
quiet text field. Lifecycle and consent controls live in the overflow menu.
Tap the image, send text with Enter, use Home and Android Back/Recents, copy a
screenshot, reconnect, disconnect, or explicitly shut down the device. A device
also viewed by another chat cannot be shut down by this chat.

Capture remains a progressive PNG stream: one capture plus a 500ms delay, up to
about 2 fps. Each capture is bounded; PNGs are at most 5 MiB, 8192 pixels per axis,
and 32 million pixels. Decoding happens off the UI thread. The simulator RPC
queue holds at most two frames and discards excess frames without blocking
unrelated replies. Hidden/dropped viewers cancel subscriptions and evict their
rendered image. UI progress polls every 350ms; the ready catalog polls every four
seconds. Progress comes from the host, never from client-side simulated timers.
Forwarded simulator operations have eight minutes of headroom for cold boots,
native readiness checks, and first-agent preparation.

## Host setup

Install Node.js 22.12+ with npm on the execution host and make it available to
the engine, including launches outside a terminal. Restart the engine after
changing its PATH or SDK environment. The in-app **Turn on devices** action
handles the two pinned helpers; these prerequisites still belong to the host.

On **macOS**, install/select full Xcode, finish its first-launch/license steps,
and install an iOS runtime/device. Device Hub boots and attaches its native iOS
helper. Manual taps/Home/ASCII keyboard input use its input-only HID socket;
a separate idb installation is not required by the production engine. Screenshots
use `xcrun simctl io <UDID> screenshot`. iOS requires macOS. The direct native
constructor used by command-runner fixtures retains its idb fallback.

For **Android**, install SDK Platform-Tools, Emulator, a system image, and an AVD;
accept SDK licenses interactively. Set `ANDROID_HOME`/`ANDROID_SDK_ROOT`, or use
the usual per-user SDK location. Zuse passes a resolved SDK root to its helpers.
Java/Command-line Tools are needed for AVD management, rather than for running
an existing AVD. Device Hub boots the AVD; native adb checks its name/serial and
`sys.boot_completed`. The stable app id is `android:<AVD name>`. Reused serials
are rejected before capture, input, or shutdown. Physical phones are excluded.
Linux requires working emulator hardware acceleration/KVM. Manual adb text
supports printable ASCII without `%`; agent input uses the installed helper.

Closing a pane does not issue a device-shutdown request. Explicit shutdown uses
the hub after cross-chat ownership checks. Engine shutdown stops owned helpers;
verify native helper/device lifetime on macOS and a real Android SDK host before
claiming hardware acceptance.

## Agent tools

| Tool | Effect |
| --- | --- |
| `device_list` | Requirements, setup progress, devices, this chat's sessions |
| `device_open` | Boot/register a session and prepare the owned agent daemon |
| `device_screenshot` | Actual PNG image content and dimensions |
| `device_input` | Normalized tap, text, Home; Android Back/Recents |
| `device_close` | Disconnect; optional explicit device shutdown |

Native application tools use the existing `zuse_` prefix and approval categories.
The engine keeps the CLI target private and fixed; generic shell launchers and
arbitrary agent CLI arguments are not exposed through these tools.

## Validation and captures

`cargo test -p zuse-simulator --lib` covers platform discovery, native and managed
boot paths, serial correlation, consent, cache validity, duplicate installations,
late cancellation, agent-install revocation, cross-chat setup ownership, helper
versions, input bounds, and descendant-process termination. Engine tests check
RPC ownership, partial configuration, plan mode, frames, and revocation. UI tests
check frame bounds, taps, selection, hidden subscriptions, and setup-step states.

An explicit network smoke test installs both real pinned packages into a temporary
prefix, verifies the agent CLI version, starts the loopback hub and reads its
catalog, then starts/stops an owned agent daemon. It sends no boot/shutdown/input
requests. It is ignored in the default suite because it needs Node/npm/network:

```sh
cargo test -p zuse-simulator --lib setup::smoke::pinned_helpers_install_verify_and_start_on_linux -- --ignored --nocapture
```

The production native pane fixture captures scripted host-progress states:
[onboarding](screenshots/device-simulator/simulator-onboarding.png),
[checking](screenshots/device-simulator/simulator-checking.png),
[hub installation](screenshots/device-simulator/simulator-installing-hub.png),
[hub startup](screenshots/device-simulator/simulator-starting-hub.png),
[device discovery](screenshots/device-simulator/simulator-discovering.png),
[agent installation](screenshots/device-simulator/simulator-installing-agent.png),
[hub failure/retry](screenshots/device-simulator/simulator-install-error.png), and
[agent failure/retry](screenshots/device-simulator/simulator-agent-error.png).
These are native UI renders with fixture progress, rather than a recording of
the network smoke test. The picker and dark/light viewer use synthetic device
and sample app data. No real simulator/emulator execution is implied by captures.

## Mac and real-emulator acceptance

1. Check missing/old Node, missing npm, no SDK/Xcode, and empty device catalogs.
   Opening/rechecking onboarding must not install or boot anything. Verify the
   Node download action and engine restart after PATH changes.
2. Enable with agent access off: install/start the pinned hub, show each host
   phase, and reach the picker without installing agent tools. Re-enable with
   warm caches and confirm reuse. Then allow agents and verify the separate install.
3. Cancel hub download/startup and agent install; retry network/verification
   failures. Check staging cleanup, process groups, no late-ready state, and that
   another enabled chat keeps its shared hub running.
4. On a Mac, boot iPhone/iPad, watch frames, tap corners, type ASCII punctuation,
   use Home, copy a screenshot, reconnect, disconnect, and shut down. Verify Retina
   scaling, orientation, native HID readiness, and helper recovery after a crash.
5. Use both MCP and native agent tools on a real device. Verify first XCTest
   preparation, fixed target/config/session, real CLI click/type/Home, plan-mode
   denial, and immediate daemon/worker termination on consent revocation.
6. Run an Android AVD on macOS and Linux/KVM: cold boot, serial changes,
   Back/Recents, text, screenshot, and shutdown. Hide/close panes and stop the
   engine; check helper cleanup and expected persistence of booted devices.
7. Exercise remote-host routing, a slow peer connection, and two chats viewing
   one device. Agent sessions and package installation must stay on the owning host.

Automatic SDK/JDK/Xcode installation or license acceptance, global persisted
simulator preferences, high-frame-rate/H.264 transport, gestures/rotation controls,
accessibility snapshot tools, manual Unicode IME input, and app install/launch
wrappers remain deferred.

## Reference audit

Reference: [original PR #908](https://github.com/swarajbachu/zuse/pull/908) and
[setup/Java follow-up #915](https://github.com/swarajbachu/zuse/pull/915).
The setup port follows the current original's `simulator-pane.tsx`,
`simulator-parts.tsx`, `simulator-view.ts`, simulator contracts, server controller,
`toolchain.ts`, `npm-tool-installer.ts`, and session `launcher.ts`. The original
also exposes an agent CLI launcher and accessibility tools; this port keeps the
typed MCP/native tool interface while using the same installed agent helper for
input. Rust owns process lifetime, consent, state, and progressive PNG transport.
