-- Rust task admissions are independent of expiring execution intents. Runtime
-- history is encrypted per message and paginated by the guest's actual ordinal.
CREATE TABLE IF NOT EXISTS zuse_rust_task_admissions (
    workspace_id text PRIMARY KEY REFERENCES api_cloud_workspaces(workspace_id) ON DELETE CASCADE,
    session_id text NOT NULL,
    ciphertext text NOT NULL CHECK(octet_length(ciphertext)<=262144),
    created_at bigint NOT NULL
);
CREATE TABLE IF NOT EXISTS zuse_rust_task_history_sessions (
    workspace_id text NOT NULL REFERENCES api_cloud_workspaces(workspace_id) ON DELETE CASCADE,
    session_id text NOT NULL,
    runtime_generation bigint NOT NULL,
    stream_epoch text NOT NULL,
    stream_version bigint NOT NULL,
    message_count bigint NOT NULL CHECK(message_count>=0),
    ciphertext text NOT NULL CHECK(octet_length(ciphertext)<=2097152),
    updated_at bigint NOT NULL,
    PRIMARY KEY(workspace_id,session_id)
);
CREATE TABLE IF NOT EXISTS zuse_rust_task_history_messages (
    workspace_id text NOT NULL,
    session_id text NOT NULL,
    message_sequence bigint NOT NULL CHECK(message_sequence>0),
    projection_cursor text NOT NULL,
    ciphertext text NOT NULL CHECK(octet_length(ciphertext)<=1048576),
    PRIMARY KEY(workspace_id,session_id,message_sequence),
    FOREIGN KEY(workspace_id,session_id) REFERENCES zuse_rust_task_history_sessions(workspace_id,session_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS zuse_rust_preparation_logs (
    workspace_id text NOT NULL REFERENCES api_cloud_workspaces(workspace_id) ON DELETE CASCADE,
    operation_id text NOT NULL,
    ciphertext text NOT NULL CHECK(octet_length(ciphertext)<=131072),
    updated_at bigint NOT NULL,
    PRIMARY KEY(workspace_id,operation_id)
);
CREATE TABLE IF NOT EXISTS zuse_rust_tool_output_refs (
    workspace_id text NOT NULL REFERENCES api_cloud_workspaces(workspace_id) ON DELETE CASCADE,
    blob_ref text NOT NULL,
    session_id text NOT NULL,
    PRIMARY KEY(workspace_id,blob_ref)
);
CREATE TABLE IF NOT EXISTS zuse_rust_tool_output_chunks (
    workspace_id text NOT NULL,
    blob_ref text NOT NULL,
    byte_offset bigint NOT NULL CHECK(byte_offset>=0),
    next_offset bigint NOT NULL,
    total_bytes bigint NOT NULL CHECK(total_bytes>=0 AND total_bytes<=33554432),
    fingerprint text NOT NULL,
    ciphertext text,
    object_key text,
    ready boolean NOT NULL DEFAULT false,
    PRIMARY KEY(workspace_id,blob_ref,byte_offset),
    FOREIGN KEY(workspace_id,blob_ref) REFERENCES zuse_rust_tool_output_refs(workspace_id,blob_ref) ON DELETE CASCADE,
    CHECK(next_offset>=byte_offset AND next_offset<=total_bytes),
    CHECK(ciphertext IS NOT NULL OR object_key IS NOT NULL)
);
