-- Canonical task command/queue ownership. Payload and presentation are encrypted;
-- delivery state remains in zuse_rust_mailbox_commands, never model completion.
ALTER TABLE zuse_rust_account_event_outbox
 DROP CONSTRAINT zuse_rust_account_event_outbox_kind_check;
ALTER TABLE zuse_rust_account_event_outbox
 ADD CONSTRAINT zuse_rust_account_event_outbox_kind_check
 CHECK(kind IN ('workspace','conversation','connection','authentication','github','environment','task-command-queue','task-command-progress'));
CREATE TABLE IF NOT EXISTS zuse_rust_task_command_policies (
 workspace_id text PRIMARY KEY REFERENCES api_cloud_workspaces(workspace_id) ON DELETE CASCADE,
 owner_id text NOT NULL,
 policy_revision bigint NOT NULL CHECK(policy_revision>0),
 ciphertext text NOT NULL CHECK(octet_length(ciphertext)<=262144),
 created_at bigint NOT NULL, updated_at bigint NOT NULL
);
CREATE TABLE IF NOT EXISTS zuse_rust_task_command_sessions (
 workspace_id text NOT NULL REFERENCES zuse_rust_task_command_policies(workspace_id) ON DELETE CASCADE,
 session_id text NOT NULL CHECK(length(session_id) BETWEEN 1 AND 512),
 thread_id text NOT NULL,
 state text NOT NULL CHECK(state IN ('active','archived')),
 queue_paused boolean NOT NULL DEFAULT false,
 metadata_ciphertext text CHECK(octet_length(metadata_ciphertext)<=65536),
 created_at bigint NOT NULL,
 PRIMARY KEY(workspace_id,session_id)
);
CREATE TABLE IF NOT EXISTS zuse_rust_task_command_actions (
 workspace_id text NOT NULL REFERENCES zuse_rust_task_command_policies(workspace_id) ON DELETE CASCADE,
 command_id text NOT NULL CHECK(length(command_id) BETWEEN 1 AND 512),
 session_id text NOT NULL,
 actor_id text NOT NULL,
 request_digest text NOT NULL,
 action_kind text NOT NULL,
 acceptance jsonb NOT NULL CHECK(jsonb_typeof(acceptance)='object'),
 created_at bigint NOT NULL,
 PRIMARY KEY(workspace_id,command_id),
 FOREIGN KEY(workspace_id,session_id) REFERENCES zuse_rust_task_command_sessions(workspace_id,session_id)
);
CREATE TABLE IF NOT EXISTS zuse_rust_task_command_queue (
 workspace_id text NOT NULL,
 session_id text NOT NULL,
 queue_id text NOT NULL CHECK(length(queue_id) BETWEEN 1 AND 512),
 message_id text NOT NULL CHECK(length(message_id) BETWEEN 1 AND 512),
 actor_id text NOT NULL, actor jsonb,
 revision bigint NOT NULL CHECK(revision>0),
 position bigint NOT NULL CHECK(position>=0),
 state text NOT NULL CHECK(state IN ('queued','claimed','deleted')),
 hold_for_turn_end boolean NOT NULL,
 ciphertext text NOT NULL CHECK(octet_length(ciphertext)<=262144),
 claimed_command_id text,
 edit_lease_hash text, edit_actor_id text, edit_device_id text, edit_instance_id text, edit_expires_at bigint,
 created_at bigint NOT NULL, updated_at bigint NOT NULL,
 PRIMARY KEY(workspace_id,session_id,queue_id),
 UNIQUE(workspace_id,session_id,message_id),
 FOREIGN KEY(workspace_id,session_id) REFERENCES zuse_rust_task_command_sessions(workspace_id,session_id) ON DELETE CASCADE,
 CHECK((state='claimed')=(claimed_command_id IS NOT NULL)),
 CHECK((edit_lease_hash IS NULL)=(edit_expires_at IS NULL))
);
CREATE INDEX IF NOT EXISTS zuse_rust_task_command_queue_head ON zuse_rust_task_command_queue(workspace_id,session_id,position,queue_id) WHERE state='queued';
-- Durable, bounded server reconciliation for queue promotion after a lost hint.
CREATE TABLE IF NOT EXISTS zuse_rust_task_command_drains (
 workspace_id text NOT NULL, session_id text NOT NULL,
 revision bigint NOT NULL CHECK(revision>0), next_attempt_at bigint NOT NULL,
 last_error text,
 PRIMARY KEY(workspace_id,session_id),
 FOREIGN KEY(workspace_id,session_id) REFERENCES zuse_rust_task_command_sessions(workspace_id,session_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS zuse_rust_task_command_drains_due ON zuse_rust_task_command_drains(next_attempt_at,workspace_id,session_id);
CREATE TABLE IF NOT EXISTS zuse_rust_task_command_execution (
 workspace_id text NOT NULL,
 command_id text NOT NULL,
 session_id text NOT NULL,
 message_id text,
 fingerprint text NOT NULL,
 state text NOT NULL CHECK(state IN ('queued','starting','running','waiting-input','completed','cancelled','failed','outcome-unknown')),
 priority smallint NOT NULL DEFAULT 0 CHECK(priority IN (0,1)),
 wait_for_control_id text,
 runtime_generation bigint,
 storage_incarnation_id text,
 runtime_revision bigint NOT NULL DEFAULT 0 CHECK(runtime_revision>=0),
 runtime_report_digest text, unknown_reviewed boolean NOT NULL DEFAULT false,
 turn_id text, error_code text,
 presentation_ciphertext text NOT NULL CHECK(octet_length(presentation_ciphertext)<=262144),
 created_at bigint NOT NULL, updated_at bigint NOT NULL,
 PRIMARY KEY(workspace_id,command_id),
 FOREIGN KEY(workspace_id,session_id) REFERENCES zuse_rust_task_command_sessions(workspace_id,session_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS zuse_rust_task_command_execution_lane ON zuse_rust_task_command_execution(workspace_id,session_id,state);
-- Policy writes and command/queue/progress transitions are small invalidations.
-- No command text, ciphertext, recipient credentials or tool output leaves PG.
CREATE OR REPLACE FUNCTION zuse_rust_task_command_event() RETURNS trigger LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
DECLARE owner text;
BEGIN
 SELECT account_id INTO owner FROM api_cloud_workspaces WHERE workspace_id=NEW.workspace_id;
 IF owner IS NOT NULL THEN
  INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,workspace_id,payload,created_at)
  VALUES(owner,CASE WHEN TG_TABLE_NAME='zuse_rust_task_command_queue' THEN 'task-command-queue' ELSE 'task-command-progress' END,NEW.session_id,NEW.workspace_id,jsonb_build_object('commandAdmissionVersion',2),NEW.updated_at);
 END IF;
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS zuse_rust_task_command_queue_event ON zuse_rust_task_command_queue;
CREATE TRIGGER zuse_rust_task_command_queue_event AFTER INSERT OR UPDATE ON zuse_rust_task_command_queue FOR EACH ROW EXECUTE FUNCTION zuse_rust_task_command_event();
DROP TRIGGER IF EXISTS zuse_rust_task_command_execution_event ON zuse_rust_task_command_execution;
CREATE TRIGGER zuse_rust_task_command_execution_event AFTER INSERT OR UPDATE ON zuse_rust_task_command_execution FOR EACH ROW EXECUTE FUNCTION zuse_rust_task_command_event();

-- Emit only changes to the actionable error, never reconciliation ticks.
CREATE OR REPLACE FUNCTION zuse_rust_task_drain_event() RETURNS trigger LANGUAGE plpgsql SET search_path FROM CURRENT AS $$
DECLARE owner text;
BEGIN
 SELECT account_id INTO owner FROM api_cloud_workspaces WHERE workspace_id=NEW.workspace_id;
 IF owner IS NOT NULL THEN
  INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,workspace_id,payload,created_at)
  VALUES(owner,'task-command-queue',NEW.session_id,NEW.workspace_id,jsonb_build_object('commandAdmissionVersion',2),floor(extract(epoch FROM clock_timestamp())*1000)::bigint);
 END IF;
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS zuse_rust_task_drain_error_event ON zuse_rust_task_command_drains;
CREATE TRIGGER zuse_rust_task_drain_error_event AFTER UPDATE OF last_error ON zuse_rust_task_command_drains FOR EACH ROW WHEN (OLD.last_error IS DISTINCT FROM NEW.last_error) EXECUTE FUNCTION zuse_rust_task_drain_event();

CREATE UNIQUE INDEX IF NOT EXISTS zuse_rust_task_command_message_once ON zuse_rust_task_command_execution(workspace_id,session_id,message_id) WHERE message_id IS NOT NULL;
-- Bounded in-flight ciphertext checkpoints make chunk delivery retryable without
-- keeping SQL locks across R2. Confirmed objects retain small immutable receipts.
CREATE TABLE IF NOT EXISTS zuse_rust_task_asset_chunks (
 workspace_id text NOT NULL,
 asset_id text NOT NULL,
 chunk_sequence integer NOT NULL CHECK(chunk_sequence>=0 AND chunk_sequence<40),
 size_bytes integer NOT NULL CHECK(size_bytes>0 AND size_bytes<=524288),
 sha256 text NOT NULL,
 ciphertext text CHECK(octet_length(ciphertext)<=1100000),
 ready boolean NOT NULL DEFAULT false,
 updated_at bigint NOT NULL,
 PRIMARY KEY(workspace_id,asset_id,chunk_sequence),
 FOREIGN KEY(workspace_id,asset_id) REFERENCES zuse_rust_launch_assets(workspace_id,asset_id) ON DELETE CASCADE,
 CHECK(ready OR ciphertext IS NOT NULL)
);
-- Deliberately no workspace/account FK: unknown remote writes can finish after
-- the corresponding application rows and ciphertext checkpoints are removed.
CREATE TABLE IF NOT EXISTS zuse_rust_object_write_intents (
 intent_id text PRIMARY KEY,
 workspace_id text NOT NULL,
 account_id text NOT NULL,
 object_key text NOT NULL,
 purpose text NOT NULL CHECK (purpose IN ('tool','asset-chunk','asset-manifest')),
 generation bigint NOT NULL CHECK (generation>0),
 source_machine_id text,
 storage_incarnation_id text,
 destruction_fence bigint NOT NULL CHECK (destruction_fence>=0),
 created_at bigint NOT NULL,
 next_reconcile_at bigint NOT NULL,
 last_empty_at bigint,
 cleanup_attempts bigint NOT NULL DEFAULT 0 CHECK (cleanup_attempts>=0)
);
CREATE INDEX IF NOT EXISTS zuse_rust_object_write_intents_due ON zuse_rust_object_write_intents(next_reconcile_at,intent_id);
CREATE INDEX IF NOT EXISTS zuse_rust_object_write_intents_workspace ON zuse_rust_object_write_intents(workspace_id);
