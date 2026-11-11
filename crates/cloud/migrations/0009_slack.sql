-- Explicitly opt-in Rust Slack ownership. Does not adopt existing installations.
CREATE TABLE IF NOT EXISTS zuse_rust_slack_installs (
    team_id text PRIMARY KEY,
    team_name text NOT NULL,
    bot_user_id text NOT NULL,
    credentials text NOT NULL,
    generation text NOT NULL
);
CREATE TABLE IF NOT EXISTS zuse_rust_slack_members (
    team_id text NOT NULL REFERENCES zuse_rust_slack_installs(team_id),
    user_id text NOT NULL,
    account_id text NOT NULL,
    actor_id text NOT NULL,
    organization_id text,
    settings jsonb NOT NULL,
    revision text NOT NULL,
    PRIMARY KEY(team_id,user_id)
);
-- A Slack sign-in waits here until the Zuse account that started it confirms
-- the Slack user and workspace from inside Zuse. Only then is a member linked.
CREATE TABLE IF NOT EXISTS zuse_rust_slack_pending (
    pending_id text PRIMARY KEY,
    team_id text NOT NULL REFERENCES zuse_rust_slack_installs(team_id) ON DELETE CASCADE,
    user_id text NOT NULL,
    user_name text,
    account_id text NOT NULL,
    actor_id text NOT NULL,
    payload text NOT NULL,
    expires_at bigint NOT NULL
);
CREATE INDEX IF NOT EXISTS zuse_rust_slack_pending_owner ON zuse_rust_slack_pending(account_id,actor_id);
CREATE TABLE IF NOT EXISTS zuse_rust_slack_oauth (
    state_hash text PRIMARY KEY,
    browser_hash text NOT NULL,
    authorized boolean NOT NULL DEFAULT false,
    payload text NOT NULL,
    expires_at bigint NOT NULL
);
CREATE TABLE IF NOT EXISTS zuse_rust_slack_jobs (
    sequence bigint GENERATED ALWAYS AS IDENTITY,
    channel_id text NOT NULL,
    message_ts text NOT NULL,
    thread_ts text NOT NULL,
    team_id text NOT NULL,
    event_id text NOT NULL,
    payload text NOT NULL,
    phase text NOT NULL DEFAULT 'queued',
    attempts integer NOT NULL DEFAULT 0,
    next_at bigint NOT NULL,
    created_at bigint NOT NULL,
    PRIMARY KEY(team_id,event_id),
    UNIQUE(team_id,channel_id,message_ts)
);
CREATE INDEX IF NOT EXISTS zuse_rust_slack_jobs_due ON zuse_rust_slack_jobs(next_at,sequence) WHERE phase NOT IN ('done','dead');
CREATE INDEX IF NOT EXISTS zuse_rust_slack_jobs_admission ON zuse_rust_slack_jobs(team_id,channel_id,thread_ts,sequence) WHERE phase IN ('queued','context','dispatch','enqueue');
CREATE TABLE IF NOT EXISTS zuse_rust_slack_threads (
    team_id text NOT NULL,
    channel_id text NOT NULL,
    thread_ts text NOT NULL,
    account_id text NOT NULL,
    actor_id text NOT NULL,
    workspace_id text,
    session_id text,
    creation_request text,
    PRIMARY KEY(team_id,channel_id,thread_ts),
    CHECK ((workspace_id IS NOT NULL AND session_id IS NOT NULL) OR creation_request IS NOT NULL)
);
