-- Original deletion dates plus Rust-owned durable cleanup. No subscription/billing ownership changes.
ALTER TABLE api_cloud_workspaces ADD COLUMN IF NOT EXISTS deleted_at bigint;
ALTER TABLE api_cloud_workspaces ADD COLUMN IF NOT EXISTS deletion_tombstone_expires_at bigint;
CREATE TABLE IF NOT EXISTS zuse_rust_deletions (
 workspace_id text PRIMARY KEY,
 account_id text NOT NULL,
 operation_id text NOT NULL UNIQUE,
 command_digest text NOT NULL,
 idempotency_digest text,
 generation bigint NOT NULL CHECK(generation>0),
 phase text NOT NULL CHECK(phase IN ('providers','objects','metadata','deleted')),
 targets jsonb NOT NULL DEFAULT '[]',
 requested_at bigint NOT NULL,
 completed_at bigint,
 expires_at bigint
);
CREATE TABLE IF NOT EXISTS zuse_rust_provider_creates (
 operation_id text PRIMARY KEY,
 workspace_id text NOT NULL,
 machine_name text NOT NULL,
 machine_id text,
 outcome text NOT NULL CHECK(outcome IN ('pending','confirmed','rejected','unknown')),
 updated_at bigint NOT NULL
);
