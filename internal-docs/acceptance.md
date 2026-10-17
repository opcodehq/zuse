# Implementation and acceptance status

Status: `source` = adapted implementation, not yet compiled/verified; `pending` = work remains; `verified` = scenario passed with recorded evidence.

| Capability | Status | Evidence / remaining work |
| --- | --- | --- |
| Cargo monorepo and native source foundation | source | Workspace metadata resolves; first engine build in progress |
| Native shell, sidebar, chat, composer, settings | source | Integrate Zuse behavior and verify native build |
| Files/editor, terminal, Git/worktrees/PR panes | source | Validate lifecycle and complete parity |
| Claude/Codex/Cursor/OpenCode/Pi/ACP protocols | source | Audit capabilities; fixtures and live checks pending |
| Gemini, Kiro, remaining Zuse provider parity | pending | Port missing protocol and account behavior |
| Zuse harness and multi-agent orchestration | pending | Port from existing local source and tests |
| Durable queues and steering | source | Reconcile with current Zuse queue/error/recovery semantics |
| Executor v2 plugins | pending | Implement hosted service client and per-session tool gateway |
| Voice, interactive visuals, appearance | source | Native platform and browser capability integration pending |
| GPUI Web/WASM | pending | First build/input/streaming/upload/screenshot milestone |
| Localhost and LAN serving | pending | Authenticated HTTP/WebSocket service |
| Pairing and encrypted relay | pending | Rust relay and native/browser client integration |
| Fresh profile and local persistence | source | Separate native paths and crash recovery validation |
| macOS packaging and native acceptance | pending | Actual Mac build and runtime evidence required |
| Linux packaging and native acceptance | pending | X11/Wayland runtime and installation checks required |
| Browser deployment replacement | pending | Production-ready artifact and cutover configuration |

## Verification log

- Initial discovery: workspace has no prior commits or application files; existing application is available read-only in the sibling checkout.
- Native source foundation imported with license attribution and renamed Rust packages; no runtime behavior has yet been claimed verified.
