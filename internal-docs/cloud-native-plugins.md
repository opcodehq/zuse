# Rust plugin API with Executor v2

The Rust API runs the actual pinned Executor v2 SDK through a private Bun worker.
`packages/executor-v2/vendor/provenance.json` identifies upstream commit
`50bea37fe5d146701990901a878ea940c5012564`; the source archive is retained and its
SHA-256 is checked by every build. The original Zuse bridge was imported from
`swarajbachu/zuse` commit `e98e3050e65532f3222d56d8d0a47d45990a6bf4`.

The adapter uses Executor's PostgreSQL storage provider instead of Cloudflare
SQLite. OAuth, account/profile binding, MCP transports, input/output validation,
normalized tool schemas and provider revocation execute in the imported SDK.
The earlier handwritten Rust OAuth and MCP client implementations have been removed.
Rust owns public authentication, tenant authorization, connection UI metadata,
opaque callback tickets, definition pinning and outbound network enforcement.

## Configure and migrate

The API container includes Bun 1.3.10 and both generated worker bundles. For local
runs, put Bun on PATH. `ZUSE_CLOUD_EXECUTOR_BINARY` can select another Bun executable;
`ZUSE_CLOUD_EXECUTOR_SCRIPT` selects the packaged private worker entry point.

Set the existing cloud database and WorkOS configuration, plus:

- `ZUSE_CLOUD_PLUGIN_ENCRYPTION_KEY`: dedicated 32-byte base64/base64url key.
- `ZUSE_CLOUD_PUBLIC_ORIGIN`: externally reachable HTTPS API origin.
- `ZUSE_CLOUD_PLUGIN_APP_ORIGIN`: HTTPS app origin for OAuth returns.
- `ZUSE_CLOUD_ALLOWED_ORIGINS`: permitted browser origins, including the app.

Run this explicitly before enabling traffic:

```sh
cargo run -p zuse-cloud-api -- plugin-migrate
# With the cloud image: replace its usual `serve` argument with `plugin-migrate`.
```

This applies the idempotent plugin metadata migration and Executor's own supported
schema migrations under a PostgreSQL advisory lock. Normal startup never migrates.
The database role needs write access to `zuse_plugin_*`, `zuse_executor_state` and
Executor's `executor_*` tables; migration also requires schema DDL privileges.
The ordinary catalog pool remains read-only. `/readyz` verifies both the plugin
metadata and actual SDK schema through the worker.

This remains a fresh vault. Existing Workers credentials are not imported. Rows
created by the abandoned native prototype are marked `needs-auth` during the
explicit migration; they must reconnect through Executor. Stored prototype
ciphertext is retained until removal. Existing Workers callbacks cannot be
continued against the new vault. Complete pending sign-ins before cutover or
restart them afterward.

A domain-separated key is derived from the plugin master key. The SDK encrypts
its grants/account credentials, and the host encrypts source/blob state. Rust
seals custom endpoints, callback material and SDK references with tenant/type/ID
associated data. Retain the master key across releases. Key rotation requires a
separate re-encryption procedure or an intentional vault reset.

## API and guest execution

The management/tool contracts remain `/v1/plugins` and `/v1/plugins/tools`.
Connection rename, duplicate-label rejection, custom servers, enable/disable and
OAuth polling/cancellation/completion are supported. Schema responses are the
actual Executor descriptors, including app/deployment identity and normalized
read-only metadata.

Organization clients use `/v1/organization-workspaces/:org/v1/plugins` and its
`/tools` counterpart. The verified tenant must agree across the route, workspace
header and request body. Members can execute organization-owned connections;
management requires an administrator. Organization discovery follows the existing
rollout allowlist and retrieves authorized display names from WorkOS.

`POST /v1/plugins/:tenant/mcp` uses the official MCP SDK server for parsing,
negotiation and responses. A validation-only pass precedes any external tool
execution. Browser MCP protocol headers are allowed by CORS; unapproved Origin
headers are explicitly rejected. The upstream client supports both Streamable
HTTP (including SSE responses) and legacy HTTP+SSE negotiation.

The cloud guest installs `RuntimePluginClient` in the engine. Each invocation
reads the current renewable workspace credential from the supervisor's watch
channel, so it requires no saved WorkOS session. It rejects expired credentials,
closed supervisors and changed owner/workspace/generation. The server derives the
tenant after verifying Rust ownership, enrollment, generation and deletion fences.
The engine checks the chat's session permissions before invoking this client.

## Execution and failure guarantees

A per-tenant PostgreSQL transaction advisory lock serializes invocation,
connection changes, callbacks and maintenance across API replicas. Disconnect
cannot report success ahead of a call waiting on token renewal. It fences the
connection and delegates account deletion/revocation to Executor before returning.
Calls are never replayed automatically after an uncertain result. Executor may
repeat credential-rejected discovery and queries according to its own read-only
classification; mutations are not replayed.

Connections pin the endpoint, discovery URL, scopes, auth method and other source
definition fields. Changes require reconnecting; old credentials are never sent
to a newly resolved endpoint. SDK references contain the persisted app/profile
identity, preserving behavior across worker restarts.

Rust limits active plugin operations to two per API instance. The private worker
also caps concurrent requests and enforces an aggregate 8 MiB HTTP response
budget per operation. Discovery reports budget/protocol errors instead of silently
truncating at 1,000 tools or returning duplicate cursor-loop results. SDK metadata
caches use its built-in byte limits; each operation closes its SDK runtime, so
process-wide custom catalog caches cannot grow without bound. Ordered, bounded
search discovery returns a full first page without waiting for later stalled
connections.

All worker HTTP, including SDK global-fetch transports, passes through a private
authenticated Rust proxy. It checks HTTPS/public DNS destinations, rejects
private/reserved addresses, strips hop-by-hop headers and disables redirects and
environment proxies. Loopback HTTP can only be selected through explicit test
configuration, never production environment parsing. Proxy streams are bounded. Cancelled discovery and worker
deadlines abort network work while allowing bounded session cleanup. Admitted operations retain their
tenant fence through completion even when the HTTP requester disconnects;
disconnect cannot overtake an orphaned token renewal or invocation.
Both private listeners bind loopback and require a fresh process-specific bearer.

A parent-side watchdog terminates a worker that exceeds its request deadline,
including CPU stalls that prevent a JS timer from firing. Unexpected worker exit
fails its current request. A later request starts a new worker against the
retained SDK database, without retrying the failed call.
Diagnostics retain safe operation and SDK failure tags; provider bodies, tokens,
URLs and SQL errors are not returned or logged by the bridge.

## Verification

```sh
node packages/executor-v2/build.mjs --check
cargo check -p zuse-cloud-api -p zuse-cloud-guest
cargo test -p zuse-cloud --lib
cargo test -p zuse-engine --lib runtime_plugins_work_without_workos
cargo test -p zuse-cloud-runtime --features engine --lib plugin_credentials_rotate
ZUSE_CLOUD_TEST_DATABASE_URL=postgres://...@127.0.0.1:5432/test \
  cargo test -p zuse-cloud --test postgres native_plugins -- --ignored
```

The database scenarios execute the real generated Executor bundle against an
isolated PostgreSQL schema and local MCP/OAuth fixtures. They cover both migrations,
restart, encrypted state, PKCE/ticket replay, refresh/revocation, tenant/runtime
fences, schema shape, invalid input/output, unknown tools, malformed initialization
and JSON-RPC, cursor loops, catalogs beyond 1,000 tools, byte budgets, definition
changes, rename, browser origins/CORS and a refresh/disconnect race across separate
API instances, including an aborted HTTP caller. Guest tests cover renewable
credentials and the engine's permission check without WorkOS.

Live provider acceptance and production load testing remain deployment checks.
No deployment or production database mutation is part of this implementation.

## Scope

This imports the actual Executor SDK while retaining the old Zuse bridge's remote
MCP product scope. It does not expose arbitrary source deployment, general app
storage, stdio processes, skills or workflow/approval APIs through the public Rust
API. These were not fully exposed by the old bridge either. The original Zuse
project-automation scheduler, trigger delivery and execution remain a separate
subsystem to port; importing Executor does not implement them.
