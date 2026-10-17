# Implementation and acceptance status

Status: `source` = adapted implementation; `compiled` = compile check passed; `pending` = work remains; `verified` = the named scenario passed with recorded evidence. A partial scenario does not establish full feature acceptance.

| Capability | Status | Evidence / remaining work |
| --- | --- | --- |
| Cargo monorepo and native source foundation | compiled | Engine and browser server compile checks passed |
| Native shell, sidebar, chat, composer, settings | source | Integrate Zuse behavior and verify native build |
| Files/editor, terminal, Git/worktrees/PR panes | source | Validate lifecycle and complete parity |
| Claude/Codex/Cursor/OpenCode/Pi/ACP protocols | source | Audit capabilities; fixtures and live checks pending |
| Gemini, Kiro, remaining Zuse provider parity | pending | Port missing protocol and account behavior |
| Zuse harness and multi-agent orchestration | pending | Port from existing local source and tests |
| Durable queues and steering | source | Reconcile with current Zuse queue/error/recovery semantics |
| Executor v2 plugins | pending | Implement hosted service client and per-session tool gateway |
| Voice, interactive visuals, appearance | source | Native platform and browser capability integration pending |
| GPUI Web/WASM | compiled | Initial WASM build passed; browser shell renders and keyboard input reaches composer; full integration pending |
| Shared transcript model | verified | 30 reconciliation/folding tests passed |
| Browser authentication boundary | verified | 3 origin/cookie isolation tests passed |
| Localhost and LAN serving | compiled | Authenticated HTTP/WebSocket server compiles; runnable engine binary is linking |
| Owned GPU toolkit | source | Independent `opcodehq/zkit` snapshot prepared; browser platform validation and component cleanup underway |
| Pairing and encrypted relay | pending | Rust relay and native/browser client integration |
| Fresh profile and local persistence | source | Separate native paths and crash recovery validation |
| macOS packaging and native acceptance | pending | Actual Mac build and runtime evidence required |
| Linux packaging and native acceptance | pending | X11/Wayland runtime and installation checks required |
| Browser deployment replacement | pending | Production-ready artifact and cutover configuration |

## Verification log

- Initial discovery: workspace has no prior commits or application files; existing application is available read-only in the sibling checkout.
- Native source foundation imported with license attribution and renamed Rust packages; no runtime behavior has yet been claimed verified.
- Initial engine/server compile checks passed. Shared model tests: 30 passed. Browser origin/cookie tests: 3 passed.
- Browser runtime exposed an application-owner lifetime bug; the entrypoint now retains its embedded application handle. A real GPU canvas renders the shell and accepts composer input. Engine connection, uploads and complete browser workflows remain pending.
