-- Pull requests owned by chats, and the events waiting for those chats.
--
-- Each pull request has at most one owning chat per Zuse account: the chat
-- that last opened or pushed to it, or that a person handed it to. Only the
-- owner receives its events. The GitHub webhook writes inbox rows using only
-- the database; the engine that hosts the chat reads them when the
-- 'pull-request' account hint tells it to, delivers them, and acknowledges them.
-- Pending events follow the pull request to a new owner.
CREATE TABLE IF NOT EXISTS zuse_rust_pull_request_owners (
    account_id text NOT NULL,
    -- GitHub's numeric repository id survives renames and transfers.
    repository_id bigint NOT NULL CHECK(repository_id>0),
    number bigint NOT NULL CHECK(number>0),
    -- The installation that covered the repository at claim time; webhooks
    -- from any other installation never touch this row.
    installation_id bigint NOT NULL CHECK(installation_id>0),
    repository text NOT NULL CHECK(length(repository) BETWEEN 3 AND 201),
    actor_id text NOT NULL,
    chat_key text NOT NULL CHECK(length(chat_key) BETWEEN 6 AND 205),
    claimed_by text NOT NULL CHECK(claimed_by IN ('created','pushed','manual','branch')),
    claimed_at bigint NOT NULL,
    url text NOT NULL CHECK(length(url)<=512),
    title text NOT NULL DEFAULT '' CHECK(length(title)<=512),
    state text NOT NULL CHECK(state IN ('open','closed','merged')),
    draft boolean NOT NULL DEFAULT false,
    head_ref text CHECK(length(head_ref)<=255),
    head_sha text CHECK(length(head_sha)<=64),
    base_ref text CHECK(length(base_ref)<=255),
    -- The cloud computer hosting the chat when it is a cloud chat: the cloud
    -- delivers its events itself. NULL for chats on a desktop engine.
    workspace_id text CHECK(length(workspace_id)<=200),
    -- A merge-conflict check is due (after a push to the branch or its base),
    -- how often it found GitHub still computing, and the head:base pair last
    -- reported as conflicting.
    conflict_due_at bigint,
    conflict_attempts integer NOT NULL DEFAULT 0,
    conflict_sha text CHECK(length(conflict_sha)<=130),
    -- The last check result delivered, and the commit it was for: a pass is
    -- only worth delivering after a failure.
    checks text CHECK(checks IN ('failed','passed')),
    checks_sha text CHECK(length(checks_sha)<=64),
    policy jsonb NOT NULL DEFAULT '{}' CHECK(octet_length(policy::text)<=2048),
    updated_at bigint NOT NULL,
    PRIMARY KEY(account_id,repository_id,number)
);
CREATE INDEX IF NOT EXISTS zuse_rust_pull_request_owners_chat
 ON zuse_rust_pull_request_owners(account_id,actor_id,chat_key);
-- Webhook lookups: by pull request, and by head commit for check results
-- (GitHub leaves `pull_requests` empty on checks of fork pull requests).
CREATE INDEX IF NOT EXISTS zuse_rust_pull_request_owners_number
 ON zuse_rust_pull_request_owners(repository_id,number);
CREATE INDEX IF NOT EXISTS zuse_rust_pull_request_owners_head
 ON zuse_rust_pull_request_owners(repository_id,head_sha) WHERE state='open';
CREATE INDEX IF NOT EXISTS zuse_rust_pull_request_owners_base
 ON zuse_rust_pull_request_owners(repository_id,base_ref) WHERE state='open';
CREATE INDEX IF NOT EXISTS zuse_rust_pull_request_owners_conflict_due
 ON zuse_rust_pull_request_owners(conflict_due_at) WHERE conflict_due_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS zuse_rust_pull_request_owners_hosted
 ON zuse_rust_pull_request_owners(workspace_id) WHERE workspace_id IS NOT NULL;

-- Sealed event payloads. The delivery id plus a per-event suffix
-- deduplicates GitHub retries.
CREATE TABLE IF NOT EXISTS zuse_rust_pull_request_inbox (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    account_id text NOT NULL,
    repository_id bigint NOT NULL,
    number bigint NOT NULL,
    event_id text NOT NULL CHECK(length(event_id)<=256),
    payload text NOT NULL,
    created_at bigint NOT NULL,
    FOREIGN KEY(account_id,repository_id,number)
     REFERENCES zuse_rust_pull_request_owners(account_id,repository_id,number) ON DELETE CASCADE,
    UNIQUE(account_id,event_id)
);
CREATE INDEX IF NOT EXISTS zuse_rust_pull_request_inbox_owner
 ON zuse_rust_pull_request_inbox(account_id,repository_id,number);
CREATE INDEX IF NOT EXISTS zuse_rust_pull_request_inbox_created
 ON zuse_rust_pull_request_inbox(created_at);

-- `@zuse` on a pull request no chat of the writer's owns. The writer's
-- engine starts a chat for it in a project with that repository.
CREATE TABLE IF NOT EXISTS zuse_rust_pull_request_mentions (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    account_id text NOT NULL,
    actor_id text NOT NULL,
    repository_id bigint NOT NULL,
    number bigint NOT NULL,
    installation_id bigint NOT NULL,
    event_id text NOT NULL CHECK(length(event_id)<=256),
    payload text NOT NULL,
    created_at bigint NOT NULL,
    UNIQUE(account_id,event_id)
);
CREATE INDEX IF NOT EXISTS zuse_rust_pull_request_mentions_actor
 ON zuse_rust_pull_request_mentions(account_id,actor_id);
CREATE INDEX IF NOT EXISTS zuse_rust_pull_request_mentions_created
 ON zuse_rust_pull_request_mentions(created_at);

-- 'pull-request' change hints. Like the other hints they carry only the chat
-- key, never event content; apps refetch through the authorized reads. The
-- API only forwards this kind to sockets whose ticket asks for it.
ALTER TABLE zuse_rust_account_event_outbox
 DROP CONSTRAINT zuse_rust_account_event_outbox_kind_check;
ALTER TABLE zuse_rust_account_event_outbox
 ADD CONSTRAINT zuse_rust_account_event_outbox_kind_check
 CHECK(kind IN ('workspace','conversation','connection','authentication','github','environment','task-command-queue','task-command-progress','machine-image','billing','computer-request','pull-request'));

-- A new owner, a release, or a change to what the chat shows (title, state,
-- checks, event settings). Head commit updates from pushes stay silent.
CREATE TRIGGER zuse_rust_pull_request_owner_hints
AFTER INSERT OR DELETE ON zuse_rust_pull_request_owners
FOR EACH ROW EXECUTE FUNCTION zuse_rust_change_hint_outbox('pull-request','account_id','chat_key');
CREATE TRIGGER zuse_rust_pull_request_owner_update_hints
AFTER UPDATE ON zuse_rust_pull_request_owners
FOR EACH ROW WHEN ((NEW.chat_key,NEW.title,NEW.state,NEW.draft,NEW.checks,NEW.policy,NEW.head_ref)
    IS DISTINCT FROM (OLD.chat_key,OLD.title,OLD.state,OLD.draft,OLD.checks,OLD.policy,OLD.head_ref))
EXECUTE FUNCTION zuse_rust_change_hint_outbox('pull-request','account_id','chat_key');
-- New events and mentions: the account's engines read them at once.
CREATE TRIGGER zuse_rust_pull_request_inbox_hints
AFTER INSERT ON zuse_rust_pull_request_inbox
FOR EACH ROW EXECUTE FUNCTION zuse_rust_change_hint_outbox('pull-request','account_id','account_id');
CREATE TRIGGER zuse_rust_pull_request_mention_hints
AFTER INSERT ON zuse_rust_pull_request_mentions
FOR EACH ROW EXECUTE FUNCTION zuse_rust_change_hint_outbox('pull-request','account_id','actor_id');
