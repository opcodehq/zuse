# Private secret requests

Status: private bearer-token operations implemented; verification results are recorded in the change report.

## T3 source review

Inspected cached pingdotgg/t3code revision 572cf74275bc0fc4dcdaee754db64fb3d5b575c1.
See apps/server/src/secrets/SecretRequests.ts and
apps/server/src/mcp/toolkits/orchestrator/tools.ts in that repository.

T3 records request metadata and status separately from values. Its backend stores
values behind references, validates project ownership and a 24-hour lifetime,
and serializes consumption so a reference is used once. Failed deletion prevents
delivery. Answer handling checks that the request is pending and its run active.

Its optional file delivery writes through an exclusive temporary file (0600)
in a private directory (0700), renames it, and removes the stored copy. Cleanup
runs at startup and hourly. File cleanup uses modification time; this is not a
hard access deadline, and commands running as the owner can read the file.
The UI must not describe this mode as preventing agent access to the value.

## Zuse boundary

The existing viewport question UI submits answers through the durable
respondInput command. Native harness callbacks return those answers as tool text.
Neither route is suitable for secret values.

## Implemented flow

Both the desktop composer and web viewport render a dedicated password input.
It never borrows ordinary composer input, draft state, or durable answers.
Native tools and the MCP catalog expose `request_secret` with label, reason,
HTTPS URL, method (GET/POST/PUT/PATCH/DELETE), and optional JSON body.
The card presents that exact operation for approval or decline. It expires
in ten minutes and does not permit shell/file delivery.

`SubmitPrivateSecret` is a transient, device-routed RPC. It validates the
request's chat/request binding and active turn before resolving the input with
only a saved/declined marker. Ordinary QueueCommand submissions targeting a
private question are rejected before persistence. MCP requests use their origin
chat and one-shot RPC delivery rather than automatic replay.

The process-local broker uses random IDs, scoped ownership, bounded entries and
values, atomic one-use consumption, and zeroizing stored strings. RAII cleanup
removes requests on decline, cancellation, timeout, and dropped operations.
Values are never written to a secret file. Restart invalidates all requests.

The backend inserts a sensitive Authorization header for the exact approved
request. Redirects and proxy environment inheritance are disabled. DNS addresses
are checked against a conservative public-address policy and pinned for the
request; lookup and HTTP execution have time limits. Only HTTP status is
returned, with generic errors. Response bodies and headers are withheld even
when a remote server echoes credentials.

This first consumer supports bearer-authenticated HTTPS operations only.
It does not hand reusable credentials to shell commands or generic plugins,
and does not return API response data such as newly created resource IDs.
Additional consumers require a reviewed output contract.

Example agent tool call (the credential is entered only in the private card):

```json
{"label":"GitHub API token","reason":"Check API access","method":"GET","url":"https://api.github.com/user"}
```

The result is an HTTP status. The backend intentionally withholds `/user`'s
response body. Secret submission is bound to a fresh engine-instance namespace
as well as the chat and pending request, so profiles cannot reuse each other's
request metadata even when chat IDs overlap.

## Security boundary

This protects normal agent context, transcripts, durable queues, and tool output.
It does not isolate the engine's address space from a hostile process with the
same OS identity, a debugger, administrator access, or a compromised UI/host.
Achieving that stronger guarantee requires an independently permissioned broker
process and OS restrictions on agent execution. This change does not claim that
boundary. Header/transport libraries and input editors may retain transient
copies; zeroizing broker storage is not a promise that every memory copy is wiped.

The authenticated user approves the exact destination and payload. As with any
credential entry, the receiving service must be trusted with the token.

## Future stronger isolation contract

1. An agent requests a secret with a label, purpose, and supported consuming
   operation. The transcript contains only metadata and pending/saved/declined/
   expired status. User input is masked and never copied to ordinary answers.
2. An authenticated, authorized transient endpoint accepts the value on the
   execution host. Do not put values in command queues, document synchronization,
   transcripts, telemetry, error bodies, or generic RPC request logging. Remote
   submission requires an authenticated encrypted transport.
3. Store values in a broker isolated from the agent OS identity. Return an opaque
   capability scoped to project, session, run, operation, expiry, and use count.
   A guessed or cross-session capability must fail. Validate scope at consumption.
4. Prefer fixed backend operations: the broker inserts credentials into an
   authenticated request to an approved destination and returns a validated,
   bounded response. It must never return authentication headers or raw request
   dumps. The agent cannot select an arbitrary destination for a credential.
5. Generic shell injection cannot promise that an agent never sees the value:
   the process can print, transform, persist, or transmit it. Environment variables,
   private files, and output redaction do not establish that guarantee. Offer any
   future command mode under an explicitly weaker, accurately labeled contract.
6. Use short deadlines checked on every consume, atomic one-use consumption,
   cancellation/revocation, bounded input sizes, and deletion on completion or
   expiry. Restart invalidates memory-only capabilities. Persistent storage needs
   a separately reviewed encryption/key-management design.

## Required verification before release

- Sentinel values absent from transcripts, durable queues, sync, logs, traces,
  errors, tool results, and reconnect snapshots.
- Cross-project/session/run use denied; expired, revoked, replayed, or concurrently
  consumed references denied; cancellation and duplicate submission handled.
- Agent filesystem and shell cannot access broker storage or broker credentials.
- Consuming operations reject arbitrary destinations and response echo of secrets.
- UI clears submitted input and offers decline; closed runs cannot accept values.

Automated tests cover broker scope, routing, replay, expiry, decline, cancellation,
public destination policy, and value-free errors. The integration test exercises
transient submission and scans chat data and persisted host files for a sentinel.
A real desktop interaction and a live credential/network operation require a
manual smoke test; do not use real credentials in automated tests.

## Completed verification

- Desktop and web viewport Cargo checks: passed. Linux development files were
  extracted locally for the check; system packages were not changed.
- Broker/security unit tests: 7 passed.
- MCP origin-binding test: 1 passed.
- Private submission integration test: 1 passed, including sentinel scans of
  transcript data and persisted host files.
- Existing native-tool regression suite: 12 passed.
- Question wizard regression tests: 8 passed.
- Git whitespace/error check: passed.

These checks used synthetic credentials. A live credential request and manual
interaction with the desktop/web secret cards were not exercised. OS-level
isolation against a hostile same-user process remains outside this change.

## Visual fixture

`private-secrets-fixture` renders the production desktop Composer's private
request card with isolated synthetic state. It captures empty and masked-token
states, without sending a credential or performing an HTTP operation. On Linux,
the capture helper reads only the fixture's own X11 window and requires Python
with Pillow and a working X11 display.

```sh
cargo run -j 2 -p zuse-ui --features appshots-fixture --example private-secrets-fixture -- /tmp/private-secret-screenshots
```

The desktop and web question UI follows the attached tray structure in
`swarajbachu/zuse` (reference commit `c02d624db23b2317c14b7aff90526828b040eb67`):
a centered tray at 14/15 width above a full-width answer composer. Private
requests use a separate masked input with Decline immediately left of Approve
and use. Ordinary questions use the composer editor.

The fixture also captures empty and typed ordinary questions. Latest verified
captures are in `.context/question-composer-screenshots/` in this checkout.
Captures succeeded, but the fixture reports a ComposerInput handle leak during
shutdown; its process exit is not a passing runtime check. The last Decline button
move was compile checked after these captures.
