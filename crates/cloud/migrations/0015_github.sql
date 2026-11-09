-- Separate authority tables deliberately leave legacy Worker-owned ciphertext untouched.
CREATE TABLE IF NOT EXISTS zuse_rust_github_users (
 actor_id text PRIMARY KEY,
 github_user_id bigint,
 profile jsonb NOT NULL DEFAULT '{}',
 sealed_credentials text,
 state text NOT NULL DEFAULT 'disconnected' CHECK (state IN ('connected','revoking','disconnected')),
 epoch bigint NOT NULL DEFAULT 0,
 version bigint NOT NULL DEFAULT 0,
 refresh_uncertain boolean NOT NULL DEFAULT false,
 updated_at bigint NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS zuse_rust_github_identity ON zuse_rust_github_users(github_user_id) WHERE state='connected';
CREATE TABLE IF NOT EXISTS zuse_rust_github_scopes (
 owner_id text PRIMARY KEY,
 epoch bigint NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS zuse_rust_github_installations (
 owner_id text NOT NULL REFERENCES zuse_rust_github_scopes(owner_id),
 installation_id bigint NOT NULL,
 metadata jsonb NOT NULL,
 allowed_repositories jsonb,
 revision bigint NOT NULL DEFAULT 1,
 observation_id text,
 state text NOT NULL CHECK(state IN ('active','suspended','removed')),
 updated_at bigint NOT NULL,
 PRIMARY KEY(owner_id,installation_id)
);
CREATE TABLE IF NOT EXISTS zuse_rust_github_operations (
 operation_id text PRIMARY KEY,
 state_hash text NOT NULL UNIQUE,
 actor_id text NOT NULL REFERENCES zuse_rust_github_users(actor_id),
 owner_id text NOT NULL REFERENCES zuse_rust_github_scopes(owner_id),
 actor_epoch bigint NOT NULL,
 scope_epoch bigint NOT NULL,
 browser_hash text,
 phase text NOT NULL CHECK(phase IN ('created','authorizing','exchanging','choosing','complete','cancelled','failed')),
 sealed_pending text,
 expires_at bigint NOT NULL,
 created_at bigint NOT NULL
);
CREATE INDEX IF NOT EXISTS zuse_rust_github_pending ON zuse_rust_github_operations(actor_id,owner_id,expires_at);
CREATE TABLE IF NOT EXISTS zuse_rust_github_deliveries (
 delivery_id text PRIMARY KEY,
 body_hash text NOT NULL,
 completed boolean NOT NULL DEFAULT false,
 received_at bigint NOT NULL
);
