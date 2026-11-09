# Cloud verification against Boxd and Boat

## November 9, 2026: adapter lifecycle on all four providers

`crates/cloud/examples/provider_lifecycle_probe.rs` drives our Rust provider
adapters directly against the staging accounts. For each provider it creates a
computer from the newest staging managed base, waits for the installed runtime,
writes a disk marker, sleeps and resumes, rechecks the runtime and marker, then
deletes and confirms absence. `--runtime PATH` also uploads and runs a current
runtime build, and environment checks record what app features need.

| Provider | Create | Runtime ready | Sleep settled | Resume to runtime | Disk kept |
| --- | ---: | ---: | ---: | ---: | --- |
| Boat | 0.4–0.9 s | 4.6–16.7 s | 2.0–5.6 s | 0.8 s | Yes |
| E2B | 1.1–1.2 s | 0.8–2.7 s | 0.1 s | 0.6–0.7 s | Yes |
| Daytona | 0.8–2.2 s | 0.1–0.2 s | 1.6–2.1 s | 0.9–1.0 s | Yes |

- The bases are from October 31 (runtime 0.25.1). Runtime 0.26.0
  (`7ae4b2179f3e`, from this branch) also runs on all three and reports its
  capabilities. Republish each base before an end-to-end app test.
- `sshd` and passwordless `sudo` exist on Boxd, Boat and E2B. Daytona's
  container base has no `sshd`: the installer now adds the binary (no
  listening service, no shared host keys), and the guest explains the gap
  until the base is republished.
- Daytona computers are containers, so `/proc` reports the host (48 CPUs,
  ~395 GB). The Performance panel now reads cgroup limits when set; on
  Daytona it reports 1 CPU and 2 GiB. VMs are unchanged.
- Sixteen probe computers were created and all were confirmed deleted.

Verified October 29, 2026 against the configured live provider accounts. Provider
infrastructure works, but **the deployed cloud application is not ready end to
end**. These checks do not establish working chat, credential delivery, Git setup,
agent startup, or first-token latency.

## Repeated timing follow-up

[Fresh timing measurements](cloud-timing-results.md) add three Boxd cycles, four
immediate Boat attempts and one forced cold restore. All eight machines were
cleaned up. One Boat Stop did not confirm suspension within 90 seconds; a separate
recheck passed. The earlier table below remains the original verification run.

## Live results

| Scenario | Create request to successful command | Resume request to successful command | Disk continuity | Process continuity |
| --- | ---: | ---: | --- | --- |
| Boxd isolated machine | 818 ms | 454 ms | Pass | Pass |
| Boxd repeat with production Rust inspect/exec checks | 804 ms | 485 ms | Pass | Pass |
| Boat small sandbox, immediate resume | 881 ms | 1,198 ms | Pass | Pass |
| Boat forced cold restore by resizing | 938 ms | 1,584 ms | Pass | Lost, as expected |

These are individual samples, not percentiles or complete workspace startup times.
Authentication is measured separately and excluded from these figures. Pause/stop
acknowledgement is separate from settled suspension: in the first run, Boxd took
another 2.48 seconds and Boat another 12.26 seconds to reach their sleeping states.

The Boxd/Boat lifecycle calls used the pinned official Boxd SDK and Boat HTTP API.
The second Boxd run additionally used **our production Rust token exchange, machine
inspection and command-execution adapter** on the exact isolated machine, before
and after wake. That verifies Rust execution encoding/decoding and fresh
authentication on both sides of resume. It does not claim that the Rust allocation
worker, prepared-image launch or credential broker was exercised.

Boat's cold-restore test preserved the home-directory marker but changed boot ID
and lost the RAM-only socket responder. Any Boat integration must handle runtime
process restart and reconnection on cold restore; recent-stop memory continuity
cannot be assumed indefinitely.

## Application verification and blockers

| Layer | Result |
| --- | --- |
| Live Boxd/Boat authentication and lifecycle | Pass |
| Production Rust Boxd inspect/exec before/after resume | Pass |
| Real Rust gateway + PostgreSQL + client/runtime WebSockets | Regression passes in an isolated localhost test database |
| Gateway authorization/fencing and multiplexing | Covered by that real-socket regression |
| Supabase transport | Live application roles verified over TLS; separate private environment schemas and roles provisioned |
| Production API `api-rust.zuse.sh/readyz` | HTTP 200 with public TLS, verified from owner Mac |
| Staging API `api-rust-stag.zuse.sh/readyz` | HTTP 200 with public TLS, verified from owner Mac |
| External application schema | Eleven checksummed migrations in each private Supabase environment schema; no customer data imported |
| API ticket/enrollment and gateway routing | Co-located in deployed Rust API on Hetzner; valid real-runtime/client pair acceptance remains pending |
| Rust Boat integration | Missing: only Boxd has a provider adapter and the allocation worker selects Boxd |
| Complete provider → runtime → relay → user chat flow | Not verified; blocked by the above deployment/integration gaps |

The localhost gateway test is a separate component regression, not a live
Hetzner/Supabase/provider end-to-end test. No test fixture schema was installed in
Supabase. No real model, GitHub credential or user chat was used.

## Safety and evidence

Four uniquely named isolated machines were created. All four were deleted and
absence confirmed. Existing user machines, DNS, account settings and the Supabase
schema/data were unchanged. The disposable gateway test database ran only in this
development workspace and was stopped after the regression; no Postgres server
ran on Hetzner. No warm pool or keepalive policy was introduced.

Receipts (gitignored):

- `.context/cloud-provider-verification-current.json`: Boxd and immediate Boat.
- `.context/cloud-provider-verification-boxd-rust.json`: Rust Boxd checks before/after wake.
- `.context/cloud-provider-verification-boat-cold.json`: forced Boat cold restore.
- `.context/cloud-endpoint-verification-current.json`: HTTPS readiness checks.
- `.context/cloud-current-gateway-regression.log`: passing PostgreSQL/WebSocket regression.
- `.context/deployment-tools/supabase-verification.json`: prior external DB verification.

Four harness safety tests pass, including cleanup and redaction if the optional
Rust child probe fails. The enhanced Rust example builds and its default mode
remains a non-mutating authentication/absent-identity lookup. With explicit
benchmark-machine ID and expected name, it checks identity and runs only a fixed
`printf` round trip, without creating, pausing, resuming or deleting machines.

## Next required work

The [Hetzner hosting rollout](hetzner-deployment.md) is live. Remaining work is the
real-agent flow: standalone model account/credential authority, GitHub scoped access,
attested Rust guest and harness base/provider builds, artifact/history configuration,
account/entitlement migration with encryption-key handling, and authenticated
message/reconnect/pause/resume acceptance. Boat also needs its Rust adapter and cold
runtime restart path. Creation remains disabled until those checks pass.

The [delivery checklist](cloud-computers-delivery.md) tracks agent authentication,
versioned environment setup and optional prepared artifacts. Consult its current
live evidence separately from these earlier provider timing measurements.

The current result is **provider readiness confirmed, full cloud launch pending**.
