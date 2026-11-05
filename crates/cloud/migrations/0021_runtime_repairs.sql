-- Same-machine/same-storage supervised repair. No allocation/sleep/delete job.
CREATE TABLE zuse_rust_runtime_repairs (
 operation_id text PRIMARY KEY,
 workspace_id text NOT NULL REFERENCES api_cloud_workspaces(workspace_id),
 generation bigint NOT NULL CHECK(generation>0),
 source_machine_id text NOT NULL,
 storage_incarnation_id text NOT NULL,
 actor_id text NOT NULL,
 idempotency_key text NOT NULL,
 pause_operation_id text NOT NULL,
 resume_operation_id text NOT NULL,
 phase text NOT NULL CHECK(phase IN ('quiescing','stopping','stopped','starting','waiting-for-runtime','attention-required','ready','failed')),
 command_ids jsonb NOT NULL DEFAULT '[]' CHECK(jsonb_typeof(command_ids)='array' AND jsonb_array_length(command_ids)<=128),
 stop_receipt jsonb,
 start_receipt jsonb,
 recovery jsonb NOT NULL DEFAULT '{}',
 error_code text,
 resolution text CHECK(resolution IN ('continue','discard')),
 created_at bigint NOT NULL,
 run_started_at bigint NOT NULL,
 updated_at bigint NOT NULL,
 UNIQUE(workspace_id,idempotency_key),
 CHECK(stop_receipt IS NULL OR octet_length(stop_receipt::text)<=65536),
 CHECK(start_receipt IS NULL OR octet_length(start_receipt::text)<=65536),
 CHECK(octet_length(recovery::text)<=65536)
);
CREATE UNIQUE INDEX zuse_rust_runtime_repair_active ON zuse_rust_runtime_repairs(workspace_id)
WHERE phase NOT IN ('ready');
-- Publish only operation phase/attention, never receipts or command ciphertext.
CREATE FUNCTION zuse_rust_runtime_repair_event() RETURNS trigger LANGUAGE plpgsql
SET search_path FROM CURRENT AS $body$
DECLARE owner text; BEGIN
 IF TG_OP='UPDATE' AND (NEW.phase,NEW.resolution,NEW.error_code) IS NOT DISTINCT FROM (OLD.phase,OLD.resolution,OLD.error_code) THEN RETURN NEW; END IF;
 SELECT account_id INTO owner FROM api_cloud_workspaces WHERE workspace_id=NEW.workspace_id;
 INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,workspace_id,payload,created_at)
 VALUES(owner,'workspace',NEW.operation_id,NEW.workspace_id,
 jsonb_build_object('runtimeRepair',true,'phase',NEW.phase,'requiresAttention',NEW.phase IN ('attention-required','failed')),NEW.updated_at);
 RETURN NEW;
END $body$;
CREATE TRIGGER zuse_rust_runtime_repair_events AFTER INSERT OR UPDATE ON zuse_rust_runtime_repairs
FOR EACH ROW EXECUTE FUNCTION zuse_rust_runtime_repair_event();
