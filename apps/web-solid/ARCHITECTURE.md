# Zuse web client (Solid)

A DOM client for the Zuse engine. Desktop (`crates/ui`) is the reference for behaviour and appearance. The engine owns all data; this app shows it and sends requests.

## Layout

| Path | Owns |
| --- | --- |
| `src/core` | Transport, RPC, auth, the store, view state, the selected chat's transcript. Plain TypeScript, no components. |
| `src/core` helpers | `util.ts` (`uuid`, `errorText`, compact `timeAgo`, `maskedEmail`, `deviceOnline`), `paths.ts` (host path handling for folder browsers). Areas import these instead of keeping their own copies. |
| `src/ui` | Tokens (`tokens.css`), the desktop icon set (`Icon`), shared controls (`primitives.tsx`), `clipboard.ts` (`writeClipboard`, works on plain-http origins), `harness.ts` (`harnessIcon`, `harnessTint`), `imageUrl.ts` (the only way to turn image bytes into a URL: raster types get a blob URL, SVG a data URL, anything else octet-stream), `markdown.ts` (the one sanitizer for markdown written by agents or third parties: raw HTML shows as text, images become links, no inline styles). |
| `src/shell` | The window frame and theme. It mounts one component per area and never needs editing by areas. Settings, Usage, Plugins, New project and the archive load on demand and are warmed when the first screen is idle. |
| `src/features/<area>` | One directory per area. Each area exports exactly the components named below. |

Area exports, mounted by `src/shell/App.tsx`:

| Area | Exports | Mounted |
| --- | --- | --- |
| `sidebar` | `Sidebar` | Left column |
| `workspace` | `TopBar`, `RightPanel` | Above the chat; right column when `view.rightPanel` is set |
| `transcript` | `Transcript` | Chat body (fills, scrolls) |
| `composer` | `Composer` | Below the transcript. Also renders the landing heading when no chat is selected. |
| `projects` | `NewProject`, `ProjectArchive` | Routes `newProject`, `archive` |
| `settings` | `Settings`, `Usage` | Routes `settings`, `usage` |
| `plugins-cloud` | `Plugins` | Route `plugins` |
| `connection` | `ConnectionGate`, `DevicesSettings`, `AccountSignIn` | Wraps the frame; owns auth, pairing and reconnect screens. May call `setTransportFactory` before `start()` runs. `DevicesSettings` is the Settings → Remote access page (mount it for section `devices`); `AccountSignIn` is the engine-run account sign-in row. |
| `extras` | `Extras` | Always mounted: palette, shortcut help, global overlays. |

`extras` also exports pieces other areas mount: `VisualPart` (a transcript `visual` part: `<VisualPart chatId={id} part={part} />`), `EmptyState`, `Skeleton`, `ErrorScreen`, `ImportChatButton` (the landing's "Import chat" chip), `openPalette`, `openShortcuts`, `openImportChat`, `startVoice`, `endVoice` and `voiceCall`. Session chimes and browser banners live in Settings (`features/settings/alerts.ts`); `extras/notifications.ts` only adds the connection-loss cue and reads the same Settings preference keys.

## Data flow

- `rpc.call(method, params)` returns a promise. `rpc.subscribe(method, params, onItem, onEnd)` returns `{ cancel }`. Method names come from `M` in `core/methods.ts`, generated from Rust with `bun run gen:methods`.
- Interrupted reads fail with a retryable error. Interrupted mutations reject with `RpcError.outcomeUnknown = true`: say the action may have completed; never resend automatically.
- `onConnected(epoch => subscription)` runs on every connection and cancels on disconnect. Use it for standing watches, not component effects. When registered while already connected it runs on the next microtask, so a lazily loaded module finishes evaluating first; still declare module state before any top-level `createEffect` that writes it.
- A dropped connection marks a request as possibly completed only when it is a mutation. `isRead` in `core/rpc.ts` extends the generated list with `Watch*`, `Get*`, `List*`, `Search*` and a few lookups.
- `state` (`core/store.ts`) holds normalized data: `chats`, `sessions` (keyed by chat id), `spaces`, `devices` (records by id), `repos`, `harnesses`, `transcript`, `queue`, `account`, `connection`, `epoch`. Snapshots are merged with `reconcile` so unchanged rows keep identity.
- `view` (`core/view.ts`) holds what is open: `route`, `selectedChatId`, `draftProjectPath`, `rightPanel`, and persisted `preferences`. Use `openChat(id)` and `navigate(route)`. The open chat, draft project and right panel survive a reload (per tab, `sessionStorage`); a restored chat that no longer exists is dropped once the chat list arrives, and an open chat deleted elsewhere moves the view to its project's new-chat canvas (the composer carries the draft over).
- At 800 px and below (`narrowLayout()`) the sidebar is a drawer (`view.drawerOpen`, closed by any navigation) and the right panel fills the window. Show or hide the sidebar with `sidebarVisible()` / `setSidebarVisible()`, never by writing `preferences.sidebarOpen` directly.
- `core/chat.ts` keeps `state.transcript` in sync for `view.selectedChatId` via `WatchDocMessages` (reset plus deltas, with append length and count tripwires that trigger a resubscribe). Text appends mutate a single part, so render parts by key and never rebuild the list on a token.
- Chat-scoped requests use `chatRequest(chatId, params)`, which adds `targetDeviceId`.
- Add a domain to the store when an area needs shared data; keep area-only data in a module-level store inside the area.

## Rules

- Components read `state` and `view`; never copy server data into signals. Local UI state only (menus, hover, drafts in progress).
- Solid: do not destructure props, use `For`/`Show`/`Index`, use `splitProps` to forward attributes.
- Long lists are virtualized with `@tanstack/solid-virtual`.
- Markdown and agent content are sanitized before reaching the DOM, through `ui/markdown.ts` (also for provider content such as PR descriptions and review comments). Only bundled SVG icons use `innerHTML`. Streaming markdown re-renders only the growing list item, table cell or tail block, and spaces out renders that cost more than a few ms; code is tokenized in a worker and long blocks colour only the lines near the viewport.
- Never put engine or agent bytes behind a blob URL yourself: use `ui/imageUrl.ts`, since a blob URL opened as a page runs with this origin.
- Every control is a labelled, keyboard-reachable element. Shortcuts go through `core/keys.ts`; a shortcut's `when` can hand the key back to the browser or the focused editor.
- Store writes that can fail validate first: `applyTranscriptFrame` checks every tripwire before it touches the store, since a throw inside a store write leaves observers stale.
- Avoid descendant `:has()` on ancestors of the transcript; every streamed DOM change re-evaluates it. Use child combinators.
- Colours come from tokens only. Light and dark must both work. Shared surfaces live in `tokens.css` (`--frame`, `--card-border`, `--border-alpha`, `--float-chrome`, `--float`, `--hairline`, `--wash`, `--scrim`, `--tooltip`); area variables alias them rather than repeating values.
- Switches use the `.toggle` class (desktop's 32 by 20 settings switch), via `Toggle` or settings' `Switch`. Tooltips use `.tooltip` (desktop's text tooltip). Dropdown and context menus are non-modal (`modal={false}`): modal menus lock page scroll, and Kobalte can loop forever looking for a scroll parent inside virtualized lists.

## Commands

Node 20.19+ is required (system Node 18 is too old; use `~/.config/nvm/versions/node/v24.18.1/bin`).

```sh
bun install
bun run typecheck && bun run lint
ZUSE_WEB_OUT=/tmp/my-build bun run build   # private output; default is dist/web-solid
node e2e/smoke.mjs                          # real engine + headless Chromium
node e2e/journey.mjs                        # full user journey, first-load and long-stream numbers
node e2e/defects.mjs                        # regression checks for reviewed defects (lost answers, deleted chats, …)
node e2e/review-fixes.mjs                   # later review round: desync freeze, queue edit reload, find in chat, focus, …
node e2e/transcript/long-reply.mjs [4]      # a 120K+ char streamed reply from a real engine; LONG_REPLY_BASELINE=1 compares
node e2e/transcript/stream-bench.mjs [4]    # per-frame cost of a streaming list/table/paragraph; BENCH_CODE=1500 for code
zuse serve --assets dist/web-solid          # serve the production build
```

`e2e/harness.mjs` provides `buildApp()`, `startEngine({ assets, agent: { reply, chunks } })` with a fixture agent and a disposable profile, and `openBrowser(url)` with `eval`, `waitFor`, `click`, `press`, `type`, `key` and `screenshot`. `window.__zuse` exposes `{ state, view }` read-only for tests.
