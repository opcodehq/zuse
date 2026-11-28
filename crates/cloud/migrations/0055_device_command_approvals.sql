-- Enrollment IDs identify a single incarnation of a connector. Re-enrollment
-- uses a new ID so a remembered decision cannot transfer to another host.
CREATE TABLE IF NOT EXISTS api_bridge_computers (
    owner_id text NOT NULL,
    enrollment_id text NOT NULL,
    key_thumbprint text NOT NULL,
    label text NOT NULL,
    platform text NOT NULL CHECK (platform IN ('macos', 'linux', 'windows')),
    PRIMARY KEY (owner_id, enrollment_id),
    UNIQUE (owner_id, key_thumbprint)
);

CREATE TABLE IF NOT EXISTS api_bridge_attachments (
    owner_id text NOT NULL,
    workspace_id text NOT NULL,
    chat_id text NOT NULL,
    enrollment_id text NOT NULL,
    -- False for the row a primary claim adds: it holds the device's requests
    -- but doesn't make it a secondary the agent may target.
    explicit boolean NOT NULL DEFAULT true,
    PRIMARY KEY (owner_id, workspace_id, chat_id, enrollment_id),
    FOREIGN KEY (owner_id, enrollment_id) REFERENCES api_bridge_computers ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, chat_id) REFERENCES zuse_rust_task_command_sessions ON DELETE CASCADE
);
-- The computer the owner last sent a message from, proven by its own key.
CREATE TABLE IF NOT EXISTS api_bridge_primaries (
    owner_id text NOT NULL,
    workspace_id text NOT NULL,
    chat_id text NOT NULL,
    enrollment_id text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (owner_id, workspace_id, chat_id),
    FOREIGN KEY (owner_id, workspace_id, chat_id, enrollment_id)
        REFERENCES api_bridge_attachments ON DELETE CASCADE
);
-- Admission time (ms) of the message that set it; a late claim for an older
-- message never replaces a newer one. 0 for a new chat's first claim.
ALTER TABLE api_bridge_primaries ADD COLUMN IF NOT EXISTS ordered_at bigint NOT NULL DEFAULT 0;
-- A cleared default keeps its row (no computer) so its ordering still holds.
ALTER TABLE api_bridge_primaries ALTER COLUMN enrollment_id DROP NOT NULL;
CREATE TABLE IF NOT EXISTS api_bridge_requests (
    request_id uuid PRIMARY KEY,
    owner_id text NOT NULL,
    workspace_id text NOT NULL,
    chat_id text NOT NULL,
    enrollment_id text NOT NULL,
    computer_name text NOT NULL,
    os_user text NOT NULL,
    shell text NOT NULL,
    cwd text NOT NULL,
    command text NOT NULL,
    -- Per-request deadline, bounded to one hour.
    timeout_ms integer NOT NULL CHECK (timeout_ms BETWEEN 1000 AND 3600000),
    decision text CHECK (decision IN ('once', 'session', 'always', 'deny')),
    -- The device's role in the chat when the request was made.
    role text NOT NULL DEFAULT 'secondary' CHECK (role IN ('primary', 'secondary')),
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (owner_id, workspace_id, chat_id, enrollment_id)
        REFERENCES api_bridge_attachments ON DELETE CASCADE
);
-- Every request ID ever submitted in a chat. Detaching deletes a request's
-- rows, but never this record, so a resubmitted ID can't run again.
CREATE TABLE IF NOT EXISTS api_bridge_request_ids (
    request_id uuid PRIMARY KEY,
    owner_id text NOT NULL,
    workspace_id text NOT NULL,
    chat_id text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (workspace_id, chat_id) REFERENCES zuse_rust_task_command_sessions ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS api_bridge_requests_chat
    ON api_bridge_requests(owner_id, workspace_id, chat_id, created_at DESC);
CREATE TABLE IF NOT EXISTS api_bridge_session_grants (
    owner_id text NOT NULL,
    workspace_id text NOT NULL,
    chat_id text NOT NULL,
    enrollment_id text NOT NULL,
    PRIMARY KEY (owner_id, workspace_id, chat_id, enrollment_id),
    FOREIGN KEY (owner_id, workspace_id, chat_id, enrollment_id)
        REFERENCES api_bridge_attachments ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS api_bridge_always_grants (
    owner_id text NOT NULL,
    enrollment_id text NOT NULL,
    PRIMARY KEY (owner_id, enrollment_id),
    FOREIGN KEY (owner_id, enrollment_id) REFERENCES api_bridge_computers ON DELETE CASCADE
);

CREATE OR REPLACE FUNCTION zuse_clear_archived_bridge_attachments() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='api_cloud_workspaces' THEN
        DELETE FROM api_bridge_attachments WHERE workspace_id=NEW.workspace_id;
    ELSE
        DELETE FROM api_bridge_attachments
        WHERE workspace_id=NEW.workspace_id AND chat_id=NEW.session_id;
    END IF;
    RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS bridge_chat_archived ON zuse_rust_task_command_sessions;
CREATE TRIGGER bridge_chat_archived AFTER UPDATE OF state ON zuse_rust_task_command_sessions
FOR EACH ROW WHEN (NEW.state='archived' AND OLD.state <> NEW.state)
EXECUTE FUNCTION zuse_clear_archived_bridge_attachments();

DROP TRIGGER IF EXISTS bridge_workspace_closed ON api_cloud_workspaces;
CREATE TRIGGER bridge_workspace_closed AFTER UPDATE OF desired_state ON api_cloud_workspaces
FOR EACH ROW WHEN (NEW.desired_state IN ('archived','deleted') AND OLD.desired_state <> NEW.desired_state)
EXECUTE FUNCTION zuse_clear_archived_bridge_attachments();

CREATE TABLE IF NOT EXISTS api_bridge_tasks (
    task_id text PRIMARY KEY, owner_id text NOT NULL, enrollment_id text NOT NULL,
    dynamic boolean NOT NULL DEFAULT false, command text NOT NULL, cwd text NOT NULL, os_user text NOT NULL, expires_at timestamptz NOT NULL,
    FOREIGN KEY(owner_id,enrollment_id) REFERENCES api_bridge_computers ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS api_bridge_runs (
    request_id uuid PRIMARY KEY REFERENCES api_bridge_requests ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    task_id text NOT NULL, claimed boolean NOT NULL DEFAULT false, claimed_at timestamptz,
    -- The host's last report while running; silence past the unknown-outcome
    -- window shows "Outcome unknown" and the request is never re-run.
    heartbeat_at timestamptz,
    progress text, result jsonb
);

-- Single-use connector proofs (replay protection), kept five minutes.
CREATE TABLE IF NOT EXISTS api_bridge_proofs (
    thumbprint text NOT NULL,
    jti text NOT NULL,
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (thumbprint, jti)
);
CREATE INDEX IF NOT EXISTS api_bridge_proofs_expiry ON api_bridge_proofs(expires_at);

DO $grants$
DECLARE app_role text := CASE current_schema()
    WHEN 'zuse_staging' THEN 'zuse_cloud_staging'
    WHEN 'zuse_production' THEN 'zuse_cloud_production' END;
BEGIN
    IF app_role IS NOT NULL AND EXISTS (SELECT FROM pg_roles WHERE rolname = app_role) THEN
        EXECUTE format('GRANT SELECT, INSERT, DELETE ON api_bridge_attachments, api_bridge_session_grants, api_bridge_always_grants, api_bridge_requests TO %I', app_role);
        EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON api_bridge_computers, api_bridge_tasks, api_bridge_runs, api_bridge_primaries TO %I', app_role);
        EXECUTE format('GRANT SELECT, INSERT, DELETE ON api_bridge_proofs TO %I', app_role);
        EXECUTE format('GRANT SELECT, INSERT ON api_bridge_request_ids TO %I', app_role);
        EXECUTE format('GRANT UPDATE(decision) ON api_bridge_requests TO %I', app_role);
        EXECUTE format('GRANT UPDATE(explicit) ON api_bridge_attachments TO %I', app_role);
    END IF;
END $grants$;
