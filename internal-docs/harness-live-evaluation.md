# Internal native transport check

2026-10-27. Cloud VM only. The requested models were `claude-sonnet-5-5` and
`claude-opus-5-5`, both configured with **medium** reasoning. No fallback model or
lower effort was selected.

## Root cause and corrected native OAuth mode

The initial 429 interpretation was incomplete. The user's account showed only
11% session usage and 45% weekly usage. A capped check through the bundled client
succeeded with the same credential, exact Sonnet model and medium effort. Its
provider headers also reported the subscription windows as allowed. Quota reads
with either token returned 403 because the tokens lack `user:profile`; that was
not an inference-authentication failure.

Controlled changes isolated a missing request-body attribution requirement:

| Controlled request | Result |
| --- | --- |
| Stock client request | HTTP 200 |
| Remove only its billing-attribution block | HTTP 200 |
| Remove only its SDK-identification block | HTTP 200 |
| Remove both blocks | HTTP 429, `rate_limit_error: Error` |
| Native Rust request plus native-version billing attribution only | HTTP 200 |

Changing the stock client's user-agent to the native runtime's name still worked.
Removing account metadata or lowering its output ceiling to the native limit also
worked. Adding beta flags alone to the native request did not resolve the error.
These observations identify an OAuth request-compatibility problem, rather than
exhausted account quota, for the original failures. The provider returned a generic
429 instead of identifying the absent attribution context.

`authentication: "anthropic-oauth"` is now an explicit native transport mode. It:

- Requires Messages protocol and a declared subscription route.
- Uses Bearer authentication and the two required OAuth/coding beta flags.
- Inserts a stable, dedicated `x-anthropic-billing-header` system block using the
  native crate's **actual** version and the coding-harness CLI entrypoint.
- Preserves the caller's instructions and their existing cache breakpoint.
- Includes the generated attribution in the diagnostic prefix fingerprint.

This mode does not claim the application was built with the vendor SDK or change
its user-agent to the bundled client's identity. Generic Bearer and API-key modes
retain their previous behavior. The generated context stays out of durable
canonical history and is generated once in each wire request.

Both direct native smoke checks now complete successfully: `claude-sonnet-5-5`
and `claude-opus-5-5`, each at medium. Regression tests cover required attribution,
automatic beta flags, empty system instructions, protocol validation, cache-boundary
preservation and stable fingerprints across ordinary turns.

One initial coding trial per model exercised real native tools. Opus passed
the clamp acceptance test in five physical requests. Sonnet stopped after two
requests with `invalid_tool_arguments`. A subsequent captured stream isolated a
valid parameterless call that sent only an empty `input_json_delta`. The native
parser now preserves initial input for empty deltas, while rejecting malformed
nonempty JSON and references to unknown blocks. Fragmented transport tests cover
these cases, and the original coding task subsequently passed. Initial failures
remain in the discovery evidence. See the [reference evaluation](harness-reference-evaluation.md)
for repeated coding measurements; the earlier checks alone do not establish
comparative quality or justify enabling context reductions.

At that historical checkpoint the shared ceiling had **33/300 reservations** consumed,
including earlier diagnostics and one unused preparation reservation. The subsequent comparison continues from that same ledger; its report records
the updated count. Corrected smoke and coding evidence is in
`.context/native-oauth-fixed/`; controlled differential requests are in the
`.context/rate-limit-*/` directories. Credentials are absent from those reports.

## Historical initial observations (before the attribution fix)

The credential inherited from Conductor's environment reached the provider:

- Authenticated model inventory returned HTTP 200 and listed both exact model IDs.
- A deliberately invalid model returned HTTP 404 model validation, without inference.
- Both actual native Rust smoke requests returned HTTP 429.
- A subsequent small Sonnet request, still at medium, returned HTTP 429 with
  `rate_limit_error`. The message was generic and no `Retry-After` was supplied.

| Model | Configured reasoning | Native smoke | Reported token/cache usage |
| --- | --- | --- | --- |
| `claude-sonnet-5-5` | medium | HTTP 429; no completed inference | unknown |
| `claude-opus-5-5` | medium | HTTP 429; no completed inference | unknown |

A second check used the credential subsequently provided by the user, with its
wrapped lines joined in memory. Both exact models at medium again returned HTTP
429 through the native evaluator. A small Sonnet diagnostic also returned 429;
no retry/reset headers or token usage were supplied. This does not establish
successful inference or validate the new credential independently of the throttle.
During the initial retest, the credential was handed to child evaluators through a
scoped environment variable and was not written into profile files, command
arguments or reports. At the user's subsequent storage request, it was saved
outside the workspace in `~/.config/zuse-harness/internal-auth/credentials.json`
(mode `0600`, directory `0700`). The adjacent `with-auth` launcher sets
`CLAUDE_CODE_OAUTH_TOKEN` for one requested process. Local handoff and CLI auth
status were verified without another model request; its fingerprint matches the
credential used for the preceding failed smoke checks.

Those initial failures established inventory access but no successful inference.
The attribution investigation above supersedes the initial unknown-cause diagnosis.

At that earlier checkpoint, the persistent global ledger had **9 of 300 reservations** consumed: eight observed
HTTP requests and one unused preparation reservation retained conservatively.
Six HTTP requests attempted inference; none completed. Inventory and invalid-model
probes are also counted conservatively. No API-equivalent dollar estimate or actual
subscription charge is inferred. At that point 291 reservations remained; the corrected checkpoint above supersedes
that count. Retain the ledger when resuming.

## Credential handoff

Native routes accept generic `authentication: "bearer"` and the explicit
`authentication: "anthropic-oauth"` mode above; API-key protocol
defaults remain unchanged. Optional provider beta flags are bounded and validated
before dispatch. Default-valued new fields are omitted from route serialization
so existing continuation/cache route fingerprints remain compatible.

Configured application connections accept `credentialEnv` (`apiKeyEnv` remains an
alias), `authentication`, `betas` and an explicit `subscription` billing declaration.
This is an explicit internal route; credentials are not automatically copied from
other applications or refreshed by importing their private credential stores.
Existing account-service renewal for the original subscription routes is unchanged.

The evaluation connection file accepts exactly one of `accessToken` or
`credentialEnv`. These internal profiles use `credentialEnv` only. The evaluator
reads the token from its environment and scopes account identity to its fingerprint;
it never writes the token into profiles, commands, reports or journals. A declared
subscription route is configuration metadata, not proof of actual billing charges.

Example connection file, kept private with mode `0600`:

```json
{
  "connectionId": "internal-evaluation",
  "credentialEnv": "CLAUDE_CODE_OAUTH_TOKEN",
  "model": "claude-sonnet-5-5",
  "reasoning": "medium",
  "contextWindowTokens": 200000,
  "route": {
    "endpoint": "https://api.anthropic.com/v1/messages",
    "protocol": "messages",
    "authentication": "anthropic-oauth",
    "subscription": true,
    "capabilities": {
      "images": true, "tools": true, "reasoning": true, "promptCache": true
    },
    "maxOutputTokens": 4096
  }
}
```

The context value is a conservative evaluation ceiling, not a measured provider
context-window entitlement. Both smoke profiles use the same limits. Change only
`model` to select the requested Opus ID. Token environment variables remain supplied
by the owning cloud runtime; no token contents are included here.

## Failure classification and verification

The first live smoke exposed an evaluation bug: a provider error on the final
allowed request was labelled `capped` based on the kernel admission counter alone.
The runner now uses the durable transport-budget denial as the authority. It allows
one denied internal admission to report that condition, while the physical request
ceiling remains six (one for a smoke). HTTP 429 is reported as a provider failure.
Original smoke events are preserved unchanged and their classification correction
is recorded in the report.

Checks passed:

- Harness tests, including Bearer/API-key header isolation, medium reasoning fields,
  beta validation, default route compatibility and environment credential selection.
- Application provider configuration, native session paths and model-service fixtures.
- Local HTTP end-to-end evaluation: 12 coding runs, three smoke runs, a six-request
  cap and a separate one-request HTTP 429 failure. All checks passed, with 61 local
  requests on 17 TCP connections and zero external model calls during those fixtures.
- Clippy with the existing three documented style-lint exceptions.

Cloud-local evidence lives in this worktree's ignored `.context/`:

- `live-native-medium/report.json` and per-model smoke events/configuration.
- `live-user-token-medium/report.json` and its per-model events record the supplied-credential retest.
- `native-oauth-auth-probe.json`, `native-oauth-model-inventory.json` and
  `native-oauth-rate-limit-diagnosis.json` contain sanitized probe results.
- `harness-evaluation-global.ledger` is the persistent shared 300-reservation ceiling.
- `harness-oauth-offline-validation/` contains the local integration evidence.

The full comparative suites remain pending. Preserve failed runs, retain the
shared ledger, and keep the selected model IDs and medium effort when continuing.
The limited successful checks do not enable context reductions by default.
