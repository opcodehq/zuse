# Changelog

## Unreleased

### Added

- Add opt-in Slack cloud tasks with threaded status/results, sender/channel attribution, and a workspace/repository connection flow in Cloud settings. Server credentials stay encrypted; enablement requires the Slack migration and a rebuilt compatible runtime image.
- Link a chat or a Zapper to a Slack channel from its right-click menu. Finished replies post into a Slack thread, your replies in that thread come back to the chat, and Zappers post under their own name and avatar. Their worker threads get their own threads in the same channel. Requires the new Slack migration and re-authorizing the Slack app for its added scopes.

## 0.27.4 - 2026-11-22

### Added

- Appearance: themes, an Add theme dialog and animated wallpapers.
- Zappers: soft pixel avatars that react (or your own picture), a PR tab with Merge, Open on GitHub and the missing PR facts, and a visible archive button.
- A background work row in the tray for running subagents and tasks.

### Changed

- Cloud is chosen in one place, the computer picker: one "This computer" row, the cloud size inline on the Cloud row, and the Worktree/Local chip only for chats on this computer.
- The first message of a new cloud chat pins to the top like any send and stays there when the computer starts and the reply arrives.
- Cloud chats show a working spinner on their tab and sidebar row; the cloud icon is green only while the computer is up.
- Settings navigation is reordered and the account card matches the other cards.

### Fixed

- New cloud chats appear in the sidebar right away and stay there.
- The Queued panel lists every queued cloud message and drops only the ones the agent picked up.
- Cloud usage loads for accounts with image builds.
- Usage settings page scrolling and chart tooltip.

## 0.27.3 - 2026-11-20

### Added

- Zappers: a cleaner New Zapper screen with a searchable repository picker, a first-run interview that writes the Zapper's own versioned INSTRUCTIONS.md, closable issue and document tabs, compact "Sent from Zapper" turns with a Zapper icon on its tool calls, and subagent chips that open in the side pane.
- In-app What's new.
- Remote access appears in every build.

### Changed

- New cloud chats show your first message instantly, at the top like any message, with the agent's icon on the tab and a sidebar row right away; it becomes one bubble when the computer's copy arrives.
- A cloud chat that is still starting belongs to its own tab: other tabs, new chats and the model picker keep working. Closing that tab shuts down any computer it started.
- Local and cloud projects link by GitHub repository; projects with the same name show as owner/repo, and a linked repository is a single row in the project picker.
- User chat messages render as Markdown.
- Zapper "Needs you" lists only real asks, Zapper-started threads stay out of the project chat list, and Zapper briefs are shorter.

### Fixed

- Cloud chats no longer show a stale "Waiting for computer…" or "Agent working…" after the computer is ready or the turn ends.
- A failed cloud start shows the server's reason with Retry and Delete, and Delete keeps your message; refused messages name the real reason instead of "busy".
- One unreadable cloud chat no longer hides the whole cloud chat list in the sidebar.
- Antigravity account removal, current ACP releases, and Zapper commands that kept failing.

## 0.27.2 - 2026-11-20

### Added

- Cloud first run: your cloud image builds by itself once repositories are connected (no setup script needed) and rebuilds when repositories change, with one progress row in Setup, First chat and Overview.
- The workspace menu offers Worktree, Local and Cloud. A new chat in a project backed by a cloud repository opens on Cloud; picking Cloud for a local project uses the matching cloud repository and connects it if needed.
- The First chat step picks a repository and opens a Cloud chat with a short guide and one-click prompts.
- Add repository opens a searchable palette: repositories grouped by GitHub account with avatars and update times, multi-select from the keyboard, connected repositories marked, and quick links to manage GitHub access, install on another account or refresh.
- GitHub connect finishes automatically: the confirmation page opens Zuse, which confirms only the connection it started.

### Changed

- Coding agent connections: one row per agent with a single action, sign-in inside the row (device code with copy and an Open sign-in page link, Claude setup token, or API key), and a menu for adding accounts or disconnecting.
- Update status is an icon button in the sidebar footer instead of a separate row.

### Fixed

- History search matches commit SHAs by prefix instead of fuzzy-matching every commit's SHA.

## 0.27.1 - 2026-11-19

### Added

- Remote access you choose and trust: phones pair by QR, link or SSH (tunnelling through SSH when the computer only listens locally), Wi-Fi and Tailscale/Headscale can be offered separately, and Settings → Remote access has one card with a styled pairing QR that switches to connected once the device pairs. `zuse pair --json` prints the pairing for scripts.

### Fixed

- Cloud works for signed-in accounts that don't use workspace sync: the cloud sidebar, Billing, Usage and new cloud chats no longer fail with "Sign in to access cloud workspaces" or "Could not load cloud workspaces".

## 0.27.0 - 2026-11-19

### Added

- Cloud plans billed in machine minutes: **Cloud** ($20) and **Cloud Max** ($50, machines up to 8 vCPU / 32 GB), with a $1 first-week trial and optional pay-as-you-go minutes up to a cap you set. Settings shows Cloud to signed-in accounts based on their subscription, with no restart.
- Redesigned Cloud onboarding, Billing, Usage, Repositories and GitHub settings, with plan cards, a Buy more minutes grid and one floating bar for unsaved changes.
- Archive cloud chats: the computer sleeps right away and is deleted after 7 days.
- **Open via SSH** lists the editors and terminals installed on your computer, plus Copy SSH command.
- Sidebar chat rows show Archive on hover, a cloud icon for cloud chats (green while running) and a details card with title, project, location and branch.
- Every Claude Code model is available in cloud chats.
- Zappers run on issues: each issue has one owner, a **Needs you** status for questions to you, done criteria, new avatars and a rebuilt Overview.
- Agents can show a plugin connect card in chat, and `/mcp` shows a card with the agent's MCP servers and your Zuse plugins.
- Plugin logos on mentions and plugin tool calls.
- Each agent's own `/` commands appear in the composer, with a per-agent **Provider commands** switch under Settings → Providers → Settings → Completion.
- Oh My Pi as a built-in agent provider.

### Changed

- Cloud chats behave like local chats: queued and sent messages appear instantly, a stopped queue shows **Queue paused** with Resume and resumes when you send a new message, and messages steer into a running turn when the agent supports it.
- One Cloud option in destination pickers, a simpler cloud chat header with agent tab icons and a live status color, and a single cloud computer card with status, resources, SSH and sync.
- Nested menus open to the side, and load and usage bars use dithered meters.
- Typing `zuse` in a terminal, or an agent running it, now prints help instead of opening a second window. Open the app with `zuse app`; the dock, Start menu, `zuse:` links and GNOME-style Linux launchers work as before. Mistyped commands such as `zuse commands` report an unknown command. On Linux launchers that do not record the started process (KDE, for example), the first launcher click after updating from 0.26.1 can do nothing: the previous version's installer writes the old desktop entry. That start repairs the entry, so the next click opens Zuse, or run `zuse app`.
- Name the chat's agent in the composer's empty `/` menu, so a command that only exists in another agent reads as that agent's catalog instead of a missing feature.
- Checkout diff review moves into the sidebar; pull request editing, filter tabs, the review toolbar, git glyphs, tabs and the working loader are refreshed.
- Attachment previews in the composer and chat: staged documents preview inline, sent images show as thumbnails and files as compact tiles.
- Render Markdown tables with compact rows, rounded borders, clearer headers, and better wrapping.
- Richer file previews, inline diffs in chat, and cards for generated images.
- Cloud settings cards use the same framed sections as the rest of Settings.
- The plugin sign-in page opens automatically, and the Plugins page explains why plugins are unavailable.

### Fixed

- Cloud computers never stay awake unnoticed: quiet computers sleep reliably, and machines are regularly reconciled with your account so leftovers are slept or removed.
- Cloud chats and settings update from pushed events instead of polling.
- The cloud tab shows its agent icon from the first frame, archived cloud chats stay hidden, and cloud repositories prepare reliably after a computer is resized or recreated.
- False account or server changes no longer block new cloud chats.
- Sending a command picked from the `/` menu in a new OpenCode 2.x chat no longer fails with "no longer available in this project" while OpenCode is still loading its command list.
- OpenCode 2.x answers its command catalog only after its config and plugins load; Zuse now waits for that list instead of reporting no commands, so commands like `/goal` appear in the composer.
- On Linux, an agent whose builds exhaust memory no longer takes the whole app down: each agent runs in its own systemd scope, and Zuse stops the agent using the most memory (with a notice in its chat) before the system's OOM handling kills the app.
- Keep the Settings Back to app button clickable on Windows by excluding it from the window-drag region.
- Archive chats that have open checkout terminals.
- More reliable update footer, restart flow and Linux self-updates, CLI and browser startup, desktop shutdown and standalone installs.

## 0.26.1 - 2026-11-15

### Added

- Choose Stable or Preview updates in General settings. Your selection is saved, preview versions are detected correctly, and switching tracks cancels stale downloads safely.
- First-run setup for agents, sign-in, MCP servers, and importing chats from previous Zuse and supported agent apps.
- Saved browser page zoom controls and keyboard shortcuts.
- A terminal-friendly standalone `zuse` command alongside desktop downloads.

### Changed

- Refined composer, model picker, attachment previews, file drops, and workspace menus.
- Improved pull request layout, review controls, and commit navigation, with automatic follow-up on failing checks and review feedback.
- A redesigned drag-to-Applications macOS installer.

### Fixed

- Correct browser page sizing on high-DPI Linux desktops.
- Avoid idle redraw loops when chat timestamps are ahead of the system clock.
- Preserve source context for imported and moved chats.

Includes the features and fixes from the earlier 0.26.0 previews.

## 0.26.0-preview.5 - 2026-11-11

### Added

- Search from the browser address bar, see local previews and recent pages on the browser home page, and mark up screenshots with a pen, boxes and arrows on macOS and Linux.
- Mermaid diagrams in pull request descriptions and comments.
- A Favorites view in the model picker, with reasoning, Fast mode and context options in a separate menu.
- Force delete for sessions whose archive fails, with a confirmation that explains what is removed.
- Codex chats can ask question cards outside Plan mode.
- Opt-in setup and compatibility checks for a separately installed Opcode computer-use runtime.
- A small runner game in the status strip while an agent works.

### Changed

- Zots are now called Zappers, with redrawn avatars, clearer thread status in the sidebar and archiving from thread rows.
- Workflow runs show as a compact summary in the transcript, and their agents open in the right pane.
- The pull request panel has a new header, a clearer checks list, and a merge menu with auto-merge and force merge.
- Earlier messages load inline as you scroll up, replacing the Load earlier messages button.
- The sidebar uses terminal-style loaders for running chats and projects.
- Cloud computers (beta) start faster, support SSH and local sync, and use browser-only sign-in.
- Release builds are cached and run in parallel, with Intel macOS cross-compiled on Apple silicon.

### Fixed

- Check for Updates no longer fails with connection refused; it reads the latest GitHub release.
- The pull request panel works with Ubuntu's packaged GitHub CLI.
- Native harness models use their real context windows instead of a 32K fallback, stream thinking separately, and no longer get single-character titles.
- Intel macOS release builds link the microphone permission helper correctly.

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
