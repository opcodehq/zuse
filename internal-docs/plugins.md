# Hosted plugins and agent workspaces

The engine connects native and browser settings plus every agent's shared MCP tools to the hosted Executor v2 service. Provider adapters do not receive cloud access tokens or plugin credentials. Session policy permits discovery in read-only chats but rejects external tool calls before sending a request. Plugin calls are not automatically replayed after an uncertain response.

## Workspace selection

Settings → Plugins lists Personal plus accessible organizations from the account membership API. Only memberships with content access appear. Selecting a workspace uses the explicit `SelectPluginWorkspace` action with `{action:"list", tenantId}`. The engine first reads and validates that workspace's authorized plugin snapshot, then saves the selection for the current account in the profile's plugin database. Ordinary `Plugins` list reads do not change the selection. Both controls belong to the connected profile and reject foreign `targetDeviceId` values.

A chat binds to the current account and selected plugin workspace on its first plugin discovery or invocation. Later settings changes affect chats that have not used plugins yet. Existing chats keep their bound workspace through engine restarts. Switching accounts cannot reuse a chat's binding. Historical bindings that predate workspace support remain Personal, including when their first post-upgrade invocation happens after selecting an organization. Preferences are account-specific and contain workspace identifiers, not credentials.

Management, OAuth ticket completion, discovery, schema lookup and execution all retain their explicit workspace. Personal requests use `x-zuse-workspace: personal`. Organization requests use both the organization header and the versioned organization-workspace API route. An API deployment that does not understand that route rejects the request instead of executing it against Personal. The hosted service still authorizes membership and administration rights for each operation.

The membership lookup is optional metadata. An unavailable lookup preserves the authorized selected snapshot; it never changes an existing session's workspace or retries a tool in Personal. Account cancellation, sign-out and identity changes still abort or reject the request. Plugin responses are bounded, redirect forwarding is disabled, and another workspace's snapshot cannot replace the requested one.

An uncertain workspace-selection receipt triggers a read of the saved selection, preserving the previous rows while awaiting confirmation. It does not repeat the selection action. Browser reconnect uses the same shared state; stale responses remain scoped to their original account and request.

## Local stdio servers

Settings → Plugins → Add MCP server offers Remote and Local in the Personal workspace. Local takes a name, a command, literal arguments (one per line) and `NAME=value` environment lines; the command field also accepts one pasted stdio JSON configuration (`{command,args,env}` or `{mcpServers:{<one entry>}}`). Organization workspaces only offer Remote.

`addLocalServer` never reaches the hosted API. The engine validates the request, starts the command with a cleared environment (a small allowlist plus the user's login-shell `PATH`, then the configured variables), and saves it only after MCP initialization and `tools/list` succeed. Servers are stored per account at `<data>/local-plugins/<sha256(subject)>.json` (directory 0700, file 0600, atomic replacement). A malformed file fails closed: nothing is listed, launched or overwritten. Replayed requests are deduplicated by `requestId`, writes are serialized and each account may keep 50 servers. Snapshots carry the command line but never environment values.

Local rows use `local-` identifiers. Removing, disconnecting or turning off such a row is handled by the engine and stops its process. Each enabled server runs lazily as one process group that is killed when it is turned off, removed, the account changes or the engine exits. Chats bound to the account's Personal workspace see local tools through the same `plugins_list/search/schema/call` gateway tools: discovery merges hosted and local results and stays available if one source fails, while schema and call requests for `tools.local-…` addresses go to the local process. The read-only session refusal applies before any local call, uncertain call outcomes retire the process without retrying, and hosted runtimes never launch local servers.

## Verification and remaining acceptance

Three trusted-TLS contract scenarios exercise six simultaneous cross-workspace calls, scoped management/OAuth routes, membership choices, read-only policy, invalid scopes, mismatched responses, account switching during a request and cancellation without replay. Two SQLite scenarios cover migration, restart, stable workspace bindings and account isolation. Shared protocol and viewport regressions cover uncertain selection receipts and read reconciliation.

These are local contract and state scenarios. The current preview is signed out of the hosted account, so live account/organization authorization, actual Executor plugin execution across installed providers, OAuth renewal/revocation and physical Mac/Linux workflows remain acceptance requirements. Source/build checks do not establish those live results.
