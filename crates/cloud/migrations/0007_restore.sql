-- Replacement compute is separate from initial allocation; original storage identity survives.
ALTER TABLE zuse_rust_recovery_operations ADD COLUMN IF NOT EXISTS restore_generation bigint;
CREATE TABLE IF NOT EXISTS zuse_rust_restores (
 operation_id text PRIMARY KEY,
 archive_operation_id text NOT NULL UNIQUE REFERENCES zuse_rust_recovery_operations(operation_id),
 workspace_id text NOT NULL,
 generation bigint NOT NULL CHECK(generation>0),
 command_id text NOT NULL,
 snapshot_id text NOT NULL,
 machine_name text NOT NULL UNIQUE,
 machine_id text,
 running_since bigint,
 sealed_boot_token text,
 boot_expires_at bigint NOT NULL,
 phase text NOT NULL CHECK(phase IN ('planned','allocated','restoring','restored','launching','ready')),
 created_at bigint NOT NULL,
 updated_at bigint NOT NULL,
 UNIQUE(workspace_id,generation)
);
