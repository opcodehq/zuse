# Standalone GitHub migration parity checklist

Authoritative source: original implementation at `a0155d2dd11f07495a99d3737c9bd40a5c8b4508`. The reference checkout HEAD is different; every entry below was read with `git show a0155d2dd11f07495a99d3737c9bd40a5c8b4508:<path>`, not from its working tree.

Each unchecked case requires a corresponding Rust/shared-client regression test and the same behavior through the standalone authority. Existing upstream TypeScript tests, compilation and bridge responses are not evidence that the standalone port works. Live credentials are a separate deployment prerequisite.

## Current release evidence

- Implemented: the existing Worker bridge preserves upstream OAuth and installations; local install initiation permits organization members to connect their own account, while installation management remains administrative.
- Enabled: standalone native GitHub is enabled only for the explicitly verified staging owner canary. Other cohorts retain the bridge; production is unchanged.
- Live verified: the staging owner completed personal and opcodehq installation consent (130 visible repositories), authorized private opcodehq/zuse through the ordinary API, and used the same retained Boxd computer to fetch the private repository and push a unique temporary branch with the scoped user identity. The API created draft PR17; it was closed and the branch deleted. This does not certify the native Git panel/agent gh path, outside collaborators or WorkOS organization execution.
- Signing key: the owner transferred the staging key privately to `/etc/zuse/cloud/staging/github-app-staging.pem` (root, mode0600). Signed `/app` verified App5150694, slug `zuse-staging`; authenticated installation discovery returned three active installations. API-only signing/OAuth/webhook file copies are prepared with least-privilege mounts; the production native cohort remains off. The older supplied key failed the production App ID; the owner explicitly deferred production key work. Installation discovery does not prove member authorization, private Git delivery or parity.

## Native implementation evidence (current working tree)

An opt-in native authority now exists alongside the unchanged default bridge. Migration0015 uses separate native tables and leaves legacy Worker ciphertext untouched. It persists actor/scope/browser-bound OAuth operations, explicit installation choices, actor-owned encrypted user credentials, serialized/coalesced rotation with durable uncertainty, installation observation fencing, signed webhook deduplication/refetch, repository-scoped user tokens, generation/recipient-bound runtime Git grants and intended-identity PR creation. Native login never falls back to an installation administrator.

The first targeted native suite passed **5/5**, including one isolated localhost PostgreSQL/HTTP flow covering actual persisted OAuth, member existing-link restrictions, concurrent rotation once, rotation survival through repository outage, scoping and disconnect retry. Expanded webhook ordering/observation fencing, actual browser-consent routes, prepared-storage Git delivery and account-switch fencing now pass in the same native suite. Separate isolated organization permission/departure and personal-connection ownership fixtures passed. The detailed cases in this paragraph remain source/fixture evidence; the narrower staging live evidence is listed above.

Organization task admission pins the actor and idempotency; existing entitlement/capacity checks remain. Source now includes active-member checks, private-by-default sharing policy, gateway ticket/connection/revalidation guards and queued-author rechecks. GitHub-managed enrollment checks fail closed when native credentials are missing. The latest source adds separate organization joining authority: original-compatible organization locks, full WorkOS pagination, five-seat accounting including pending invitations/reservations, ten-minute durable reservations, safe managed-member reactivation, admin block/restore, fresh full GitHub roster authorization, stale-observation fencing and leased durable autojoin/departure jobs. Real isolated SQL/HTTP fixtures cover concurrency, uncertain WorkOS replies, identity uniqueness, admin/member rules, expired job leases and failure recovery. These do not replace live WorkOS/GitHub organization acceptance or prove all original cases below.

Shared native/web setup now observes the pending operation, opens the browser, refreshes bounded sign-in status, supports installation recovery/cancellation and manages native disconnect/revocation retry. Unknown GitHub identities must choose in the cookie-bound OAuth browser before an app bearer can finalize; app-side selection alone is rejected. A real-router form/CSP/origin regression passes. Verified-user chooser bypass, full install/approval handoff and exact transitional callback-routing cases remain unchecked. The guest private Git helper has focused fixtures and staging private fetch/push evidence; the API draft PR path was verified separately. The new per-Core managed gh launcher still requires a new guest/base and native/agent live acceptance. Task grants pin the immutable WorkOS actor, repository and GitHub user ID; switching GitHub accounts fails explicitly. Setup instructions are in `cloud-github-native-setup.md`. Production rollout remains disabled; the staging cohort is restricted to the verified owner.

## Focused multi-member acceptance

The isolated PostgreSQL/HTTP `standalone_oauth_refresh_scoping_revocation_and_callback_fences` regression now completes separate browser-bound OAuth exchanges for two organization members with distinct GitHub identities and credentials. Catalog and scoped-token HTTP requests use each actor's own token. Disconnecting the first actor clears only that actor's encrypted authorization; the second actor can still list and mint its own repository credential, and its token was never sent to revocation. The disconnected actor cannot borrow the remaining member's identity.

The same fixture links one installation to two enabled owners and one disabled control. A signed installation event refetches current GitHub state exactly once per enabled existing link; repeating the delivery does not repeat those calls. Each enabled owner's observation advances, the disabled owner's observation stays unchanged, and installation-link and enrollment counts stay unchanged. These are isolated authority regressions, not live multi-member WorkOS acceptance.

The same real PostgreSQL/HTTP fixture now primes a second authority instance's membership cache, completes an actual mailbox lease/Applied acknowledgment, queues another member-authored command, and opens member/admin WebSockets through the production gateway. A signed public-route departure webhook changes durable policy while the second authority's in-memory counter stays unchanged. The departed actor's old ticket and new WebSocket upgrade are denied; the existing socket closes with authorization code4102 through the gateway supervisor. Dispatch rejects the never-leased queued member command, while an administrator's command still leases and its socket responds to Ping. The earlier Applied receipt remains byte-for-byte unchanged. This closes the isolated departure/gateway/dispatch regression; live multi-member WorkOS acceptance remains separate.

Scoped installation unlink intentionally preserves the actor's global OAuth identity, including when its last link in the selected workspace is removed. This avoids breaking authorization in another scope. Explicit account disconnect performs global revocation. The pinned original single-scope final-link revocation test is therefore not marked equivalent; trustworthy all-scope absence detection is still needed before restoring that narrower automatic revocation behavior.

## Source regression cases

### `infra/api/test/unit/cloud-github-app.test.ts`

Source SHA-256: `cba3b00093c977db9a5e348e1054c84e78a77b738ae1f62fe6b14967c1abc426`.

- [ ] selects only an installation that grants the workspace repository
- [ ] normalizes GitHub's PKCS#1 download for jose
- [ ] forwards an exact staging issuer from the production callback
- [ ] never forwards unknown issuers or callbacks already on staging
- [ ] an installation ID and install state alone cannot link an unverified GitHub account
- [ ] links a just-installed organization when the GitHub user is already the actor's
- [ ] still asks an unknown GitHub user to choose, so a foreign link cannot attach their account
- [ ] resumes after approval on GitHub and then links without another choice

### `infra/api/test/unit/cloud-github-user.test.ts`

Source SHA-256: `2eae892ac83a28f23b23ff78466cc5047dfc643e866cd3079c54b5a50013bcb2`.

- [ ] parallel workspaces refresh once and keep the rotated token when repository access fails
- [ ] missing authorization returns no credential; expired authorization fails instead of falling back
- [ ] disconnect revokes the authorization and scoped tokens before deleting encrypted credentials
- [ ] disconnect refreshes expired access and preserves the rotation when revocation must retry
- [ ] concurrent installation disconnects revoke authorization when the final link is removed
- [x] organization members retain separate user tokens while sharing installation access
- [ ] account GitHub observations work without a runtime and recheck installation access before cache reuse

### `infra/api/test/unit/cloud-github-webhook.test.ts`

Source SHA-256: `10a3958b3e3a07cb88d9fe872310d592a87021111bc6ae8aaab39a85a45c2dfb`.

The native `standalone_oauth_refresh_scoping_revocation_and_callback_fences` PostgreSQL/HTTP regression covers signed organization events changing the matching policy revision in the same transaction as durable reconciliation scheduling. It rejects an earlier authorization snapshot, preserves off-cohort policies and enrollment counts, deduplicates one delivery, and invalidates again for a distinct delivery. This durable change is visible to other workers; the local membership counter alone is insufficient. It does not prove the remaining multi-member or live webhook cases below.

- [x] organization membership events invalidate durable eligibility without enrolling anyone
- [ ] bounds chunked bodies without trusting Content-Length
- [ ] rejects tampered signatures and malformed events before calling GitHub
- [ ] fails closed without a secret; accepts signed ping and ignores unrelated events
- [x] reconciles all linked workspaces, tolerates duplicates and never auto-enrolls
- [ ] out-of-order deleted payload does not delete a live installation; missed events reconcile on refresh
- [ ] stale concurrent refreshes cannot overwrite a newer observation or recreate removed links

### `infra/api/test/unit/github-organizations.test.ts`

Source SHA-256: `7b73702e9b99cf60af3b4faf38418d369c9d536fc35f65065d007ed04e7206fc`.

- [ ] auto-joins linked GitHub accounts listed in the roster, once, as Member
- [ ] does not auto-join from account linking or webhooks when rollout denies the team
- [ ] re-admits an auto-joined member who left GitHub and came back
- [ ] skips accounts without a linked GitHub account or outside the roster
- [ ] does not auto-join while auto-join is off
- [ ] joins linked roster members as soon as auto-join turns on
- [ ] reports the linked GitHub account
- [ ] paginates private membership and coalesces concurrent verification
- [ ] counts pending invitations and serializes concurrent joins at the final seat
- [ ] serializes an invitation against joining
- [ ] preserves durable enrollment when WorkOS accepts but its response is lost
- [ ] does not convert an existing manually admitted member
- [ ] stops new joins when disabled without revoking existing GitHub members
- [ ] revokes departures and supports rejoining when GitHub membership returns
- [ ] blocks reconnect access on suspension without deactivating membership
- [ ] denies disconnected installations even when a successful roster is cached
- [ ] expires successful checks and fails closed during an outage
- [ ] admin removal blocks rejoining until explicitly restored
- [ ] forbids role promotion for GitHub-managed memberships
- [ ] requires admin authority, connected ownership and Members permission for enabling
- [ ] binds single-use OAuth state to the account and browser
- [ ] prevents linking one GitHub identity to multiple Zuse accounts
- [ ] expires OAuth state without linking an identity
- [ ] auto-joins when GitHub is linked and lets an account switch GitHub accounts
- [x] rechecks gateway and queued-command authority after a GitHub departure

### `infra/api/test/unit/github-approval-handoff.test.ts`

Source SHA-256: `a6084c8cd9d44363f7de1e6f278e8cb62099d3bcca971adffd4747d4efba8350`.

- [ ] GitHub outages fail the approval check instead of appearing as a missing grant

### `infra/api/test/unit/github-callback-page.test.ts`

Source SHA-256: `c5bd93410911e6c1b2eb71bd4775a8beaa1ceb97a65b09345a4599819ae9b894`.

- [ ] renders a flat, self-contained connection page
- [ ] keeps the callback uncacheable and strips referrers
- [ ] approval polling can only connect to this API under the page CSP

### `infra/api/test/integration/github-member-scope.pg.test.ts`

Source SHA-256: `fc69c456bada09dd6e07985ed16ea500df361307379fdff193ebce232933a908`.

- [ ] Review and port the entrypoint/scenario definitions; no literal test title was extracted.

### `infra/api/test/integration/github-joining.pg.test.ts`

Source SHA-256: `91461418887080154465cb02389d00864129246ba543c9965e25d62ee118dcee`.

- [ ] Review and port the entrypoint/scenario definitions; no literal test title was extracted.

### `infra/api/test/integration/github-authorization.browser.ts`

Source SHA-256: `c779409c519f479ecca31e8f0502b0a8ff5c1863939350d206994d2fb5623f48`.

- [ ] Review and port the entrypoint/scenario definitions; no literal test title was extracted.

### `apps/server/test/unit/cloud-github-auth.test.ts`

Source SHA-256: `0d3ca72249df7b829e844124c4f1c5a570c9f7a050ab5e6b5131d9264175cd5d`.

- [ ] installation repairs multiple helpers and overrides inherited stale credentials

### `apps/server/test/unit/git-github-credentials.test.ts`

Source SHA-256: `59dee66a80f1132c63e901be8c9bb4b25f9ef6cd857554fb6af226976f18334b`.

- [ ] cloud API credentials retain the broker's immutable actor context and expiry
- [ ] cloud broker denial never falls back to a broader environment token
- [ ] an authenticated actor never falls back to desktop tokens before cloud setup

### `apps/server/test/unit/git-github-request-scope.test.ts`

Source SHA-256: `b707c53e59ad3a4b7692f6f2cd3b8fc4c2c3734bb165fb217c3f83774a66812c`.

- [ ] authenticated actors get independent immutable API and native Git scopes
- [ ] desktop requests retain desktop credential authority

### `apps/renderer/test/unit/connect-github.test.ts`

Source SHA-256: `d5de3909241f8caa360d8407603c81743ddad8fe526ab302017f83e7257d8d8a`.

- [ ] reserves the browser before requesting a workspace installation connection
- [ ] propagates connection failures so settings can show an error

### `apps/renderer/test/unit/cloud-workspace-github.test.tsx`

Source SHA-256: `81ba97c1574a553d63bfb62e0581eaf2adef8aea49f24d7242d03723114eaf1f`.

- [ ] uses the GitHub App as the only repository authentication setup
- [ ] shows linked personal or organization installations
- [ ] uses one GitHub connection for repository access and personal identity

## Parameterized source scenarios

- [ ] Expand and port every table row: **rejects retired and non-exact staging issuer %s** (`infra/api/test/unit/cloud-github-app.test.ts:110`).
- [ ] Expand and port every table row: **links to %s from GitHub %s without an install callback, only after explicit selection** (`infra/api/test/unit/cloud-github-app.test.ts:127`).
- [ ] Expand and port every table row: **handles installation status %s without hiding outages** (`infra/api/test/unit/cloud-github-app.test.ts:449`).
- [ ] Expand and port every table row: **authorization prepares account-bound encrypted credentials with commit name $commitName** (`infra/api/test/unit/cloud-github-user.test.ts:58`).
- [ ] Expand and port every table row: **a GitHub member connects an %s-existing installation after owner approval without a founder Zuse account** (`infra/api/test/unit/github-approval-handoff.test.ts:60`).
- [ ] Expand and port every table row: **does not grant member access for membership status $status, state $state and push $push** (`infra/api/test/unit/github-approval-handoff.test.ts:388`).
- [ ] Expand and port every table row: **an invalid broker refresh preserves a still-valid token (%j)** (`apps/server/test/unit/cloud-github-auth.test.ts:55`).

## Additional end-to-end gates

- [ ] Scope callback state to the initiating account, organization and browser transaction; verify production/staging routing.
- [ ] Private personal, organization and collaborator repository fetch with the selected user identity.
- [ ] A member can reuse an approved organization installation without taking the installing administrator’s identity.
- [ ] Revoked or expired authorization never falls back to another user or broader installation credentials.
- [ ] Selected repository changes, pagination, suspension, removal, transfers and missed/duplicate/out-of-order webhooks converge.
- [ ] Private membership and cached roster authorization expire safely and invalidate on disconnect.
- [ ] Concurrent refresh and disconnect preserve rotated credentials without resurrecting revoked access.
- [ ] Fetch, push and create a PR from a real task; confirm intended GitHub identity and least-privilege grants.
- [ ] Rehearse existing user/organization/seat migration and one-writer handoff before removing the bridge.

Inventory includes 62 literal source test cases. Parameterized and browser scenarios require manual expansion; this count is not a parity coverage claim.

## Latest source regression additions

Migration0018 adds coalesced durable GitHub joining jobs; verified webhook receipts and joining requests commit atomically. API owns the WorkOS/GitHub authority. The provider worker requests a separate ten-second signed allocation authorization bound to exact operation, task and generation before provider effects; it does not receive API OAuth/signing-file mounts. Tests reject using that proof as a gateway/client credential, another task or an expired proof; the full org SQL fixture denies departed-actor allocation authorization.

`.context/github-joining-parity-tests.log`: latest native9+worker-proof1+org-execution1 tests passed; native suite0.93s, scope proof0s, org fixture0.37s. `.context/github-joining-api-check.log`: API main metadata51.55s; `.context/github-joining-database-bootstrap-tests.log`: isolated bootstrap6tests2.34s through0018. These fixtures use simulated WorkOS/provider effects and do not claim live acceptance.

The subsequent explicit account-consent/pending-approval and verified-identity continuation source passed the actual-router native suite9/9 in1.04s (`.context/github-approval-continuation-tests.log`). A new identity can be saved with explicit browser consent independently of installation approval, then reopened against the verified authority identity without another OAuth/account choice. This is source/fixture evidence and remains behind disabled native rollout; live approval/continuation acceptance is still required.

### Scoped installation unlink and outside collaborators (source candidate)

Personal scope can link an installation visible to its own OAuth user even when
that user is an outside collaborator. Selection records only currently writable
repositories returned for that exact user/installation. It does not establish
organization membership, change enrollment, or borrow the installing admin's
identity. Organization scope retains its active-membership requirement. The pinned
TypeScript access helper also rejected outside collaborators; this is additional
coverage requested by the Cloud Computers plan, rather than a claimed literal
port of that rejection.

`POST /v1/cloud/github/remove` accepts `{installationId}` and requires scoped
installation administration. It removes the current scope's authorization,
increments its authorization epoch and cancels pending chooser transactions.
Grant minting rechecks the exact active link/subset and epoch after upstream I/O;
a pre-existing chooser cannot resurrect the removed link. Other scopes and the
actor's global OAuth identity remain intact. Explicit Disconnect still revokes
the global identity. Unlike the original single-scope final-installation removal,
this endpoint does **not** automatically disconnect the actor after their last
local link: the actor may serve separate organization scopes. No trusted global
absence proof is currently implemented, so the original final-link auto-revoke
case remains a documented semantic difference, not a passed parity claim.

The focused isolated PostgreSQL OAuth/scoping fixture passes, including a real
concurrent scoped-token mint blocked across unlink, shared identity preservation,
and rejected stale scope epochs. This still requires a subsequent staging
artifact; fixture success does not imply deployed or live-verified support.
