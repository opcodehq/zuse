# Rust provider integration boundaries

The provider adapters implement transport and lifecycle contracts; they are not
complete task orchestration or release certification. The deployed worker still
selects Boxd. Keep allocation disabled until the managed base and full agent flow
are certified for the selected provider.

## Contracts checked against provider sources

| Provider | Pinned interface | Important distinction |
| --- | --- | --- |
| Boxd | gRPC-web SDK 0.2.14 fixtures | Suspended memory uses ResumeVm; hibernation uses WakeVm; stopped machines use StartVm. |
| Boat | Public REST v1 | Stop has a durable operation identity; an archived machine does not complete a pending Stop. |
| E2B | Sandbox control API and envd Connect JSON | Infrastructure API keys stay at the control plane. Guest requests use the sandbox's access token. |
| Daytona | v0 REST API, OpenAPI 1.0 and SDK 0.227.0 | Snapshot class determines the resulting sandbox class. Persistent policy and toolbox trust must be checked. |

Boat's Stop ends processes and retains a disk snapshot, rather than preserving
memory. Its ordinary stopped disk expires after 14 days; saved snapshots have a
different retention contract. The controller needs an archive/recovery deadline
before that expiry. Resume readiness must include runtime and service restoration.
[Boat concepts](https://docs.boat.dev/concepts),
[Stop lookup](https://docs.boat.dev/api/reference/sandboxes/get-stop).

E2B returns `envdAccessToken` separately from its infrastructure API key. Inspection
uses GET only, including for paused machines. Connecting/resuming is an explicit
mutation. The Rust adapter bounds guest-token caches and consumes the process exit
receipt immediately, without waiting for an HTTP proxy to close the stream.
[E2B guest authentication](https://e2b.dev/docs/api-reference/envd/get-the-environment-variables).

Daytona's stable API exposes container and Linux VM classes. The selected prepared
snapshot determines the class; the adapter checks the returned class against its
configured class. It requests disabled automatic deletion and no destructive TTL,
and rejects returned policies that impose a positive TTL or automatic deletion.
The provider may impose organization/class limits; a restricted account remains
unavailable until its actual persistent lifecycle is certified. The alpha v2 API
is not silently substituted.
[Daytona API](https://app.daytona.io/api),
[Daytona lifecycle controls](https://www.daytona.io/docs/en/go-sdk/daytona/).

## Safety and reconciliation

Every mutation is attempted once per adapter call. Timeouts, broken execution
streams, malformed mutation receipts and server failures can mean the operation
was accepted. They return an uncertain outcome. A controller must inspect the
original resource/operation before deciding its next action; retrying allocation
blindly can create a second billable machine.

Boat receipts retain the original Stop ID across serialization and process restart.
Pending, provider retry, completion and supersession are distinct. A bounded or
repeating reconciliation listing cannot establish resource absence. Provider
status alone never certifies the agent handshake, Docker or declared services.

Daytona's toolbox URL is accepted only from a fresh control-plane response and an
explicit trusted origin. Infrastructure credentials never follow redirects or an
arbitrary saved URL. Bootstrap bytes use the filesystem upload protocol, not a
shell command containing the credential. E2B envd similarly receives only a guest
token. These boundaries have HTTP fixture coverage.

## Runtime performance work still required

The current sleep path requests quiescence, then calls `EngineCore::shutdown`
before acknowledging it. This kills agent processes and PTYs even when a provider
could preserve memory. A sub-second memory resume therefore needs a durable held
execution state with closed command admission, checked flushes and retained
processes. Cold providers still need process restoration. Do not claim a fast
provider wake time as end-to-end agent readiness.

Repository/environment preparation also currently precedes the guest event
connection. Moving progress and connectivity earlier must preserve storage and
credential enrollment checks and prevent turn admission before the checkout,
environment, credentials and agent handshake are ready.
