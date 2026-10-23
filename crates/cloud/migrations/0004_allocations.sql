-- Initial allocation intent and provider receipts survive worker/process failure.
-- This never transfers ownership or authorizes replacing retained runtime storage.
CREATE TABLE IF NOT EXISTS zuse_rust_allocations (
    operation_id text PRIMARY KEY,
    workspace_id text NOT NULL,
    generation bigint NOT NULL CHECK (generation > 0),
    snapshot_id text NOT NULL,
    machine_name text NOT NULL UNIQUE,
    machine_id text,
    storage_incarnation_id text NOT NULL,
    sealed_boot_token text,
    boot_expires_at bigint NOT NULL,
    phase text NOT NULL CHECK (phase IN ('planned','creating','allocated','launching','ready')),
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    UNIQUE (workspace_id, generation)
);
