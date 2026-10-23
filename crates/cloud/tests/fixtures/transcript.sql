-- Existing encrypted checkpoint metadata, never writable runtime history.
CREATE TABLE api_cloud_transcript_checkpoints (
    workspace_id text NOT NULL, session_id text NOT NULL,
    runtime_generation bigint NOT NULL, stream_epoch text NOT NULL,
    stream_version bigint NOT NULL, object_key text NOT NULL,
    ciphertext_sha256 text NOT NULL, ciphertext_bytes bigint NOT NULL,
    created_at bigint NOT NULL, PRIMARY KEY(workspace_id,session_id)
);
