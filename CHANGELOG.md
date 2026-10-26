# Changelog

## 0.25.1 - 2026-10-26

### Changed

- Refine the shared desktop and browser composer, navigation, project selection, and local and Cloud conversation creation flows.
- Clean up General, account, provider, model, plugin, and Remote access settings while preserving drafts and pending operations.
- Improve saved conversation and Cloud archives with search, paging, read-only previews, restore feedback, local file status, and backup recovery details.

### Added

- Shared recorded usage dashboards, cached model pricing, and provider history imports.
- Account-bound Cloud source preparation, model configuration, queued prompt feedback, and synchronized notification preferences.
- Local terminal selection for synchronized Cloud checkouts, with account ownership checks and archive/sign-out cleanup. The complete local-terminal UI lifecycle still requires acceptance testing.


## 0.25.0 - 2026-10-17

### Added

- Rust native desktop application, local engine, and GPU-rendered browser client with shared session and workspace protocols.
- Durable Zuse agent runtime with native coding tools, supervised commands, process notifications, persistent chat support, and recovery review with explicit retry controls.
- Encrypted model account storage, authorization services, and account settings in native and browser clients.
- Shared Zuse identity and hosted agent plugins, plus additional model provider integrations.
- Browser workspace editing, uploads, durable queues, image attachment previews, and structured tool activity.
- Persisted local tool results with paged transcript details and workspace diff review.

### Fixed

- Preserve conversation context when switching providers and background work across agent turn boundaries.
- Persist steering cancellation across stop and resume, and retain workspace ownership until host cleanup completes.
- Protect pending file edits and saves when navigating between workspace files.
- Complete local browser chat controls and native provider settings labels.
- Refine browser navigation and preserve in-progress work across connection interruptions and reconnects.

### Release scope

- Includes all changes on `main` through `4a41b7f85d7c924d480f6ab69ea96dc31574b12d`, with the 0.25.0 version update.
- Source release. This repository does not currently contain an installer publishing workflow. Platform and integration acceptance status remains documented in `internal-docs/acceptance.md`.
