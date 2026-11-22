# Issues in cloud Postgres, with a top-level Issues tab

Status: proposal for owner review (ZAP-24, step 1). No product code changes with this doc.

## 1. Why

Today a Zapper's issues are a `Vec<ZotIssue>` inside its state: one JSON blob per
Zapper in the `zots` table of the home device's engine SQLite
(`crates/engine/src/zots.rs:265`, mutations in `crates/engine/src/zots/issues.rs`).
There is no cloud copy. The web app sees issues only through the home engine
(`crates/viewport/src/zot_issues.rs`), and numbers are per Zapper (`ZAP-n`,
`ZotIssue::code`).

The owner chose cloud Postgres as the home for issues so that they can:

- be browsed across every Zapper from one top-level Issues page, on desktop and web;
- be read when the home device is off;
- later be linked to GitHub automatically.

The page has these filters: For you, Assigned to me, Created by me, Mentioned and
All. Issues are grouped by status, and each row shows blocking/blocked-by chips,
the owner's avatar and the issue's age.

## 2. What exists today

### 2.1 Issues in the engine

| Piece | Where | Notes |
|---|---|---|
| Model | `crates/protocol/src/zots.rs:653-875` | `ZotIssue{id, number, details, created_at, updated_at, threads, pull_requests, activity}`. Status has 8 values. Priority has 4. Owner is `Zapper \| User \| Thread{chat_id}`. `relations` is an untyped list of ids. `parent` is optional. |
| Commands | `ZotAction::{CreateIssue, EditIssue, LinkIssueThread, LinkIssuePullRequest, UnlinkIssueThread, UnlinkIssuePullRequest, CommentIssue, AskUser, CloseIssue, Delegate{issue_id}}` | Ids are caller-minted slugs, unique within one Zapper. `EditIssue` sends the whole `details`. |
| Rules | `zots/issues.rs` | `validate` sets size limits, checks relations and parent exist, and checks the owning thread belongs to the Zapper. `transition` holds the coordinator's evidence rules. `normalize` keeps owner and status consistent. `hand_back` runs when the user answers. `canonical_pr` only accepts HTTPS GitHub PR URLs. |
| Atomicity | `Zots::command_as` (`zots.rs:579`) | Receipt, revision check (`zot_targets`, scoped per issue through `issue_target`), the issue change and the `zot_events` notification all commit in one SQLite transaction. |
| Worker reports | `issues::report` (`zots/issues.rs:539`) | Appends `workerReport` activity to every linked issue. On a blocked or failed report it moves the issue to Blocked and hands it to the Zapper. |
| Prompt | `issues::coordinator_view` | Gives every coordinator turn the open board, with recent activity clipped. |
| Worker scope | `rpc/zots.rs:18-33` | A worker thread may only comment on, or link a PR to, issues it is linked to. |

### 2.2 The cloud server

| Area | What it does |
|---|---|
| Server | `crates/cloud` (bin `apps/cloud`, `zuse-cloud-api serve`). axum, with nearly every handler in `routes.rs`. Handlers start with `state.access(&headers, AccessKind::Content)`. |
| Database | sqlx 0.8 with runtime query strings, no `query!` macros. The HTTP `Store` uses the read-only pool. Writes go through feature "controls" (`ProjectControl`, `ComputerControl`, …) that hold the shared pool. |
| Migrations | `crates/cloud/migrations/NNNN_name.sql`. They are applied out-of-band by `infra/cloud/database.py`: the hard-coded `FILES` tuple, one psql transaction, checksums in `zuse_cloud_schema_migrations`. Startup never runs DDL. New tables must `GRANT` to the restricted app role (pattern in `0038_app_role_grants.sql`). Code embeds migrations with `include_str!` for tests. |
| Conventions | `text` ids, `bigint` epoch-ms timestamps, `text` + `CHECK` enums. `owner_id` is the tenant key on every row. Idempotency uses `idempotency_key` + `request_digest`. `revision bigint` provides optimistic concurrency. |
| Tenancy and auth | WorkOS JWT bearer tokens. There are no users or orgs tables. The `x-zuse-workspace` header gives `owner_id = <userId>` for personal scope or `organization:<orgId>` for org scope. Org membership and role (`admin`, `member`, `billing`) come live from WorkOS (`access.rs:54`). |
| Realtime | Account events WebSocket `/v1/cloud/events`. Payloads are invalidations only (kind + resource id) and clients refetch. The kinds are a `CHECK` list, and the reusable `zuse_rust_change_hint_outbox` trigger is in `0049_change_hints.sql`. |
| Per-owner counters | `zuse_rust_account_event_cursors` uses an `INSERT … ON CONFLICT DO UPDATE … RETURNING` allocator. |
| Engine → cloud | `crates/engine/src/cloud.rs` `CloudApi` (reqwest, 20 s timeout, bearer token plus `x-zuse-workspace`). There is no generic offline outbox for cloud calls. The engine has local outbox precedents (`zot_outbox`, chat docs). |
| Web → cloud | The browser app (`apps/web`, viewport compiled to wasm) never calls `/v1/cloud` directly. It talks to an engine (paired home engine or the account/managed connection), and that engine calls the cloud. No token crosses into the viewport. |
| Tests | `crates/cloud/tests/postgres.rs`: an isolated localhost Postgres schema per test, routes driven with `oneshot`, and `TestIdentity`. |

## 3. Decisions in this design

1. **Cloud Postgres is the source of truth. Every engine keeps a local mirror plus an outbox.**
   - Zapper turns keep working offline, and the Issues tab opens instantly from the mirror.
   - The mirror converges through a cloud change feed.
2. **Zapper rules stay in the engine. Data rules are shared with the cloud.**
   - Only the Zapper's home engine knows its workers and threads, so it enforces the Zapper rules: `transition`, worker scope, `hand_back`, `zot_events` notifications.
   - The pure checks (sizes, PR canonicalisation, `normalize`, relation sanity) move into one module that both engine and cloud compile (slice 1), so the two sides cannot drift.
3. **The tenant is the cloud `owner_id`, which is the "workspace".**
   - That is a personal account or `organization:<id>`. Every issue, number, relation and activity row carries `owner_id`, and every query filters on it.
   - A Zapper's issues belong to the scope its home engine is signed into.
4. **Numbers are per workspace, with a workspace key.**
   - A code reads `<KEY>-<n>`, for example `ZUS-12`.
   - Existing `ZAP-n` codes are kept as per-Zapper aliases, so nothing that cites an old code breaks (section 7).
5. **Issues may exist without a Zapper.** The Issues tab can create one directly. `zapper_id` is nullable.
6. **Clients reach the cloud through their engine**, as they do today.
   - Desktop and web call engine RPCs, and the engine calls the cloud API with the signed-in user's token.
   - A browser path that reads the cloud directly is left as an open question (section 13).

## 4. Postgres schema

New file: `crates/cloud/migrations/0055_issues.sql`.
- Use the next free number at merge time, because other sessions add migrations too.
- Add the file to `FILES` in `infra/cloud/database.py`.
- Grant the new tables to the app role in the same file.
- Follow the existing conventions: `text` ids, `bigint` ms timestamps, unquoted `cloud_*` names, `CHECK` enums.

```sql
-- One row per workspace: issue key and counters.
CREATE TABLE IF NOT EXISTS cloud_issue_heads (
  owner_id      text PRIMARY KEY,
  key           text NOT NULL CHECK (key ~ '^[A-Z][A-Z0-9]{1,5}$'),
  next_number   bigint NOT NULL DEFAULT 1,
  next_change   bigint NOT NULL DEFAULT 1,      -- change-feed cursor source
  updated_at    bigint NOT NULL
);

-- Zappers known to the cloud, so issues can show a Zapper avatar and name and
-- "Zappers I own" can be resolved. Registered by the home engine.
CREATE TABLE IF NOT EXISTS cloud_zappers (
  owner_id       text NOT NULL,
  zapper_id      text NOT NULL,
  created_by     text NOT NULL,                 -- WorkOS user id of the Zapper's person
  home_device_id text NOT NULL,
  name           text NOT NULL,
  avatar_mascot  text, avatar_color text,
  deleted_at     bigint,
  created_at     bigint NOT NULL, updated_at bigint NOT NULL,
  PRIMARY KEY (owner_id, zapper_id)
);

CREATE TABLE IF NOT EXISTS cloud_issues (
  owner_id      text NOT NULL,
  issue_id      text NOT NULL,                  -- global, client-minted 'iss_<uuid>' (offline create)
  number        bigint,                         -- NULL only until assigned; assigned in the create txn
  zapper_id     text,                           -- NULL = workspace issue with no Zapper
  slug          text,                           -- the Zapper's caller-minted issueId (e.g. 'issues-top-level-tab')
  title         text NOT NULL CHECK (octet_length(title) BETWEEN 1 AND 500),
  description   text NOT NULL DEFAULT '' CHECK (octet_length(description) <= 1048576),
  done_criteria text NOT NULL DEFAULT '' CHECK (octet_length(done_criteria) <= 65536),
  out_of_scope  text NOT NULL DEFAULT '' CHECK (octet_length(out_of_scope) <= 65536),
  status        text NOT NULL CHECK (status IN ('backlog','todo','inProgress','needsYou','blocked','inReview','done','canceled')),
  priority      text NOT NULL CHECK (priority IN ('urgent','high','normal','low')),
  -- exactly one owner (assignee) at a time
  assignee_kind text CHECK (assignee_kind IN ('user','zapper','thread')),
  assignee_user text,                           -- WorkOS user id when kind='user'
  assignee_chat text,                           -- thread chat id when kind='thread'
  creator_kind  text NOT NULL CHECK (creator_kind IN ('user','zapper','thread')),
  creator_user  text,                           -- person behind the action (always set when known)
  parent_issue  text,
  due_date      text CHECK (due_date ~ '^\d{4}-\d{2}-\d{2}$'),
  labels        text[] NOT NULL DEFAULT '{}',
  outcome       text CHECK (outcome IN ('merged','dropped')),
  closed_at     bigint,
  revision      bigint NOT NULL DEFAULT 1,      -- per-issue optimistic concurrency
  change_seq    bigint NOT NULL,                -- from cloud_issue_heads.next_change
  created_at    bigint NOT NULL,
  updated_at    bigint NOT NULL,
  idempotency_key text NOT NULL,                -- the creating mutation id
  PRIMARY KEY (owner_id, issue_id),
  UNIQUE (owner_id, number),
  UNIQUE (owner_id, zapper_id, slug),
  UNIQUE (owner_id, idempotency_key),
  FOREIGN KEY (owner_id, parent_issue) REFERENCES cloud_issues(owner_id, issue_id),
  CHECK ((assignee_kind = 'user') = (assignee_user IS NOT NULL)),
  CHECK ((assignee_kind = 'thread') = (assignee_chat IS NOT NULL))
);
CREATE INDEX cloud_issues_feed     ON cloud_issues(owner_id, change_seq);
CREATE INDEX cloud_issues_assignee ON cloud_issues(owner_id, assignee_user) WHERE closed_at IS NULL;
CREATE INDEX cloud_issues_creator  ON cloud_issues(owner_id, creator_user);
CREATE INDEX cloud_issues_zapper   ON cloud_issues(owner_id, zapper_id, status);

-- Legacy and renamed codes, so 'ZAP-7' keeps resolving inside its Zapper.
CREATE TABLE IF NOT EXISTS cloud_issue_aliases (
  owner_id  text NOT NULL,
  zapper_id text NOT NULL,
  code      text NOT NULL,                      -- 'ZAP-7'
  issue_id  text NOT NULL,
  PRIMARY KEY (owner_id, zapper_id, code),
  FOREIGN KEY (owner_id, issue_id) REFERENCES cloud_issues ON DELETE CASCADE
);

-- Directed relations. 'blocks' drives the blocking / blocked-by chips;
-- 'related' holds today's untyped relations.
CREATE TABLE IF NOT EXISTS cloud_issue_relations (
  owner_id   text NOT NULL,
  from_issue text NOT NULL,
  to_issue   text NOT NULL,
  kind       text NOT NULL CHECK (kind IN ('blocks','related')),
  created_at bigint NOT NULL,
  PRIMARY KEY (owner_id, from_issue, to_issue, kind),
  CHECK (from_issue <> to_issue),
  FOREIGN KEY (owner_id, from_issue) REFERENCES cloud_issues ON DELETE CASCADE,
  FOREIGN KEY (owner_id, to_issue)   REFERENCES cloud_issues ON DELETE CASCADE
);
CREATE INDEX cloud_issue_relations_to ON cloud_issue_relations(owner_id, to_issue);

-- Threads that worked on the issue.
CREATE TABLE IF NOT EXISTS cloud_issue_threads (
  owner_id  text NOT NULL,
  issue_id  text NOT NULL,
  chat_id   text NOT NULL,
  device_id text,                               -- where the thread runs, for "open thread"
  linked_at bigint NOT NULL,
  position  bigint NOT NULL,                    -- keeps today's link order (hand_back picks newest)
  PRIMARY KEY (owner_id, issue_id, chat_id),
  FOREIGN KEY (owner_id, issue_id) REFERENCES cloud_issues ON DELETE CASCADE
);

-- Pull requests. Repo and number are parsed from the canonical URL so a later
-- GitHub sync can match webhooks without parsing strings.
CREATE TABLE IF NOT EXISTS cloud_issue_pull_requests (
  owner_id   text NOT NULL,
  issue_id   text NOT NULL,
  url        text NOT NULL,                     -- canonical https://github.com/o/r/pull/n
  repo_owner text NOT NULL, repo_name text NOT NULL, pr_number bigint NOT NULL,
  title      text NOT NULL CHECK (octet_length(title) <= 500),
  state      text CHECK (state IN ('open','draft','merged','closed')),  -- NULL until GitHub sync exists
  linked_at  bigint NOT NULL,
  PRIMARY KEY (owner_id, issue_id, url),
  FOREIGN KEY (owner_id, issue_id) REFERENCES cloud_issues ON DELETE CASCADE
);
CREATE INDEX cloud_issue_prs_repo ON cloud_issue_pull_requests(owner_id, repo_owner, repo_name, pr_number);

-- Append-only activity: comments, questions, status changes, worker reports.
CREATE TABLE IF NOT EXISTS cloud_issue_activity (
  owner_id    text NOT NULL,
  activity_id text NOT NULL,                    -- the mutation / command id (today: command_id or event id)
  issue_id    text NOT NULL,
  actor_kind  text NOT NULL CHECK (actor_kind IN ('user','zapper','thread','system')),
  actor_user  text,                             -- WorkOS user id when known
  actor_ref   text,                             -- today's free-form `actor` (chat id, Zapper id)
  kind        text NOT NULL,                    -- created|updated|comment|question|closed|thread|pullRequest|workerReport|...
  body        text NOT NULL CHECK (octet_length(body) <= 65536),
  reply_to    text,                             -- one level deep, as today
  change_seq  bigint NOT NULL,
  created_at  bigint NOT NULL,
  PRIMARY KEY (owner_id, activity_id),
  FOREIGN KEY (owner_id, issue_id) REFERENCES cloud_issues ON DELETE CASCADE
);
CREATE INDEX cloud_issue_activity_issue ON cloud_issue_activity(owner_id, issue_id, created_at);
CREATE INDEX cloud_issue_activity_feed  ON cloud_issue_activity(owner_id, change_seq);

-- @mentions resolved to people, for the Mentioned filter.
CREATE TABLE IF NOT EXISTS cloud_issue_mentions (
  owner_id    text NOT NULL,
  issue_id    text NOT NULL,
  user_id     text NOT NULL,
  activity_id text NOT NULL DEFAULT '',         -- '' when the mention is in the description
  created_at  bigint NOT NULL,
  PRIMARY KEY (owner_id, issue_id, user_id, activity_id),
  FOREIGN KEY (owner_id, issue_id) REFERENCES cloud_issues ON DELETE CASCADE
);
CREATE INDEX cloud_issue_mentions_user ON cloud_issue_mentions(owner_id, user_id);

-- Mutation receipts: idempotent replays of the same mutation id.
CREATE TABLE IF NOT EXISTS cloud_issue_receipts (
  owner_id       text NOT NULL,
  mutation_id    text NOT NULL,
  request_digest text NOT NULL,
  result         jsonb NOT NULL,
  created_at     bigint NOT NULL,
  PRIMARY KEY (owner_id, mutation_id)
);
```

The same migration also does two things to account events:
- It adds `'issue'` to the `zuse_rust_account_event_outbox` kind `CHECK`.
- It attaches `zuse_rust_change_hint_outbox('issue','owner_id','issue_id')` triggers to `cloud_issues` and `cloud_issue_activity`. A burst inside one transaction collapses to one hint.

How the counters work:
- **Numbering.** A create allocates `number` and `change_seq` in its own transaction from `cloud_issue_heads`, with `UPDATE … SET next_number = next_number + 1 … RETURNING`. The update takes the row lock, so numbers per workspace have no gaps and no duplicates.
- **Workspace key.** `key` is created on first use. The default comes from the workspace name (three upper-case letters, `ISS` as a fallback) and can be renamed later. Renaming changes only how codes are displayed, because aliases store full codes.
- **Change feed.** Every mutation stamps the touched issue and its new activity rows with a fresh `next_change`, so a single cursor per workspace covers everything.

How today's protocol fields map onto the schema:

| `ZotIssue` field | Column |
|---|---|
| `id` | `slug`. `issue_id` is a new global id. |
| `number` | Kept as the alias `ZAP-<n>`. `number` is the new workspace number. |
| `details.owner` | `assignee_*`. `User` becomes the Zapper's person (`cloud_zappers.created_by`). |
| `details.relations` | `cloud_issue_relations` with `kind='related'` |
| `details.parent` | `parent_issue` |
| `threads` | `cloud_issue_threads` (keeps order) |
| `pull_requests` | `cloud_issue_pull_requests` |
| `activity[*]` | `cloud_issue_activity`, keeping `id`, `created_at`, `actor`, `kind`, `text` and `reply_to` verbatim |
| `assigned` | Derived from `assignee_kind IN ('zapper','thread')`. Not stored. |

## 5. Where the code lives in the cloud server

- `crates/cloud/src/issues.rs` (with `issues/` submodules if it grows) holds:
  - `IssueControl`, which owns the shared write pool, like the other controls;
  - store reads on the read-only pool.
- Routes go in `crates/cloud/src/routes/issues.rs`, merged into `router()` like `github_organizations::routes()`.
- The feature is gated by `ZUSE_CLOUD_ISSUES=true` and by `db::schema::present("cloud_issues")`, so the server runs fine before the migration is applied.
- Pure rules come from the shared module (slice 1) so validation matches the engine byte for byte.
- Access:
  - `AccessKind::Content` for everything; `admin` and `member` may read and write.
  - Renaming the workspace key needs `admin`.
  - Personal scope sees only its own `owner_id`.

## 6. API

All routes take `Authorization: Bearer` and `x-zuse-workspace`, and filter by `access.owner_id`. Bodies are camelCase JSON, like the rest of the API.

| Method and path | Purpose |
|---|---|
| `GET /v1/cloud/issues?view=forYou\|assigned\|created\|mentioned\|all&status=&zapper=&q=&cursor=` | List issues with what a row needs: code, title, status, priority, assignee, creator, age, and blocks/blockedBy codes. Paged. Not every client will use it (see the mirror), but it makes the API complete and testable. |
| `GET /v1/cloud/issues/{idOrCode}` | Full issue: the issue, its relations, threads, PRs, and the first page of activity. `{code}` accepts `KEY-n`. A legacy `ZAP-n` also needs `?zapper=`. |
| `GET /v1/cloud/issues/{id}/activity?before=` | Older activity pages. |
| `GET /v1/cloud/issues/changes?after=<seq>&limit=` | The change feed: issue snapshots and new activity rows with `change_seq > after`. Returns `{items, next, head}`. Used by engines to fill their mirrors. |
| `POST /v1/cloud/issues/mutations` | A batch of up to 100 mutations, applied in order. Each one is in its own transaction (details below). |
| `POST /v1/cloud/issues/import` | Migration (section 7): one Zapper's full `ZotIssue` list, idempotent by `(owner_id, zapper_id, slug)`. |
| `PUT /v1/cloud/zappers/{zapperId}` | The home engine registers or renames a Zapper and sets its avatar. `DELETE` marks it deleted. |
| `GET/PUT /v1/cloud/issues/settings` | The workspace key. |

How a mutation works:
- A mutation is `{mutationId, issueId, baseRevision?, op}`.
- Ops:
  - `create{slug?, zapperId?, fields}`;
  - `patch{fields}`, a field-level set produced by diffing `EditIssue` against the issue the engine saw;
  - `comment{text, replyTo?}`;
  - `ask{question, recommendation?}`, which sets the user as assignee and appends a question;
  - `close{outcome, note}`;
  - `linkThread` / `unlinkThread`, `linkPullRequest` / `unlinkPullRequest`;
  - `setRelations{blocks, blockedBy, related}`;
  - `report{kind, text, blockedHandBack}`, for worker reports.
- Each mutation returns one of:
  - `applied{issue, revision, number}`;
  - `duplicate` (same id and digest, so the stored result is replayed);
  - `conflict{current}`;
  - `rejected{code}`.

Concurrency follows today's `ConflictScope`:
- `patch`, `ask` and `close` carry `baseRevision`. They conflict only when that one issue changed since.
- Comments, links and reports commute and never conflict.

Realtime uses the existing account events socket. An engine asks for the `issue` hint kind and refetches the change feed when one arrives. Polling the feed every 60 s is the fallback.

## 7. Zapper commands, offline behaviour and the engine mirror

### 7.1 Local tables (engine SQLite, `zots/state.sqlite`)

- `issues_mirror`: one row per issue the engine can see in its workspace, holding the full snapshot as JSON, `revision` and `change_seq`.
- `issues_mirror_activity`: activity rows, indexed per issue.
- `issues_outbox`:
  - columns `mutation_id PK, issue_id, body, base_revision, phase(pending|sent|done|conflict|rejected), attempts, error`;
  - written in the same transaction as the local change, so the existing atomicity holds: a command either commits locally together with its outbox row, or not at all.
- `issues_cursor`: the change-feed cursor per workspace.
- `issues_home`: per Zapper `local | importing | cloud`, plus import digest and time (section 8).

### 7.2 Applying a Zapper command (createIssue, editIssue, closeIssue, askUser, commentIssue, link*, unlink*, delegate with an issueId)

1. `Zots::command_as` runs as today: receipt, revision check, then `issues::apply`.
   - The difference is that it reads and writes the issue in `issues_mirror`, not in `zot.issues`.
   - Every Zapper rule stays where it is: `transition`, worker scope, `hand_back`, notifications to `zot_events`.
2. In the same transaction it appends the matching cloud mutation to `issues_outbox`. The mutation id is the command id, so a retry after a lost response cannot apply twice.
3. A background pusher drains the outbox in order with `POST /mutations`. It backs off exponentially, waits for sign-in and wakes on network changes.
4. `applied` updates the mirror row with the cloud's number and revision.
   - `conflict` rebases: the engine refetches, re-runs the Zapper rule check against the fresh issue, and re-sends if the change still makes sense.
   - Otherwise it marks the row `conflict` and posts an `issueChanged` event to the Zapper ("your edit to ZUS-12 lost to a newer change: …"), so the coordinator sees it on its next turn.
   - `rejected` happens only if the two sides disagree on rules. The engine records it as a Zapper event and logs it (without the content).
5. The `Zot` that commands return and that `coordinator_view` builds still includes `issues`, projected from the mirror for that Zapper. The protocol, the prompts and today's inspector keep working unchanged while the slices land.

### 7.3 Offline or when the cloud is unreachable

- Every Zapper command still succeeds locally. The Zapper keeps planning, delegating and commenting, and the outbox queues the changes.
- An issue created offline has no number yet:
  - surfaces show `ZUS-…` (pending);
  - the coordinator view sends `code: null, pending: true`;
  - Zappers address issues by slug, so nothing waits on the number.
- The Issues tab reads the mirror, so it works offline with a "Changes waiting to sync" footer and a count.
- If the engine has never signed in, issues stay local only (`issues_home = local`), exactly as today. They are imported the first time the engine signs in.
- Changes made elsewhere (web, another desktop) arrive through the feed.
  - When such a change touches an issue owned by a Zapper whose home is this engine, the engine runs the same reactions the local path runs. For example, a person's comment on a `needsYou` issue triggers `hand_back` and an `issueChanged` event.
  - It sends those reactions back up as follow-up mutations, with ids derived from the triggering activity id so they stay idempotent.
  - The engine of the Zapper's home device is therefore the only place Zapper logic runs, and the cloud stays a store with tenancy and data rules.
- Worker threads on other devices keep sending commands to the home engine over peer RPC as today. They never talk to the issue API.

### 7.4 Issues created from the Issues tab

- They go through the engine's `IssueCommand` RPC. The engine applies them to the mirror optimistically, sends them through the same outbox and shows them once they are acknowledged.
- Assigning such an issue to a Zapper sets `zapper_id`. That Zapper's home engine sees it on the feed and posts an `issueChanged` event, and the Zapper takes it from there with its usual tools.

## 8. Migrating existing per-Zapper issues without loss

The import is engine-driven and runs once per Zapper. It is idempotent, verified, and reversible until the cleanup slice.

1. Preconditions:
   - the engine is signed in;
   - the cloud reports the `issues` capability;
   - `issues_home` for the Zapper is `local`.
2. `PUT /v1/cloud/zappers/{id}` registers the Zapper.
3. The engine sends `POST /v1/cloud/issues/import` with `{zapperId, issues: [ZotIssue…], digest}`. The server:
   - upserts each issue by `(owner_id, zapper_id, slug)`;
   - assigns workspace numbers in `(created_at, legacy number)` order;
   - writes alias `ZAP-<n>` for each issue;
   - copies threads (keeping order), PRs, `related` relations, parent, and every activity row with its original id, timestamp, actor and kind;
   - maps owner `User` to the Zapper's person;
   - keeps `created_at` and `updated_at`.
4. The server returns per-issue counts and a digest of what it stored. The engine compares them against the local blob: number of issues, activity rows per issue, threads, PRs, relations.
   - Any mismatch leaves the Zapper `local` and logs the difference (ids only). Nothing switches over.
5. On a match, the engine fills `issues_mirror`, sets `issues_home = cloud`, and from then on writes through the mirror and outbox.
   - Commands that arrive during the import wait on the Zapper's admission lock, which the import holds briefly. No change can fall between the snapshot and the switch.
6. The JSON blob's `issues` is **left in place, frozen**, for at least two releases.
   - Downgrading to an older build still shows the pre-switch issues.
   - A `zuse` support path can re-import or restore from it.
   - The cleanup slice removes it once the owner says so.

The import maps legacy ids like this:
- A Zapper's `relations` and `parent` refer to slugs. The import resolves them within the same Zapper, as `validate` does today.
- Codes cited in old text, such as "ZAP-3" in a comment, keep resolving through `cloud_issue_aliases` within that Zapper. Clients render the new code next to the old one.

## 9. Desktop and web Issues tab

### 9.1 Placement

- Desktop:
  - a new `Route::Issues` in `crates/ui/src/shell.rs:821`;
  - a rail icon in the bottom bar next to Pull Requests;
  - a tab title "Issues".
- Web: the same view, added to the browser shell's navigation.
- The page itself is a new `crates/viewport/src/issues_page.rs`, so native and browser share it. The existing Zapper inspector `IssueEvent::ShowIssues` (`viewport/src/zot_issues.rs:25`) routes there with that Zapper as a filter.
- The issue detail reuses the existing detail view in `zot_issues.rs`, fed from the mirror instead of `Zot.issues`.

### 9.2 Layout

The reference screenshot sets the look.

- Header: "Issues", with search and a new-issue button.
- Filter tabs:

  | Tab | Shows |
  |---|---|
  | For you | issues where the assignee is me or a Zapper I own, OR the creator is me or a Zapper I own (match any) |
  | Assigned to me | the assignee is me |
  | Created by me | the creator is me, or the person behind a Zapper or thread I own |
  | Mentioned | I have a row in `cloud_issue_mentions` |
  | All | the whole workspace |

  - "A Zapper I own" means `cloud_zappers.created_by = me`.
  - The Assigned to me tab shows a count.
- Filter chips:
  - an editable row under the tabs that shows what the tab means (Assignee is either of Me / My Zappers);
  - plus optional Zapper, status, priority and label chips;
  - "Clear" removes them.
- Groups:
  - collapsible sections in `ZotIssueStatus::BOARD` order: Needs you, In progress, In review, Blocked, Todo, Backlog, Done, Canceled;
  - each with a count;
  - Done and Canceled start collapsed;
  - empty open groups are hidden.
  - "Needs you" means `needsYou` with me as assignee. The same status with someone else as assignee groups as "Needs <name>" under the same header.
- Row:
  - status glyph, code (`ZUS-1`), title;
  - chips "Blocking N" and "Blocked by ZUS-1" (red when the blocker is open);
  - right side: priority bars, the owner's avatar (a person, a Zapper mascot, or a thread with its Zapper's avatar), and age (`2w`, from `created_at`).
  - A queued thread shows "Queued", as `issue_status_label` does today.
- Live updates: the mirror changes, the engine sends its usual state notification, and the page re-renders.

### 9.3 Engine RPCs

Both desktop and web use these.

- `IssuesQuery {view, filters, cursor}` → rows from the mirror. The filtering runs locally, so it is fast and works offline.
- `IssueGet {id|code}` → the detail. The engine fetches older activity pages on demand.
- `IssueCommand {mutation}`: for human edits. It is validated by the shared rules and goes through the mirror and outbox.
- `IssueMembers` → people in the workspace (name and avatar) for avatars, the assignee picker and @mention completion. The engine fetches them through the cloud (WorkOS org members). See open question 4.

## 10. Auth and per-user scoping

- Tenancy:
  - every cloud row has `owner_id`;
  - every query is `WHERE owner_id = access.owner_id`;
  - composite foreign keys `(owner_id, issue_id)` make cross-tenant links impossible at the schema level, following the `cloud_tasks` pattern.
- Personal workspace: only that user. Org workspace: any `admin` or `member` reads and writes every issue in the org, which matches how the shared Zappers board works today.
- Identity of actions:
  - `creator_user` and `actor_user` come from the authenticated principal (the engine's signed-in user), never from the request body;
  - `actor_kind` and `actor_ref` (Zapper id or thread chat id) come from the engine, which already attributes commands (`_zotActor`).
- Assigning a user checks that the user belongs to the workspace (WorkOS membership). Personal scope only allows the caller.
- Engines read only their own workspace's feed. A device that is signed out stops syncing and keeps its mirror. Signing into a different account clears the mirror for the old scope.
- Data rules are enforced again on the server, because a modified client can send anything. Zapper rules such as `transition` evidence are not enforced on the server: they guide the coordinator and are not security boundaries.
- The text of issues and comments becomes plaintext in Postgres, isolated per tenant, like project settings. Transcripts stay end-to-end encrypted. See open question 3.

## 11. Room for GitHub linking (not built here)

- `cloud_issue_pull_requests` already stores `repo_owner`, `repo_name`, `pr_number` and a nullable `state`.
- A later slice can reuse the existing GitHub integration (`crates/cloud/src/github`, `/v1/cloud/github/*`). It could:
  - fill `state` from webhooks;
  - auto-link PRs whose branch or body cites `KEY-n`;
  - suggest `close{merged}` when a linked PR merges.
- If issues ever mirror GitHub Issues, a future `cloud_issue_external(owner_id, issue_id, provider, external_id, url)` table is the place. Nothing in this design blocks it.

## 12. Risks

| Risk | Mitigation |
|---|---|
| The mirror and the cloud diverge after an outbox bug. | Mutation ids equal command ids, so retries are idempotent. Use per-issue revisions. A periodic full-hash check per Zapper (`GET /changes?after=0` digest) repairs the mirror from the cloud. Stay in shadow mode (dual write, read the blob) for one slice before reads switch. |
| Zapper rules run on the home engine while humans edit in the cloud, so reactions are delayed while that engine is offline. | Today the issue is invisible while that engine is offline, so this is strictly better. The reaction runs when the engine returns, idempotent per activity id. |
| Migration loses data. | Verified import with digests. The blob stays frozen, not deleted, and its removal is a separate, owner-approved slice. |
| Migration numbers collide with other sessions (billing adds migrations). | Pick the number at merge time, and add the file to `database.py` `FILES` in the same PR. |
| WorkOS membership is called on every request, which adds latency to list and feed calls. | Engines read from the mirror, so the API sees mostly small feed and mutation calls. Add a short membership cache in `Identity` if needed (separate change). |
| The mirror grows large in big workspaces. | The mirror keeps all issue rows, but activity only for open issues plus the last N per closed issue. Older activity pages load on demand. |
| Codes change (`ZAP-n` → `KEY-n`). | Aliases. Both codes are shown during the transition. Zapper prompts get both, so old references resolve. |
| Plaintext issue content in the cloud. | Owner decision (open question 3). Tenant isolation and the existing database access controls apply. |
| Parallel threads edit `zots.rs` and `zots/issues.rs`. | Slices keep the engine change set small and rebase often. |

## 13. Open questions for the owner

1. **Numbering.** One number per workspace with a workspace key (`ZUS-12`), old `ZAP-n` kept as aliases? (Recommended.) Or keep numbering per Zapper, with one key per Zapper?
2. **Web reads without an engine.** Is web through an engine (home or managed connection) enough for now? (Recommended, and it matches today's architecture.) Or should the browser call `GET /v1/cloud/issues` directly, which needs a browser-held token and CORS?
3. **Plaintext content.** Is plaintext issue text in Postgres acceptable? (Needed for search and GitHub linking.) Or should bodies be encrypted like transcripts?
4. **People list.** For avatars, the assignee picker and @mentions, is a cloud endpoint that lists WorkOS org members acceptable? Personal workspaces have only the user and their Zappers.
5. **Org visibility.** Can every member of an org see every issue in it? Or should issues of a Zapper be visible only to its person and to the people it mentions?
6. **Deleting a Zapper.** Should its issues stay in the workspace with the Zapper shown as deleted (recommended), or be deleted along with it?
7. **Relations.** Today's untyped relations become `related`. Should the Zapper command schema gain `blocks` / `blockedBy` in `details` now (slice 6), so the chips have data from day one?

## 14. Build plan: one PR per slice

Each slice gets its own child issue, keeps tests green, and ships behind a flag until slice 7.

| # | Slice | Contents | Verifies |
|---|---|---|---|
| 1 | Shared issue rules | Move the pure checks (`validate` size and format rules, `canonical_pr`, `normalize`, status and priority enums) into a module both engine and cloud depend on (`zuse-protocol::issues`, or a small `zuse-issues` crate). No behaviour change. | The existing engine issue tests, plus unit tests of the module. |
| 2 | Cloud schema and API | `0055_issues.sql` plus `database.py` `FILES` and grants. `IssueControl`, `routes/issues.rs`: mutations, changes, import, zappers, settings, list and get. Gated by `ZUSE_CLOUD_ISSUES` and schema presence. No clients yet. | `crates/cloud/tests` Postgres cases: tenancy isolation, numbering under concurrency, idempotent replay, conflict, import round-trip with full activity, `infra/cloud/tests/database.py`. |
| 3 | Issue change hints | `'issue'` account event kind (migration on the outbox `CHECK` and triggers), and the engine opt-in in `cloud/events.rs`. | Postgres test that a mutation emits one hint, and an engine events test. |
| 4 | Engine mirror and outbox (shadow) | `issues_mirror`, `issues_outbox`, the pusher and the feed puller. Zapper commands dual-write: the blob is still read, and the outbox is filled. The Zapper registry is pushed. Behind an engine flag. | Engine tests with a fake cloud: offline queue then drain, lost response retry, conflict rebase, reactions to remote comments. |
| 5 | Import and read switch | Per-Zapper verified import and `issues_home` states. `zot.issues` is projected from the mirror. The blob is frozen. | Tests: import of a fixture blob is lossless (counts and digests), a mismatch stays local, downgrade-safe. |
| 6 | Model additions | Typed relations (`blocks`, `blockedBy`), user assignees, workspace codes, and pending-number handling in the protocol and coordinator view. Backward-compatible serde. | `cargo test -p zuse-protocol`, engine prompt tests. |
| 7 | Desktop Issues tab | `IssuesQuery`, `IssueGet`, `IssueCommand` and `IssueMembers` RPCs. `viewport/src/issues_page.rs`. `Route::Issues` and the rail entry. The inspector's "View all" routes there. | `zuse-ui` tests and a `crates/ui/examples/issues-page-fixture.rs` with screenshots on a `-screenshots` branch. |
| 8 | Web Issues tab and Mentioned | Wire the page into `apps/web` navigation. @mention parsing writes `cloud_issue_mentions`. Mentioned filter. | Viewport tests and web screenshots. |
| 9 | Cleanup (owner-gated) | Remove the frozen blob `issues` and the projection shims once two releases have shipped. | Engine tests. |
| — | Future (separate issue) | GitHub PR state sync and auto-linking (section 11). | — |

Slices 1 to 3 do not depend on each other's code and can be reviewed in parallel. Slices 4 and 5 are the risky ones and should land one at a time.
