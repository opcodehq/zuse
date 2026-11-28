-- The folder a chat is synced into on one of the owner's computers, reported
-- by that computer with its own key when it claims the chat. Commands the
-- chat runs there start in it unless the agent names another directory.
-- Additive: NULL (not synced) for every existing row.
ALTER TABLE api_bridge_attachments ADD COLUMN IF NOT EXISTS synced_cwd text
    CHECK (synced_cwd IS NULL OR length(synced_cwd) BETWEEN 1 AND 4096);

DO $grants$
DECLARE app_role text := CASE current_schema()
    WHEN 'zuse_staging' THEN 'zuse_cloud_staging'
    WHEN 'zuse_production' THEN 'zuse_cloud_production' END;
BEGIN
    IF app_role IS NOT NULL AND EXISTS (SELECT FROM pg_roles WHERE rolname = app_role) THEN
        EXECUTE format('GRANT UPDATE(synced_cwd) ON api_bridge_attachments TO %I', app_role);
    END IF;
END $grants$;
