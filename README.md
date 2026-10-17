# Zuse Native

Rust desktop and browser control plane for coding agents. The native application uses GPUI; the browser target uses GPUI Web compiled to WebAssembly. The engine owns local processes, workspaces, durable conversations and agent execution.

This repository is under active implementation. See [the implementation plan](internal-docs/implementation-plan.md) and [acceptance status](internal-docs/acceptance.md) for the distinction between adapted source, compiled features and verified behavior.

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

## License

AGPL-3.0-only. Adapted third-party source retains its applicable attribution and license in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

The pinned Zkit dependency is public and can be fetched without GitHub credentials.
