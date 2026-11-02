-- Original snapshot storage schema, without production build import.
CREATE TABLE api_cloud_snapshot_leases (
 account_id text NOT NULL,
 owner text NOT NULL,
 mode text NOT NULL DEFAULT 'exclusive' CHECK (mode IN ('shared', 'exclusive')),
 expires_at bigint NOT NULL,
 PRIMARY KEY (account_id, owner)
);
CREATE TABLE api_cloud_snapshots (
 provider text NOT NULL CHECK (provider = 'box'),
 snapshot_id text NOT NULL,
 account_id text NOT NULL,
 build_id text NOT NULL,
 state text NOT NULL CHECK (state IN ('creating','retained','deleting','deleted')),
 created_at bigint NOT NULL,
 record jsonb NOT NULL,
 PRIMARY KEY (provider, snapshot_id)
);
CREATE UNIQUE INDEX api_cloud_snapshots_retained_idx ON api_cloud_snapshots(account_id,provider) WHERE state = 'retained';
CREATE INDEX api_cloud_snapshots_account_idx ON api_cloud_snapshots(account_id, state);
CREATE INDEX api_cloud_snapshots_live_idx ON api_cloud_snapshots(state) WHERE state != 'deleted';
CREATE INDEX api_cloud_snapshots_unsettled_idx ON api_cloud_snapshots(account_id) WHERE state = 'deleted' AND (record->>'retainedAtMs')::bigint IS NOT NULL AND COALESCE((record->>'checkpointAtMs')::bigint, (record->>'retainedAtMs')::bigint) < (record->>'stoppedAtMs')::bigint;
CREATE TABLE api_snapshot_price_schedule (
 provider text NOT NULL,
 version text NOT NULL,
 monthly_micros bigint NOT NULL CHECK (monthly_micros >= 0),
 duration_ms bigint NOT NULL CHECK (duration_ms = 2592000000),
 PRIMARY KEY (provider, version)
);
INSERT INTO api_snapshot_price_schedule VALUES ('box','boat-snapshot-2026-10-v1',1700000,2592000000);
CREATE FUNCTION api_snapshot_price_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 RAISE EXCEPTION 'Snapshot price schedules are immutable; insert a new version';
END;
$$;
CREATE TRIGGER api_snapshot_price_immutable BEFORE UPDATE OR DELETE ON api_snapshot_price_schedule FOR EACH ROW EXECUTE FUNCTION api_snapshot_price_immutable();
