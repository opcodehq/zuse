# Executor v2 audit resolution

The original audit compared Zuse `e98e3050e65532f3222d56d8d0a47d45990a6bf4`,
its bundled Executor `50bea37fe5d146701990901a878ea940c5012564`, and the abandoned
native plugin executor prototype. This change imports the actual bundled SDK;
it does not continue maintaining a separate OAuth or MCP client implementation.
See [configuration and verification](cloud-native-plugins.md).

| Finding | Resolution | Evidence |
| --- | --- | --- |
| H1 Disconnect/refresh race | Database tenant fence spans the full operation across API instances. An admitted operation retains it even if its HTTP caller disappears. Disconnect delegates SDK deletion/revocation before success. | Real-SDK integration race with two controls, blocked refresh, aborted caller, and a post-disconnect invocation. |
| H2 Definition drift | Seal SDK references and pin the complete definition/auth fingerprint. Endpoint/scope/source changes fail before sending credentials. | Integration changes a sealed server endpoint and verifies rejection without provider requests. |
| H3 Missing validation | Use SDK MCP metadata, input/output schema compilation, result validation and tool resolution. Preserve typed host failures in the imported adapter. | Wrong/missing input, unlisted tool, invalid MCP content, invalid structured output and unusable output schema probes. |
| H4 Guest unwired | Install runtime plugin client during guest startup. Read renewed credentials from its supervisor instead of WorkOS. | Engine test with a signed-out account and session permission checks; guest HTTP test verifies rotation, expiry and identity/generation/closed-supervisor fences. |
| H5 Resource bounds | Admission limits, private-worker concurrency limit, aggregate byte/request budgets and bounded streams. SDK instances close after each operation; their caches have upstream byte limits. | Oversized catalog refusal, successful discovery beyond 1,000 tools, source review of limits. Production load acceptance remains outstanding. |
| M1 Schema shape | Return `e.tools.get`, including normalized metadata and app/deployment identity. | Descriptor assertions and worker restart preserving IDs. |
| M2 Unlisted calls | Resolve through actual SDK/tool router before invocation. | Unknown tool cannot increment provider call counter. |
| M3 Protocol validation | Official SDK client and server validate initialize, JSON-RPC and MCP result envelopes. | Malformed initialization and incoming JSON-RPC rejected. |
| M4 Pagination | Upstream duplicate/cursor-loop detection and discovery bounds; no successful partial result at the old native 1,000-tool ceiling. | Loop rejection and finding the 1,001st tool. Aggregate host budgets fail explicitly. |
| M5 Nonrenewable expiry | Delegate token lifetime handling to the SDK. | A grant with 30 seconds remaining and no refresh token remains usable without renewal. |
| M6 Refresh classification | Preserve typed SDK failures and selected account IDs. Use SDK renewal classification, valid-token fallback and reconnect state. | Transient renewal outage retains a usable token; revoked grant reaches `needs-auth`; mutation failure never replays. |
| M7 OAuth discovery | Actual SDK challenge/resource/issuer discovery and audience rules run through policy-checked HTTP. | Real SDK OAuth fixture plus pinned source/provenance verification. Live-provider acceptance remains outstanding. |
| M8 Requested scopes | Original bridge explicitly passes configured scopes or `[]`; no native scope expansion. | OAuth URL assertion and imported adapter source. |
| M9 OAuth validation | Actual oauth4webapi/SDK registration, token, issuer and conditional OpenID validation, rather than permissive native parsing. | Verified source archive, full bundle type/build check and real SDK OAuth round trip. Conditional OpenID providers require staging acceptance. |
| M10 Revocation | Delegate connection/account removal to SDK best-effort provider revocation. | Local provider receives revocation on disconnect. Provider-side success is inherently conditional. |
| M11 Transports/cleanup | SDK client supports Streamable HTTP and legacy HTTP+SSE; owns session cleanup and explicit unsupported-interaction errors. Auth probe recognizes legacy negotiation. | Legacy SSE connection/invocation and stream cleanup. Public human-approval/resume UI remains outside the original bridge scope. |
| M12 Cache/search | Fresh scoped SDK runtime, upstream byte-bounded caches; ordered bounded discovery returns a full first page without awaiting later stalled jobs. | Discovery/restart tests and removal of the native cache/join-all implementation. |
| M13 HTTP/callback behavior | Explicit Origin rejection, browser MCP headers and official server responses. Callback transition is an explicit fresh-vault cutover. | Origin, preflight and malformed-RPC checks. Old Workers callback state is not transferred; complete or restart those sign-ins at cutover. |
| M14 Rename | Restore management request and duplicate-label enforcement under the tenant fence. | Rename integration assertion; protocol/client checks. |
| L1 Operations/metadata | Retain SDK operation spans and safe failure tags, explicit readiness/migrations, restart recovery and authorized WorkOS organization names. Display lookup does not gate tool calls. | Type/build checks, safe failure logs, worker restart and packaged-bundle loading. |

The Postgres tests use the actual generated SDK bundle, not an imitation of its
response shapes. The guest tests exercise both credential delivery and the
engine's session-policy boundary. Existing WorkOS/tenant authentication checks,
private-address DNS filtering, encrypted storage and no automatic mutation replay
remain in place.

This closes the audited prototype defects at the implementation and local
verification level. It is not a claim of exhaustive penetration testing, live
provider acceptance or measured production capacity. No deployment is performed.

Upstream capabilities that the old remote-MCP bridge deliberately did not expose
(arbitrary app deployment, general app storage, stdio execution, skills and public
workflow/approval APIs) are not enabled implicitly by importing the SDK. The Zuse
project-automation scheduler and trigger/execution port remain separate work.
