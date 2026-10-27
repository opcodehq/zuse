# Internal native transport check

2026-10-27. Cloud VM only. The requested models were `claude-sonnet-5-5` and
`claude-opus-5-5`, both configured with **medium** reasoning. No fallback model or
lower effort was selected.

## Observations

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

These results establish inventory access and attempted model routing, not successful
inference, actual reasoning execution, task quality, cache hits or cost savings.
The exact throttle/quota cause and reset time are unknown. Further live comparisons
were stopped rather than repeatedly issuing requests against the same failure.

The persistent global ledger has **9 of 300 reservations** consumed: eight observed
HTTP requests and one unused preparation reservation retained conservatively.
Six HTTP requests attempted inference; none completed. Inventory and invalid-model
probes are also counted conservatively. No API-equivalent dollar estimate or actual
subscription charge is inferred. There are 291 reservations left for subsequent
live work; retain the ledger when resuming.

## Credential handoff

Native routes now accept explicit `authentication: "bearer"`; API-key protocol
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
    "authentication": "bearer",
    "betas": ["oauth-2025-04-20"],
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

The two model suites have not proceeded to coding/cache comparisons. Resume those
with the same model IDs and medium effort once requests can complete; do not reset
the global ledger or enable context reductions as a result of these failed smokes.
