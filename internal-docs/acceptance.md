# Implementation and acceptance status

Status: `source` = adapted implementation; `compiled` = compile check passed; `pending` = work remains; `verified` = the named scenario passed with recorded evidence. A partial scenario does not establish full feature acceptance.

| Capability | Status | Evidence / remaining work |
| --- | --- | --- |
| Cargo monorepo and native source foundation | compiled | Engine and browser server compile checks passed |
| Native shell, sidebar, chat, composer, settings | source | Integrate Zuse behavior and verify native build |
| Files/editor, terminal, Git/worktrees/PR panes | source | Validate lifecycle and complete parity |
| Claude/Codex/Cursor/OpenCode/Pi/ACP protocols | source | Audit capabilities; fixtures and live checks pending |
| Gemini and Kiro CLI adapters | compiled | Registered on the shared ACP engine; CLI launch/model/effort and complete tool/turn lifecycle fixtures pass. Live logins, provider account settings and custom ACP parity remain pending. |
| Zuse harness and multi-agent orchestration | source | Rust kernel, durable recovery, nested concurrent agents, mailboxes, shared budget/account broker, compaction, and bundled SDK transport are implemented and fixture-tested. Native file/process tools and their permission host also pass kernel integration fixtures. The engine model service consumes account storage and the shared broker/SDK bridge. A selectable Zuse provider/session adapter now connects this to the standard chat stream. Live authorization, authenticated recovery controls and full application acceptance remain pending. See `harness.md`. |
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

- CI runs `39024439637` (Settings/account integration) and `39024600601` (native root ownership) passed all jobs, including macOS/Linux native compilation, WASM, engine tests, and model-transport fixtures. This does not establish native runtime or live model-account acceptance.

- Native command milestone: the full harness suite passes 24 tests, including eight process scenarios for stdin/frame ordering, UTF-8 cursors, per-agent isolation, output bounds, deadlines, process-group cleanup, notification recovery without replay, launch handshakes, physical recovery leases, shutdown admission races, and slow-reader framing. The server binary builds and its private command-host entry point passes a real launch/output/exit smoke. Native tool dispatch, selectable Zuse provider/session adaptation, live conversations, and updated Mac/Linux CI remain pending.

- CI run `39025842571` passed all jobs for native command supervision, including macOS/Linux native compilation and the browser/engine/model-transport jobs.
- Native coding tools milestone: 32 harness tests pass, including eight native-tool scenarios and a scripted-model durable-kernel workflow that performs real file and command operations. An additional cancellation regression covers both token cancellation and dropped tool futures. The integrated server check and formatting check pass. The application provider/session adapter, live models, UI workflows and updated native CI remain pending.

- Native-tool CI run `39026828839` passed Mac/Linux native compilation, browser, and model transport. Its engine test job failed because the fresh runner lacked `rg` (search fixture exit 127); the engine test setup now installs ripgrep. No full CI success is claimed for that run.
- Account-backed model-service milestone: 13 model-account tests and 9 engine model/lifecycle tests pass. The service checks per-account live inventories, provider/credential matching, root/child affinity, quota fallback, reconnect generations, cancellation and shutdown. Owned renewal persists a rotated token after its caller is cancelled. The integrated server compile check passes. These are component fixtures; selectable Zuse sessions and live model authorization remain pending.

- Turn-lifecycle milestone: 35 harness tests pass. Successful native commands transfer to session ownership; turn stop preserves them and independent children, while explicit stop/close still terminates owned work. Root continuation and model/reasoning/context reconfiguration preserve the runtime. Steering ACKs follow durable consumption, survive replay, and remain withheld after failed persistence. The application provider/session adapter and live queue/stop UI acceptance remain pending.

- CI runs `39028800424` (account-backed transport) and `39029080325` (turn boundaries) passed all jobs, including Mac/Linux native compilation, browser, engine and model transport.
- Application-provider milestone: Zuse is registered in the native engine catalog with the shared model service, persistent sessions, normalized chat/child/tool events, input approvals, uploads, durable steering ACKs, stop/continue and journal resume. Scripted-model fixtures exercise real file edits and the application event stream; this does not prove live provider/plugin authorization or full native/browser UI acceptance. Recovery controls, provider-switch history, OS sandboxing and packaged runtimes remain pending.

- Adapter validation: 35 harness tests, five application session fixtures and 13 session-engine tests pass. A 12-test native engine regression filter also passes (overlapping those adapter/origin tests). Server/provider test targets compile, formatting passes, and the GPUI Web/WASM bundle builds. Pre-admission Stop and idle-runtime retirement regressions are covered. Updated desktop CI and live provider/plugin/UI acceptance are not claimed by these fixtures.

- CI run `39031148407` passed all jobs at `ba9876b`, including macOS/Linux native compilation, browser, engine and model transport. The preceding application-provider run failed on two missing native UI label mappings; those mappings are fixed in this passing commit. This establishes compilation, not native runtime acceptance.
- Stop/steering cancellation milestone: 36 harness tests, eight native application fixtures, 14 session-engine tests, one shared receiver test and 15 provider stop fixtures pass locally. The application fixtures route queued steering through the actual document/session engine, verify durable cancellation on turn stop and runtime termination, and resume the journal without replaying withdrawn text. A delayed cancelled frame cannot apply its model configuration. Failed checkpoint cancellation preserves mailbox order; consumed history remains intact. The integrated server check and formatting check pass. Live UI/credential/plugin acceptance remains pending.
