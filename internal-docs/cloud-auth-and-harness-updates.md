# Browser sign-in and managed harness updates

Reviewed October 29, 2026 using official OpenAI and Anthropic documentation.
This records the target policy and implementation status. Legacy device-auth
credential delivery and offline harness release management are now implemented;
no OAuth client was registered, customer login performed, or fleet release deployed.

## Authentication: separate identity, model access and token ownership

Browser sign-in can replace device-code entry in the user experience. Both are
initial authorization mechanisms and should stay outside normal workspace
creation/resume. Browser sign-in does not make access tokens permanent, and
storing a credential record in Postgres does not refresh it automatically.

OpenAI documents three distinct cases for app-server: API-key authentication,
Codex-managed browser/device authentication with automatic refresh, and external
tokens whose lifecycle belongs to the embedding application. Current guidance
directs commercial/hosted services to Sign in with ChatGPT rather than the CLI's
app-server login mechanism. The old repository's broker is migration/source
evidence, not proof that it is an appropriate hosted authentication product path.
[Codex app-server authentication](https://learn.chatgpt.com/docs/app-server#auth-endpoints)

The website SIWC guide currently describes a selected-commercial-partner trial
and requires an issued OAuth client and registered callbacks. Identity-only
sign-in establishes a Zuse account/session but does not, by itself, authorize
inference against the user's ChatGPT plan. Confirm the approved hosted plan-use
integration before implementing subscription-backed cloud execution; the public
open-source/self-hosted guide is not an automatic entitlement for a managed
multi-tenant service.
[Website integration](https://developers.openai.com/siwc/website)

The public plan-use documentation requires the app to refresh near access-token
expiry, persist replacement credentials, and serialize refreshes per renewable
session. Its current Codex integration supplies the access token through a custom
Responses provider; renewal requires restarting app-server with the replacement
token and resuming the stored thread. Do not assume the old experimental
`chatgptAuthTokens` callback is the contract for the new SIWC path.
[Accounts/sessions](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions),
[Codex integration](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server)

## Proposed hosted credential design, subject to supported integration access

- Keep existing Zuse authentication separate from connecting a ChatGPT model
  account. Link the validated external registration to the already authenticated
  Zuse account, with fresh state/nonce/PKCE and registered environment callbacks.
- Store validated account/registration identity, granted model-use permissions,
  expiry, credential generation and encrypted token material in private Postgres
  tables. Keep wrapping keys in deployment secrets outside the database. A bare
  signed-in flag or identity token is insufficient for inference.
- One logical backend authority owns renewal. Use a durable per-registration
  refresh lease and generation compare-and-swap so concurrent Cloudflare requests
  cannot race a rotating token. Write the rotated refresh token and updated access
  token/expiry together; a stale lease owner cannot overwrite newer credentials.
  This does not require LISTEN/NOTIFY or an account VM for routine OAuth renewal.
- Workspaces receive only the selected account's permitted execution credential;
  refresh tokens stay at the authority. Never put them in snapshots or the
  WebSocket relay. Cache still-valid access credentials in process memory and
  coalesce refresh requests.
- Use an OAuth library for protocol mechanics where suitable; its use does not
  remove our responsibility for correct persistence, concurrency and revocation.
- Match the account's model-use approval and the approved integration's scopes.
  Handle revoked/expired renewable sessions by asking the user to reconnect.
- For a mode that requires replacing app-server on renewal, retain the thread ID
  and durable execution receipts. Renew before a new turn when needed; coordinate
  restart/resume without blindly resubmitting a potentially executed turn.

This centralizes refresh handling. It does not eliminate it. Faster returning
workspace launches come from reusing authorization and moving grant retrieval off
the runtime/checkout critical path where possible, not from repeating a different
login flow at each launch.

## Harness release management proposal

Treat the Zuse adapter, provider executable and any SDK dependency as one tested
compatibility set. Codex's installed version is already pinned in the original
repository; a new upstream release should become a candidate rather than silently
replace that pin everywhere.

1. Check official release/package metadata on a schedule, for example hourly.
   Download exact candidate versions into immutable artifact directories and
   verify available upstream integrity/signature information. Do not install
   `latest` during workspace startup.
2. Build a manifest recording the Zuse adapter/runtime version, exact Claude and
   Codex versions, architecture, artifact digests, auth/protocol capabilities and
   compatible session-state formats. Keep staging and stable release pointers.
3. Run protocol fixtures and smoke tests: streaming, tools, permissions, Stop,
   steering, resume, selected-account identity, approved credential delivery and
   renewal. Test old conversation continuation and rollback behavior, not just
   `--version`. Run candidate integration on isolated Boxd and Boat machines with
   dedicated supported test credentials.
4. Promote to staging and then a small canary set. Promote stable only after the
   checks and observed behavior pass. A failing candidate leaves stable unchanged.
5. Refresh maintained provider base images asynchronously. Distribute compatible
   harness bundles independently of project dependency snapshots so every repo
   does not need rebuilding for a CLI patch. A cached computer can use its tested
   bundled version while a later version downloads outside the first-turn path.
6. Install versions side by side. Bind each running harness process to an explicit
   immutable executable path. Adopt compatible updates at a safe new-session or
   turn boundary; do not replace or kill an active process. An incompatible state
   format remains pinned until its migration/continuation path is validated.
7. Retain the previous version and its required state until no active or suspended
   workspace needs it. Rolling back the release pointer is not sufficient after an
   irreversible session-state migration; such changes need an explicit plan.

For managed Claude installations, disable autonomous updates in the distributed
runtime and let this release pipeline own the version. Anthropic distinguishes
`DISABLE_AUTOUPDATER` (background checks) from `DISABLE_UPDATES` (all update paths)
and documents versioned installation and binary-manifest verification.
[Claude version management](https://code.claude.com/docs/en/setup)

Existing workspace Git state, branch names and user package setup remain separate
from harness updates. No fetch/rebase/rename is added to resume, and no warm pool
is needed. Auth refresh never triggers an image build. New CLI code and new model
availability are different releases and may require different compatibility tests.

## Current status

Implemented in this branch:

- Rust cloud mode registers brokered Codex alongside Claude and preserves the
  existing local/device onboarding paths. Workspace creation accepts the attested
  Codex adapter and preserves the legacy device-token delivery mode when the build
  advertises it.
- The Codex resolver is independent of the onboarding mechanism. It consumes the
  original sealed access-token or generic API-key grant, pins account identity,
  coalesces concurrent requests and bypasses rejected cached tokens on renewal.
  The authority remains the only refresh-token writer. Runtime generation checks
  apply before cached delivery and after an authority round trip.
- Codex authenticates before thread/model/skill operations, uses ephemeral auth
  storage, redacts broker login failures/stderr, and services bounded renewal
  callbacks outside the streaming/Stop loop. Synthetic tests exercise these paths.
- `infra/cloud/harness-release.py` stages exact native CLI binaries with hashes,
  runtime/adapter/platform/state metadata; requires passing release-bound evidence
  to promote staging/stable; retains old artifacts and supports rollback. Guest
  launch selects immutable paths without network/install work. Workspace pins
  preserve the selected version across suspend/resume. Claude updates are disabled
  in managed releases.
- An hourly GitHub workflow reports upstream candidates without changing stable.
  It begins scheduling after merge. [Operator instructions](../infra/cloud/HARNESS_RELEASES.md).

- Steps 5-7 are implemented as harness bundles
  ([operator flow](../infra/cloud/HARNESS_BUNDLES.md)): one exact CLI version per
  agent and architecture, fetched with npm signature and SHA-512 checks, signed
  and uploaded per environment; a server registry with staging, canary and
  stable channels; background delivery to computers, side-by-side install,
  adoption at quiescent boundaries without touching running processes,
  state-format pinning, retention of the previous version, rollback by
  retiring a bundle, and attestation of what each computer runs.

Not implemented/deployed: hosted SIWC registration/browser flow, standalone
Supabase encrypted credential authority and migration, live credential/model
compatibility checks, and the first real bundle registration. Release promotion has not been performed;
provider allocation benchmarks are not model/auth acceptance evidence.

The hosted integration requirements above still apply. Retaining legacy device
wire compatibility does not establish commercial hosted plan access. The future
SIWC backend can implement a separate resolver rather than coupling the custom
harness to device-code onboarding or assuming identical renewal protocols.
