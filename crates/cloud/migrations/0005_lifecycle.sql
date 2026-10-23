-- Source identity remains fixed throughout pause/archive/resume. No retirement permission.
CREATE TABLE IF NOT EXISTS zuse_rust_lifecycle_operations (
    operation_id text PRIMARY KEY,
    workspace_id text NOT NULL,
    command_id text NOT NULL,
    generation bigint NOT NULL CHECK (generation > 0),
    source_machine_id text NOT NULL,
    storage_incarnation_id text NOT NULL,
    action text NOT NULL CHECK (action IN ('pause','archive','unarchive','resume')),
    phase text NOT NULL CHECK (phase IN ('quiescing','quiesced','pausing','paused','waking','launching','ready')),
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    UNIQUE (workspace_id,command_id)
);
