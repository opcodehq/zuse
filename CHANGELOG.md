# Changelog

## 0.26.0-preview.4 - 2026-11-09

### Added

- Zots coordinators and a control SDK for driving Zuse programmatically.
- Use local CLI models and sign-ins in the Zuse harness.

### Changed

- Every release now ships all download formats together: DMG, Setup EXE, DEB, RPM and archives for each platform.
- Pick the fastest route to paired computers and refine mobile chat.
- Improve the workspace panel and terminal layout.
- Save permission choices as the default for new chats.
- Reduce idle agent retention.

## 0.26.0-preview.3 - 2026-11-09

### Added

- Inline UI blocks for agent-generated cards, charts, tables, steps and forms, with replies sent back into the conversation.
- Fold completed turns into expandable summaries, and show file mentions, attachments and handoff context as message chips.
- Native iOS and Android app sources with Rust remote-access integration. Mobile store downloads are not included in this desktop release.
- Create folders directly from the New project picker, and open Review in a side tab.

### Changed

- Refresh the workspace with dithered buttons, icons and working indicators, clearer pull request and chat status glyphs, translucent panes and a floating session card.
- Share Git status and pull request subscriptions across views, keep sidebar change counts live, and simplify the Git panel and inline review threads.
- Move updates into a quiet footer control with release notes, allow restart to wait for agents, preserve verified staged updates, and offer retry after installation failures.
- Start new worktrees from the latest remote commits with progress feedback.
- Archive threads by stopping their agents, subagents and background commands; keep the worktree when shutdown cannot complete safely.

### Fixed

- Prevent Linux file-watcher exhaustion by sharing workspace watches and bounding watcher allocation.
- Keep Git status fresh during long builds and preserve unrelated pull request action errors when subscriptions recover.
- Avoid a link-detector panic on non-ASCII text, and keep chat search above the transcript fade.
- Preserve drafts and retry failed UI-block replies, release inactive form views, and retain context-chip padding.
- Resume interrupted turns after an update restart without scheduling continuation after an ordinary quit or a user stop.

## 0.26.0-preview.2 - 2026-11-05

### Added

- Composer keyboard editing, a kill ring, saved drafts, sent-message history search, and emoji completion.
- Chat keyboard shortcuts, a shortcut reference dialog, and find in chat, including focused side-chat navigation.
- Streaming dictation that transcribes phrase by phrase while retaining draft text and cancellation controls.
- Browser-based Zuse sign-in with a fallback when the native loopback redirect is unavailable.
- A Muse Code sign-in action and translated setup guidance.
- Signed, notarized macOS DMGs and archives for Apple silicon and Intel, Windows Setup and portable ZIP, Linux DEB/RPM/archives for x64 and ARM64, and a checksum-verifying Linux terminal installer.

### Changed

- Plan mode is a separate composer control in the add menu, independent of agent access settings. The read-only access option is removed.
- Tool groups summarize their activity and show the running call. Empty projects are hidden and running background commands appear once.
- Interface translations and shared CJK font coverage include the new composer, shortcut, sign-in, and dictation controls.

### Fixed

- Preserve custom keyboard shortcuts, saved drafts, and the current draft when composer lists or dialogs are dismissed.
- Pass unmatched file globs through Claude's zsh commands without shell expansion errors.
- Keep saved runtime credentials in sync, retain plugin error codes, resolve pull requests through branch merge refs, and limit background-stop confirmation to the relevant chat and tasks.
- Use parsed naming replies for chat titles and exclude archived chats from update gating.

## 0.26.0 - 2026-11-03

The first stable release of the native Zuse app since 0.25.0. It also includes every change from the 0.25.1 previews.

### Added

- Downloads for Windows (x86_64, portable zip), macOS (Apple silicon, signed and notarized) and Linux (x86_64). The Windows zip is not code-signed yet, so SmartScreen asks for confirmation on first launch.
- Muse Code as a built-in agent provider: live model list, approvals, resume, interrupt, `/compact`, image attachments, and install and update through Meta's script.
- Interface languages: English, French, German, Simplified Chinese, Traditional Chinese, Japanese and Korean across the desktop app, browser and CLI, following the system language by default and switchable without losing drafts.
- Native Rust model harness: direct Responses, Messages, GenerateContent and Chat Completions transport, durable transition storage with recovery review, bounded child agents, concurrent independent reads, local skills and lazily pooled MCP connections. The production JavaScript model bridge is removed.
- New project palette: open a folder, clone a repository (search your GitHub repositories or paste a Git URL) or create an Empty, Next.js or Turborepo project, optionally published as a private GitHub repository. Cloning and scaffolding run in the background with progress, cancel and retry above the composer.
- Local stdio MCP servers in Plugins: add one by pasting its configuration; its tools reach every agent through the plugin gateway.
- Claude Code workflow runs appear as live phase boards with agent tiles, counts, timers and details.
- Saved approval rules: allow a tool for this chat or always for the project from the permission card, and review or remove rules in the permission inspector. Rules never exceed the chat's access mode.
- iOS Simulator and Android emulator pane beside chat, with device boot and shutdown, live frames, manual input and optional agent control.
- Pull request cards in chat with branch history, conflict previews, check status and resolve, ready and merge actions, plus a Create PR composer action.
- Pull request CI watching with failure notifications and opt-in automatic repairs, limited to three attempts per failure and never force-pushing.
- Private secret requests: agents can ask for a credential in a masked input and use it for one approved HTTPS request without it entering the model context or chat history.
- Background task tracking with per-task stop, and pull request updates on session cards.

### Changed

- Neutral grey theme in dark and light: a lighter base palette, a translucent sidebar set apart from the conversation panel, flat menus and dialogs, and sentence-case group headings.
- Command palette, folder browser and New project share one card with an inset results panel and every shortcut in the footer.
- Chat chrome: provider tiles for running agents, a single Thinking chip, the attached question tray above the composer, and the Zuse working indicator.

## 0.25.1 Preview 5 - 2026-10-26

### Fixed

- Use the existing Codex ChatGPT sign-in for composer dictation, matching Electron’s recording upload and transcription flow.
- Remove the speaker control, spoken-response settings, and local speech-model download/runtime.
- Keep microphone capture in memory, cancel uploads when dictation is cancelled, and preserve drafts on transcription failures.

## 0.25.1 Preview 4 - 2026-10-26

### Fixed

- Match the Electron composer’s skill suggestions with original monospace names, the stacked skill icon, compact single-line descriptions, and mouse selection.
- Keep advertised skill names in inserted chips while preserving their source identity and undo history.
- Search skill descriptions and abbreviated names, with bounded filtered results.

## 0.25.1 Preview 3 - 2026-10-26

### Changed

- Show Composer, Links, Thread names, and Worktrees directly in General settings.
- Match all 35 Electron keyboard commands and defaults, including separate chat, tab, and pane navigation, composer planning, and editor annotations.
- Import custom Electron keyboard rules during onboarding or from Keyboard Shortcuts, preserving rule order and disabled commands.

### Fixed

- Cloud uses the same account API endpoint as Electron; the previous endpoint has an expired TLS certificate.
- Bundle plugin logos for Stripe, PostHog, WorkOS, Customer.io, and Better Stack.
- Keep composer and editor shortcuts inside their input contexts, including when an embedded browser page has focus.

## 0.25.1 Preview 2 - 2026-10-26

### Added

- Native GPUI onboarding and Settings import from Electron, with profile review, optional settings, local backups, progress, and resumable imports.
- Import reports for unavailable attachments and preferences or local plugins that GPUI cannot yet apply; hosted plugins remain available through the same Zuse account.

### Changed

- Keep simple dictation beside the composer model selector, add system response playback, and remove live Codex orchestration controls from the desktop UI.
- Use provider marks instead of generic robot icons, keep missing CLIs out of the picker, and offer Gemini through the ACP adapter catalogue.
- Align review chrome with the main workspace, add file review markers and patch copy, and use the reference archive artwork.

### Fixed

- Floating notices no longer consume clicks on unrelated controls.
- Signed-out accounts show a Sign in label; the Cloud badge requires verified entitlement, and Cloud session failures no longer masquerade as a signed-out General account.
- Imported main chats now appear in project lists, with conversation tabs and archived threads grouped correctly.

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
