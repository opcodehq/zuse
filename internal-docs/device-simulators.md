# Device simulator pane

The chat's right-pane launcher and `+` menu offer **Simulator**. Enable device
support, inspect setup requirements, and choose an iOS Simulator or Android AVD.
The screen stays beside the conversation. Touch the displayed image to tap;
use the text field and Type/Enter, Home, Screenshot, and Android Back/Recents.
Screenshot copies the PNG to the clipboard. Close releases the chat's session;
Shut down also powers off the device. Reconnect rediscovers and boots the same
stable device id after a stopped device or replaced Android serial.

Device support and **Agent access** default off, separately for each chat. Consent
and open sessions are in engine memory and reset when that engine restarts.
Engine shutdown cancels in-flight capture/boot commands and retires streams;
already booted devices stay running.
Disabling support releases sessions but leaves devices running. Consent changes
take effect immediately, including while another request is booting a device. Hiding the pane
suspends its catalog poll and cancels its frame subscription; it does not shut
down devices. Closing a tab releases that chat's sessions after any in-flight boot completes. A device shared
with another chat cannot be shut down until the other chat releases it. Deleting
a chat releases its consent and sessions; streams also check membership on each
frame to catch deletion or host changes arriving through sync.

## Design

```text
SimulatorPane (crates/ui/src/simulator.rs)
  → device-addressed RPC (crates/protocol/src/simulators.rs, methods.rs)
    → EngineRpc (crates/engine/src/rpc/simulators.rs)
      → Simulators (crates/simulator/)
        ├─ iOS: xcrun simctl → installed idb for input
        └─ Android: SDK emulator / avdmanager / adb
MCP device_* → SimulatorTool → same controller
Native harness zuse_device_* → application-tool policy → same MCP catalog
```

The engine validates chat membership and execution-host ownership before every
request. The UI addresses that host with `targetDeviceId`; simulator methods and
frame streams forward over existing peer RPC. Unary simulator requests have
five minutes of forwarding headroom for discovery and a cold boot. Agent requests take their chat and
host from the MCP origin, require the user's per-chat consent, and cannot change
consent. Native harness mutations follow the existing approval policy. The engine
independently denies agent open/close/input for read-only (plan) chat configs.
Discovery and screenshots remain reads. Screenshot tools return actual image
content in MCP and native model inputs, with dimensions as MCP text metadata.

Capture is a progressive PNG stream: one capture followed by a 500ms delay
(up to approximately 2 fps), not the original's H.264/video helper transport.
Capture commands have deadlines and output bounds; PNGs are at most 5 MiB, 8192
pixels per axis, and 32 million pixels. Base64 frames fit the workspace gateway's
8 MiB payload limit. Image decoding is off the UI thread and bounded. Simulator
checked RPC subscriptions queue at most two frames and discard excess frames without
blocking other replies. Stream errors clear the image and offer Reconnect.
Dropping the subscription cancels capture; there is no detached capture producer.
The pane polls the catalog every four seconds while visible to notice devices
opened by agents. It retains one rendered image and evicts replaced GPU images.

## Host setup

Nothing installs tools, accepts licenses, creates AVDs, or boots a device during
discovery. Commands use argument arrays; only `adb shell input text` needs device
shell quoting, which is escaped before dispatch. Physical Android phones are
excluded. Android actions verify the serial still belongs to the selected AVD
before acting; a stale session can always be closed without shutdown.

### iOS

Use macOS with full Xcode selected by `xcode-select`; complete Xcode's first
launch and license flow yourself. Install an iOS runtime and create a device in
Xcode's device settings. Discovery uses `xcrun simctl list devices available
--json`, filters unavailable and non-iOS devices, and groups newest runtimes first.
Boot uses `simctl boot` then `bootstatus -b`; shutdown and PNG screenshots also
use `simctl` with an explicit UDID.

Touch, typing, and Home require an installed [idb and idb_companion](https://fbidb.io/docs/installation/)
and a companion connected for that UDID. Make `idb` available on the execution
host's PATH, including when launching the app outside a terminal; restart the
host after changing its PATH. The pane reports a missing CLI and
returns companion guidance when input fails. Normalized image taps convert to
logical points using `SIMULATOR_MAINSCREEN_SCALE`. idb text input supports ASCII;
Unicode input through a device IME is deferred. The helper is not bundled or
automatically installed. iOS does not appear bootable on Linux or Windows.

### Android

Install Android Studio, SDK Platform-Tools, Emulator, and a system image. Create
an AVD in Device Manager (or with `avdmanager`). Accept required SDK licenses
interactively. Set `ANDROID_SDK_ROOT` or `ANDROID_HOME`, or put SDK tools on PATH.
The controller also checks the usual per-user SDK directories on each platform.
Restart the Zuse host after installing tools or changing its SDK/PATH environment,
then use Refresh; command paths are resolved when that engine starts.
Command-line AVD management requires a compatible JDK through JAVA_HOME/PATH and Android
SDK Command-line Tools; that optional prerequisite is shown independently.
A preconfigured emulator does not need Java to run.

Discovery uses `emulator -list-avds`, probes `avdmanager list avd -c`, and maps
`adb devices` emulator serials to AVD names with `adb -s <serial> emu avd name`.
AVD ids stay stable across boots (`android:<AVD name>`). Boot runs `emulator -avd
<name> -no-window -no-audio` and waits for the selected AVD's
`sys.boot_completed=1`. This may take two minutes. Hardware acceleration/KVM must
be available on Linux. Screenshots use `adb exec-out screencap -p`; taps, text,
and buttons use `adb shell input`. Text currently supports printable ASCII
without `%`; unsupported text returns an explicit error rather than being
silently mangled. Unicode Android IME support is deferred.

## Agent tools

| Tool | Arguments | Effect |
| --- | --- | --- |
| `device_list` | none | Setup requirements, devices, this chat's sessions |
| `device_open` | `deviceId` | Boot and register a chat session |
| `device_screenshot` | `deviceId` | PNG image and dimensions |
| `device_input` | `deviceId`, `input` | `{type:"tap",x,y}` (0–1), `{type:"text",text}`, or `{type:"home"}`; Android also `back`, `recents` |
| `device_close` | `deviceId`, optional `shutdown` | Release the session, optionally power off |

Native harness names carry the existing `zuse_` prefix. The separate native
filesystem/process tool module needs no device dependency: application tools
already bridge the engine MCP catalog and its approval categories.

## Validation and remaining acceptance

Linux fixtures cover device discovery and physical-phone exclusion, boot serial
correlation, iOS boot waiting and pixel-to-point taps, text quoting, platform
gates, consent and session membership, revocation, cross-chat shutdown, reused
serials, malformed images/coordinates, command deadlines, and output bounds.
Engine tests exercise RPC ownership, plan mode, screenshot/frame dispatch and
revocation. MCP tests check origin binding and image content; the RPC queue test
checks a slow device viewer cannot block another reply. UI tests check frame
validation, letterboxing, selection, and hidden-view cancellation.

The production-shell visual fixture renders synthetic device data. Its captured
[setup](screenshots/device-simulator/simulator-setup.png),
[picker](screenshots/device-simulator/simulator-picker.png),
[dark viewer](screenshots/device-simulator/simulator-viewer-dark.png), and
[light viewer](screenshots/device-simulator/simulator-viewer-light.png) show layout
and theme behavior, including the device text field. They do not show a live device.
See the development guide for the reproduction command.

Before claiming hardware acceptance, verify on a Mac:

1. Missing Xcode, unselected developer directory, incomplete first launch, no
   installed runtime, and missing idb/companion show actionable setup states.
2. Boot a shut-down iPhone and iPad, watch live frames, tap all corners, type,
   press Home, copy a screenshot, close, reopen, and shut down. Check Retina
   scale and orientation after rotation in the system Simulator application.
3. Enable agent access and drive/inspect the same device from both an external
   MCP harness and a native harness; plan-mode mutations must fail.
4. Hide/switch/close panes and windows; check capture processes stop, frame
   memory stays bounded, and the device remains running unless shut down.
5. Test a real Android AVD on macOS and Linux with KVM, including a cold boot,
   serial change, Back/Recents, typing/quoting, screenshot, and host routing.
6. Exercise a remote chat over the peer link and a slow connection, including
   disconnect/reconnect and two chats viewing one device.

Automatic tool downloads/fixes, settings-wide simulator preferences, H.264/high
frame-rate transport, gestures/rotation controls, accessibility snapshots,
Unicode IME input, and device app install/launch wrappers are deferred.
No hardware run is claimed by the Linux fake-device tests.

## Reference audit

Behavior reference: [original PR #908](https://github.com/swarajbachu/zuse/pull/908)
and [Java/setup follow-up #915](https://github.com/swarajbachu/zuse/pull/915).
Inspected the current original app's `apps/renderer/src/components/simulator/*`,
`apps/renderer/src/lib/simulator-{client-bus,view}.ts`, settings simulator pane,
`packages/contracts/src/simulators.ts`, `apps/server/src/simulators/*`,
`packages/client-runtime/src/simulators/{video,reveal}.ts`, and
`packages/agents/src/drivers/simulator-tools.ts`. The original registers tools
through its MCP gateway/provider-MCP session and native harness app-tools bridge;
`device_open` returns an agent-device launcher. This port exposes direct bounded
input and screenshot tools instead. It reuses behavior and theme conventions,
not the original Node helper implementation.
