# Implementation and acceptance status

Status: `source` = adapted implementation; `compiled` = compile check passed; `pending` = work remains; `verified` = the named scenario passed with recorded evidence. A partial scenario does not establish full feature acceptance.

| Capability | Status | Evidence / remaining work |
| --- | --- | --- |
| Cargo monorepo and native source foundation | compiled | Engine and browser server compile checks passed |
| Native shell, sidebar, chat, composer, settings | source | Integrate Zuse behavior and verify native build |
| Files/editor, terminal, Git/worktrees/PR panes | source | Validate lifecycle and complete parity |
| Claude/Codex/Cursor/OpenCode/Pi/ACP protocols | source | Audit capabilities; fixtures and live checks pending |
| Gemini and Kiro CLI adapters | compiled | Registered on the shared ACP engine; CLI launch/model/effort and complete tool/turn lifecycle fixtures pass. Live logins, provider account settings and custom ACP parity remain pending. |
| Zuse harness and multi-agent orchestration | source | Rust kernel, durable recovery, nested concurrent agents, mailboxes, shared budget/account broker, compaction, and bundled SDK transport are implemented and fixture-tested. Native tools, model OAuth/vault integration, provider registration/event adaptation and authenticated recovery controls remain pending. See `harness.md`. |
| Durable queues and steering | source | Reconcile with current Zuse queue/error/recovery semantics |
| Executor v2 plugins | source | Hosted client is wired through shared engine identity, durable session bindings, provider-independent MCP discovery/call tools, and native/browser Settings. Native loopback OAuth confirmation is implemented. Live account/tenant/plugin execution, API-key/custom server entry, and complete UI acceptance remain pending. |
| Voice, interactive visuals, appearance | source | Native platform and browser capability integration pending |
| GPUI Web/WASM | compiled | Shared editing engine, denser shell, project picker, file browsing/editing, queue controls, model picker, account usage, and PTY pane now compile for WASM. Earlier chat smoke passed; the expanded flows still need runtime acceptance. |
| Shared transcript model | verified | 30 reconciliation/folding tests passed |
| Browser authentication boundary | verified | 5 tests passed: origin/cookie isolation, foreign-origin upload rejection, and byte-preserving chunked uploads |
| Localhost and LAN serving | compiled | Running engine serves authenticated HTTP/WebSocket sessions; LAN TLS remains pending |
| Owned GPU toolkit | compiled | Independent `opcodehq/zkit` snapshot published; pinned Git dependency passes browser build; component cleanup underway |
| Pairing and encrypted relay | pending | Rust relay and native/browser client integration |
| Fresh profile and local persistence | source | Separate native paths and crash recovery validation |
| macOS packaging and native acceptance | pending | Actual Mac build and runtime evidence required |
| Linux packaging and native acceptance | pending | X11/Wayland runtime and installation checks required |
| Browser deployment replacement | pending | Production-ready artifact and cutover configuration |

## Verification log

- Initial discovery: workspace has no prior commits or application files; existing application is available read-only in the sibling checkout.
- Native source foundation imported with license attribution and renamed Rust packages; no runtime behavior has yet been claimed verified.
- Initial engine/server compile checks passed. Shared model tests: 30 passed. Browser origin/cookie tests: 3 passed.
- Browser runtime exposed an application-owner lifetime bug; the entrypoint now retains its embedded application handle. A real GPU canvas renders the shell and connects to the local engine. Keyboard testing exposed missing GPUI composer focus and browser text-event routing. Both are fixed; a fresh typed prompt produced the exact requested Claude response through the running engine. Uploads and complete browser workflows remain pending. The current browser layout is an integration scaffold; visual parity with the native UI is not accepted.

- Zkit browser text handling verified with both physical key events and browser text insertion. The first complete typed prompt is preserved, and Claude returned `ZUSE_INPUT_OK`. Toolkit fix published in `6823c76`.

- Workspace integration milestone: WASM build passed against the published Zkit Git pin, including its existing multiline editor and code editor. Browser server binary builds. Five browser boundary/upload tests pass. The shared editor replaces the scaffold input implementation rather than duplicating its editing behavior.
- The expanded browser shell renders and connects to the local engine. File editing uses expected checkout/content hashes and retains edits on conflicts. Queue actions use the engine's durable queue. Complete visual and runtime workflow acceptance remains pending.
- Zkit is public; an unauthenticated Git fetch succeeds. CI no longer requires a toolkit access secret. Remote native checks remain pending; no CI pass is claimed.

- Provider milestone: engine/server compile checks passed with Gemini and Kiro registered. Three focused ACP CLI tests pass, including the two new provider process/turn scenarios. No live Gemini/Kiro credential verification is claimed.
- Zkit styled layer published in `85f4503`; delegated native/WASM checks and 26 component tests passed. Gallery styling and native platform runtime acceptance remain pending.

- Account/plugin integration: integrated server compile, WASM build, and workspace formatting checks pass. Four account tests and two gateway/callback tests pass: concurrent refresh rotation, late-result/account-switch fences, damaged-session fallback, durable bindings, and loopback callback isolation. Two MCP tests pass for origin attribution and cancellation reaching the engine while preserving the shared client. No live hosted account or plugin execution pass is claimed.
- Public Zkit CI reached the native compiler and exposed a malformed CoreText method name. Platform identifier repair is published in `322c43e`; Linux toolkit platform check passed. The app pin is updated; Mac native acceptance remains pending.

- Public-dependency CI run `39020031912` passed the browser, engine, and Ubuntu native compile jobs at `9857de6`. The Mac compile failed on the toolkit CoreText API rename described above. This proves the native Linux source foundation compiles; it does not verify the new account UI, native runtime workflows, or packaging.

- The account/plugin milestone passes 25 focused tests: 4 account, 2 gateway/callback, 2 MCP, and 17 RPC transport tests. Hosted calls opt into cancellation without changing cancellation behavior for existing workspace mutations. Updated native Settings source awaits the next Mac/Linux CI compile.

- CI run `39021451590` passed the Mac and Ubuntu native compile jobs, including the account/plugin Settings source. The browser job also passed. The engine test job remains in progress; native runtime and live hosted-plugin acceptance are still pending.
- Rust harness component milestone: 13 Rust tests pass (9 runtime/recovery, 3 broker, 1 actual bundled-process fixture). SDK protocol fixtures also pass. This component is not yet a selectable application provider; full harness integration and live model/account acceptance remain pending.
- CI run `39022923827` passed all jobs for the harness component milestone.
- Model-account source milestone: encrypted local storage, account-scoped storage leases, ChatGPT browser OAuth, SuperGrok device authorization, refresh handling, metadata controls, and engine RPC integration compile with the server. Settings integration, focused account tests, and live authorization acceptance remain pending.

- Model-account integration milestone: 11 focused tests pass, including an actual local signed identity-provider/PKCE exchange, concurrent refresh rotation, record-context tampering, shared JSON compatibility, disconnect fencing, and cancellation cleanup. Native Providers and browser Settings now contain model-connection controls and a remote-browser callback-paste fallback. The integrated server check and browser WASM build pass. Updated native compile, live provider/hosted storage, UI workflows, and harness consumption remain pending.

- Native harness ownership composition is implemented through `Engine::open_native`, with OS-held per-root locks and private root-bound journals. Cross-process ownership and host-cleanup ordering fixtures pass; the selectable application harness and native tools remain pending.
