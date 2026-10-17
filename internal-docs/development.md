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
