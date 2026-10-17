# Zuse Native

Rust desktop and browser control plane for coding agents. The native application uses GPUI; the browser target uses GPUI Web compiled to WebAssembly. The engine owns local processes, workspaces, durable conversations and agent execution.

This repository is under active implementation. See [the implementation plan](internal-docs/implementation-plan.md) and [acceptance status](internal-docs/acceptance.md) for the distinction between adapted source, compiled features and verified behavior.

Chat access controls and their current provider-specific limits are described in [permissions](internal-docs/permissions.md).

## Repository

- `apps/desktop`: native entrypoint and service commands.
- `crates/ui`: native shell, transcript, composer, settings, files, terminal, voice and visuals.
- `crates/engine`: local sessions, persistence, workspaces and execution.
- `crates/providers`: provider wire protocols and narrowly scoped SDK adapters.
- `crates/protocol`, `crates/rpc`, `crates/client`: shared types, transports and client logic.
- Focused crates own documents, synchronization, themes, Markdown, audio and previews.

Rust dependencies and versions are managed in the root Cargo workspace. JavaScript is limited to provider SDK bridges and generated browser bindings.

## Development

Install the stable Rust toolchain with rustfmt and Clippy. Linux native builds require the platform development libraries listed in `internal-docs/development.md`.

```sh
cargo check -p zuse-engine
cargo run -p zuse
```

For the browser target, install nightly Rust with `rust-src` and the `wasm32-unknown-unknown` target, plus Trunk 0.21.14. Then build and serve the same local engine:

```sh
bash scripts/build-web.sh
cargo run -p zuse-cli -- --ipc-port 0 --data-dir .context/local-profile
```

Open the printed private link in a WebGPU-capable browser. Local development does not require a hosted Zuse account; the selected agent still needs its provider credentials.

For LAN access, supply a certificate trusted by the connecting device and a matching private key. Replace the example hostname with your certificate's LAN hostname or IP:

```sh
cargo run -p zuse-cli -- --listen 0.0.0.0:27656 --ipc-port 0 \
  --tls-cert certs/lan-cert.pem --tls-key certs/lan-key.pem \
  --public-origin https://zuse.local:27656 --data-dir .context/lan-profile
```

Direct HTTPS uses Secure browser cookies and WSS for agent RPC. See [development setup](internal-docs/development.md) for reverse-proxy configuration and platform requirements. Packaging and full cross-device runtime acceptance remain in progress.

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
