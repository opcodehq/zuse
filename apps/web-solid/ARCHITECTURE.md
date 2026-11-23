# Zuse web client (Solid 2)

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
| `zappers` | `ZapperPage`, `ConfigPage`, `ZapperSidebar` | Dedicated coordinator/worker route, inspector and sidebar group |
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
- Solid 2: do not destructure props. Use `For`/`Show`, `For keyed={false}` for indexed lists, and `omit(props, ...keys)` to forward attributes. DOM types and rendering come from `@solidjs/web`; stores come from `solid-js`. Effects use separate compute and apply callbacks; return cleanup from apply or `onSettled`.
- Store setters mutate drafts. Writes commit on the next microtask; perform dependent reads after that commit, or use the draft inside the same setter. Use `flush` only at imperative boundaries that require committed state, never inside an effect. Reconcile existing subtrees; assign the initial value when the subtree is null.
- Native `data-*`, ARIA and enumerated attributes use string tokens (`"true"` / `"false"`) where the text value matters; boolean false removes attributes in Solid 2.
- Long lists use `ui/virtualizer.ts`, a Solid 2 adapter over `@tanstack/virtual-core`. It reconciles measured rows by index so measurements preserve mounted rows. Replace it with the upstream adapter once a Solid 2-compatible version is published.
- Markdown and agent content are sanitized before reaching the DOM, through `ui/markdown.ts` (also for provider content such as PR descriptions and review comments). Only bundled SVG icons use `innerHTML`. Streaming markdown re-renders only the growing list item, table cell or tail block, and spaces out renders that cost more than a few ms; code is tokenized in a worker and long blocks colour only the lines near the viewport.
- Never put engine or agent bytes behind a blob URL yourself: use `ui/imageUrl.ts`, since a blob URL opened as a page runs with this origin.
- Every control is a labelled, keyboard-reachable element. Shortcuts go through `core/keys.ts`; a shortcut's `when` can hand the key back to the browser or the focused editor.
- Store writes that can fail validate first: `applyTranscriptFrame` checks every tripwire before it touches the store, since a throw inside a store write leaves observers stale.
- Avoid descendant `:has()` on ancestors of the transcript; every streamed DOM change re-evaluates it. Use child combinators.
- Colours come from tokens only. Light and dark must both work. Shared surfaces live in `tokens.css` (`--frame`, `--card-border`, `--border-alpha`, `--float-chrome`, `--float`, `--hairline`, `--wash`, `--scrim`, `--tooltip`); area variables alias them rather than repeating values.
- Switches use the `.toggle` class (desktop's 32 by 20 settings switch), via `Toggle` or settings' `Switch`. Tooltips use `.tooltip` (desktop's text tooltip). Dropdown and context menus are non-modal (`modal={false}`): modal menus lock page scroll, and Kobalte can loop forever looking for a scroll parent inside virtualized lists.

## Commands

Node 22.12+ is required (system Node 18 is too old; use `~/.config/nvm/versions/node/v24.18.1/bin`).

```sh
bun install
bun run typecheck && bun run lint
ZUSE_WEB_OUT=/tmp/my-build bun run build   # private output; default is dist/web-solid
node e2e/solid2.mjs                         # development runtime: frames, batching, cleanup, virtualization and Kobalte
node e2e/smoke.mjs                          # real engine + headless Chromium
node e2e/journey.mjs                        # full user journey, first-load and long-stream numbers
node e2e/defects.mjs                        # regression checks for reviewed defects (lost answers, deleted chats, …)
node e2e/review-fixes.mjs                   # later review round: desync freeze, queue edit reload, find in chat, focus, …
node e2e/transcript/long-reply.mjs [4]      # a 120K+ char streamed reply from a real engine; LONG_REPLY_BASELINE=1 compares
node e2e/transcript/stream-bench.mjs [4]    # per-frame cost of a streaming list/table/paragraph; BENCH_CODE=1500 for code
zuse serve --assets dist/web-solid          # serve the production build
```

`e2e/harness.mjs` provides `buildApp()`, `startEngine({ assets, agent: { reply, chunks } })` with a fixture agent and a disposable profile, and `openBrowser(url)` with `eval`, `waitFor`, `click`, `press`, `type`, `key` and `screenshot`. `window.__zuse` exposes `{ state, view }` read-only for tests.

## Solid 2 dependency boundary

`solid-js` and `@solidjs/web` move together; `@solidjs/vite-plugin` compiles JSX and owns the development checks. `tsconfig.json` points JSX types at `@solidjs/web`, and ESLint uses its Solid 2 configuration. `ZUSE_SOLID_DEV=1` also enables the development runtime and readable output in a production-style build for engine-backed diagnostics.

Kobalte is pinned to `2.0.0-alpha.4`, its published Solid 2 line. Its peer ranges still pin an older Solid 2 RC, so the package overrides keep its primitives and the app on the same stable Solid 2 runtime. The dialog/menu regression checks exercise this boundary. Kobalte's alpha currently emits development relay diagnostics; those upstream warnings remain visible.

## Zappers

`WatchZappers` is a standing connection watch. `core/zappers.ts` determines ownership, including previous coordinators and thread descendants. Owned threads are excluded from ordinary sidebar and search results; `openChat` routes them into the Zapper conversation and inspector. The regular list waits for the first ownership snapshot to avoid briefly leaking Zapper threads. Released workers return to their projects; archived workers remain accessible from the archive.

The inspector mirrors desktop Overview, Issues, Threads, Drive and History. Commands carry an expected revision and stable command ID. A lost response preserves the exact command for an explicit idempotent retry. Delegated chat IDs derive from that command ID. Document edits/restores carry an expected file version. Display attribution parses the engine envelope without modifying the stored or submitted prompt. Pixel mascots are generated from the shared Rust presentation definitions using `scripts/gen-zapper-mascots.py`.

Run `node e2e/zappers.mjs` for browser checks against a disposable engine and a scripted agent (creation, ownership, adoption, delegation, decisions, issues/comments, document versions/restores, reload and release).

Zapper inspector tabs follow the desktop icon/label strip. Resource reads coalesce bursts of watch updates into one active request and a trailing refresh; late responses from another Zapper are ignored. Reconnect epochs refresh the inspector even when revisions do not change. Server snapshots reconcile into existing rows, and document drafts retain their DOM identity through updates. Composer and Zapper surfaces use spacing and background tones instead of decorative borders; the composer editor overrides the global focus ring while controls retain keyboard focus indicators.

## Hosted browser

`bash scripts/build-web.sh --solid` builds this client with `/app/` asset URLs.
The existing WASM build remains available without that flag. Deploy the Solid
output as `ZUSE_CLOUD_BROWSER_ASSETS` on the cloud server built with its
`hosted-browser` feature. The server's root redirects to `/app/`. Both
`ZUSE_CLOUD_BROWSER_ORIGIN` and `CLOUD_PUBLIC_API_URL` must name the same exact
HTTPS origin; a second public hostname must redirect to that canonical origin.

Register `<canonical origin>/app/` as a redirect URI on the configured WorkOS
client before enabling browser sign-in. Hosted sign-in uses a same-tab AuthKit
redirect and server-held PKCE verifier, bound to the opaque browser session.
The authorization code and state are removed from the address before analytics
starts, then redeemed once over the authenticated controller RPC. The controller
stores refresh credentials only in memory. Controller restarts/expiry require
sign-in again.

`controllerReady` from `/api/session/check` enables separate web onboarding:
welcome, account, plan, GitHub/repository image setup, first cloud chat. Completion
is keyed by account and organization in browser storage. Local and paired clients
keep their normal startup. Choosing a workspace opens `#cloud=` public metadata
in an isolated tab. `CloudTransport` holds a `CloudEnvironment` subscription and
forwards engine requests through `CloudRpc`; account/cloud settings remain on the
controller. The normal chat, Zapper, thread, workspace and transcript surfaces
therefore run against the selected cloud engine. Account changes close access.

Subagent chips open the Agents surface. It tries the frozen transcript blob first,
then watches the original subagent doc, preserving the source chat's device route
for copied spawn chips. Closing/hiding the panel cancels its watch; reconnects
refresh it. Only spawn calls can link to subagent docs.

`bun run e2e:web-parity` runs browser regressions for onboarding isolation, icon
aliases, live/frozen agent transcripts, reconnect cleanup, cloud RPC forwarding,
cancellation and account revocation without provisioning paid cloud resources.
Production OAuth, provider allocation and live domain routing require deployment
verification separately.

Hosted onboarding follows desktop `Wizard` geometry (520px welcome/account
column, 56px top inset, 24/30 headings, accent progress rail and footer actions).
Cloud creation and Settings share `ui/ModelSelect`, a searchable provider-rail
menu using desktop icons, 28px model rows and keyboard navigation. Permission
and repository controls use the shared non-modal menus. The conversation
composer retains its existing desktop picker with reasoning/fast controls.

`node e2e/capture-preview.mjs` captures onboarding, sign-in, the complete
workspace/Agents pane and the composer picker using fixture data in the actual
app shell. Set `ZUSE_PREVIEW_OUT` to choose the output directory.

Zapper forms use execution-device model catalogs through `zappers/Controls.tsx`;
worker/coordinator models are selected from the same provider menu as Cloud.
Issue editing swaps the contents of one dialog rather than nesting dialogs.
`bun run e2e:zapper-polish` checks issue edit/save, completion criteria, Escape
and close controls, search/reset, switching back from workspace panels, nested
model-picker focus, and narrow-screen inspector navigation. All use fixture data
without cloud allocation. Preview captures include issue lists/details and the
narrow inspector.
