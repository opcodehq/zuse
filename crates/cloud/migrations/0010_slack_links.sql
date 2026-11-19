-- Zuse chats linked to Slack threads. Applied after 0009_slack.sql.
-- A link belongs to one Zuse account and actor; uninstalling the Slack app
-- removes its links, mirrored-reply receipts and queued thread replies.
CREATE TABLE IF NOT EXISTS zuse_rust_slack_links (
    link_id text PRIMARY KEY,
    account_id text NOT NULL,
    actor_id text NOT NULL,
    chat_key text NOT NULL,
    team_id text NOT NULL REFERENCES zuse_rust_slack_installs(team_id) ON DELETE CASCADE,
    channel_id text NOT NULL,
    channel_name text NOT NULL,
    thread_ts text NOT NULL,
    sync boolean NOT NULL DEFAULT true,
    created_at bigint NOT NULL,
    UNIQUE(account_id,actor_id,chat_key),
    UNIQUE(team_id,channel_id,thread_ts)
);
-- One row per mirrored reply; `ts` is NULL while the post is in flight.
CREATE TABLE IF NOT EXISTS zuse_rust_slack_link_posts (
    link_id text NOT NULL REFERENCES zuse_rust_slack_links(link_id) ON DELETE CASCADE,
    message_id text NOT NULL,
    ts text,
    created_at bigint NOT NULL,
    PRIMARY KEY(link_id,message_id)
);
-- Thread replies from the linked Slack member, sealed, waiting for the engine.
-- A mention fires both `app_mention` and `message`; the message timestamp
-- deduplicates them, the event ID deduplicates Slack retries.
CREATE TABLE IF NOT EXISTS zuse_rust_slack_inbox (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    link_id text NOT NULL REFERENCES zuse_rust_slack_links(link_id) ON DELETE CASCADE,
    event_id text NOT NULL,
    message_ts text NOT NULL,
    payload text NOT NULL,
    created_at bigint NOT NULL,
    UNIQUE(link_id,event_id),
    UNIQUE(link_id,message_ts)
);
CREATE INDEX IF NOT EXISTS zuse_rust_slack_inbox_created ON zuse_rust_slack_inbox(created_at);
