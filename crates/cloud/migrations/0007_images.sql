CREATE TABLE IF NOT EXISTS zuse_rust_image_operations (
 build_id text PRIMARY KEY,
 account_id text NOT NULL,
 record jsonb NOT NULL,
 created_at bigint NOT NULL,
 updated_at bigint NOT NULL
);
CREATE INDEX IF NOT EXISTS zuse_rust_image_operations_account_idx ON zuse_rust_image_operations(account_id);
