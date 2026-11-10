-- Explicit replacement, separate from a user's archive/retention policy.
-- The original is never retired before a verified encrypted recovery bundle.
CREATE TABLE zuse_rust_computer_recreations (
 operation_id text PRIMARY KEY,
 workspace_id text NOT NULL REFERENCES api_cloud_workspaces(workspace_id),
 owner_id text NOT NULL,
 actor_id text NOT NULL,
 idempotency_key text NOT NULL,
 provider_id text NOT NULL CHECK(provider_id IN ('boxd','boat','e2b','daytona')),
 source_generation bigint NOT NULL CHECK(source_generation>0),
 source_machine_id text NOT NULL,
 storage_incarnation_id text NOT NULL,
 managed_release_id text NOT NULL,
 source_artifact_id text NOT NULL,
 candidate_sha256 text,
 pause_operation_id text NOT NULL,
 archive_operation_id text,
 restore_operation_id text,
 restore_generation bigint,
 phase text NOT NULL CHECK(phase IN ('quiescing','capturing','retiring','restoring','waiting-for-runtime','ready','attention-required','cancelled')),
 retry_phase text CHECK(retry_phase IN ('quiescing','capturing','retiring','restoring','waiting-for-runtime')),
 verified_archive_sha256 text,
 error_code text,
 due_at bigint NOT NULL,
 created_at bigint NOT NULL,
 updated_at bigint NOT NULL,
 UNIQUE(workspace_id,idempotency_key),
 CHECK(restore_generation IS NULL OR restore_generation=source_generation+1),
 CHECK(verified_archive_sha256 IS NULL OR verified_archive_sha256 ~ '^[a-f0-9]{64}$')
);
CREATE UNIQUE INDEX zuse_rust_computer_recreation_active
 ON zuse_rust_computer_recreations(workspace_id) WHERE phase NOT IN ('ready','cancelled');
CREATE INDEX zuse_rust_computer_recreation_due
 ON zuse_rust_computer_recreations(due_at,operation_id)
 WHERE phase NOT IN ('ready','cancelled','attention-required');
-- Only safe progress invalidations enter the account feed.
CREATE FUNCTION zuse_rust_computer_recreation_event() RETURNS trigger LANGUAGE plpgsql
SET search_path FROM CURRENT AS $body$
BEGIN
 IF TG_OP='UPDATE' AND (NEW.phase,NEW.error_code) IS NOT DISTINCT FROM (OLD.phase,OLD.error_code) THEN RETURN NEW; END IF;
 INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,workspace_id,payload,created_at)
 VALUES(NEW.owner_id,'workspace',NEW.workspace_id,NEW.workspace_id,
 jsonb_build_object('recreation',true,'phase',NEW.phase,'requiresAttention',NEW.phase='attention-required'),NEW.updated_at);
 RETURN NEW;
END $body$;
CREATE TRIGGER zuse_rust_computer_recreation_event
 AFTER INSERT OR UPDATE ON zuse_rust_computer_recreations
 FOR EACH ROW EXECUTE FUNCTION zuse_rust_computer_recreation_event();
