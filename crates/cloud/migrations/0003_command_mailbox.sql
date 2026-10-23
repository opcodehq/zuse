-- Durable encrypted envelopes; no plaintext command payload is stored here.
CREATE TABLE IF NOT EXISTS zuse_rust_mailboxes (
    workspace_id text PRIMARY KEY,
    sequence bigint NOT NULL DEFAULT 0,
    revision bigint NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS zuse_rust_mailbox_commands (
    workspace_id text NOT NULL REFERENCES zuse_rust_mailboxes(workspace_id),
    command_id text NOT NULL,
    workspace_sequence bigint NOT NULL,
    revision bigint NOT NULL,
    lane text NOT NULL,
    fingerprint text NOT NULL,
    envelope jsonb NOT NULL,
    envelope_bytes bigint NOT NULL,
    state text NOT NULL CHECK (state IN ('accepted','waiting-for-runtime','leased','lease-expired-awaiting-fence','blocked','applied','rejected','expired','cancelled','outcome-unknown')),
    ever_leased boolean NOT NULL DEFAULT false,
    lease_token text,
    lease_deadline bigint,
    runtime_generation bigint,
    provider_sandbox_id text,
    storage_incarnation_id text,
    category text,
    result_iv text,
    result_ciphertext text,
    accepted_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    PRIMARY KEY (workspace_id,command_id),
    UNIQUE (workspace_id, workspace_sequence)
);
CREATE INDEX IF NOT EXISTS zuse_rust_mailbox_order ON zuse_rust_mailbox_commands(workspace_id,lane,workspace_sequence);
CREATE INDEX IF NOT EXISTS zuse_rust_mailbox_changes ON zuse_rust_mailbox_commands(workspace_id,revision);
