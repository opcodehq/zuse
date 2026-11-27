-- Setup chats: an agent on a setup computer proposes a new machine image
-- configuration (setup script and repository list) for the owner to apply.
--
-- A proposal holds only what the owner reviews: the script, repository
-- identities and a note. The server refuses token-shaped values before they
-- get here. Applying one saves the account's recipe and repositories through
-- the normal writers and queues the normal rebuild; this table only records
-- the decision.
CREATE TABLE IF NOT EXISTS cloud_machine_setup_proposals(
  proposal_id text PRIMARY KEY,
  owner_id text NOT NULL,
  workspace_id text NOT NULL,
  chat_id text NOT NULL,
  base_revision bigint NOT NULL,
  current_setup_script text NOT NULL,
  setup_script text NOT NULL,
  current_repositories jsonb NOT NULL DEFAULT '[]',
  repositories jsonb NOT NULL DEFAULT '[]',
  notes text NOT NULL DEFAULT '',
  state text NOT NULL CHECK(state IN ('proposed','applied','discarded','superseded')),
  applied_revision bigint,
  created_at bigint NOT NULL,
  decided_at bigint
);
CREATE INDEX IF NOT EXISTS cloud_machine_setup_proposals_workspace_idx
  ON cloud_machine_setup_proposals(workspace_id, created_at DESC);
CREATE INDEX IF NOT EXISTS cloud_machine_setup_proposals_owner_idx
  ON cloud_machine_setup_proposals(owner_id, created_at DESC);
DO $grants$
DECLARE schema_name text := current_schema(); app_role text;
BEGIN
 app_role := CASE schema_name WHEN 'zuse_staging' THEN 'zuse_cloud_staging' WHEN 'zuse_production' THEN 'zuse_cloud_production' END;
 IF app_role IS NOT NULL AND EXISTS(SELECT 1 FROM pg_roles WHERE rolname=app_role) THEN
  EXECUTE format('GRANT SELECT,INSERT,UPDATE ON TABLE %I.cloud_machine_setup_proposals TO %I',schema_name,app_role);
 END IF;
END $grants$;
