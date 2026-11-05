# Cloud parity inventory

Architecture update (2026-10-29): the owner selected a standalone Rust database with
full retained-data migration. Shared-schema requirements below describe the frozen
reference and earlier rollout approach. Follow the current
[cloud performance decisions](cloud-performance-architecture.md), including no warm
pools and unchanged-checkout resume, for new implementation work.

Frozen reference: `swarajbachu/zuse@2016cf4f0cc240e6797916dd5cab1d8bebab89ae` (October 22, 2026).
Inspect with `git -C .context/zuse-ref show 2016cf4:<path>`; the checkout's HEAD may be older.
This is a source inventory, not evidence that a service is deployed or credentials are available.
Checklist entries describe upstream behavior to verify in the native app; unchecked means parity is unverified.

## Ownership and implementation boundary

- [ ] Rewrite the Zuse-owned control plane in Rust at `api-rust.zuse.sh`, retaining existing Postgres/R2 records, WorkOS identity, billing configuration and provider accounts.
- [ ] Replace the guest with the existing Rust engine, preserving writable conversations, command receipts, repositories, agents and PTYs through an explicit storage migration.
- [ ] Implement native UI and cloud transport/projection adapters against the frozen contracts.
- [ ] Keep the HTTP account control plane separate from live gateway RPC, durable mailbox commands and encrypted read-only transcript projections.
- [ ] Retain one supervisor per environment for chat, shell, files, Git, reviews and terminal resources.
- [ ] Negotiate protocol/capabilities before opening tools; preserve cached state on transport failure.
- Existing Rust `crates/rpc` uses its own `{id, method, params}` protocol; `managed.rs` connects `/api/account-rpc`.
- Upstream uses Effect RPC and `zuse-workspace-v3` gateway negotiation. Existing Rust account sockets are not proof of cloud guest compatibility.
- Existing JavaScript runtimes remain authoritative until individually migrated. Their enrollment, receipts, archives and update contracts are compatibility requirements for the Rust replacement.
- The approved scope includes the Rust guest, compatible protocol/storage migration and new images, alongside native UI parity.

Source: [architecture](https://github.com/swarajbachu/zuse/blob/2016cf4f0cc240e6797916dd5cab1d8bebab89ae/internal-docs/cloud/architecture.md), `packages/client-runtime/src/environment-runtime.ts`, `packages/contracts/src/workspace-gateway.ts`.

## Product and UI checklist

- [ ] Cloud onboarding: account sign-in, subscription/access, GitHub, repositories and agent authentication.
- [ ] Hosting options: managed subscription, connected provider key, custom snapshot; clear stale snapshot status when switching.
- [ ] Run on picker: local/remote/cloud destination, provider, repository, branch, agent/model and machine size.
- [ ] First prompt creates the workspace and durable launch intent; display startup phases without blocking draft entry.
- [ ] Chat works after the client closes; accepted work belongs to the hosted runtime.
- [ ] Composer trays: connection/resume/update/error/memory notices, agent sign-in, queued prompts, background tasks, questions and private answers.
- [ ] Recover failed first-message attachments in the queue; retain command IDs and attachment ownership on retry.
- [ ] Cached chat opens immediately; checkpoint catch-up and live attachment do not erase valid history or wake paused compute merely to read.
- [ ] Lifecycle: create/connect/watch, pause/resume, restart/update, archive/unarchive and explicit permanent deletion.
- [ ] Fork in new tab shares machine files; fork in new machine copies state and selected history without replaying parent queued work.
- [ ] Top bar: cloud/provider identity, repository/branch, environment menu, preview ports, SSH/open targets and sync status.
- [ ] Environment summary: actual CPU/memory/runtime state, restart action and accurate connectivity presentation.
- [ ] Files, attachments, Git/worktrees, changes/reviews and terminal operate on the selected cloud environment.
- [ ] Cloud terminal executes remotely; local terminal executes in the synced local checkout.
- [ ] SSH preparation, command copy, editor launch and private preview forwarding use authenticated workspace access.
- [ ] Public preview creation/copy/revocation remains an explicit separate action.
- [ ] Sync toggle, local path, progress, retry/error states and archive cleanup match the remote-authoritative mirror.
- [ ] Images: provider selection, readiness, repositories, build/rebuild, build progress/logs/history and retained-image deletion.
- [ ] Custom snapshots: import ID/user/repository paths, native versus account agent/Git auth and readiness/error recovery.
- [ ] Cloud settings: setup/repositories/agents/images/usage/API keys, provider keys, scripts/environment configuration and scoped settings revisions.
- [ ] Subscription management: checkout, status, portal, invoice/payment methods, period/allowance, estimate, cap, holds, usage history and credits.
- [ ] Persistent machines: offers/create/checkout, status/cancel/recover/destroy, SSH keys/modes, private network and runtime updates/resources.
- [ ] Organizations: scope switch, independent configuration/billing, members/roles/invites, sharing policy and defaults.
- [ ] Cloud device selector: optional computer command access, independent enrollment/disable/revoke, approvals, disconnected status and explicit target binding.
- [ ] API keys and automation: create/list/revoke, public workspace/message/attachment API, result polling and signed webhooks; Slack connection/alerts.

UI entrypoints: `apps/renderer/src/components/{chat-composer,top-bar,cloud-workspace-info,cloud-connection-notice,cloud-device-selector,cloud-chat-sharing-dialog}.tsx`.
Settings: `components/settings/{cloud-workspace-pool,cloud-hosting-settings,cloud-image-build-history,cloud-snapshot-settings,cloud-machines-pane,cloud-device-access-card}.tsx`.
Latest reference includes device access split, stopped-runtime recovery, stale hosting fixes, Zots coordination, background tasks and question/private-input composer changes.

Source: [renderer components](https://github.com/swarajbachu/zuse/tree/2016cf4f0cc240e6797916dd5cab1d8bebab89ae/apps/renderer/src/components), [user guide](https://github.com/swarajbachu/zuse/blob/2016cf4f0cc240e6797916dd5cab1d8bebab89ae/internal-docs/cloud/user-guide.md).

## HTTP contracts and background execution

Paths below are endpoint families, not a claim that every path accepts every HTTP verb. Use `packages/contracts/src/api.ts` and route handlers for exact methods/payloads.

| Family | Frozen interface and owner |
| --- | --- |
| Auth | WorkOS token exchange/refresh; `infra/api/src/workos.ts`, `auth.ts`; native token persistence/refresh remains platform-owned |
| Billing | `/v1/billing/{checkout,entitlements,portal,prepaid,prepaid/checkout}` and provider webhook; `billing-operations.ts` |
| Cloud billing | `/v1/cloud/billing/{summary,usage,cap}`; `cloud-billing-routes.ts`, `cloud-billing.ts` |
| Providers | `/v1/cloud/providers`, `/provider-connections`; provider connections list/save/disconnect |
| Agent auth | `/v1/cloud/auth`, `/provision`, `/configure`, `/login/start`, `/login/:id`, `/login/:id/cancel`, `/providers/:provider` |
| GitHub | `/v1/cloud/github`, `/install`, `/callback`, `/webhook`, `/observe`, `/installations/:id` |
| Projects/images | `/v1/cloud/projects`, `/:id`, `/:id/prepare`; `/v1/cloud/image`, `/build`, `/delete`, `/snapshot` |
| Workspace lifecycle | `/v1/cloud/workspaces`, `/fork`, `/:id`, `/:id/:action`; typed actions in `cloud-workspaces.ts` |
| Sharing/settings | `/v1/cloud/settings`, `/sharing-defaults`, `/workspaces/:id/sharing` |
| Live tools | `/:id/gateway/ticket`, `/:id/gateway`, `/:id/ssh-access`, `/:id/preview-url` |
| Durable commands | `/:id/commands`, `/commands/:commandId`, `/commands/watch`, `/data-key`; encrypted mailbox envelopes/receipts |
| Catalog/history | `/v1/cloud/chats`, `/chats/changes`; workspace session transcript-checkpoint and transcript-message-page reads |
| Runtime enrollment | Workspace `/runtime/bootstrap`, `/bootstrap/ack`, `/credentials/renew`; boot/runtime credentials are distinct from account tokens |
| Runtime delivery | `/runtime/commands`, `/commands/lease`, `/commands/ack`, `/turn-events`, `/attachments/:assetId`, `/activity`, `/summary`, `/access` |
| Runtime credentials | `/runtime/github-credential`, `/runtime/providers/:provider/grant` and Codex grant compatibility route |
| Runtime projections | `/runtime/transcript-checkpoint`, `/runtime/transcript-message-page` |
| Computer bridge | Independent account discovery/enrollment/socket/revocation; workspace `/device-bridge` and `/runtime/device-bridge` |
| Persistent machines | `/v1/machine-offers`, `/v1/machines`, `/:id/{cancel,recover,destroy,boot-status}`, `/enroll`; machine RPC adds keys/network/runtime/resource tools |
| Public automation | `/v1/cloud/api-keys`; `/v1/api/projects`, `/workspaces`, `/:id/messages`, `/:id/attachments`, `/webhooks` |
| Organizations | `/v1/organizations/**`; scoped supported cloud routes use `/v1/organization-workspaces/:id` and workspace header |

- [ ] Minute cron runs machine reconciliation, cloud lifecycle reconciliation, mailbox lifecycle outbox drain, billing maintenance, webhook/command sweeps and provider usage polling.
- [ ] WorkspaceStartup Durable Object resumes bounded startup reconciliation independently of viewer presence.
- [ ] WorkspaceMailbox owns opaque encrypted command ordering/leases/wake coordination and encrypted terminal receipts.
- [ ] WorkspaceGateway owns authenticated opaque live routing, generation checks and connection availability, not chat persistence.
- [ ] CloudAuthExecutor serializes account credential operations; account authority owns reusable provider auth and refresh.
- [ ] DeviceBridgeGateway owns independent enrolled-device sockets/secret hashes; PluginVault owns plugin capability secrets.
- [ ] Slack uses the configured queue; financial/usage export uses durable outbox delivery and reconciliation.
- [ ] Runtime writer fencing, monotonic generations/cursors, idempotent domain commits and bounded retries prevent duplicate turns/stale updates.

Source: [API paths](https://github.com/swarajbachu/zuse/blob/2016cf4f0cc240e6797916dd5cab1d8bebab89ae/packages/contracts/src/api.ts), `cloud-workspaces.ts`, `cloud-auth.ts`, `cloud-billing.ts`, `machines.ts`, [Worker jobs](https://github.com/swarajbachu/zuse/blob/2016cf4f0cc240e6797916dd5cab1d8bebab89ae/infra/api/src/worker.ts#L775).

## Identity, Postgres mapping and storage

- WorkOS JWT verification uses remote JWKS, configured issuer rules and expected client ID; principal `accountId = sub`, `orgId = org_id` or `organization_id`.
- Personal records use the authenticated account owner; organization records use `organization:<id>`. Scope header is `x-zuse-workspace: personal|organization:<id>`.
- API rechecks WorkOS membership and supported scoped-route access; malformed/conflicting scopes fail closed. Admins manage configuration/billing; billing-only members cannot read content.
- Sharing tickets bind actor/permission; runtime and gateway enforce View/Edit plus current membership/policy. A link does not itself grant access.
- `api_entitlements`: entitlement ID, account ID, kind/offer/provider/subscription, status, period/paid-through/end/credit and timestamps.
- Entitlement kinds: `persistent-machine`, `cloud-workspace`, `usage-credits`; statuses: `pending`, `active`, `grace`, `ended`.
- Cloud access accepts active/grace or ended with remaining paid-through access; an expired paid-through denies access. Do not infer entitlement solely from a cached UI flag.
- `api_cloud_projects`: account/repository identity/URL/name/default branch/visibility/Git connection, environment JSON, secret bindings, configuration digest, inclusion/state/idempotency.
- `api_cloud_project_builds`: account/project/provider preparation/build artifacts and state. Account images are provider-specific, not universal runtime images.
- `api_cloud_workspaces`: workspace/account/project/build/provider/sandbox, chat/initial-session/branch/base-ref, lifecycle/desired/runtime state, status, credentials, wrapped transcript key and request config.
- Workspace rows retain archive timestamps/deadline, deletion tombstone expiry, reconciliation lease/revision/next action, activity/running/delete timestamps and idempotency.
- Workspace state: queued/provisioning/setup/ready/pausing/paused/resuming/archiving/archived/deleting/deleted/failed; desired state: ready/paused/archived/deleted; runtime state: offline/connecting/online.
- Uniqueness binds account/idempotency, provider/sandbox and chat; live project/branch uniqueness excludes deleted rows.
- `api_cloud_workspace_command_receipts`: workspace+command primary key, action and workspace revision.
- `api_cloud_workspace_launch_intents`: workspace/account/chat/session/turn/unique command ID, ciphertext and expiry; temporary sealed initial command, not canonical transcript.
- `api_cloud_workspace_runtime_summaries`: workspace, runtime generation, summary revision, active session/head version, title/activity and native agent access; metadata only.
- `api_cloud_transcript_checkpoints`: workspace+session primary key, runtime generation, stream epoch/version, R2 key, ciphertext hash/size and timestamp.
- `api_cloud_auth_authorities`: account primary key, provider/sandbox/storage incarnation/auth epoch/toolchain/state, lease/revision/status; reusable credentials belong to authority, not per-chat copies.
- Additional identity/catalog tables: environments, environment credentials, devices, DPoP/revocation, GitHub users/installations/identities/org membership, catalog heads/changes, workspace settings and model connections/leases.
- Billing tables: periods, provider price schedules/evidence/delivery/finalization, usage, reservations, immutable ledger, cap commands, outbox, reconciliation attempts/results, platform/statement costs and billing events.
- Stripe tables: customers, meter deliveries, subscription migrations and recovery migrations; retain original period/subscription provider ownership.
- Automation tables: hashed API keys, webhook endpoints/deliveries, sealed API message rows and compact turn receipts; these are not runtime transcript authority.
- Snapshots/provider connections: `api_cloud_snapshots`, snapshot leases/price schedules and encrypted provider connections. Run existing migrations; do not create an unrelated native cloud schema.

Source: [Postgres schema](https://github.com/swarajbachu/zuse/blob/2016cf4f0cc240e6797916dd5cab1d8bebab89ae/infra/api/drizzle/schema.ts), `infra/api/drizzle/migrations`, `workos.ts:230–290`, `cloud-entitlement.ts`, [organization rules](https://github.com/swarajbachu/zuse/blob/2016cf4f0cc240e6797916dd5cab1d8bebab89ae/internal-docs/cloud/organization-workspaces.md).

### Transcript, archive and local mirror formats

- Runtime SQLite is writable authority; R2 checkpoints/pages are encrypted read-only projections; native cache is a disposable qualified projection.
- Transcript envelope is JSON `{version:1, compression:"gzip", iv, ciphertext}`; base64url values, 32-byte AES-GCM key and 12-byte IV.
- Transcript AAD binds workspace/session/epoch/stream version/schema version and head versus page boundary; validate hash, identity and cursor before applying.
- Archives pause the existing machine for a ten-day reopen window; unarchive returns to Active without waking compute.
- Full recovery is separately gated by `CLOUD_WORKSPACE_RECOVERY_ENABLED`, default off; transcript checkpoints alone cannot justify deleting the machine.
- Recovery metadata lives at workspace `requestConfig.recoveryArchive`: version/id/sourceSandboxId/archive+expiry times/phase/verifiedChunks/optional manifest.
- Recovery phases: uploading, verifying, retiring, retired, restoring, restored. Persist verification barrier before source-machine retirement.
- Manifest v1 contains operation/workspace/chat/session IDs, chunk SHA-256 array (1–8192) and whole-archive SHA-256.
- Recovery captures original data directory, `/home/repos`, Claude and Codex native state, SQLite/WAL; integrity and expected identities must pass.
- Compressed archive chunks are independently AES-256-GCM encrypted with operation/order binding, 4 MiB before encryption/base64, immutable content-addressed storage; manifest uploaded last.
- Restore stages/validates before installation and preserves original identity; backup failure retains source; restore failure retains archive/partial target.
- Explicit deletion purges recovery data and emits a thirty-day tombstone; archive expiry does not logically delete history.
- Local mirror selects Git tracked and nonignored untracked content, respects nested ignore rules/submodules, verifies staged downloads and publishes per-file renames.
- `.zuse-sync.json` records workspace and owned/pending paths; sibling `.zuse-sync-cache` stores verified objects/journal. Only owned paths are eligible for removal.
- Initial sync starts immediately, subsequent scans thirty seconds after completion; hints respect fifteen-second minimum. Disable/disconnect retains files; archive cleanup preserves local changes/extras.

Source: `packages/utils/src/cloud-transcript-crypto.ts`, [archive schema](https://github.com/swarajbachu/zuse/blob/2016cf4f0cc240e6797916dd5cab1d8bebab89ae/infra/api/src/cloud-workspace-archive-state.ts), [archive recovery](https://github.com/swarajbachu/zuse/blob/2016cf4f0cc240e6797916dd5cab1d8bebab89ae/internal-docs/cloud/workspace-archive-recovery.md), `internal-docs/cloud/file-sync.md`.

## Service prerequisites, native status and acceptance

- Services: WorkOS AuthKit/JWKS/API, Cloudflare Worker/Durable Objects/R2/Hyperdrive, Postgres, configured sandbox providers, GitHub app/user auth and Stripe (Polar compatibility for existing subscribers).
- Required bindings include `CLOUD_TRANSCRIPTS`, `HYPERDRIVE`, `WORKSPACE_GATEWAY`, `WORKSPACE_MAILBOX`, `WORKSPACE_STARTUP`, `CLOUD_AUTH_EXECUTOR`, `DEVICE_BRIDGE_GATEWAY`, `PLUGIN_VAULT`; Slack queue when enabled.
- Guest requires exact compatible signed runtime artifact/template, runtime/provider credentials and Git/toolchain; recovery additionally needs Node `node:sqlite`, Python 3 and temporary disk.
- Secrets remain in Worker configuration/account authority/native credential vault; client never receives Stripe, WorkOS or provider administrative secrets.
- Billing reference: $40 monthly base, $35 included attributable provider cost, 5% markup above allowance, default $25 pre-tax cap; server ledger determines charges/admission.
- Stripe checkout/portal/webhooks, tax configuration, financial/telemetry exports and enforcement have separate configuration/rollout gates; source config is not live payment verification.
- Native baseline has WorkOS session/refresh, organizations, managed remote connection machinery, environment-aware host RPC and local/native engine tools.
- At inventory time no dedicated native cloud crate/control adapter was found; complete hosted billing/images/lifecycle/mailbox/checkpoints/guest wire compatibility and cloud UI parity are unverified.
- Re-evaluate this status against implementation diffs; local auth/remote capability alone does not satisfy hosted cloud acceptance.
- Acceptance must exercise actual subscription checkout/webhook/portal and ledger usage, image rebuild/logs, create/send/reopen, paused read versus wake, tool routing, terminal/SSH/preview and verified local sync.
- Verify organization scope/View versus Edit, optional device access, archive/unarchive/delete and offline/reconnect receipt deduplication with real service fixtures.
- Measure matched provider/repository/runtime conditions: initial prompt accepted, runtime ready, command delivered, first token, warm reopen and long-history catch-up.
- Upstream sub-five-second Boxd preparation is a target, not a universal end-to-end first-token result; compare identical milestones and report evidence.

Source: [production configuration](https://github.com/swarajbachu/zuse/blob/2016cf4f0cc240e6797916dd5cab1d8bebab89ae/infra/api/wrangler.production.jsonc), [Stripe operations](https://github.com/swarajbachu/zuse/blob/2016cf4f0cc240e6797916dd5cab1d8bebab89ae/internal-docs/cloud/stripe-billing.md), `internal-docs/cloud/resume-latency.md`; native `crates/{client,engine}/src/auth.rs`, `crates/rpc/src/{lib,managed}.rs`.

## Simulator host capability

Simulator contracts/RPCs and MCP/native application tools now route to the chat's
execution host. Native panes can view a remote host through peer RPC. Linux hosts
report iOS as unavailable and probe Android SDK tools without installing them.
The managed cloud WebSocket path's smaller message cap is not hardware-verified;
use peer routing for large PNG frames. See [device-simulators.md](device-simulators.md)
for scope and remaining acceptance.

Simulator helper setup runs on the execution host through the same addressed RPC:
pinned npm installs are profile-scoped, shared across that host's enabled chats,
and supervised by its engine. Progress is returned in `State.setup`; configure
returns promptly while the host job runs. Remote host installation, native-device
operation, and long/slow cold boots still need the hardware acceptance checklist.
