# Pull request links

Status, 2026-11-28: built (see "Built" at the end). It needs the GitHub App
changes under "Prerequisites" before any event arrives.

A chat can own pull requests. When something happens on a pull request it
owns (a comment, a review, a check result, a conflict, a merge), the chat
hears about it right away and can act on it. Events reach the chat by
GitHub webhook and the account event socket, never by polling GitHub.

## Today

- A chat stores no pull request. The pull request shown for a chat is
  looked up from its branch with `gh pr list --head` each time
  (`crates/engine/src/change_requests.rs`), so a chat shows at most one, and
  only while it stays on that branch.
- PR watch (`crates/engine/src/pr_watch.rs`) is keyed by chat and pull
  request URL. It polls `gh pr view` every 20 to 60 seconds per watch. It
  pauses when the chat's branch stops matching. It runs repair turns
  through `dispatch_ci_repair`, which bypasses the command queue and gives
  the turn no `MessageSource`.
- The cloud GitHub App webhook (`POST /v1/cloud/github/webhook`,
  `crates/cloud/src/github/native.rs`) verifies, deduplicates and records
  every delivery. It only acts on installation and membership events, so
  pull request events are dropped.
- Slack thread links (`crates/cloud/src/slack/links.rs`,
  `crates/engine/src/slack_links.rs`) are the pattern to follow:
  - a cloud link table with a reverse lookup;
  - a database-only intercept in the webhook;
  - a sealed inbox;
  - delivery to the chat as an identified, queued `Run` tagged with a
    `MessageSource`.

## Model

**Ownership is held on the pull request.** Each pull request has at most
one owning chat, and only the owner receives its events. One chat can own
any number of pull requests: several repositories, a stack, or a follow-up
fix.

- The owner changes to **the chat that last pushed** to the pull request's
  head branch, or to the chat that opened it. A person can also hand a pull
  request to a chat ("Chat about this pull request", or an `@zuse` mention,
  see below).
- Other chats can still **reference** a pull request and show its status.
  They get no events, so one review comment never wakes three agents.
- The owner can release a pull request. It then has no owner until
  something claims it again.

Cloud Postgres (`zuse_rust_pull_request_owners`):

| column | |
|---|---|
| `account_id`, `repository_id`, `number` | primary key: one owner per pull request per Zuse account; `repository_id` survives renames |
| `installation_id` | the App installation that covered the repository at claim time; webhooks from any other installation are ignored |
| `repository`, `url`, `title`, `state`, `draft`, `head_ref`, `head_sha` | kept current from webhooks; `head_sha` matches check results on fork pull requests |
| `actor_id`, `chat_key` | the owner (`chat:{id}`, the same key Slack links use) |
| `claimed_by`, `claimed_at` | `created`, `pushed`, `manual`, or `branch` (a branch claim never takes a pull request another chat owns) |
| `checks`, `checks_sha` | the last check result delivered, so a pass is only reported after a failure |
| `policy` | per-event settings as JSON, see below |

Indexes: `(account_id, actor_id, chat_key)` to list a chat's pull requests,
`(repository_id, number)` and `(repository_id, head_sha)` for webhooks.
Releasing a pull request deletes its row and its pending events.

The engine keeps a copy of its account's owned pull requests in
`pull-request-links.json`, so the UI can list them offline. The copy is
refreshed when a `pull-request` hint arrives. It is never polled.

## Claiming

| trigger | who reports it |
|---|---|
| The agent runs `gh pr create` | engine (already detected by `opened_pull_request` in `pr_watch.rs`) → `POST /v1/cloud/github/pull-requests/claims` |
| A pull request is created from the Git panel or the cloud create endpoint | the endpoint claims directly |
| The chat pushes to a branch that has an open pull request | cloud chats: the runtime's git credential helper already sees the push. Local chats: the engine reports `{repository, branch, sha}` after a push it observes. The webhook's `pull_request.synchronize` confirms the sha, and an unconfirmed claim never takes ownership. |
| A person picks "Chat about this pull request" | client |
| `@zuse` in a comment on a pull request with no owner | webhook: starts a chat in the project that has the repository, and that chat owns the pull request |
| A Zapper worker links a pull request to an issue | not built yet |

A local chat can only claim pull requests in repositories the account's
GitHub App installation covers. Outside an installation the chat can still
reference the pull request but gets no events, and the UI says so.

## Events

Webhook events to subscribe the App to:

- `pull_request`: opened, synchronize, closed, reopened, ready_for_review
- `pull_request_review`: submitted
- `pull_request_review_comment`: created
- `issue_comment`: created, only when the issue is a pull request
- `check_suite`: completed
- `check_run`: completed, failures only
- `workflow_run` is not needed: `check_suite` covers it.

Each event has a setting on the owner's policy, with defaults:

| event | default |
|---|---|
| CI failed | wake |
| all checks finished | wake once, only if something failed earlier |
| human review or comment | wake |
| bot comment (`[bot]` users or a configured list) | ignore |
| merge conflict | not built yet: GitHub sends no webhook when mergeability changes |
| merged or closed by someone other than the agent | context |

- **wake** runs a turn now.
- **context** adds the event to the chat's next turn.
- **ignore** drops it.

Rules that keep this from getting noisy:

- **Debounce.** Review comments arriving within 60 seconds are combined
  into one delivery. This replaces PR watch's 90 second quiet window, but
  the delay is held server side and no polling is involved. A failing check
  wakes the chat at once. Later failures in the same check suite are
  combined into it.
- **Loop guard.** After 10 wakes in a row triggered only by comments, with
  no human message in the chat, waking stops until someone writes in the
  chat.
- **Stale chats.** Conflict, merge and close only wake a chat that a person
  wrote in during the last 3 days. Archived chats never wake; their events
  are kept as context.
- **Echo suppression.** Events the chat caused itself (its own push, its
  own merge, comments posted with its token) are dropped. The runtime's
  token tells us which those are. For local chats the engine records what
  it pushed and merged.

## Delivery

The webhook handler only touches the database, mirroring
`intercept_linked_reply`:

1. Deduplicate the delivery, as today.
2. Look up the owner by `(installation_id, repository_id, number)`. For a
   check suite, match its `head_sha` against the owner rows of that
   repository.
3. Apply the policy. If the event survives, seal the payload into
   `zuse_rust_pull_request_inbox` with key `(owner, delivery_id)`. The
   payload is the author, event kind, URL, a short body (truncated) and the
   names of the failed checks.

From there it reaches the chat with no polling:

- **Cloud chats:** the API enqueues a `messages.send` command on the task
  mailbox with a `github` actor marker. `crates/cloud-runtime/src/mailbox.rs`
  already admits the `slack` marker; `github` would be admitted the same
  way. The message carries `MessageSource::GitHub`.
- **Local chats:** the same transaction adds a `pull-request` row to the
  account event outbox. It is a hint kind that is only sent when the ticket
  asks for it, like `computer-request` in #399. When the engine sees the
  hint on its socket it fetches the inbox, delivers each message and acks
  it. It reads the inbox on its own only while the socket is down
  (`Feed::live` is false), which is the same fallback every feed reader
  has.
- **Engine delivery:** generalise `slack_links::deliver` into a shared
  `inbound::deliver(chat, source, text, idempotency_key)`. It pushes a
  user `SessionMessageEntry` with `MessagePart::MessageSource` and queues
  an identified `Run`. A **context** event pushes the entry but queues no
  run.

New variant in `crates/protocol/src/slack.rs`:

```rust
MessageSource::GitHub {
    repository: String,
    number: u64,
    url: String,
    event: GitHubEventKind, // Comment, Review, ReviewComment, ChecksFailed, ChecksPassed, Conflict, Merged, Closed
    author: Option<String>,
}
```

The transcript renders this as a chip, like the Slack one, linking to the
comment or check on GitHub.

## What happens to PR watch

PR watch polling is removed once webhook delivery is live:

- auto-fix becomes the CI-failed and conflict settings on the owner's
  policy;
- `failure_key`, `conflict_key` and the attempt budget move into the cloud
  policy evaluation;
- the follow-up prompt text in `follow_up_prompt` is reused for the CI
  message;
- CI log fetching (`source_control_ci.rs`) and the review threads query
  (`source_control_review_threads.rs`) remain as tools the agent calls
  after it wakes.

`change_requests.rs` keeps working out a chat's own pull request from its
branch, but only to offer a claim, as "This branch has #123. Own it?", or
to claim it automatically when the chat pushed that branch.

## UI

- **Chat header and session info:** a row of chips for the pull requests
  the chat owns (number, state, checks) and the ones it references (shown
  muted). The pull request panel gets a tab per owned pull request.
- **Pull request panel:** an owner row ("Owned by <chat>", Open chat /
  Take over / Release), plus the event settings.
- **Sidebar:** the status line under a chat shows "Checks running",
  "Checks failing" or "Ready to merge" for its owned pull requests.
- **Agent tools (MCP):**
  - `claim_pull_request`, `release_pull_request`, `list_pull_requests`
  - `reply_to_pull_request` (comment or review thread reply)
  - `pull_request_checks` (status plus failed logs), wrapping
    `source_control_ci.rs`

## Prerequisites

- **Webhook cutover.** The staging App still sends webhooks to the legacy
  `/v1/cloud/github/callback` with no secret
  (`cloud-github-native-setup.md`). Move it to `/v1/cloud/github/webhook`
  and turn on the secret before any of this runs. Production needs the
  same.
- **App permissions.** The App needs Checks: read, Issues: read (for issue
  comments) and Pull requests: read/write, and must subscribe to the events
  above plus `push` (for conflict checks). Existing installations must accept the new permissions. Until they
  do, those accounts get no events, and the pull request panel says so.

## Build order

1. **Ownership.**
   - the cloud table and claim, release and list endpoints;
   - claims from `gh pr create`, the Git panel and the cloud create
     endpoint;
   - the engine mirror, protocol types, and chips in web-solid and native.

   Useful on its own: a chat shows all of its pull requests.
2. **Webhook to the chat.**
   - pull request event handlers, policy, inbox and debounce;
   - `MessageSource::GitHub` and the shared `deliver`;
   - mailbox delivery for cloud chats, the hint plus inbox for local chats.
3. **Push claims and echo suppression.** Claims from the credential helper
   and from pushes the engine observes, confirmed by `synchronize`.
4. **Retire PR watch polling.** Move auto-fix into the policy.
5. **`@zuse` mentions** and "Chat about this pull request" on unowned pull
   requests, plus Zapper issue claims.

## Built

- **Cloud** (`crates/cloud/src/github/native/pull_requests.rs`).
  - Migration `0059_pull_request_owners.sql` holds:
    - the owner table, which also records the host workspace for cloud
      chats, the base branch and the conflict-check state;
    - the sealed inbox;
    - the mentions table;
    - `pull-request` hint triggers on all three.
  - Claim, release, list, event settings, inbox, ack and mentions are
    served under `/v1/cloud/github/pull-requests/` (routes in
    `crates/cloud/src/routes/github_pull_request_links.rs`).
  - Webhooks are routed inside the delivery's transaction.
    `pull_requests/events.rs` classifies payloads as a pure function.
    The handled events are `pull_request`, `push`, `issue_comment`,
    `pull_request_review`, `pull_request_review_comment`, `check_run` and
    `check_suite`.
  - **Cloud chats.** A pull request opened from, or pushed to, the branch
    of a cloud computer in the account is linked to that computer's chat
    when no chat owns it yet. Forks are excluded.
  - **The worker** (`pull_requests/worker.rs`) is woken after each webhook,
    with a 10-second timer for checks that come due later.
    - It queues events for cloud chats into their conversation through
      the cloud chat command queue, which wakes the computer. Feedback
      waits for 20 seconds of quiet. Context waits for the next event
      that starts a turn.
    - **Conflicts.** A push to a pull request or to its base branch
      schedules a check 30 seconds later. The worker reads mergeability
      with a read-only installation token, retries while GitHub is still
      computing it, and reports each head and base pair once.
  - **Mentions.** `@zuse` from a connected Zuse user (personal accounts) on
    a pull request none of their chats owns is recorded for their engine.
  - **Echo handling.** Comments written by the owner's own GitHub user are
    usually the agent posting with their token. They are delivered as
    context unless they mention `@zuse`.
  - **Tests** (`pull_requests/database_tests.rs`, run with
    `ZUSE_CLOUD_TEST_DATABASE_URL` and `--include-ignored`):
    - the desktop flow;
    - cloud-chat links;
    - push scheduling;
    - mentions;
    - the conflict check against a mock GitHub.
- **Engine** (`crates/engine/src/pull_request_links.rs`).
  - Follows the account feed and reads the inbox and mentions when a hint
    arrives. While the socket is down it reads them every 60 seconds.
  - Delivers events as one `MessageSource::GitHub` message per chat.
    Feedback waits for 20 seconds of quiet. The loop guard, stale-chat rule
    and archived rule apply.
  - Keeps context events for the chat's next prompt, through the
    `SessionsEngine::set_prompt_context` hook.
  - Claims a pull request when a turn runs `gh pr create`, or runs
    `git push` on a branch with an open pull request.
  - **Mentions** start a chat in a project on this computer whose `origin`
    is the repository, using the project's latest agent settings. The chat
    is linked to the pull request and asked to `gh pr checkout` it first.
  - **CI watch.** It no longer polls pull requests linked to their chat;
    those arrive by webhook. Its auto-fix switch still decides whether
    failed checks and conflicts start a turn or are only noted.
- **RPC.** `WatchPullRequestLinks` lists the account's linked pull
  requests from this computer's engine. `ClaimPullRequest`,
  `ReleasePullRequest` and `SetPullRequestPolicy` go to the engine hosting
  the chat. For a cloud chat they go through this engine, with
  `workspaceId`.
- **Web client and native app.** Both show the same things:
  - linked pull requests in the session card;
  - a link control on the pull request view ("Linked to this chat", "Link
    here instead", "Unlink");
  - event settings for each group: start a turn, note for next turn, or
    ignore.

  Opening a chat whose branch has an open pull request with no owner links
  it with source `branch`, which never takes a pull request from another
  chat.

Known limits:
- Mentions only cover personal accounts. An organization's member list is
  not read inside a webhook.
- Cloud chats get no loop guard or stale-chat rule.
- A message queued into a cloud chat resumes that chat's queue if someone
  had stopped it, as a typed message would.
- The worker's delivery to cloud chats is not covered by a test, because it
  needs the cloud chat command runtime.
