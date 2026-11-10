# Hetzner deployment verification

Deployed October 29, 2026 with the owner's authorization. The Rust API, gateway and
background worker run on the existing Hetzner server; Postgres stays on Supabase.
This completes the hosting/infrastructure rollout, not real-agent cloud acceptance.

## Verified live

| Check | Result |
| --- | --- |
| Production `https://api-rust.zuse.sh/readyz` | HTTP 200, `ready`, valid public TLS |
| Staging `https://api-rust-stag.zuse.sh/readyz` | HTTP 200, `ready`, valid public TLS |
| Requests from owner's Mac using normal DNS | Both reach `157.180.87.181`, HTTP 200, TLS verification successful |
| Authoritative DNS / public Cloudflare and Google resolvers | Both A records point to Hetzner |
| API containers | Healthy, zero restarts, no OOM kills |
| Worker containers | Running, zero restarts; one-shot reconciliation passed before startup |
| Unauthenticated workspace listing | HTTP 401 in both environments |
| Supabase transport for application roles | Verified TLS |
| Environment isolation | Distinct private schemas/logins and independent signing/encryption keys |
| Restricted roles | Cannot use the other environment schema or update migration ledgers |
| Fresh database contents | Eleven migrations per schema; zero workspace/customer fixture rows imported |
| Existing Zuse Node process and cloudflared tunnel | Original processes remain alive |

The cloud development workspace's recursive resolver temporarily retained the old
addresses and encountered their old expired certificate. Direct DNS-overrides to
Hetzner still verified the public certificate, and the owner's Mac plus external
resolvers verified the normal public route. TLS verification was never disabled.

Release: `workspace-49dc17109162`.
Binary SHA-256: `49dc1710916229369d748a3b105c5bfcc58643103b40d3335328994d997dfc05`.
API image ID: `sha256:f2caafec831865e1cf7ec43917b58e73a6d5b573ea72ae367cd3081a3cf09f9e`.
This is a native test build with debug symbols disabled, not an optimized,
load-qualified production release. The previous image/release remains available.

Five containers run: API plus worker for each environment, and shared Caddy.
The observed combined idle working-set memory was approximately 27 MiB; the host
still had over 1 GiB available. No workload was running and no real client/runtime
WebSocket pair was connected. These samples do not establish capacity.

Worker idle polling is now configurable from 250 to 60,000 ms and defaults to
1,000 ms. Both deployed workers use 1,000 ms. This reduces scan frequency from the
previous 250 ms loop; durable event-driven wakeups remain unimplemented.

## Test now

Open either readiness link above to verify hosting. For native API testing, the
engine supports `ZUSE_CLOUD_API_URL=https://api-rust-stag.zuse.sh`; the application
continues using its original API by default. Authentication and real user actions
still need their own acceptance checks. The new database is empty.

Do not call this a working Create Workspace → agent reply flow yet. Workspace
creation remains disabled by the empty account allowlist. Required next work:

1. Finish standalone model-account/device-auth onboarding and credential authority.
   The Rust grant consumer works with synthetic tests; it is not an onboarded live
   Claude/Codex account. Do not assume the old authority can validate newly created
   records in a different database.
2. Configure GitHub app/scoped repository access and the project setup/build path.
3. Build/attest/distribute the actual Rust guest and tested harness base to Boxd,
   then enable the appropriate image-preparation worker. Release-management tooling
   alone does not produce a ready provider snapshot.
4. Configure the current transcript/artifact storage path, or finish the planned
   Postgres hot-history replacement. No R2 credentials were configured by this deploy.
5. Migrate the intended account/entitlement/data records with the original encryption
   keys or a reviewed re-encryption process. No legacy rows were copied implicitly.
6. Test a real message through the user app, provider runtime and public gateway,
   then reconnect, pause/resume, cold recovery and measure agent acknowledgement.
7. Add the full Rust Boat adapter before advertising Boat workspace creation; the
   current deployed allocation worker uses Boxd. Boat Stop diagnostics exist in the
   measurement harness, not as a deployed provider integration.

[Operations and rollback](../infra/cloud/single-host/README.md).
Gitignored evidence: `.context/hetzner-deployment-verification.json`,
`.context/deployment-tools/hetzner-release.json`, `.context/hetzner-api-build-final.log`.
Six real PostgreSQL bootstrap/role tests and five deployment/Compose tests passed;
invalid worker polling bounds were rejected by the compiled CLI. No real model turn,
GitHub token delivery or provider image build was part of these deployment checks.
