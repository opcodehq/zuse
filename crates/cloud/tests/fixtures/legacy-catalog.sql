-- Catalog subset with the original api_* column names and storage formats.
-- Runtime history and credentials are deliberately absent from public responses.
CREATE TABLE api_entitlements (
    entitlement_id text PRIMARY KEY, account_id text NOT NULL, kind text NOT NULL,
    offer_id text, provider text NOT NULL, provider_subscription_id text,
    status text NOT NULL, paid_through bigint, credit_balance bigint,
    created_at bigint NOT NULL, updated_at bigint NOT NULL
);
CREATE TABLE api_machines (
    machine_id text PRIMARY KEY, account_id text NOT NULL, entitlement_id text NOT NULL,
    state text NOT NULL, created_at bigint NOT NULL
);
CREATE TABLE api_cloud_workspaces (
    workspace_id text PRIMARY KEY, account_id text NOT NULL, project_id text NOT NULL,
    build_id text NOT NULL, provider text NOT NULL, provider_sandbox_id text,
    runtime_boot_token_hash text, runtime_credential_hash text, wrapped_transcript_key text,
    runtime_state text NOT NULL, chat_id text NOT NULL, initial_session_id text NOT NULL,
    branch text NOT NULL, base_ref text NOT NULL, state text NOT NULL, desired_state text NOT NULL,
    status_code text NOT NULL, request_config jsonb NOT NULL DEFAULT '{}',
    archive_requested_at bigint, archive_delete_at bigint, lease_expires_at bigint,
    revision bigint NOT NULL, created_at bigint NOT NULL, updated_at bigint NOT NULL,
    last_activity_at bigint NOT NULL
);
INSERT INTO api_entitlements VALUES
 ('ent-alice','user-alice','cloud-workspace','cloud-workspace-standard-v1','stripe','sub-preserved','active',9999999999999,NULL,100,100),
 ('ent-bob','user-bob','cloud-workspace','cloud-workspace-standard-v1','polar','legacy-sub','active',9999999999999,NULL,100,100),
 ('ent-org','organization:org-team','cloud-workspace','cloud-workspace-standard-v1','stripe','sub-org','active',9999999999999,NULL,100,100);
INSERT INTO api_cloud_workspaces VALUES
 ('ws-alice','user-alice','project','image','boxd','machine-alice','BOOT_HASH_PRIVATE','CREDENTIAL_HASH_PRIVATE','WRAPPED_KEY_PRIVATE','offline','chat-alice','session-alice','feature','main','archived','archived','archived','{"runtimeGeneration":2}',100,2592000100,NULL,5,1,3,2),
 ('ws-bob','user-bob','project','image','boxd','machine-bob',NULL,NULL,NULL,'online','chat-bob','session-bob','feature-bob','main','ready','ready','ready','{}',NULL,NULL,NULL,1,1,2,1),
 ('ws-org-private','organization:org-team','project-org','image','boxd','machine-org',NULL,NULL,NULL,'offline','chat-org','session-org','private','main','paused','paused','paused','{"sharingPolicy":{"audience":"private","permission":"view","creatorSubject":"user-admin","creatorMembershipId":"member-admin","grants":[]}}',NULL,NULL,NULL,1,1,2,1),
 ('ws-org-shared','organization:org-team','project-org','image','boxd','machine-shared',NULL,NULL,NULL,'offline','chat-shared','session-shared','shared','main','paused','paused','paused','{"sharingPolicy":{"audience":"organization","permission":"view","creatorSubject":"user-admin","creatorMembershipId":"member-admin","grants":[]}}',NULL,NULL,NULL,1,1,2,1);

-- Original runtime columns and launch table used by bootstrap acceptance.
ALTER TABLE api_cloud_workspaces ADD COLUMN runtime_boot_token_expires_at bigint;
ALTER TABLE api_cloud_workspaces ADD COLUMN next_action_at bigint NOT NULL DEFAULT 0;
CREATE TABLE api_cloud_workspace_launch_intents (
    workspace_id text PRIMARY KEY, account_id text NOT NULL, chat_id text NOT NULL,
    session_id text NOT NULL, turn_id text NOT NULL, command_id text NOT NULL,
    ciphertext text NOT NULL, expires_at bigint NOT NULL, created_at bigint NOT NULL
);
