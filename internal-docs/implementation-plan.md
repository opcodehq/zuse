# Native and browser replacement plan

## Completion objective

Finish the standalone native macOS/Linux and browser replacement: shared Rust/GPUI UI, Rust engine and Zuse harness, complete local workflows, all supported providers, concurrent-agent Executor v2 plugins, direct serving, paired encrypted remote access, packaging and integrated validation. Keep the tracked execution goal active until requirements pass; do not label adapted source or fixture-only evidence as live verification.

## Decisions

- This repository becomes the standalone replacement. Preserve the old application until native and browser acceptance pass.
- Start fresh in a separate native profile. Do not migrate or share the old SQLite database.
- Use Zuse's layout and behavior as the main design. Include local capabilities from the native reference: animation, voice, visuals, previews, themes and navigation.
- Use GPUI on desktop and GPUI Web/WASM in browsers. Validate browser input/rendering before expanding shared views.
- Maintain the GPU toolkit independently in `opcodehq/zkit`, with a clean initial source snapshot and retained legal attribution. Zuse pins published toolkit revisions as Git dependencies; no submodule is required. Opus 5.5 owns a focused reusable-component cleanup task and may import/adapt licensed component implementations as its base.
- Rust owns sessions, tools, queues, permissions, persistence and orchestration. Pinned SDK-only bridges are allowed and load on demand.
- Keep hosted Executor v2 and existing account services. Retiring Electron does not require retiring these services.
- Include localhost/LAN serving and hosted access through paired devices and an authenticated encrypted outbound relay.
- Defer hosted compute provisioning, mobile applications and replicated multi-device stores.

## Delivery sequence

1. Establish workspace, attribution, toolchains, feature inventory and a native/browser GPUI vertical slice. Isolate native-only dependencies from WASM client and views.
2. Adapt native workflows and provider protocols. Implement the Zuse harness, durable queue semantics and plugin service gateway. Use one canonical Rust protocol across clients.
3. Complete chat/composer, onboarding, archives, settings, files/editor, terminal, Git/worktrees/PRs, usage, uploads, permissions and nested agents.
4. Complete browser parity, direct serving, pairing/relay, native previews, voice, interactive visuals and appearance.
5. Bound memory/GPU/cache usage, package macOS and Linux, validate installation and full workflows, then prepare the production browser cutover and Electron retirement.

## Architecture

Entry points live under `apps/`; focused capabilities live under `crates/`. UI and client code use engine commands/events; provider adapters do not depend on UI. Storage owns schema migrations and journals. Browser filesystem and process operations run on the engine host. Platform interfaces supply dialogs, clipboard, audio, network transport and file selection.

Native reference modules are the implementation starting point, not evidence of accepted behavior. Avoid duplicating provider/runtime behavior while porting existing Zuse semantics. Retain source provenance and legal attribution separately from product terminology.

## Validation strategy

Write meaningful behavior tests alongside implementation. Run focused checks at integration milestones, then the complete integrated setup near completion. Do not repeatedly run broad suites without a relevant change or failure.

Acceptance includes provider protocol failures/recovery; queue races and crash recovery; child-agent mailboxes and tool authorization; concurrent plugin calls and OAuth renewal/revocation; native and browser end-to-end workflows; direct and relayed reconnect; and both-platform installation. Screenshots establish visual evidence, not engine correctness. Record missing credentials or hardware explicitly. Memory reports separate UI, engine, bridges, browser helpers and agent children.
