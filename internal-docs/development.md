# Development environment

Use stable Rust, rustfmt and Clippy. Keep versions in the workspace and lockfile. Browser builds additionally require the `wasm32-unknown-unknown` target and Trunk.

## Linux native dependencies

The native platform needs a C/C++ compiler, pkg-config, CMake, OpenSSL, Fontconfig, FreeType, XCB, XKBCommon/X11, Wayland, Vulkan loader, ALSA and Clang development files. Embedded browser previews additionally require WebKitGTK 4.1 and JSON-GLib development files. Package names vary by distribution.

The cloud development VM runs Amazon Linux 2023. WebKitGTK availability must be resolved for its native preview build; lack of a package must not silently produce a nonfunctional preview feature.

## Validation policy

Use `cargo check` for integration feedback. Keep tests centered on observable behavior and run relevant suites at milestones. Run native and browser acceptance separately: a web screenshot does not verify platform-specific desktop integration.

## Account and hosted plugins

Local sessions do not require a cloud account. Settings → Accounts includes the engine-owned Zuse account and managed plugin connections. Sign-in opens device authorization; profile metadata and progress stream through `CloudAccountStatus`. Refresh credentials are stored in the device profile's `account/session.json` with private Unix permissions and atomic replacement. Access tokens stay in engine memory and never appear in account RPC responses.

`ZUSE_API_URL` selects an HTTPS API origin (default `https://api.zuse.sh`). `ZUSE_WORKOS_CLIENT_ID` selects the public identity client for that deployment. Native plugin OAuth returns to an available loopback port from 8976–8979; the engine consumes the account-bound confirmation ticket. Browser clients running against localhost use that callback too. Remote browser clients currently use the hosted web return and still need end-to-end acceptance.

All injected agents use the same `plugins_list`, `plugins_search`, `plugins_schema`, and `plugins_call` MCP tools. The gateway derives the origin chat from the injected MCP context, rejects calls in read-only sessions, and stores chat/account bindings in `plugin-sessions.sqlite`. Switching accounts requires a new plugin session for previously bound chats. Discovery can reconnect; external calls are never automatically replayed after an uncertain transport failure. MCP cancellation cancels in-flight hosted tool calls, and transport closure cancels the connection’s outstanding engine requests; cancellation cannot reverse a side effect already performed by the remote service.

The HTTP connection pool and refresh authority are shared across concurrent agents. Plugin credentials and tool execution remain in the engine. Live OAuth, custom API-key/server entry, all tenant management controls, and full platform acceptance remain tracked in the acceptance matrix.

## Local browser engine

`cargo run -p zuse-cli -- --listen 127.0.0.1:27656 --ipc-port 0 --data-dir .context/local-profile` starts an isolated profile with an available agent-tool RPC port. Relative profile paths are resolved before workspace creation. Each launched agent receives this engine's actual RPC endpoint. SIGTERM and Ctrl+C both drain the runtime. Keep the printed browser access link private.

For an interrupted-write UI fixture, build and run `cargo run -p zuse-engine --example recovery-fixture -- <new-profile-directory>`, then serve that profile. The fixture refuses existing directories, writes an evidence file, and deliberately leaves one durable tool receipt requiring inspection. It uses a scripted model and requires no provider credentials.

## Local tool results

New tool results are saved in the profile's `docs.sqlite3` `tool_blobs` table before their references enter the transcript. Keys include the assistant entry, provider call identity, and content digest, so repeated provider call IDs cannot replace another turn's output. Output and diff save in one transaction. Deleting a chat removes its tool payloads. Ordinary transcript snapshots carry refs and byte counts, without full output text.

`FetchToolBlob` accepts `blobRef`, optional `offset`, and optional `limit`. Paged callers use UTF-8 byte cursors with a 64 KiB maximum page. Replies include `text`, `nextOffset`, `totalBytes`, and `done`. The legacy text response remains available to native clients. Both native and browser result reads address the owning engine; local tool data is not uploaded by this path. Historical results that never retained payloads cannot be recovered through these new refs.

The browser retains four result pages and virtualizes visible lines. It pages raw output and highlights complete small diff payloads; large diff JSON remains accessible as raw pages. Provider output limits remain authoritative: Claude/Codex text results are capped at 256 KiB with an explicit truncation marker; other providers keep their own adapter limits. This does not establish a complete filesystem diff for every provider.

## Browser workspace editing

File reads retain the editor value present when they started and apply only if that value is still current. Starting another read cancels the earlier request. Closing the editor cancels outstanding reads. Saves retain the starting file, checkout, and content hash with the submitted text; their acknowledgements cannot update another file. Navigation waits for a pending save even if the user undoes edits back to the earlier baseline. A successful save updates the saved baseline, while text typed afterward remains unsaved.

Workspace review loads the shared checkout summary, then reads a selected file against that summary's checksum. Large binary/truncated sources receive explicit notices. Inline textual comparison has a 100,000-line safety limit and a bounded diff computation deadline. The current editor can inspect the file separately; large source and original-format constraints still come from the engine. Directory pages append without dropping earlier entries, and the file list renders visible rows only.
