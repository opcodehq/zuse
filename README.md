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

Native/browser startup, packaging and integration commands will be documented as they become verified. Local development must not require a hosted account.

## License

AGPL-3.0-only. Adapted third-party source retains its applicable attribution and license in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
