# Slack app

The Rust Slack application runs inside `apps/cloud` (`zuse-cloud-api serve`).
`crates/cloud/src/slack` owns OAuth, signature verification and durable delivery;
the hosted runtime still owns execution and writable conversation history.

```text
Settings → Cloud → Slack
  select Personal / an accessible organization, repository and model
  authenticated host RPC → cloud API → browser-bound OAuth confirmation
  pending link → confirmed by the same Zuse account in Settings → member link
Slack mention / new DM
  HMAC + timestamp check → encrypted PostgreSQL job (acknowledge immediately)
  member/account + thread binding → idempotent cloud create
  encrypted messages.send mailbox command → runtime admission + transcript
  runtime completion receipt keyed by submitted message ID
  encrypted checkpoint → Slack thread reply update
```

## Reference and behavior

Verified against the original `apps/slack/src`, its `infra/api/src/slack` adapter,
`packages/contracts/src/{cloud-api,cloud-workspaces,session}.ts`, and PRs
[#880](https://github.com/swarajbachu/zuse/pull/880),
[#895](https://github.com/swarajbachu/zuse/pull/895), and
[#662](https://github.com/swarajbachu/zuse/pull/662).

Mention the app in a channel, or send a new DM, to start a task. An explicit
mention in an existing task thread continues its workspace. Untagged thread
replies, bot messages and message edits do not start runs. A first invitation
into an existing thread imports bounded, paginated text history as quoted,
untrusted context; normal follow-ups do not reimport it. The trigger appears
once in the provider prompt. Names use `users.info` and `conversations.info`;
missing name scopes fall back to “From Slack”. Both chat surfaces display the
sender/channel and the sender's words while preserving the full provider prompt.

Settings offers an explicit Zuse workspace picker before Slack's own workspace
selection. Personal is always a choice; organization choices come from the
server's enabled organization IDs and verified active content membership.
Membership lookup failures fail the connection instead of silently choosing
Personal. The existing guest, gateway and credential broker reject organization
execution; the picker preserves that selected scope and explains the limitation.
Organization connection is refused before OAuth or allocation, rather than
silently using Personal. Personal workspaces support the complete task flow.
Confirmation and task/result delivery recheck membership. Only the
linked Slack member can use that account. A different member/account cannot take
over a thread. Disconnect removes that member's link; it does not uninstall the
Slack application or cancel an accepted task. Other members' links remain intact.

Only ready cloud repositories with an attested `rustSlackVersion: 1` image are
selectable. Rebuild existing images to gain attribution and completion receipt
support. Slack attribution adds a `messageSource` message part; Slack transcript
projection reads require updated native/browser clients. Agent/model choices use
the existing cloud catalog. Selecting a model
does not connect its provider credentials. Tasks use the runtime's automatic
review posture; questions and approvals that need a person still require Zuse.

## Deployment

1. Apply `crates/cloud/migrations/0009_slack.sql` to the existing cloud database
   through the normal migration process. It creates only `zuse_rust_slack_*`
   tables and does not adopt existing TypeScript installations.
2. Configure `ZUSE_SLACK_ENABLED=true`, `SLACK_CLIENT_ID`, `SLACK_CLIENT_SECRET`,
   `SLACK_SIGNING_SECRET`, and `ZUSE_SLACK_REDIRECT_URI` (an HTTPS URL ending in
   `/slack/oauth/callback`). These belong only to the cloud service.
3. Existing runtime creation, billing, identity, encryption and transcript object
   storage must be configured. `ZUSE_CLOUD_CREATE_ACCOUNTS` explicitly includes
   each enabled Personal account owner. Organization execution remains blocked by
   the existing runtime authority; this integration does not bypass those guards.
4. Rebuild and attest the runtime image. The image manifest reports Slack support;
   the image worker preserves that capability in build settings.
5. Create the Slack app from `apps/cloud/slack-manifest.example.yaml` with your
   service origin, and copy its client ID, client secret and signing secret into
   the configuration from step 2.
6. Start the existing cloud API and allocation worker. The API starts its durable
   Slack worker only when enabled; missing secrets/schema/runtime/transcript
   prerequisites fail startup. No client receives an installation token or secret.
7. With the API running, configure the event request URL (Slack verifies it
   against the live `POST /slack/events` endpoint before saving) and the OAuth
   callback URL, then authorize the requested scopes.

Public ingress is `POST /slack/events`. Settings uses authenticated
`GET /v1/cloud/slack`, `POST /v1/cloud/slack/connect`, and
`POST /v1/cloud/slack/disconnect` and `POST /v1/cloud/slack/confirm`. Browser OAuth uses `/slack/oauth/{start,authorize,callback}`.
The native/browser clients call `SlackConnections`, `SlackConnect`,
`SlackDisconnect` and `SlackConfirm` through the host's account authority. Selected workspace headers
are reauthorized server-side; the host's account binding fences stale responses.

## Security and recovery

- Verify Slack's HMAC-SHA256 signature over the exact body bytes with constant-time
  comparison and a five-minute timestamp window, before decoding JSON.
- OAuth state has ten minutes of validity, is encrypted and single use, and is
  bound to an HttpOnly/Secure/SameSite browser cookie. An explicit account/repository
  confirmation and same-origin POST precede Slack authorization. Callback code
  exchange is never automatically replayed after an uncertain failure.
- Finishing Slack sign-in in a browser only creates a pending link (sealed, 30
  minutes, single use). It becomes a member link when the same Zuse account and
  actor confirm the shown Slack user and workspace through `SlackConfirm`, with
  access rechecked; a leaked start link therefore cannot attach someone else's
  Slack user. Existing links are unchanged until a confirmation succeeds.
- Bot tokens and queued request/context/result bodies are AES-GCM sealed with
  purpose/tenant/identity-bound AAD. Only safe identifiers and settings are public.
- Event ID and `(team,channel,message timestamp)` uniqueness deduplicate retries
  and duplicate delivery types. The per-thread creation request is frozen and sealed
  before creation; retries retain the original prompt, branch and model even after
  a lost response. The frozen prompt is cleared after its workspace mapping commits.
  Commands retain their IDs, timestamps and fingerprints. Admission
  ordering is per thread, while due-time scheduling prevents polling jobs from
  starving newer requests. Cloud admission remains idempotent.
- Public command enqueue strips actor metadata. Only the internal Slack path can
  set the server-owned Slack ingress marker. The runtime rejects source attribution
  without that marker; organization actor objects remain unsupported by that path.
- One PostgreSQL transaction advisory lock serializes Slack workers across API
  replicas. Slack `Retry-After` pauses the local worker; job backoff is durable.
- Completion comes from the runtime's first terminal receipt for the submitted
  user message, not adjacency, “idle”, tool output or private model reasoning.
  The last 128 completion receipts remain in the encrypted conversation projection.
- Progress uses a stable Slack `client_msg_id`; the final result updates the saved
  reply timestamp. Slack's post deduplication is best effort: a crash after posting
  but before saving its timestamp can duplicate a progress reply. It cannot duplicate
  the cloud run or mailbox command. Updates after the timestamp is saved are safe
  to repeat. A deleted or uneditable reply gets a persisted replacement intent and
  a new stable reply ID before result delivery resumes.
- Jobs stop status polling after 24 hours, become `dead` after 48 hours if delivery
  still cannot finish, and retain encrypted receipts for 30 days. Inspect phase,
  attempts and `next_at` for operational recovery; never blindly replay a completed
  command under a new ID. Expired OAuth sessions and old terminal jobs are swept in
  bounded batches. Uninstallation deletes the installation credentials and links.

## Validation and remaining acceptance

Fixture tests exercise raw signed mention/DM/thread payloads, signature tampering
and expiry, duplicate deliveries, restart after mailbox enqueue, sender/channel
attribution, OAuth browser/CSRF/replay/expiry boundaries, workspace selection,
organization scope/access revocation and pre-allocation execution gating, cross-account thread takeover, encrypted result
correlation and token redaction. Runtime/doc/UI tests cover the attribution trust
boundary, persistent metadata, bounded terminal receipts and visible user text.

Run the focused commands recorded in the PR's Evidence section. PostgreSQL tests
are opt-in, create random private schemas on a localhost-only database, and delete
only those schemas. No production Slack, identity, provider or account credentials
are needed for fixtures.

Live Slack installation/sign-in, actual hosted provider execution, rendered native
and browser visual acceptance and production rollout remain separate acceptance
gates. Deferred from the original: organization execution (requires the cloud
organization runtime/credential adapter), image/file imports (explicit no-run reply),
App Home account linking and repository modals, shared installer accounts, alert
automations, optional untagged follow-ups, native Slack loading indicators,
token streaming/cancellation, and the dedicated Slack sidebar mark. Existing
TypeScript installations require a separately planned ownership/data migration.
