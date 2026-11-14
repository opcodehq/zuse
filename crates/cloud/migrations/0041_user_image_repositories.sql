-- User images also carry the account's repositories: a depth-1 snapshot of
-- each included repository's default branch, with no remote or credential.
-- `repositories_digest` is the repository set the image was planned from (the
-- hourly staleness pass compares it with the account's current set);
-- `repositories` records which repositories were baked at which commits, and
-- which were left out and why. Both stay NULL for images without repositories.
-- The table's existing grants cover the new columns.
ALTER TABLE cloud_machine_images ADD COLUMN IF NOT EXISTS repositories_digest text
    CHECK (repositories_digest IS NULL OR repositories_digest ~ '^[0-9a-f]{64}$');
ALTER TABLE cloud_machine_images ADD COLUMN IF NOT EXISTS repositories jsonb
    CHECK (repositories IS NULL OR (jsonb_typeof(repositories) = 'object' AND octet_length(repositories::text) <= 65536));
