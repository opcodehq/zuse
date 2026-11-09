# Zuse Native

Rust desktop and browser control plane for coding agents. The native application uses GPUI; the browser target uses GPUI Web compiled to WebAssembly. The engine owns local processes, workspaces, durable conversations and agent execution.

## Install

Download **[v0.26.0-preview.3](https://github.com/opcodehq/zuse/releases/tag/v0.26.0-preview.3)**:

| Platform | Downloads |
| --- | --- |
| macOS Apple silicon | [DMG](https://github.com/opcodehq/zuse/releases/download/v0.26.0-preview.3/zuse-0.26.0-preview.3-macos-aarch64.dmg) · [tar.gz](https://github.com/opcodehq/zuse/releases/download/v0.26.0-preview.3/zuse-0.26.0-preview.3-macos-aarch64.tar.gz) |
| macOS Intel | [DMG](https://github.com/opcodehq/zuse/releases/download/v0.26.0-preview.3/zuse-0.26.0-preview.3-macos-x86_64.dmg) · [tar.gz](https://github.com/opcodehq/zuse/releases/download/v0.26.0-preview.3/zuse-0.26.0-preview.3-macos-x86_64.tar.gz) |
| Windows x64 | [Setup EXE](https://github.com/opcodehq/zuse/releases/download/v0.26.0-preview.3/zuse-0.26.0-preview.3-windows-x86_64-setup.exe) · [Portable ZIP](https://github.com/opcodehq/zuse/releases/download/v0.26.0-preview.3/zuse-0.26.0-preview.3-windows-x86_64.zip) |
| Linux x64 | [DEB](https://github.com/opcodehq/zuse/releases/download/v0.26.0-preview.3/zuse-0.26.0-preview.3-linux-x86_64.deb) · [RPM](https://github.com/opcodehq/zuse/releases/download/v0.26.0-preview.3/zuse-0.26.0-preview.3-linux-x86_64.rpm) · [tar.gz](https://github.com/opcodehq/zuse/releases/download/v0.26.0-preview.3/zuse-0.26.0-preview.3-linux-x86_64.tar.gz) |
| Linux ARM64 | [DEB](https://github.com/opcodehq/zuse/releases/download/v0.26.0-preview.3/zuse-0.26.0-preview.3-linux-aarch64.deb) · [RPM](https://github.com/opcodehq/zuse/releases/download/v0.26.0-preview.3/zuse-0.26.0-preview.3-linux-aarch64.rpm) · [tar.gz](https://github.com/opcodehq/zuse/releases/download/v0.26.0-preview.3/zuse-0.26.0-preview.3-linux-aarch64.tar.gz) |

- **macOS:** open the signed, notarized DMG and drag **Zuse** into **Applications**.
- **Windows:** run Setup for a per-user installation, or extract the portable ZIP. Windows downloads are unsigned.
- **Linux:** install the DEB/RPM with apt/dnf, or use the terminal installer below.

Verify downloads against [SHA256SUMS](https://github.com/opcodehq/zuse/releases/download/v0.26.0-preview.3/SHA256SUMS).

Install the preview from a Linux terminal:

```sh
(
  set -e
  installer=$(mktemp)
  trap 'rm -f "$installer"' EXIT
  curl -fsSL https://github.com/opcodehq/zuse/releases/download/v0.26.0-preview.3/install.sh -o "$installer"
  sh "$installer" --version 0.26.0-preview.3
)
```

The terminal installer requires Python 3.11+, verifies download checksums and
installs for the current user into `~/.local/share/zuse`, with launchers in
`~/.local/bin`. After installation, run `zuse`. Linux uses Ubuntu 24.04/glibc
2.39 as its build baseline. See [installation and runtime requirements](internal-docs/packaging.md)
for system dependencies, alternative formats and uninstall commands.

This repository is under active implementation. See [the implementation plan](internal-docs/implementation-plan.md) and [acceptance status](internal-docs/acceptance.md) for the distinction between adapted source, compiled features and verified behavior.

Chat access controls and their current provider-specific limits are described in [permissions](internal-docs/permissions.md).
The [native Rust harness](internal-docs/harness-native-capabilities.md) provides durable orchestration, bounded parallel reads, local skills and lazy external tools.

The Rust hosted-cloud rewrite and its acceptance checklist are tracked in
[cloud implementation](internal-docs/cloud-implementation.md). Its current API supports
authenticated catalog/archive reads and explicitly gated runtime enrollment, renewal
and gateway transport, plus encrypted text-message mailbox admission and recovery.
The guest executable passes local process acceptance; live
provider execution, the full archive lifecycle and cutover remain in progress.

## Repository

- `apps/desktop`: native entrypoint and service commands.
- `apps/cloud`, `crates/cloud`: hosted API, authorization and lifecycle coordination.
- `apps/cloud-runtime`, `crates/cloud-runtime`: enrolled guest and persisted storage identity.
- `crates/ui`: native shell, transcript, composer, settings, files, terminal, voice and visuals.
- `crates/engine`: local sessions, persistence, workspaces and execution.
- `crates/providers`: provider wire protocols and narrowly scoped SDK adapters.
- `crates/protocol`, `crates/rpc`, `crates/client`: shared types, transports and client logic.
- Focused crates own documents, synchronization, themes, Markdown, audio and previews.

Rust dependencies and versions are managed in the root Cargo workspace. JavaScript is limited to provider SDK bridges and generated browser bindings.

## Development

For native app bundles and user-level Linux installation, see
[local packaging](internal-docs/packaging.md).

Install the stable Rust toolchain with rustfmt and Clippy. Linux native builds require the platform development libraries listed in `internal-docs/development.md`.

```sh
python3 scripts/cargo_dev.py check -p zuse-engine
python3 scripts/cargo_dev.py run -p zuse
```

The Cargo launcher shares build output across Git worktrees and avoids rebuilding
unchanged icon assets. Native and browser dev launchers use it automatically.
See [build speed and disk usage](internal-docs/development.md#build-speed-and-disk-usage)
for cache locations and debugging options.

For the browser target, install nightly Rust with `rust-src` and the `wasm32-unknown-unknown` target, plus Trunk 0.21.14. Then build and serve the same local engine:

```sh
bash scripts/dev-web.sh
```

The command builds the browser and matching engine from the same checkout, then starts a persistent local workspace in `.context/local-profile`. Pass server options such as `--listen 127.0.0.1:3000` or `--data-dir /path/to/profile` to override the defaults. Run `bash scripts/dev-web.sh --help` for usage.

Open the printed private link in a WebGPU-capable browser. Local development does not require a hosted Zuse account; the selected agent still needs its provider credentials.

A thread can contain multiple conversations. Use the **+** beside its conversation tabs to start another chat in the same checkout, with its own history and agent session. Archiving or restoring the thread includes its conversations.

Browser drafts are saved per engine and tab, including committed attachment references. Reopening a chat restores its unsent draft; interrupted sends and queue edits appear as recovery copies for review. Fresh sessions can recover saved copies from other tabs. Attachment copies retain their execution host, and storage failures show a retry control while preserving the live text.

The browser’s **Settings → Providers** page includes installed versions, update actions and usage. Expand a provider to choose Notify, Auto when idle or Off, inspect progress, or copy a manual update command. The refresh control checks versions on the connected computer. Enable provider update notifications in General to show update notices above the composer.

For LAN access, supply a certificate trusted by the connecting device and a matching private key. Replace the example hostname with your certificate's LAN hostname or IP:

```sh
cargo run -p zuse-cli -- --listen 0.0.0.0:27656 --ipc-port 0 \
  --tls-cert certs/lan-cert.pem --tls-key certs/lan-key.pem \
  --public-origin https://zuse.local:27656 --data-dir .context/lan-profile
```

Direct HTTPS uses Secure browser cookies and WSS for agent RPC. See [development setup](internal-docs/development.md) for reverse-proxy configuration and platform requirements. Packaging and full cross-device runtime acceptance remain in progress.

## Languages

Choose **Settings → General → Language** to follow the system language or select
English, French, German, Simplified Chinese, Traditional Chinese, Japanese, or
Korean. Desktop and browser choices are saved locally. CLI output uses
`zuse-server --language zh-Hans` or `ZUSE_LANGUAGE=zh-Hans`; command and protocol
spellings stay the same. Non-English translations are previews pending screen
review. See [localization](internal-docs/localization.md) for contribution and
review instructions.

## Voice

Hold the microphone beside the composer model selector to dictate. Words appear in the draft phrase by phrase; release to finish. Cancelling keeps your existing draft.

Enable dictation and choose a microphone in **Settings → Voice**. Dictation uses your existing Codex ChatGPT sign-in; no local speech-model download or realtime session is required.

## Import from Electron

The native GPUI app offers an import when it first finds an Electron profile on a fresh workspace. You can also open **Settings → General → Import from Electron** at any time. Review the detected profile, choose whether to bring over supported settings, and select **Import**. **Start fresh** skips migration.

The import preserves projects, active and archived chats, conversation tabs, message history, available attachments, and compatible preferences. It creates local backups before writing GPUI data and supports resuming an interrupted import without duplicating chats. The original Electron profile stays in place. Existing GPUI conversations and later edits to already imported chats are preserved.

Sign in with the same Zuse account to access your hosted plugins. Electron-only preferences and local plugin definitions are retained in the private backup; command-based Electron plugins are not yet supported in GPUI. The review and completion screens report unavailable attachments and settings that could not be applied.

## Projects

Choose **New project** to open a folder, clone a repository, or create an Empty,
Next.js, or Turborepo project. Projects go on this computer; when another paired
computer is online, the dialog offers it as well. Recent projects and folder search
are available without waiting for GitHub.

GitHub repository browsing and optional private repository creation use `gh auth login`
on the execution host. Next.js and Turborepo starters require Bun there. Cloning and
scaffolding run in the background: you can compose a prompt while progress appears
above the composer, then send once the files are ready. Failed operations offer retry;
a publishing failure preserves the local project.

## Paired access

In the browser workspace, open **Settings → Devices**, name the device and create an invitation. Share its private link with that device. Opening it imports paired access and clears the secret from the address bar. The tab retains access across reloads; revoke it from the host's pairing list to disconnect it. Paired chat, settings and file uploads use encrypted RPC.

For the desktop app, save the copied invitation in a private text file and open:

```sh
zuse --paired-link-file ~/.zuse/paired-engine-link.txt
```

An explicit paired connection never falls back to another local engine. Remote destinations require trusted HTTPS; localhost HTTP is supported for development. Native Settings → Devices includes pairing controls. Outbound relay hosting is available through the server commands below; physical desktop acceptance and complete remote connection management remain in progress. See [paired access](internal-docs/remote.md) for the current transport and acceptance scope.

## License

AGPL-3.0-only. Adapted third-party source retains its applicable attribution and license in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

The pinned Zkit dependency is public and can be fetched without GitHub credentials.


### Outbound paired relay

The relay serves the browser bundle and forwards encrypted records. Pairing keys stay on the execution host. Generate an independent routing credential and copy it privately to the host:

```sh
umask 077
openssl rand -hex 32 > relay-host-token
cargo run -p zuse-relay -- --host-token-file relay-host-token --assets dist/web
```

Then start the execution host with outbound relay access:

```sh
cargo run -p zuse-cli --bin zuse-server -- --relay-origin http://127.0.0.1:27657 --relay-host-token-file relay-host-token
```

Create an invitation from the execution host's Settings → Devices. Its link points to the relay. The host opens outbound sockets; agent RPC, file uploads and subscriptions still authenticate and decrypt on the execution host. Remote relay origins require trusted HTTPS. For LAN/public serving, use `--tls-cert`/`--tls-key`, or explicitly configure `--public-origin https://…` behind a trusted HTTPS reverse proxy. The routing credential authorizes host registration only; it cannot authenticate paired RPC.

This is an operator-managed relay. Embedded-desktop relay configuration, account-scoped broker provisioning, encrypted end-to-end heartbeat acceptance and production deployment remain pending. See [remote access](internal-docs/remote.md).

Control icons use the bundled Hugeicons free set. Builds with `HUGEICONS_LICENSE_KEY` prepare licensed solid rounded icons for both clients; [icon build configuration](internal-docs/icons.md) covers the optional version pin and CI setup.
