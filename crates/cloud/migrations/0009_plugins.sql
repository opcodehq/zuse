-- Managed plugins. Credentials, custom server definitions and OAuth attempt
-- material are sealed with the plugin key and bound to their tenant and row.
CREATE TABLE IF NOT EXISTS zuse_plugin_servers (
 tenant_id text NOT NULL,
 server_id text NOT NULL CHECK(server_id ~ '^custom-[0-9a-f]{32}$'),
 sealed text NOT NULL,
 created_at bigint NOT NULL,
 PRIMARY KEY(tenant_id,server_id)
);
CREATE TABLE IF NOT EXISTS zuse_plugin_connections (
 tenant_id text NOT NULL,
 connection_id text NOT NULL CHECK(connection_id ~ '^c[0-9a-f]{32}$'),
 subject text NOT NULL,
 plugin_id text NOT NULL,
 label text NOT NULL,
 owner text NOT NULL CHECK(owner IN ('user','organization')),
 state text NOT NULL CHECK(state IN ('connecting','connected','needs-auth','error')),
 enabled boolean NOT NULL DEFAULT true,
 auth text CHECK(auth IN ('none','oauth','header')),
 credential text,
 credential_revision bigint NOT NULL DEFAULT 0,
 created_at bigint NOT NULL,
 updated_at bigint NOT NULL,
 PRIMARY KEY(tenant_id,connection_id)
);
CREATE INDEX IF NOT EXISTS zuse_plugin_connections_subject ON zuse_plugin_connections(tenant_id,subject);
CREATE TABLE IF NOT EXISTS zuse_plugin_attempts (
 tenant_id text NOT NULL,
 attempt_id text NOT NULL CHECK(attempt_id ~ '^c[0-9a-f]{32}$'),
 subject text NOT NULL,
 plugin_id text NOT NULL,
 state text NOT NULL CHECK(state IN ('pending','connected','failed','cancelled')),
 error text,
 authorization_url text,
 sealed text NOT NULL,
 state_digest text UNIQUE,
 ticket_digest text UNIQUE,
 expires_at bigint NOT NULL,
 created_at bigint NOT NULL,
 updated_at bigint NOT NULL,
 PRIMARY KEY(tenant_id,attempt_id)
);
CREATE INDEX IF NOT EXISTS zuse_plugin_attempts_expiry ON zuse_plugin_attempts(expires_at);

-- SDK schema migrations are owned by `packages/executor-v2` and applied explicitly.
CREATE TABLE IF NOT EXISTS zuse_executor_state (id text PRIMARY KEY, sealed text NOT NULL);
ALTER TABLE zuse_plugin_connections ADD COLUMN IF NOT EXISTS executor_ref text;
ALTER TABLE zuse_plugin_connections ADD COLUMN IF NOT EXISTS definition_hash text;

-- The undeployed native prototype stored a different credential model. Never use
-- those grants against an SDK deployment or a newly resolved catalog endpoint.
UPDATE zuse_plugin_connections SET state='needs-auth'
 WHERE state='connected' AND executor_ref IS NULL;
