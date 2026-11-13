-- Server-held attachments leave object storage once the computer has them, or
-- when they expire undelivered. Rows keep only opaque ids and metadata.
--   uploading -> ready -> releasing -> released
-- `releasing` is durable before any object delete, so a failed delete is retried.
ALTER TABLE zuse_rust_launch_assets DROP CONSTRAINT IF EXISTS zuse_rust_launch_assets_state_check;
ALTER TABLE zuse_rust_launch_assets ADD CONSTRAINT zuse_rust_launch_assets_state_check
 CHECK(state IN ('uploading','ready','releasing','released'));
ALTER TABLE zuse_rust_launch_assets ADD COLUMN IF NOT EXISTS release_reason text;
ALTER TABLE zuse_rust_launch_assets ADD COLUMN IF NOT EXISTS released_at bigint;
ALTER TABLE zuse_rust_launch_assets DROP CONSTRAINT IF EXISTS zuse_rust_launch_assets_release_check;
ALTER TABLE zuse_rust_launch_assets ADD CONSTRAINT zuse_rust_launch_assets_release_check
 CHECK((state IN ('releasing','released'))=(release_reason IS NOT NULL)
  AND (release_reason IS NULL OR release_reason IN ('delivered','expired'))
  AND ((state='released')=(released_at IS NOT NULL)));
CREATE INDEX IF NOT EXISTS zuse_rust_launch_assets_release_due
 ON zuse_rust_launch_assets(created_at) WHERE state IN ('uploading','ready','releasing');
-- Who still needs a server-held asset: an admitted command (delivered once the
-- computer reports it running) or a queued message that has not been sent yet.
CREATE TABLE IF NOT EXISTS zuse_rust_launch_asset_holds (
 workspace_id text NOT NULL,
 asset_id text NOT NULL,
 kind text NOT NULL CHECK(kind IN ('command','queue')),
 session_id text NOT NULL CHECK(length(session_id) BETWEEN 1 AND 512),
 holder text NOT NULL CHECK(length(holder) BETWEEN 1 AND 1024),
 delivered_at bigint,
 created_at bigint NOT NULL,
 PRIMARY KEY(workspace_id,asset_id,kind,session_id,holder),
 FOREIGN KEY(workspace_id,asset_id) REFERENCES zuse_rust_launch_assets(workspace_id,asset_id) ON DELETE CASCADE,
 CHECK(kind='command' OR delivered_at IS NULL)
);
CREATE INDEX IF NOT EXISTS zuse_rust_launch_asset_holds_holder
 ON zuse_rust_launch_asset_holds(workspace_id,holder) WHERE kind='command';
