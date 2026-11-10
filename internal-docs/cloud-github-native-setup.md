# Standalone staging GitHub authority setup

The standalone authority is opt-in and remains disabled until its prerequisites and live tests pass. Unmigrated owners continue using the existing Worker bridge. Do not change production App configuration or remove its existing keys/secrets.

## Private staging configuration

The verified staging App is **5150694**, slug **zuse-staging**, client ID **Iv23lim6UymqlW3Tw2HE**. Its additional signing key is already on Hetzner at `/etc/zuse/cloud/staging/github-app-staging.pem` (root,0600). Production signing-key work was explicitly deferred.

An owner must create an **additional client secret** on the staging GitHub App's General page, keeping the existing client secret. Save the new secret on Hetzner without posting it in chat:

```sh
ssh root@157.180.87.181
install -d -m 700 /etc/zuse/cloud/staging
read -r -s -p 'Additional staging GitHub OAuth client secret: ' zuse_github_secret
printf '\n'
(umask 077; printf '%s' "$zuse_github_secret" > /etc/zuse/cloud/staging/github-oauth-client-secret)
unset zuse_github_secret
```

The agent can generate the webhook secret privately on Hetzner, configure the API container mounts and environment, and verify file permissions. Do not paste either secret into a terminal command, logs, or chat. API configuration supports these private files:

Create API-only copies of the originals; the API container runs as UID10001 and cannot read root0600 files even with a read-only bind mount:

```sh
ssh root@157.180.87.181 'install -d -m 700 /etc/zuse/cloud/staging/github-api && install -o 10001 -g 10001 -m 400 /etc/zuse/cloud/staging/github-app-staging.pem /etc/zuse/cloud/staging/github-api/app.pem && install -o 10001 -g 10001 -m 400 /etc/zuse/cloud/staging/github-oauth-client-secret /etc/zuse/cloud/staging/github-api/oauth-client-secret && install -o 10001 -g 10001 -m 400 /etc/zuse/cloud/staging/github-webhook-secret /etc/zuse/cloud/staging/github-api/webhook-secret'
```

Put these non-secret settings in **`/etc/zuse/cloud/staging/api.env`** (root0600), not shared `runtime.env` or worker configuration:

```text
GITHUB_APP_PRIVATE_KEY_FILE=/run/zuse/github/app.pem
GITHUB_APP_CLIENT_SECRET_FILE=/run/zuse/github/oauth-client-secret
GITHUB_WEBHOOK_SECRET_FILE=/run/zuse/github/webhook-secret
GITHUB_APP_ID=5150694
GITHUB_APP_SLUG=zuse-staging
GITHUB_APP_CLIENT_ID=Iv23lim6UymqlW3Tw2HE
ZUSE_CLOUD_GITHUB_ORIGIN=https://api-rust-stag.zuse.sh
ZUSE_CLOUD_GITHUB_WRITE_ACCOUNTS=<explicit migrated owner IDs>
```

The existing API encryption key remains in `runtime.env`. The single-host deploy helper reads the explicit rollout in optional `api.env`, validates UID10001-readable private copies, and adds `github-api.yaml` only for that environment. The overlay mounts only three individual files read-only into the API; lifecycle/auth workers receive neither the API-only environment nor those files. Original import files stay root0600 and unchanged. Use `deploy.py ... check staging` before starting services. Apply migration0015 before enabling rollout;0016 and later belong to other delivery areas. An empty/missing API-only rollout leaves the bridge and deployment unchanged.

## Staging App dashboard changes

Review these concrete endpoints before changing the **staging** App:

- User authorization callback: `https://api-rust-stag.zuse.sh/v1/cloud/github/callback`.
- Webhook URL: `https://api-rust-stag.zuse.sh/v1/cloud/github/webhook`.
- Webhook content type: `application/json`; SSL verification enabled.
- Webhook secret: private value from `/etc/zuse/cloud/staging/github-webhook-secret`.
- Subscribe to installation/repository-selection and organization/membership changes with required App permissions. Existing repository permissions must include intended contents/pull-request capabilities and organization Members access for private membership checks.

Keep any callback still required by the Worker during migration. If GitHub App setup redirects can target only one URL, keep the original setup redirect until native installation approval/setup handoff is live-verified; the native app currently starts user OAuth itself and presents an installation chooser after callback.

## Implemented API contract

- `POST /v1/cloud/github/install` returns the existing bridge URL for unmigrated owners. For a migrated owner it returns `{url,operationId,expiresAt}`; opening the URL binds the operation to a secure browser cookie before GitHub OAuth. The callback renders installation choices in that same browser; a same-origin, cookie-bound form explicitly confirms one immutable installation before any app-side selection is allowed. App bearer possession alone cannot attach a GitHub identity.
- `GET /v1/cloud/github/status?operationId=...` is actor-and-workspace scoped and returns pending state, expiry, and explicit installation choices after OAuth. `POST /v1/cloud/github/select` takes `{operationId,installationId}` and checks current member permissions and the signed-in GitHub identity. An organization member may connect their own identity to an existing organization installation; linking a new organization installation requires WorkOS administration.
- `POST /v1/cloud/github/cancel` takes `{operationId}`. `POST /v1/cloud/github/disconnect` revokes the actor's GitHub authorization before deleting encrypted credentials; a failed revocation remains retryable without enabling credential delivery.
- Runtime-only `POST /v1/cloud/workspaces/{id}/runtime/github/grant` takes `{protocolVersion:1,requestId,runtimeGeneration,credentialPublicJwk}` with runtime bearer. It scopes the pinned task actor's user authorization to the pinned repository, then seals the result to the enrolled key with task/generation/key/actor/repository metadata. Prepared native storage enrollment is sufficient for initial fetch; execution still requires committed storage enrollment.
- `POST /v1/cloud/github/pull-requests` takes `{repository,title,head,base,body}` and uses the same actor/repository-scoped authorization.

Never enable native private-computer creation merely because allocation or `/app` succeeds. Guest private credential delivery, selected-repository fetch/push/PR and the native selection dialog must all be exercised with real staging accounts.

## Evidence and remaining cases

The isolated PostgreSQL/HTTP regression exercises persisted OAuth state, browser binding, single code consumption, actor/scope separation, member-only existing installation selection, encrypted credentials, concurrent rotation once, rotated-token persistence through repository API outage, scoped private credential issuance, rejected foreign repositories, revocation retry and cancellation. Pure fixtures cover signature tampering, token expiry validation, repository permission intersection and commit identity sanitation.

These are fixture results, not live certification. Automatic verified-user no-chooser continuation, complete install/approval handoff and standalone organization enrollment/seat/rejoin parity are still separate release gates in `cloud-github-parity.md`. Production/staging Worker callback forwarding remains on the old bridge until native handoff tests pass. No customer cutover is authorized by this document.

## Current staging configuration evidence

The additional staging client secret is saved privately; the original was retained. API-only copies are UID10001/mode0400 and the API-only rollout is empty. A bounded, read-only configuration probe called GitHub's documented [Check a token endpoint](https://docs.github.com/en/rest/apps/oauth-applications#check-a-token) with a deliberately invalid synthetic token: configured client ID/secret returned404, while an intentionally invalid client secret returned401. This verifies the App/client-secret pair, without a user authorization code, user token, or grant mutation. It does not verify a real user's OAuth flow.

The guest Git suite passed3/3: sealed task/actor/key/generation/repository/expiry binding, bounded scope parsing, and a real private Unix-socket helper round-trip. The helper stores only executable/socket routing in `.git/config`; token bytes stay in zeroizing memory and the helper pipe. Admission is cached for at most10seconds, independently of the token's maximum one-hour GitHub validity, and each cache use checks the watched runtime generation/actor. A fresh managed base must advertise `rustGithubCredentialsVersion:1` before private task admission; older releases remain gated. API+guest metadata checks passed on the coherent implementation. These are fixture/compilation results; real private clone, push and pull-request creation remain live acceptance gates.


## Consent and identity fencing release gate

The initial built API artifact `f474475d…` predates the browser-consent safeguard. It must keep `ZUSE_CLOUD_GITHUB_WRITE_ACCOUNTS` empty. The newer source adds a same-cookie, same-origin browser installation form, requires explicit browser consent before an app bearer can finalize, and pins `rustGithubUserId` on tasks. The native PostgreSQL/router suite passes5/5 including a real form POST, denied foreign Origin, no-store/CSP, prepared-storage encrypted Git grant and explicit account-switch rejection. A new API artifact must be built and deployed before any native canary. Before rolling back to an artifact predating this safeguard, disable the native cohort first. Migration0015 remains unchanged.

Read-only `/app/hook/config` inspection found the staging App still sends its webhook to the legacy `https://api-staging.zuse.sh/v1/cloud/github/callback`, with JSON/SSL verification enabled and no configured webhook secret. The newly generated host secret has not thereby been installed in GitHub. Keep this distinction explicit; callback registration is separate from webhook configuration, and webhook cutover must preserve the transitional integration until approved. The App has Members:read and Contents/PullRequests:write permissions.
