-- Preallocation uploads reserve an opaque, non-reused workspace identity.
CREATE TABLE IF NOT EXISTS zuse_rust_launch_staging (
 account_id text NOT NULL,
 idempotency_digest text NOT NULL,
 workspace_id text NOT NULL UNIQUE,
 created_at bigint NOT NULL,
 expires_at bigint NOT NULL,
 PRIMARY KEY(account_id,idempotency_digest)
);
CREATE TABLE IF NOT EXISTS zuse_rust_launch_assets (
 workspace_id text NOT NULL,
 asset_id text NOT NULL,
 chunks jsonb NOT NULL DEFAULT '{}',
 manifest jsonb,
 state text NOT NULL CHECK(state IN ('uploading','ready')),
 created_at bigint NOT NULL,
 updated_at bigint NOT NULL,
 PRIMARY KEY(workspace_id,asset_id)
);
