-- Bounded encrypted opening screens are co-located with their authoritative cursor.
-- Large outputs and historical pages retain their object-storage contracts.
ALTER TABLE api_cloud_transcript_checkpoints ADD COLUMN rust_hot_head jsonb
    CHECK(rust_hot_head IS NULL OR
        (jsonb_typeof(rust_hot_head)='object' AND octet_length(rust_hot_head::text)<=266240));
