# Development environment

Use stable Rust, rustfmt and Clippy. Keep versions in the workspace and lockfile. Browser builds additionally require the `wasm32-unknown-unknown` target and Trunk.

Existing conversations can select another provider directly in the model picker.
The next send uses the saved provider/model choice and the engine's context
handoff, retaining the same chat history. Each conversation tab shows its own
provider icon. Pending first sends keep their original configuration snapshot.

## Build speed and disk usage

Use the Cargo launcher for local work:

```sh
python3 scripts/cargo_dev.py check -p zuse-engine
python3 scripts/cargo_dev.py run -p zuse
python3 scripts/cargo_dev.py test -p zuse-protocol --lib
python3 scripts/cargo_dev.py --print-target-dir
```

Native and browser development launchers select the same cache automatically.
Each repository gets one directory under `${XDG_CACHE_HOME:-~/.cache}/zuse/cargo`,
identified by its common Git directory; linked worktrees reuse it. Browser output
uses its `browser` subdirectory to isolate nightly/build-std artifacts. Set
`CARGO_TARGET_DIR` to an absolute path to override the location. CI continues to
use checkout-local `target` directories for Actions caching.

Cargo locks the shared output during builds, so simultaneous worktree builds
wait for one another. Switching branches may still rebuild changed crates;
sharing avoids maintaining a full dependency tree for every worktree. Bare
`cargo` uses the usual checkout-local target and does not get this sharing.

Development builds omit debug symbols and incremental output to reduce disk
usage, while retaining optimized dependencies for GPUI. For workspace debugging,
set `CARGO_PROFILE_DEV_DEBUG=2`; dependency debugging additionally requires
an override of `[profile.dev.package."*"].debug`. Set `CARGO_INCREMENTAL=1` to
opt into faster incremental edits at the cost of more disk space.

Cargo and native/browser development launchers check the development cache
before builds and stop if it exceeds 20 GiB, printing a cleanup command. Stop
concurrent builds, then run `python3 scripts/build_cache.py --clean` to remove
oversized development artifacts from the shared cache (or pass `--target-dir`
for an old checkout-local cache). Cargo clean does not lock out concurrent
builds, so cleanup is explicit. Release outputs and source files remain.
This is a pre-build retention budget, not a filesystem quota: one build can
exceed it. Bare `cargo` bypasses the guard and cache sharing. Changing profile
settings does not remove old artifacts.

The launcher also supplies a hash of the icon license configuration. This lets
Cargo reuse unchanged icons and their UI dependents without writing the license
key into Cargo fingerprints. Key changes are picked up on the next invocation;
restart a running native watcher after changing its environment. Bare Cargo
retains automatic license detection but must rerun the icon selector every time.

Existing checkout-local `target` directories are not moved or deleted. After
stopping builds that use an old directory, `cargo clean --target-dir /path/to/old/target`
reclaims it; the next build using that directory must compile again. To clear
the shared cache, use `python3 scripts/cargo_dev.py clean`. Avoid cleaning as
part of ordinary development, since that removes the output that speeds up builds.

## Native development reload

Run `python3 scripts/dev-native.py` on macOS or Linux. It builds the app, starts
one headless development engine, then watches Rust sources and bundled assets.
Successful builds relaunch only the native UI; active agents and chat history
stay in the engine. Failed builds leave the current window running until the
next edit. No watcher package is required.

This is automatic rebuild/relaunch, not Rust code replacement inside a running
process. Unsaved editor buffers and UI-only drafts do not survive a relaunch.
Engine or protocol changes require restarting the command so both processes use
the new engine code. Ctrl+C stops the UI and its development engine.

The default profile is `.context/native-profile`, with IPC on `127.0.0.1:27664`.
Use `--data-dir /path/to/profile` and `--ipc-port 27665` to override them;
`ZUSE_DATA_DIR` and `ZUSE_IPC_PORT` also work. An occupied port fails startup
rather than connecting to a different engine. Existing provider CLI logins work;
cloud sync is opt-in through `ZUSE_WORKOS_CLIENT_ID` or `ZUSE_EDGE_TOKEN`.

## Linux native dependencies

The native platform needs a C/C++ compiler, pkg-config, CMake, OpenSSL, Fontconfig, FreeType, XCB, XKBCommon/X11, Wayland, Vulkan loader, ALSA and Clang development files. Embedded browser previews additionally require WebKitGTK 4.1 and JSON-GLib development files. Package names vary by distribution.

The cloud development VM runs Amazon Linux 2023, whose configured package repositories lack WebKitGTK 4.1. Native builds were verified in an Ubuntu 24.04 userspace with the real WebKitGTK and JSON-GLib development packages. Use a distribution providing these packages for native development; a missing package must not silently disable previews. Compilation alone does not verify a rendered desktop or embedded browser.

On Ubuntu 24.04, install the native build dependencies with:

```sh
sudo apt-get update
sudo apt-get install -y build-essential cmake clang libclang-dev pkg-config libssl-dev \
  libfontconfig-dev libfreetype-dev libxcb1-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev libasound2-dev \
  libwebkit2gtk-4.1-dev libjson-glib-dev
```

The SDK model transport additionally requires Node.js 22 or newer. Individual agent CLIs require their own installation and provider login.

### Linux native browser scenario

`bash scripts/test-native-browser-linux.sh` builds the native browser fixture and runs the production shell with real WebKitGTK in an isolated Xvfb display. Install `xvfb xauth xdotool imagemagick dbus-x11 mesa-vulkan-drivers fonts-dejavu-core` for this scenario. It needs no provider credentials, uses temporary profile storage and a loopback website, and saves its log, PNGs and result JSON under a new `.context/native-browser-linux-*` directory.

Pass `--binary /absolute/path/to/browser-fixture` to reuse a completed build, or `--output /path/to/empty-directory` to select the evidence directory. The run has a five-minute external deadline; an independent heartbeat fails sooner if an animated page starves GPUI's main executor. Failure preserves evidence and prints the log tail. The scenario covers native browser input/compositing with synthetic conversation data, not real agent execution, physical input devices, OS portals or hardware-GPU performance.

## Validation policy

Use `cargo check` for integration feedback. Keep tests centered on observable behavior and run relevant suites at milestones. Run native and browser acceptance separately: a web screenshot does not verify platform-specific desktop integration.

### Browser input conformance

The opt-in input fixture renders the production single-line editors without an
engine connection. Build it into a separate directory:

```sh
bash scripts/build-web.sh --features browser-input-fixture \
  --dist ../../.context/browser-input-fixture-dist
```

With a dedicated Chromium session running with WebGPU enabled, pass its
`DevToolsActivePort` file to the regression runner (Node.js 22+):

```sh
node scripts/test-browser-input.mjs --assets .context/browser-input-fixture-dist \
  --devtools-file /path/to/chrome-profile/DevToolsActivePort
```

The runner serves the fixture on loopback, creates and closes its own isolated
browser context, and checks editor contents and submit events. Cases cover
paste/composition immediately after focus changes, Unicode preedit replacement
and cancellation, stale composition events after editor/window/DOM blur,
keyboard paste, and IME confirmation versus normal Enter submission. It uses
both DOM events and trusted Chromium input commands. Physical IME candidate
windows, OS clipboard menus and macOS/Linux hardware input remain separate
acceptance gates. Build the ordinary browser application with
`bash scripts/build-web.sh`.

The shared editors and plugin forms provide Cut, Copy, Paste and Select all
menus with mouse/keyboard navigation. Explicit browser Paste actions use an
asynchronous clipboard read. A changed edit, selection, focus, editability or
newer paste request retires an older result; empty or blocked reads preserve
the text. Blocked reads show the keyboard-paste fallback. The conformance runner
also covers delayed reads, undo/redo, read-only transitions, menu interactions
and WASM Meta-key editing commands.

In a Zkit checkout, `cargo --config .github/clipboard-tests.toml test --locked
-p gpui-base --lib paste` runs the native clipboard/undo regressions. The toolkit
workflow runs those checks on Linux and macOS; hosted results and physical
clipboard behavior require their own verification.

## Account and hosted plugins

Local sessions do not require a cloud account. Settings → Plugins includes the engine-owned Zuse account and managed plugin connections. Native clients whose engine runs on the same machine sign in through the browser: AuthKit redirects to an ephemeral `http://127.0.0.1:<port>/callback` loopback and the engine redeems the code with PKCE (`CloudAccountSignIn` with `{"browser": true}`). Remote browser clients, engines whose loopback cannot bind, and providers that explicitly reject the loopback redirect fall back to device authorization. `zuse login` likewise opens the browser with a loopback redirect unless `--no-browser`, `ZUSE_NO_BROWSER`, an SSH session or a missing display selects the paste-code page. Profile metadata and progress stream through `CloudAccountStatus`. Refresh credentials are stored in the device profile's `account/session.json` with private Unix permissions and atomic replacement. Access tokens stay in engine memory and never appear in account RPC responses.

Native account sign-in requires `http://127.0.0.1:*/callback` in the WorkOS client's allowed redirect URIs. The engine chooses an available loopback port and redeems the authorization code with PKCE. WorkOS supports a wildcard loopback port for native clients ([AuthKit redirect rules](https://workos.com/docs/reference/authkit/authentication/get-authorization-url)). Verify this configuration before enabling browser sign-in in a release: on November 5, 2026, an authorization request for the default public client with `http://127.0.0.1:49153/callback` redirected to `error.workos.com/redirect-uri-invalid`. The engine probes authorization without following redirects before opening the browser. An explicit `redirect-uri-invalid` response selects device authorization for native account sign-in and the hosted paste-code flow for terminal login. Signing out during this probe cancels the attempt. Browser sign-in becomes available once the redirect is registered; the fallback keeps sign-in usable until then. Headless and remote clients retain device authorization.

`ZUSE_API_URL` selects an HTTPS API origin (default `https://api.zuse.sh`). `ZUSE_WORKOS_CLIENT_ID` selects the public identity client for that deployment. Native plugin OAuth returns to an available loopback port from 8976–8979; the engine consumes the account-bound confirmation ticket. Browser clients running against localhost use that callback too. Remote browser clients currently use the hosted web return and still need end-to-end acceptance.

All injected agents use the same `plugins_list`, `plugins_search`, `plugins_schema`, and `plugins_call` MCP tools. The gateway derives the origin chat from the injected MCP context, rejects calls in read-only sessions, and stores chat/account bindings in `plugin-sessions.sqlite`. Switching accounts requires a new plugin session for previously bound chats. Discovery can reconnect; external calls are never automatically replayed after an uncertain transport failure. MCP cancellation cancels in-flight hosted tool calls, and transport closure cancels the connection’s outstanding engine requests; cancellation cannot reverse a side effect already performed by the remote service.

The HTTP connection pool and refresh authority are shared across concurrent agents. Hosted credentials and tool execution remain in the engine. Both GPUI frontends expose masked API-key entry for catalog plugins and Add MCP server with automatic sign-in, Bearer token or custom-header authentication. The shared form validates names, HTTPS URLs and headers, then submits the existing tenant-scoped plugin RPC; it never saves its credential draft in UI preferences. Account changes, disconnects, cancellation and accepted submission discard the form. Management actions respect the current workspace’s `canManage` flag. Disconnect and custom-server removal require an inline confirmation. Pending sign-in can be canceled or polled; it blocks replacing the attempt or switching workspaces until completion. Both frontends share request/response state that keeps refreshes scoped to the selected workspace, validates response scope and preserves the last valid rows on malformed replies. Uncertain mutations trigger a read, never a replay. Browser reconnection refreshes the same workspace; account changes retire old request receivers. Live OAuth, hosted execution, organization policy and full UI/platform acceptance remain tracked in the acceptance matrix.

## Local browser engine

`cargo run -p zuse-cli -- --listen 127.0.0.1:27656 --ipc-port 0 --data-dir .context/local-profile` starts an isolated profile with an available agent-tool RPC port. Relative profile paths are resolved before workspace creation. Each launched agent receives this engine's actual RPC endpoint. SIGTERM and Ctrl+C both drain the runtime. Keep the printed browser access link private.

Direct HTTPS accepts `--tls-cert` / `--tls-key` (or `ZUSE_TLS_CERT` / `ZUSE_TLS_KEY`) for a PEM certificate chain and matching PKCS#8, RSA or SEC1 key. The certificate must cover the hostname/IP used by the browser, and its issuing CA must be trusted on that device. Certificate changes take effect after restarting the server. With a wildcard listener, set `--public-origin https://<LAN-host>:<port>` to print the intended client URL and restrict browser origins to it. The origin has no trailing slash.

Without direct TLS, `--public-origin` remains available for an HTTPS reverse proxy. The proxy terminates HTTPS and forwards the browser HTTP/WebSocket routes to the private backend. Non-loopback HTTP listening requires this explicit reverse-proxy configuration; an unconfigured LAN HTTP listener fails before starting engine workers. Agent-tool IPC always binds loopback, independently of the browser listener.

HTTPS uses Secure, HttpOnly, SameSite=Strict cookies. TLS handshakes run concurrently with a bounded pending set and deadline, so a stalled handshake does not block ordinary clients. Browser subscriptions close when the server receives SIGTERM/Ctrl+C, including when the remote client is not reading; RPC input closes and its request tasks are cancelled before engine shutdown. Interrupted mutation delivery retains the existing no-automatic-replay behavior.

For an interrupted-write UI fixture, build and run `cargo run -p zuse-engine --example recovery-fixture -- <new-profile-directory>`, then serve that profile. The fixture refuses existing directories, writes an evidence file, and deliberately leaves one durable tool receipt requiring inspection. It uses a scripted model and requires no provider credentials.

## Browser development startup

Run `bash scripts/dev-web.sh` from the checkout to build the GPUI Web bundle and start its matching stable-Rust engine. Open the printed private link in a WebGPU-capable browser. The launcher allocates the agent-tool IPC port automatically and keeps local data in `.context/local-profile`, or `ZUSE_DATA_DIR` when set. All server options pass through; an explicit `--data-dir`, `--ipc-port` or `--assets` replaces the corresponding launcher default. Paths containing spaces are preserved.

For example, `bash scripts/dev-web.sh --listen 127.0.0.1:3000 --data-dir /path/to/profile` starts a separate local workspace. Use `bash scripts/dev-web.sh --help` for the startup requirements. `scripts/build-web.sh` remains available when only the browser bundle is needed. Rebuild and restart the engine alongside the browser after protocol changes.

## Local tool results

New tool results are saved in the profile's `docs.sqlite3` `tool_blobs` table before their references enter the transcript. Keys include the assistant entry, provider call identity, and content digest, so repeated provider call IDs cannot replace another turn's output. Output and diff save in one transaction. Deleting a chat removes its tool payloads. Ordinary transcript snapshots carry refs and byte counts, without full output text.

`FetchToolBlob` accepts `blobRef`, optional `offset`, and optional `limit`. Paged callers use UTF-8 byte cursors with a 64 KiB maximum page. Replies include `text`, `nextOffset`, `totalBytes`, and `done`. The legacy text response remains available to native clients. Both native and browser result reads address the owning engine; local tool data is not uploaded by this path. Historical results that never retained payloads cannot be recovered through these new refs.

The browser retains four result pages and virtualizes visible lines. It pages raw output and highlights complete small diff payloads; large diff JSON remains accessible as raw pages. Provider output limits remain authoritative: Claude/Codex text results are capped at 256 KiB with an explicit truncation marker; other providers keep their own adapter limits. This does not establish a complete filesystem diff for every provider.

## Browser workspace editing

File reads retain the editor value present when they started and apply only if that value is still current. Starting another read cancels the earlier request. Closing the editor cancels outstanding reads. Saves retain the starting file, checkout, and content hash with the submitted text; their acknowledgements cannot update another file. Navigation waits for a pending save even if the user undoes edits back to the earlier baseline. A successful save updates the saved baseline, while text typed afterward remains unsaved.

Workspace review loads the shared checkout summary, then reads a selected file against that summary's checksum. Large binary/truncated sources receive explicit notices. Inline textual comparison has a 100,000-line safety limit and a bounded diff computation deadline. The current editor can inspect the file separately; large source and original-format constraints still come from the engine. Directory pages append without dropping earlier entries, and the file list renders visible rows only.


## Browser connection recovery and navigation

The browser transports requests with a local connection generation. A socket loss retires that generation and reports its unanswered requests. After reconnect, the workspace verifies the same device identity and reopens read subscriptions. Requests from an older generation cannot reach the new socket. Sends, saves, terminal writes, queue changes and other actions are never automatically replayed. Messages whose acknowledgements were lost remain recoverable until their exact durable message ID appears in the selected transcript; ambiguous queued messages require explicit review and restoration.

Invalid transcript deltas preserve the entire last valid transcript, including text parts. The browser cancels a failed history read and requests a fresh initial snapshot, with at most three retries (200ms, 600ms, 1800ms) before showing Retry. Initial snapshots have a 15-second deadline. Sends, queue changes and approval submissions wait for a valid snapshot; recovery never replays mutations or confirms delivery from rejected rows. Drafts and unrelated alerts remain intact. Navigation, host changes and disconnection cancel old recovery tasks. The last context measurement stays marked as unconfirmed until a valid frame arrives; an exhausted retry budget shows that a refresh is needed.

Composer drafts, editor text, recovery notes and loaded output stay in memory during a connection interruption. An open file is reread before further writes or navigation. A lost save acknowledgement is reconciled against the submitted text, checkout and current disk hash; later editor changes are retained. This does not persist unsaved file buffers through a full page reload. Terminal subscriptions resume after the last sequence number; reconnect does not launch a terminal or resend a command.

The server's default browser access token lives in the data directory's `browser-access-token` file, privately readable on Unix. Concurrent startups use atomic create-if-absent publication. Explicit `--token` / `ZUSE_BROWSER_TOKEN` overrides still apply. A same-origin authenticated session probe distinguishes an expired credential from a transient connection failure without issuing another cookie.

Session navigation searches titles, projects, branches and previews locally. Ctrl/Command-K opens search and Ctrl/Command-N starts a session. The browser shell uses a 296px session rail, bounded transcript width, a two-row rounded composer that expands for longer drafts and attachments, and a combined provider/model picker. Provider badges use each chat's configured harness. Browser/native visual and interaction parity remains incomplete.


## Navigation and composer controls

Threads shows conversations by recency; Projects groups them by stable repository identity, including worktrees, with a separate projectless group. The native selector reuses the existing saved sidebar organization. The browser saves its view, collapsed groups, default provider/models and recent selections in same-origin local storage (`zuse.workspace.preferences.v1`). These preferences contain no account credentials and do not persist unsaved editor buffers or drafts across page reloads.

The combined browser picker includes a provider rail, search, recent models, default stars, keyboard selection and provider-specific catalog errors. Catalog request ownership prevents superseded or disconnected replies from replacing another provider's models. The browser prefetches enabled agents’ models when their catalog arrives, before the picker opens, and also discovers a committed chat’s agent when it is disabled. Conversations on another execution host load both agent descriptors and models from that host; local defaults and provider settings keep using the connected engine’s catalogs. Host changes retire old reads and clear the previous remote catalog. Reconnect revalidates retained catalogs once, and failed probes preserve the last valid rows until explicit refresh. Agent identity follows the saved chat configuration when refreshed local rows arrive. Sending cannot fall back to a local provider when the configured remote agent is absent; the composer keeps its draft and reports the missing host setup. Reasoning uses the Zkit stepped slider with pointer and arrow/Home/End controls; model options come from the selected provider's actual catalog. A run carries its selected model, reasoning, options and access mode. Existing chat metadata stores its initial configuration, so it is not an oracle for later per-run overrides.

Composer notices attach above the input and expose expansion, copy, dismissal and explicit message restoration. Restoring an errored message preserves durable attachment references and refuses to overwrite a draft. Native and browser send controls share the ordered dither GPU painter; browser restoration and question submission use the same action treatment. It allocates no bitmap and schedules no continuous animation. Full platform interaction and visual parity remain tracked separately.

## Native chat configuration

Model, reasoning, Context, Fast and access changes share one per-chat save coordinator. An execution-host configuration watch supplies canonical values; replicated conversation lists cannot overwrite the selected host snapshot or a pending choice. Model changes own only model-related fields, and permission changes own only the sandbox field. Conditional writes compare the current host configuration, coalesce later choices, and survive navigation. Sending and explicit queue delivery wait for confirmed configuration.

An interrupted or malformed acknowledgement triggers a fresh read and discards queued choices rather than replaying a possibly committed mutation. The access menu can refresh saved choices without writing. Attaching a replacement engine clears previous transport proof and pending intents before opening a new owner watch. This does not establish complete native transport reconnection or physical paired-device acceptance.

## Browser context status

The composer’s context ring reads `contextUsage` from the selected execution host’s `WatchDocMessages` stream. The card shows reported tokens/capacity, a 38-segment meter and remaining capacity. It distinguishes zero usage from unknown usage, hides unreported or zero capacity, clamps the displayed percentage and saturates remaining capacity at zero. It never derives occupancy from cumulative billing totals or guesses a capacity from a model name.

Changing conversations or execution hosts clears the previous measurement. Disconnection preserves the last reported value with an offline label; reconnecting keeps it marked as refreshing until a valid transcript update arrives. Malformed transcript updates do not replace the last valid measurement. The card is read-only and shares its GPU ring painter with the native indicators.

### Device simulator development

The host controller is `zuse-simulator`; its `Runner` trait allows boot, input,
serial mapping, screenshots and missing-tool states to be tested without Xcode
or an Android SDK. Start with `cargo test -p zuse-simulator --lib`,
`cargo test -p zuse-rpc --lib simulator`, `cargo test -p zuse-mcp --lib simulator`,
`cargo test -p zuse-engine --lib simulator`, and
`cargo test -p zuse-ui --lib simulator`. Device setup and Mac/Android hardware
acceptance are documented in [device-simulators.md](device-simulators.md).

For visual snapshots on a host with a display (or Xvfb), run
`cargo run -p zuse-ui --example simulator-fixture --features simulator-fixture -- .context/simulator-shots`.
The fixture uses the production chat shell with synthetic setup, picker and
screen data. Focused native pane images cover onboarding, checking, Device Hub
installation/startup/discovery, agent installation, and both retry states. The
full-shell captures cover picker and dark/light viewer; the fixture also opens
and dismisses the overflow menu. These are scripted host states, not a recording
of real downloads or device execution. It does not boot a device.
The synthetic app image has an editable source at
`crates/ui/examples/simulator-sample.svg` and a checked-in PNG used by the fixture.

On Linux, use X11 (unset `WAYLAND_DISPLAY`) or Xvfb and install ImageMagick:
the fixture explicitly presents each frame, then captures its X11 window with
`import` because GPUI's X11
backend does not implement `render_to_image`. This capture dependency is only
for the visual fixture, not the simulator pane.

The optional real helper smoke test downloads the exact reference versions into
a temporary prefix, verifies the agent CLI, reads a real loopback hub catalog,
and prepares/stops a supervised agent daemon without sending device commands:
`cargo test -p zuse-simulator --lib setup::smoke::pinned_helpers_install_verify_and_start_on_linux -- --ignored --nocapture`.
It requires Node.js 22.12+, npm, and network; it is ignored in the normal suite.
