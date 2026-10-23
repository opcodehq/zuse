-- Additive Rust-owned coordination only. Existing api_* business tables remain intact.
CREATE TABLE IF NOT EXISTS zuse_rust_control_owners (
    resource_kind text NOT NULL,
    resource_id text NOT NULL,
    backend text NOT NULL CHECK (backend IN ('legacy', 'rust')),
    generation bigint NOT NULL CHECK (generation > 0),
    updated_at bigint NOT NULL,
    PRIMARY KEY (resource_kind, resource_id)
);

CREATE TABLE IF NOT EXISTS zuse_rust_jobs (
    job_id text PRIMARY KEY,
    idempotency_key text NOT NULL UNIQUE,
    kind text NOT NULL,
    resource_id text NOT NULL,
    resource_generation bigint NOT NULL CHECK (resource_generation > 0),
    payload jsonb NOT NULL,
    state text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'running', 'completed', 'failed')),
    due_at bigint NOT NULL,
    attempts integer NOT NULL DEFAULT 0,
    max_attempts integer NOT NULL DEFAULT 12 CHECK (max_attempts > 0),
    lease_token text,
    lease_expires_at bigint,
    last_error_code text,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL
);
CREATE INDEX IF NOT EXISTS zuse_rust_jobs_due_idx ON zuse_rust_jobs (kind, due_at, lease_expires_at)
    WHERE state IN ('pending', 'running');
