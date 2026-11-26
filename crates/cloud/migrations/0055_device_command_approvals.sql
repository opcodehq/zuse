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
    PRIMARY KEY (owner_id, workspace_id, chat_id, enrollment_id),
    FOREIGN KEY (owner_id, enrollment_id) REFERENCES api_bridge_computers ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, chat_id) REFERENCES zuse_rust_task_command_sessions ON DELETE CASCADE
);
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
    timeout_ms integer NOT NULL CHECK (timeout_ms > 0),
    decision text CHECK (decision IN ('once', 'session', 'always', 'deny')),
    FOREIGN KEY (owner_id, workspace_id, chat_id, enrollment_id)
        REFERENCES api_bridge_attachments ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS api_bridge_requests_pending
    ON api_bridge_requests(owner_id, workspace_id, chat_id) WHERE decision IS NULL;
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
    task_id text NOT NULL, claimed boolean NOT NULL DEFAULT false, claimed_at timestamptz, progress text, result jsonb
);

DO $grants$
DECLARE app_role text := CASE current_schema()
    WHEN 'zuse_staging' THEN 'zuse_cloud_staging'
    WHEN 'zuse_production' THEN 'zuse_cloud_production' END;
BEGIN
    IF app_role IS NOT NULL AND EXISTS (SELECT FROM pg_roles WHERE rolname = app_role) THEN
        EXECUTE format('GRANT SELECT, INSERT, DELETE ON api_bridge_attachments, api_bridge_session_grants, api_bridge_always_grants, api_bridge_requests TO %I', app_role);
        EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON api_bridge_computers, api_bridge_tasks, api_bridge_runs TO %I', app_role);
        EXECUTE format('GRANT UPDATE(decision) ON api_bridge_requests TO %I', app_role);
    END IF;
END $grants$;
