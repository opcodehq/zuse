# Owner staging GitHub canary

Run this after the reviewed API binary, migrations through0018, private API-only
GitHub files and verified staging identity configuration are deployed. Native
GitHub starts with a single verified owner account; organization and customer
compute rollout remain separate decisions.

## Identity lane

The original pinned deployment profiles use separate WorkOS environments:

| Environment | Account API | Public WorkOS client |
| --- | --- | --- |
| Production | `https://api.zuse.sh` | `client_01KWGQ818571ARFATQ3G9AR2Y2` |
| Staging | `https://api-staging.zuse.sh` | `client_01KW6ZEZKVMZ0G429A89XZD83Q` |

The initial Rust pilot deliberately copied the production identity client. The
owner's subsequently supplied **test** API key was privately verified against
both public clients using an invalid authorization code: production returned
`invalid_client`, staging returned `invalid_grant`. This proves the staging key
belongs with the original staging client. The previous pilot account's404 alone
was insufficient evidence. Existing pilot project and financial identities stay
unchanged; a newly verified staging account receives its own explicit allowance.

On the owner's Mac, run from the synced checkout using a separate profile:

```sh
ZUSE_API_URL=https://api-staging.zuse.sh \
ZUSE_WORKOS_CLIENT_ID=client_01KW6ZEZKVMZ0G429A89XZD83Q \
ZUSE_CLOUD_API_URL=https://api-rust-stag.zuse.sh \
python3 scripts/dev-native.py --data-dir .context/staging-cloud-canary
```

The account API handles account sign-in. The Rust cloud API handles cloud setup,
computers and history. The server's staging client and JWKS must match this lane.
Sign in normally. The operator must pin the actual verified staging JWT subject
before enabling native GitHub/auth/candidate-compute cohorts; an account ID copied
from an earlier environment is not proof of ownership. Tokens and API keys stay
inside the application/private server files.

## Lightweight owner verification without rebuilding the app

`infra/cloud/staging-owner-device.py` uses the existing public WorkOS device flow
from `crates/engine/src/account.rs`; it needs no WorkOS API key. Its profile must be
outside the synced checkout. The deployed staging Rust API verifies the resulting
JWT signature, issuer, client ID, expiry and subject before the helper reports a
verified owner. An unverified decoded JWT never enables a cohort.

The operator can install the helper on Hetzner and start a private grant:

```sh
# On Hetzner, after installing the reviewed Python file at this private path:
umask 077
python3 /etc/zuse/cloud/staging/owner-canary-helper.py \
  --state-dir /etc/zuse/cloud/staging/owner-canary start
```

Standard output contains only safe status. The owner runs this once in their Mac
terminal to open the approval page privately:

```sh
ssh root@157.180.87.181 \
  'cat /etc/zuse/cloud/staging/owner-canary/approval-url' \
  | python3 -c 'import sys,webbrowser; webbrowser.open(sys.stdin.read().strip())'
```

Then the operator runs `poll --once` repeatedly at the returned provider interval,
or `poll` as a private bounded process. `verify` retries only Rust API verification
if deployment was temporarily unavailable after browser approval; consumed device
credentials are already saved. `status` reveals the public owner ID only after that
verification. Refresh rotation is saved before dependent verification, and a lost
refresh response is fenced against replay. A new private login profile is required
after an uncertain refresh rather than guessing which token remains valid.

The helper stores grants/tokens in mode0600 files under a mode0700 directory,
removes the temporary approval URL after successful authentication, refuses
checkout-local profiles and never prints device codes, verification URLs, JWTs or
refresh tokens. Keep the session file private. A Mac-only alternative is the same
helper with an outside-checkout state directory and `start --open-browser`, then
`poll`; this also avoids a full native build.

## App and private configuration

Staging GitHub App:5150694, `zuse-staging`, client
`Iv23lim6UymqlW3Tw2HE`. Production GitHub App/key configuration stays unchanged.

| Setting | Reviewed staging value |
| --- | --- |
| Additional user authorization callback | `https://api-rust-stag.zuse.sh/v1/cloud/github/callback` |
| Installation Setup URL, after new route deployment | `https://api-rust-stag.zuse.sh/v1/cloud/github/setup` |
| Future native webhook URL | `https://api-rust-stag.zuse.sh/v1/cloud/github/webhook` |

Keep existing OAuth callbacks while the bridge is in use. The new Setup URL
forwards legacy signed-state-shaped callbacks only to the exact allowlisted
original production/staging authority. That authority still verifies its signature.
Native opaque state stays bound to its original browser cookie; an installation ID
alone never links an account or authorizes repository access.

Read-only audit found the current webhook still pointing to
`https://api-staging.zuse.sh/v1/cloud/github/callback`, with JSON, verified TLS,
no configured signing secret and subscribed event array`[organization]`.
Webhook migration is a separate reviewed staging action after consent testing;
existing delivery behavior remains with the bridge until then. The private native
webhook secret is already on Hetzner; its contents must not appear in chat, command
arguments, browser URLs or logs.

The operator checks that API-only file copies are UID10001 mode0400, mounted
individually read-only, while originals remain root0600. Only API receives OAuth,
signing and WorkOS API secrets. Lifecycle/credential workers receive no GitHub
signing-key mount. Verify the deployed image includes the consent, standalone
joining and worker-scope changes before activating the owner cohort.

## Human connection and acceptance

1. Open Cloud setup → GitHub in the isolated staging profile. Confirm the native
   connection card is served, open Connect, verify the displayed GitHub identity
   and workspace, and explicitly choose the intended installation in that browser.
2. Exercise account consent separately from App approval: choose **Connect account
   while waiting for App approval**, close the dialog/app and reopen Connect. The
   saved authority identity should open Zuse's installation page directly, without
   another OAuth/account choice. After approval, refresh and select the installation.
3. Cancel an unknown-account transaction and retry. A foreign browser cookie,
   foreign Origin, consumed callback code or changed installation choice must fail.
   Native API/account identity must remain unchanged by those attempts.
4. First use a small public test repository, then an explicitly selected private
   test repository. Choose the agent connection, provider, model and computer size
   from the selected provider's catalogue. Send a task and record durable admission,
   computer readiness, harness acceptance and first model token independently.
5. Receive a real streamed reply, run a tool/test command and edit a file. Use a
   terminal, browse files and open a preview. Fetch/push a dedicated test branch
   and create a draft PR; verify the selected user's intended GitHub identity.
6. Close the app while work continues, reopen it, interrupt the network and restart
   only the staging API/gateway. Verify stable history/scrolling and no duplicated
   command/tool execution. Repeated cached history/tool expansion should not wake
   the computer or refetch retained cached ranges.
7. Sleep and resume. Check branch, uncommitted files, installed software and declared
   services; retained processes are required only for a memory-certified provider.
   Resume performs no automatic fetch, rebase or branch rename.
8. Test expired/revoked credentials and disconnect/reconnect. Recovery errors must
   identify the missing authority or GitHub approval stage; another user's or
   administrator's connection must never be selected implicitly.
9. Delete isolated test computers and close/delete test PR branches/resources created
   specifically for acceptance. Record resource IDs privately and verify provider
   absence. Retain the owner's ordinary connections unless they were explicitly
   created as disposable acceptance records.

Organization acceptance follows in a separate isolated staging organization:
admin/member/billing/view permissions, organization approval, shared installations
with distinct member identities, invitation/seat races, removal/rejoin, roster
invalidation and delayed Resume/Restore after member departure. The new WorkOS key
enables these tests after staging identity alignment; fixtures are not live proof.

## Evidence and gates

Source fixtures passed native consent/refresh/permissions, original joining seat and
recovery rules, scoped provider effects, setup routing and actor-only membership
events. The whole cloud unit suite and provider/candidate/vault checks are tracked
in the delivery checkpoint. Live private fetch/push/PR, human device/setup-token
authentication and every provider/agent combination still require separate evidence.
Record measured timings, installed artifact hashes and cleanup receipts alongside
results. A successful health check, fake-provider fixture or candidate image does
not certify the complete customer flow.
