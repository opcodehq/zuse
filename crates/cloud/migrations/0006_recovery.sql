-- A verified barrier is durable before the exact source machine can be retired.
CREATE TABLE IF NOT EXISTS zuse_rust_recovery_operations (
 operation_id text PRIMARY KEY,
 workspace_id text NOT NULL,
 generation bigint NOT NULL CHECK(generation>0),
 source_machine_id text NOT NULL,
 storage_incarnation_id text NOT NULL,
 binding jsonb NOT NULL,
 source_credential_hash text,
 phase text NOT NULL CHECK(phase IN ('uploading','verifying','retiring','retired','restoring','restored','cancelled')),
 manifest jsonb,
 expires_at bigint NOT NULL,
 verified_at bigint,
 verified_archive_sha256 text,
 running_since bigint,
 helper_stopped boolean NOT NULL DEFAULT false,
 created_at bigint NOT NULL,
 updated_at bigint NOT NULL,
 CHECK ((verified_at IS NULL)=(verified_archive_sha256 IS NULL)),
 CHECK (phase NOT IN ('retiring','retired','restoring','restored') OR (verified_at IS NOT NULL AND manifest IS NOT NULL))
);
CREATE UNIQUE INDEX IF NOT EXISTS zuse_rust_recovery_active ON zuse_rust_recovery_operations(workspace_id) WHERE phase NOT IN ('cancelled','restored','retired');
CREATE INDEX IF NOT EXISTS zuse_rust_recovery_workspace ON zuse_rust_recovery_operations(workspace_id,created_at);

-- Original telemetry tables; no new billing owner or settlement stream.
CREATE TABLE IF NOT EXISTS api_cloud_runtime_observations (
 provider text NOT NULL,provider_sandbox_id text NOT NULL,observation jsonb NOT NULL,
 PRIMARY KEY(provider,provider_sandbox_id)
);
CREATE TABLE IF NOT EXISTS api_cloud_usage_outbox (
 event_id text PRIMARY KEY,payload jsonb NOT NULL,created_at bigint NOT NULL,
 next_attempt_at bigint NOT NULL,attempt_count bigint NOT NULL DEFAULT 0,
 acknowledged_at bigint,last_error text
);
CREATE INDEX IF NOT EXISTS api_cloud_usage_outbox_pending_idx ON api_cloud_usage_outbox(next_attempt_at,event_id) WHERE acknowledged_at IS NULL;
