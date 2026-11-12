# Hetzner API, worker and WebSockets

The owner selected one Hetzner host for the Rust API, background worker and
WebSocket gateway. Postgres remains external on Supabase. Cloudflare manages DNS
and can proxy HTTPS; no Worker/Containers deployment is required.

```text
User app/browser ── HTTPS/WSS ── Caddy on Hetzner
                                  │
                         staging or production API
                         (includes WebSocket gateway)
                                  │
                    Supabase private environment schema
                                  ▲
                         environment background worker
                                  │
                              Boxd API

Sandbox runtime ── outbound WSS ── same environment API/gateway
```

Staging and production have separate Compose projects, networks, DB schemas/login
roles, encryption/signing keys and provider-secret files. The API and gateway
share one process per environment to reduce memory. Worker provider credentials
are not supplied to API containers. No Postgres server is installed on Hetzner.
The existing Zuse Node service and cloudflared tunnel remain separate.

The optional `authority` Compose profile adds one auth-worker per enabled environment
on the same host; it does not provision another server. It receives `runtime.env`
without infrastructure keys. `ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS` activates the profile
only for that environment. Apply the computers migration and use the tested
authority image before enabling it. The auth worker has its own CPU, memory, process
and temporary-storage limits. The deployment preflight uses `auth-worker --check`,
which validates tooling/schema without claiming a pending user login.

Keep `BOXD_API_KEY`, `BOAT_API_KEY`, `E2B_API_KEY`, `DAYTONA_API_KEY` and `PRIZED_API_KEY` exclusively in
the private worker configuration. A configured key alone does not certify a managed
base or a live agent. The current deployed lifecycle worker still selects Boxd;
provider-neutral adapters and orchestration are being integrated separately.

## Current deployment

- Host: `157.180.87.181`, approximately 2 GB RAM / 1 vCPU.
- Production: `https://api-rust.zuse.sh`, operator loopback port `18080`.
- Staging: `https://api-rust-stag.zuse.sh`, operator loopback port `18081`.
- Deployment path: `/opt/zuse/cloud/current/infra/cloud/single-host`.
- Config path: `/etc/zuse/cloud/{staging,production}`, directories mode 0700 and
  credential files mode 0600.
- Private Supabase schemas: `zuse_staging`, `zuse_production`.
- Restricted logins: `zuse_cloud_staging`, `zuse_cloud_production`.
- Public ports: Caddy 80/443. API operator ports bind only to host loopback.

The fresh DB bootstrap and restricted-role provisioning were applied on October
29, 2026. Each environment has eleven checksummed migrations and no customer/fixture
rows. Both roles were verified unable to access the other environment or update
migration ledgers. The Supabase administrator password stayed on Hetzner; deployed
services use only the restricted environment logins. Fresh environment keys do
not decrypt legacy records; old-data migration and key handling are separate work.

The current deployment is a test build of the native Rust executable with debug
symbols disabled, rather than a performance-qualified optimized production release.
The binary is built away from the small host, hashed, transferred as a whitelisted
artifact and installed using `infra/cloud/Dockerfile.prebuilt`. Each image tag names
its exact binary digest. Existing release directories/images are retained for
rollback. Build an optimized tested image before broad production rollout.

## Operate

```sh
cd /opt/zuse/cloud/current
python3 infra/cloud/single-host/deploy.py check staging
python3 infra/cloud/single-host/deploy.py up staging
python3 infra/cloud/single-host/deploy.py status staging
python3 infra/cloud/single-host/deploy.py logs staging

curl --fail http://127.0.0.1:18081/readyz
curl --fail https://api-rust-stag.zuse.sh/readyz
```

Use `production` for the other environment. `up` first runs `worker --check` to validate configuration and the allocation/lifecycle
schema without claiming jobs or contacting providers, then starts API and worker,
waiting for API readiness. At least one of Boxd, Boat, E2B or Daytona must be configured.
The running worker processes queued jobs after startup; the preflight does not. `check` only
validates Compose configuration without printing expanded secrets. `stop staging`
stops only that project's containers. Routine API/worker updates do not restart
Caddy or the other project.

The shared edge is managed with `check edge`, `up edge`, `status edge` and
`logs edge`. Restarting it interrupts both environments' connections. Docker
restarts API/worker/Caddy after a host reboot. The host is one shared failure domain.

## Memory and polling

Configured limits:

| Service | Production | Staging |
| --- | ---: | ---: |
| API and gateway | 384 MiB | 256 MiB |
| Background worker | 192 MiB | 128 MiB |
| Shared Caddy | 128 MiB total | shared |

The first staging idle sample used approximately 3.3 MiB for API and 3.1 MiB for
worker, with no workload and no active WebSockets. These are idle observations,
not load/capacity guarantees. Check memory/restarts under real traffic before
relying on the small host for both environments. Keep builds off this server.

`ZUSE_CLOUD_WORKER_POLL_MS=1000` is in `worker.env`; the CLI accepts 250–60,000 ms.
The old loop slept 250 ms. This reduces idle scan frequency but remains polling;
no database NOTIFY dependency was introduced. An event-driven wakeup path and
further maintenance-scan separation remain future work.

## DNS and TLS

Use DNS-only A records to `157.180.87.181` for the two API hostnames while obtaining
Caddy certificates. The names also serve WSS; extra WebSocket hostnames are not
needed. Remove conflicting records for those exact names. Cloudflare proxying can
be enabled later with Full (strict) TLS. API/relay responses are uncached.

Caddy validates the host and obtains normal public certificates. A DNS resolver
can retain the previous records for their TTL after an authoritative update.
A `curl --resolve HOST:443:157.180.87.181 https://HOST/readyz` check bypasses only DNS;
it still validates the certificate and hostname. Do not disable TLS verification.

## External DB provisioning and fresh keys

For a fresh host only, initialize config with explicit API domains and immutable
image tag. This refuses to overwrite existing files:

```sh
python3 infra/cloud/single-host/deploy.py init \
  --production-domain api-rust.zuse.sh \
  --staging-domain api-rust-stag.zuse.sh \
  --image zuse-cloud:EXACT_BINARY_DIGEST
```

Then use the root-only administrator password file to bootstrap and provision:

```sh
python3 infra/cloud/single-host/provision.py \
  --admin-password-file /root/.zuse-postgres-password \
  --database-host db.ifjixphdqgrlnfsuisug.supabase.co \
  --database-ca-file /etc/zuse/cloud/certs/supabase-ca.crt
```

This command never exports the admin password. It creates separate application
password files, verifies cross-schema isolation, gives app roles business-table
DML and sequence privileges, denies schema creation/ledger writes, and stores
fresh independent environment keys. It preserves existing configured credentials.
Supply the real deployment's public WorkOS client ID and browser CORS origins via
its options. The existing public client ID is the default for this workspace.

The Supabase direct endpoint is IPv6-only. The Compose networks enable IPv6;
verified container connectivity was established. Compose mounts the Supabase CA
read-only at `/etc/zuse/cloud/database-ca.crt`, and URLs require `verify-full`.
Do not replace this with the administrator login or disable certificate checks.
See [database bootstrap](../DATABASE.md) for the underlying migration tool.

## What this enables and what remains

This deploys the infrastructure and current Rust API/relay/lifecycle code. Workspace
creation is disabled by the empty `ZUSE_CLOUD_CREATE_ACCOUNTS` allowlist until a real
account's billing, repositories, credentials and prepared image pass acceptance.
It is not a claim that a real Claude/Codex cloud conversation is ready.

Remaining flow work includes standalone model account onboarding/credential
authority, GitHub app/scoped credentials, an attested Rust guest/harness base and
provider preparation, account data/entitlement migration, transcript/object storage
configuration, and real message/reconnect/pause/resume testing. Boat still lacks a
full Rust provider adapter; its benchmark support does not make it deployable here.
Image and billing workers are not enabled by the default Compose package.

For native testing after TLS verification, the current engine supports
`ZUSE_CLOUD_API_URL=https://api-rust-stag.zuse.sh`. The default application origin
remains the old API until deliberately switched. Do not route existing users to an
empty environment and call that a data migration.

## Verify an additional GitHub App signing key

Keep the existing production key active. Transfer the additional downloaded PEM
privately to an operator-owned file under the staging configuration directory and
set mode0600. Do not paste it into chat, command arguments, logs or source files.
Before configuring an App ID, run this read-only preflight on the server:

```sh
python3 infra/cloud/github-app-check.py \
  --key-file /etc/zuse/cloud/staging/github-app-import.pem \
  --app-id APP_ID
```

The preflight signs a short-lived JWT locally and calls GitHub's fixed `/app`
endpoint with redirects disabled. Successful output contains only the public App
ID and slug. It never changes an installation or key and does not prove private
repository access, user authorization or a completed standalone GitHub migration.
A supplied file name is not evidence of which App owns the key. Confirm the
returned identity before using it, then configure only staging until its complete
account/organization acceptance cases pass.
