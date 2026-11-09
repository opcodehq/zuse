-- Fresh standalone database compatibility baseline. No seed/customer data.
-- Exported from the source application schema at a0155d2dd11f07495a99d3737c9bd40a5c8b4508
-- using drizzle-orm 0.44.3 and drizzle-kit 0.31.4. Rust SQL currently uses these names.
-- References use the selected private schema, rather than the source public schema.
-- Index builds are transactional here: this migration only initializes an empty schema.
-- Run through infra/cloud/database.py to enforce environment, isolation and checksums.
CREATE TABLE "api_agent_activity" (
	"id" bigserial PRIMARY KEY NOT NULL,
	"environment_id" text NOT NULL,
	"account_id" text NOT NULL,
	"session_id" text NOT NULL,
	"kind" text NOT NULL,
	"title" text,
	"occurred_at" bigint NOT NULL,
	CONSTRAINT "api_agent_activity_kind_check" CHECK ("api_agent_activity"."kind" IN ('approval-needed', 'question-needed', 'completed', 'error', 'running'))
);

CREATE TABLE "api_api_keys" (
	"key_id" text PRIMARY KEY NOT NULL,
	"account_id" text NOT NULL,
	"name" text NOT NULL,
	"secret_hash" text NOT NULL,
	"prefix" text NOT NULL,
	"created_at" bigint NOT NULL,
	"last_used_at" bigint,
	"revoked_at" bigint
);

CREATE TABLE "api_api_webhook_deliveries" (
	"delivery_id" text PRIMARY KEY NOT NULL,
	"webhook_id" text NOT NULL,
	"account_id" text NOT NULL,
	"event_id" text NOT NULL,
	"event_type" text NOT NULL,
	"sealed_payload" text NOT NULL,
	"status" text NOT NULL,
	"attempts" bigint DEFAULT 0 NOT NULL,
	"next_attempt_at" bigint NOT NULL,
	"last_error" text,
	"created_at" bigint NOT NULL,
	"updated_at" bigint NOT NULL,
	CONSTRAINT "api_api_webhook_deliveries_status_check" CHECK ("api_api_webhook_deliveries"."status" IN ('pending', 'delivered', 'failed'))
);

CREATE TABLE "api_api_webhooks" (
	"webhook_id" text PRIMARY KEY NOT NULL,
	"account_id" text NOT NULL,
	"url" text NOT NULL,
	"sealed_secret" text NOT NULL,
	"description" text,
	"created_at" bigint NOT NULL,
	"disabled_at" bigint
);

CREATE TABLE "api_billing_events" (
	"event_id" text NOT NULL,
	"provider" text NOT NULL,
	"payload" jsonb,
	"received_at" bigint NOT NULL,
	"processed_at" bigint,
	CONSTRAINT "api_billing_events_provider_event_id_pk" PRIMARY KEY("provider","event_id")
);

CREATE TABLE "api_cloud_auth_authorities" (
	"status" jsonb,
	"account_id" text PRIMARY KEY NOT NULL,
	"provider" text NOT NULL,
	"provider_sandbox_id" text,
	"storage_incarnation_id" text NOT NULL,
	"auth_epoch" bigint DEFAULT 1 NOT NULL,
	"toolchain_version" text NOT NULL,
	"state" text NOT NULL,
	"provisioning_lease_owner" text,
	"provisioning_lease_expires_at" bigint,
	"revision" bigint DEFAULT 0 NOT NULL,
	"created_at" bigint NOT NULL,
	"updated_at" bigint NOT NULL,
	CONSTRAINT "api_cloud_auth_authorities_state_check" CHECK ("api_cloud_auth_authorities"."state" IN ('provisioning', 'ready', 'error'))
);

CREATE TABLE "api_cloud_billing_cap_commands" (
	"period_id" text NOT NULL,
	"idempotency_key" text NOT NULL,
	"overage_cap_micros" bigint NOT NULL,
	"created_at" bigint NOT NULL,
	CONSTRAINT "api_cloud_billing_cap_commands_period_id_idempotency_key_pk" PRIMARY KEY("period_id","idempotency_key")
);

CREATE TABLE "api_cloud_billing_ledger" (
	"entry_id" text PRIMARY KEY NOT NULL,
	"period_id" text NOT NULL,
	"account_id" text NOT NULL,
	"kind" text NOT NULL,
	"amount_micros" bigint NOT NULL,
	"source_id" text NOT NULL,
	"metadata" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"occurred_at" bigint NOT NULL,
	"created_at" bigint NOT NULL
);

CREATE TABLE "api_cloud_billing_meter_reconciliation_attempts" (
	"period_id" text NOT NULL,
	"provider" text NOT NULL,
	"attempted_at" bigint NOT NULL,
	CONSTRAINT "api_cloud_billing_meter_reconciliation_attempts_period_id_provider_pk" PRIMARY KEY("period_id","provider")
);

CREATE TABLE "api_cloud_billing_meter_reconciliations" (
	"reconciliation_id" text PRIMARY KEY NOT NULL,
	"period_id" text NOT NULL,
	"provider" text NOT NULL,
	"expected_units" bigint NOT NULL,
	"observed_units" bigint NOT NULL,
	"created_at" bigint NOT NULL
);

CREATE TABLE "api_cloud_billing_outbox" (
	"outbox_id" text PRIMARY KEY NOT NULL,
	"period_id" text NOT NULL,
	"account_id" text NOT NULL,
	"provider" text NOT NULL,
	"amount_cents" bigint NOT NULL,
	"idempotency_key" text NOT NULL,
	"attempt_count" bigint DEFAULT 0 NOT NULL,
	"next_attempt_at" bigint NOT NULL,
	"acknowledged_at" bigint,
	"last_error" text,
	"occurred_at" bigint DEFAULT floor(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::bigint NOT NULL,
	"created_at" bigint NOT NULL
);

CREATE TABLE "api_cloud_billing_periods" (
	"period_id" text PRIMARY KEY NOT NULL,
	"account_id" text NOT NULL,
	"provider_subscription_id" text,
	"billing_provider" text DEFAULT 'polar' NOT NULL,
	"status" text NOT NULL,
	"currency" text DEFAULT 'USD' NOT NULL,
	"period_start" bigint NOT NULL,
	"period_end" bigint NOT NULL,
	"base_price_micros" bigint NOT NULL,
	"included_provider_cost_micros" bigint NOT NULL,
	"markup_basis_points" bigint NOT NULL,
	"price_catalog_version" text NOT NULL,
	"overage_cap_micros" bigint NOT NULL,
	"created_at" bigint NOT NULL,
	"updated_at" bigint NOT NULL
);

CREATE TABLE "api_cloud_billing_reservations" (
	"period_id" text NOT NULL,
	"account_id" text NOT NULL,
	"resource_kind" text NOT NULL,
	"resource_id" text NOT NULL,
	"provider" text NOT NULL,
	"provider_cost_micros" bigint NOT NULL,
	"started_at" bigint NOT NULL,
	"vcpu_count" bigint NOT NULL,
	"memory_mib" bigint NOT NULL,
	"expires_at" bigint NOT NULL,
	"updated_at" bigint NOT NULL,
	CONSTRAINT "api_cloud_billing_reservations_period_id_resource_kind_resource_id_pk" PRIMARY KEY("period_id","resource_kind","resource_id")
);

CREATE TABLE "api_cloud_billing_usage" (
	"entry_id" text PRIMARY KEY NOT NULL,
	"period_id" text NOT NULL,
	"account_id" text NOT NULL,
	"resource_kind" text NOT NULL,
	"resource_id" text NOT NULL,
	"provider" text NOT NULL,
	"provider_event_id" text,
	"provider_execution_id" text,
	"started_at" bigint NOT NULL,
	"ended_at" bigint NOT NULL,
	"vcpu_count" bigint NOT NULL,
	"memory_mib" bigint NOT NULL,
	"provider_cost_micros" bigint NOT NULL,
	"status" text NOT NULL,
	"created_at" bigint NOT NULL
);

CREATE TABLE "api_cloud_catalog_changes" (
	"account_id" text NOT NULL,
	"workspace_id" text NOT NULL,
	"revision" bigint NOT NULL,
	CONSTRAINT "api_cloud_catalog_changes_account_id_workspace_id_pk" PRIMARY KEY("account_id","workspace_id")
);

CREATE TABLE "api_cloud_catalog_heads" (
	"account_id" text PRIMARY KEY NOT NULL,
	"revision" bigint DEFAULT 0 NOT NULL
);

CREATE TABLE "api_cloud_github_installations" (
	"account_id" text NOT NULL,
	"installation_id" bigint NOT NULL,
	"github_account_id" bigint NOT NULL,
	"account_login" text NOT NULL,
	"account_type" text NOT NULL,
	"avatar_url" text,
	"repository_selection" text NOT NULL,
	"allowed_repositories" jsonb,
	"suspended" boolean DEFAULT false NOT NULL,
	"created_at" bigint NOT NULL,
	"updated_at" bigint NOT NULL,
	CONSTRAINT "api_cloud_github_installations_account_id_installation_id_pk" PRIMARY KEY("account_id","installation_id"),
	CONSTRAINT "api_cloud_github_account_type_check" CHECK ("api_cloud_github_installations"."account_type" IN ('User', 'Organization')),
	CONSTRAINT "api_cloud_github_repository_selection_check" CHECK ("api_cloud_github_installations"."repository_selection" IN ('all', 'selected'))
);

CREATE TABLE "api_cloud_github_users" (
	"account_id" text PRIMARY KEY NOT NULL,
	"login" text NOT NULL,
	"name" text NOT NULL,
	"email" text NOT NULL,
	"sealed_credentials" text NOT NULL
);

CREATE TABLE "api_cloud_project_builds" (
	"build_id" text PRIMARY KEY NOT NULL,
	"project_id" text,
	"account_id" text NOT NULL,
	"provider" text NOT NULL,
	"provider_sandbox_id" text,
	"snapshot_id" text,
	"source_commit" text,
	"template_version" text NOT NULL,
	"configuration_digest" text NOT NULL,
	"settings" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"log_text" text,
	"state" text NOT NULL,
	"included" boolean DEFAULT true NOT NULL,
	"last_error_code" text,
	"idempotency_key" text NOT NULL,
	"next_action_at" bigint NOT NULL,
	"lease_owner" text,
	"lease_expires_at" bigint,
	"revision" bigint DEFAULT 0 NOT NULL,
	"created_at" bigint NOT NULL,
	"updated_at" bigint NOT NULL,
	CONSTRAINT "api_cloud_builds_state_check" CHECK ("api_cloud_project_builds"."state" IN ('queued', 'building', 'sanitizing', 'ready', 'failed'))
);

CREATE TABLE "api_cloud_projects" (
	"project_id" text PRIMARY KEY NOT NULL,
	"account_id" text NOT NULL,
	"repository_identity" text NOT NULL,
	"repository_url" text NOT NULL,
	"display_name" text NOT NULL,
	"default_branch" text NOT NULL,
	"visibility" text NOT NULL,
	"git_connection_kind" text NOT NULL,
	"cloud_environment" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"secret_bindings" jsonb DEFAULT '[]'::jsonb NOT NULL,
	"configuration_digest" text NOT NULL,
	"state" text NOT NULL,
	"included" boolean DEFAULT true NOT NULL,
	"last_error_code" text,
	"idempotency_key" text NOT NULL,
	"created_at" bigint NOT NULL,
	"updated_at" bigint NOT NULL,
	CONSTRAINT "api_cloud_projects_visibility_check" CHECK ("api_cloud_projects"."visibility" IN ('public', 'private')),
	CONSTRAINT "api_cloud_projects_state_check" CHECK ("api_cloud_projects"."state" IN ('connected', 'preparing', 'ready', 'failed'))
);

CREATE TABLE "api_cloud_provider_connections" (
	"connection_id" text PRIMARY KEY NOT NULL,
	"account_id" text NOT NULL,
	"provider" text NOT NULL,
	"envelope" text NOT NULL,
	"active" boolean DEFAULT true NOT NULL,
	"template_id" text,
	"organization" text,
	"created_at" bigint NOT NULL,
	CONSTRAINT "cloud_provider_connection_provider" CHECK ("api_cloud_provider_connections"."provider" IN ('e2b', 'boxd', 'box', 'daytona'))
);

CREATE TABLE "api_cloud_runtime_observations" (
	"provider" text NOT NULL,
	"provider_sandbox_id" text NOT NULL,
	"observation" jsonb NOT NULL,
	CONSTRAINT "api_cloud_runtime_observations_provider_provider_sandbox_id_pk" PRIMARY KEY("provider","provider_sandbox_id")
);

CREATE TABLE "api_cloud_snapshot_leases" (
	"account_id" text NOT NULL,
	"owner" text NOT NULL,
	"mode" text DEFAULT 'exclusive' NOT NULL,
	"expires_at" bigint NOT NULL,
	CONSTRAINT "api_cloud_snapshot_leases_account_id_owner_pk" PRIMARY KEY("account_id","owner")
);

CREATE TABLE "api_cloud_snapshots" (
	"provider" text NOT NULL,
	"snapshot_id" text NOT NULL,
	"account_id" text NOT NULL,
	"build_id" text NOT NULL,
	"state" text NOT NULL,
	"created_at" bigint NOT NULL,
	"record" jsonb NOT NULL,
	CONSTRAINT "api_cloud_snapshots_provider_snapshot_id_pk" PRIMARY KEY("provider","snapshot_id"),
	CONSTRAINT "api_cloud_snapshots_provider_check" CHECK ("api_cloud_snapshots"."provider" = 'box'),
	CONSTRAINT "api_cloud_snapshots_state_check" CHECK ("api_cloud_snapshots"."state" IN ('creating','retained','deleting','deleted'))
);

CREATE TABLE "api_cloud_transcript_checkpoints" (
	"workspace_id" text NOT NULL,
	"session_id" text NOT NULL,
	"runtime_generation" bigint NOT NULL,
	"stream_epoch" text NOT NULL,
	"stream_version" bigint NOT NULL,
	"object_key" text NOT NULL,
	"ciphertext_sha256" text NOT NULL,
	"ciphertext_bytes" bigint NOT NULL,
	"created_at" bigint NOT NULL,
	CONSTRAINT "api_cloud_transcript_checkpoints_workspace_id_session_id_pk" PRIMARY KEY("workspace_id","session_id")
);

CREATE TABLE "api_cloud_usage_outbox" (
	"event_id" text PRIMARY KEY NOT NULL,
	"payload" jsonb NOT NULL,
	"created_at" bigint NOT NULL,
	"next_attempt_at" bigint NOT NULL,
	"attempt_count" bigint DEFAULT 0 NOT NULL,
	"acknowledged_at" bigint,
	"last_error" text
);

CREATE TABLE "api_cloud_workspace_api_messages" (
	"message_id" text PRIMARY KEY NOT NULL,
	"workspace_id" text NOT NULL,
	"account_id" text NOT NULL,
	"seq" bigint NOT NULL,
	"role" text NOT NULL,
	"sealed_content" text NOT NULL,
	"command_id" text,
	"turn_id" text,
	"outcome" text,
	"status" text NOT NULL,
	"created_at" bigint NOT NULL,
	"delivery_attempted_at" bigint,
	"delivered_at" bigint,
	"expires_at" bigint,
	CONSTRAINT "api_cloud_workspace_api_messages_role_check" CHECK ("api_cloud_workspace_api_messages"."role" IN ('user', 'assistant')),
	CONSTRAINT "api_cloud_workspace_api_messages_status_check" CHECK ("api_cloud_workspace_api_messages"."status" IN ('pending', 'delivered', 'settled', 'failed', 'expired'))
);

CREATE TABLE "api_cloud_workspace_api_turn_receipts" (
	"workspace_id" text NOT NULL,
	"turn_id" text NOT NULL,
	"outcome" text NOT NULL,
	"settled_at" bigint NOT NULL,
	"received_at" bigint NOT NULL,
	"content_digest" text NOT NULL,
	CONSTRAINT "api_cloud_workspace_api_turn_receipts_pk" PRIMARY KEY("workspace_id","turn_id"),
	CONSTRAINT "api_cloud_workspace_api_turn_receipts_outcome_check" CHECK ("api_cloud_workspace_api_turn_receipts"."outcome" IN ('completed', 'interrupted', 'error'))
);

CREATE TABLE "api_cloud_workspace_command_receipts" (
	"workspace_id" text NOT NULL,
	"command_id" text NOT NULL,
	"action" text NOT NULL,
	"workspace_revision" bigint NOT NULL,
	"created_at" bigint NOT NULL,
	CONSTRAINT "api_cloud_workspace_command_receipts_workspace_id_command_id_pk" PRIMARY KEY("workspace_id","command_id")
);

CREATE TABLE "api_cloud_workspace_launch_intents" (
	"workspace_id" text PRIMARY KEY NOT NULL,
	"account_id" text NOT NULL,
	"chat_id" text NOT NULL,
	"session_id" text NOT NULL,
	"turn_id" text NOT NULL,
	"command_id" text NOT NULL,
	"ciphertext" text NOT NULL,
	"expires_at" bigint NOT NULL,
	"created_at" bigint NOT NULL,
	CONSTRAINT "api_cloud_workspace_launch_intents_command_id_unique" UNIQUE("command_id")
);

CREATE TABLE "api_cloud_workspace_runtime_summaries" (
	"workspace_id" text PRIMARY KEY NOT NULL,
	"runtime_generation" bigint NOT NULL,
	"summary_revision" bigint NOT NULL,
	"native_agent_access" jsonb DEFAULT '[]'::jsonb NOT NULL,
	"title" text NOT NULL,
	"last_activity_at" bigint NOT NULL,
	"last_user_message_at" bigint,
	"active_session_id" text,
	"session_head_version" bigint NOT NULL,
	"updated_at" bigint NOT NULL
);

CREATE TABLE "api_cloud_workspace_usage" (
	"event_id" text PRIMARY KEY NOT NULL,
	"workspace_id" text NOT NULL,
	"account_id" text NOT NULL,
	"provider" text NOT NULL,
	"kind" text NOT NULL,
	"quantity" bigint NOT NULL,
	"provider_event_id" text,
	"occurred_at" bigint NOT NULL,
	"created_at" bigint NOT NULL,
	CONSTRAINT "api_cloud_usage_kind_check" CHECK ("api_cloud_workspace_usage"."kind" IN ('runtime-seconds', 'lifecycle-elapsed-seconds', 'pause', 'resume', 'snapshot-bytes', 'storage-byte-seconds', 'archive', 'restore', 'delete'))
);

CREATE TABLE "api_cloud_workspaces" (
	"workspace_id" text PRIMARY KEY NOT NULL,
	"account_id" text NOT NULL,
	"project_id" text NOT NULL,
	"build_id" text NOT NULL,
	"provider" text NOT NULL,
	"provider_sandbox_id" text,
	"runtime_boot_token_hash" text,
	"runtime_boot_token_expires_at" bigint,
	"runtime_credential_hash" text,
	"runtime_state" text DEFAULT 'offline' NOT NULL,
	"chat_id" text NOT NULL,
	"initial_session_id" text NOT NULL,
	"branch" text NOT NULL,
	"base_ref" text NOT NULL,
	"state" text NOT NULL,
	"desired_state" text NOT NULL,
	"status_code" text NOT NULL,
	"wrapped_transcript_key" text,
	"archive_requested_at" bigint,
	"archive_delete_at" bigint,
	"deletion_tombstone_expires_at" bigint,
	"idempotency_key" text NOT NULL,
	"request_config" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"next_action_at" bigint NOT NULL,
	"lease_owner" text,
	"lease_expires_at" bigint,
	"revision" bigint DEFAULT 0 NOT NULL,
	"created_at" bigint NOT NULL,
	"updated_at" bigint NOT NULL,
	"last_activity_at" bigint NOT NULL,
	"running_since" bigint,
	"deleted_at" bigint,
	CONSTRAINT "api_cloud_workspaces_state_check" CHECK ("api_cloud_workspaces"."state" IN ('queued', 'provisioning', 'setup', 'ready', 'pausing', 'paused', 'resuming', 'archiving', 'archived', 'deleting', 'deleted', 'failed')),
	CONSTRAINT "api_cloud_workspaces_desired_state_check" CHECK ("api_cloud_workspaces"."desired_state" IN ('ready', 'paused', 'archived', 'deleted')),
	CONSTRAINT "api_cloud_workspaces_runtime_state_check" CHECK ("api_cloud_workspaces"."runtime_state" IN ('offline', 'connecting', 'online'))
);

CREATE TABLE "api_devices" (
	"device_id" text PRIMARY KEY NOT NULL,
	"account_id" text NOT NULL,
	"platform" text NOT NULL,
	"push_token" text,
	"dpop_jwk" jsonb,
	"dpop_thumbprint" text NOT NULL,
	"updated_at" bigint NOT NULL,
	CONSTRAINT "api_devices_platform_check" CHECK ("api_devices"."platform" IN ('ios', 'android', 'web', 'desktop'))
);

CREATE TABLE "api_dpop_proofs" (
	"thumbprint" text NOT NULL,
	"jti" text NOT NULL,
	"issued_at" bigint NOT NULL,
	"expires_at" bigint NOT NULL,
	CONSTRAINT "api_dpop_proofs_thumbprint_jti_pk" PRIMARY KEY("thumbprint","jti")
);

CREATE TABLE "api_entitlements" (
	"entitlement_id" text PRIMARY KEY NOT NULL,
	"account_id" text NOT NULL,
	"kind" text NOT NULL,
	"offer_id" text,
	"provider" text NOT NULL,
	"provider_subscription_id" text,
	"status" text NOT NULL,
	"period_start" bigint,
	"paid_through" bigint,
	"ended_at" bigint,
	"credit_balance" bigint,
	"created_at" bigint NOT NULL,
	"updated_at" bigint NOT NULL,
	CONSTRAINT "api_entitlements_kind_check" CHECK ("api_entitlements"."kind" IN ('persistent-machine', 'cloud-workspace', 'usage-credits')),
	CONSTRAINT "api_entitlements_status_check" CHECK ("api_entitlements"."status" IN ('pending', 'active', 'grace', 'ended'))
);

CREATE TABLE "api_environment_credentials" (
	"credential_id" text PRIMARY KEY NOT NULL,
	"environment_id" text NOT NULL,
	"account_id" text NOT NULL,
	"credential_hash" text NOT NULL,
	"created_at" bigint NOT NULL,
	"revoked_at" bigint
);

CREATE TABLE "api_environments" (
	"environment_id" text PRIMARY KEY NOT NULL,
	"account_id" text NOT NULL,
	"org_id" text,
	"provider_kind" text NOT NULL,
	"label" text,
	"environment_public_key" text NOT NULL,
	"http_base_url" text NOT NULL,
	"ws_base_url" text NOT NULL,
	"private_http_base_url" text,
	"private_ws_base_url" text,
	"tunnel_hostname" text,
	"tunnel_id" text,
	"dns_record_id" text,
	"tunnel_status" text,
	"linked_at" bigint NOT NULL,
	"last_seen_at" bigint,
	"runtime_version" text,
	"wire_protocol_version" bigint,
	"capabilities" jsonb,
	"sharing_audience" jsonb,
	"service_state" text,
	CONSTRAINT "api_environments_provider_kind_check" CHECK ("api_environments"."provider_kind" IN ('desktop', 'ssh', 'cloud')),
	CONSTRAINT "api_environments_tunnel_status_check" CHECK ("api_environments"."tunnel_status" IS NULL OR "api_environments"."tunnel_status" IN ('reserved', 'ready')),
	CONSTRAINT "api_environments_service_state_check" CHECK ("api_environments"."service_state" IS NULL OR "api_environments"."service_state" IN ('starting', 'healthy', 'degraded', 'stopped', 'updating'))
);

CREATE TABLE "api_github_enrollments" (
	"organization_id" text NOT NULL,
	"account_id" text NOT NULL,
	"installation_id" bigint NOT NULL,
	"data" jsonb NOT NULL,
	CONSTRAINT "api_github_enrollments_organization_id_account_id_pk" PRIMARY KEY("organization_id","account_id")
);

CREATE TABLE "api_github_identities" (
	"account_id" text PRIMARY KEY NOT NULL,
	"github_user_id" bigint NOT NULL,
	"data" jsonb NOT NULL,
	CONSTRAINT "api_github_identities_github_user_id_unique" UNIQUE("github_user_id")
);

CREATE TABLE "api_github_join_policies" (
	"organization_id" text NOT NULL,
	"installation_id" bigint NOT NULL,
	"github_org_id" bigint NOT NULL,
	"data" jsonb NOT NULL,
	CONSTRAINT "api_github_join_policies_organization_id_installation_id_pk" PRIMARY KEY("organization_id","installation_id")
);

CREATE TABLE "api_github_org_members" (
	"installation_id" bigint NOT NULL,
	"github_user_id" bigint NOT NULL,
	CONSTRAINT "api_github_org_members_installation_id_github_user_id_pk" PRIMARY KEY("installation_id","github_user_id")
);

CREATE TABLE "api_link_challenges" (
	"challenge_id" text PRIMARY KEY NOT NULL,
	"account_id" text NOT NULL,
	"challenge" text NOT NULL,
	"api_issuer" text NOT NULL,
	"expires_at" bigint NOT NULL,
	"consumed_at" bigint
);

CREATE TABLE "api_machines" (
	"machine_id" text PRIMARY KEY NOT NULL,
	"account_id" text NOT NULL,
	"offer_id" text NOT NULL,
	"idempotency_key" text NOT NULL,
	"environment_id" text,
	"provider" text NOT NULL,
	"provider_server_id" text,
	"provider_label" text NOT NULL,
	"label" text,
	"state" text NOT NULL,
	"desired_state" text NOT NULL,
	"status_code" text NOT NULL,
	"boot_phase" text,
	"stable_failure_code" text,
	"last_error" text,
	"entitlement_id" text NOT NULL,
	"enrollment_token_hash" text,
	"enrollment_expires_at" bigint,
	"enrolled_environment_public_key" text,
	"final_snapshot_id" text,
	"final_snapshot_delete_at" bigint,
	"paid_through" bigint,
	"recovery_deadline" bigint,
	"account_deletion_requested_at" bigint,
	"credential_cleanup_requested_at" bigint,
	"credential_cleanup_deadline" bigint,
	"credential_cleanup_completed_at" bigint,
	"final_snapshot_skipped" boolean DEFAULT false NOT NULL,
	"next_action_at" bigint NOT NULL,
	"attempt_count" bigint DEFAULT 0 NOT NULL,
	"lease_owner" text,
	"lease_expires_at" bigint,
	"revision" bigint DEFAULT 0 NOT NULL,
	"created_at" bigint NOT NULL,
	"updated_at" bigint NOT NULL,
	"suspended_at" bigint,
	"destroyed_at" bigint,
	CONSTRAINT "api_machines_state_check" CHECK ("api_machines"."state" IN ('creating', 'bootstrapping', 'enrolling', 'ready', 'suspending', 'suspended', 'resuming', 'destroying', 'destroyed', 'failed')),
	CONSTRAINT "api_machines_desired_state_check" CHECK ("api_machines"."desired_state" IN ('ready', 'suspended', 'destroyed'))
);

CREATE TABLE "api_master_ownership" (
	"account_id" text NOT NULL,
	"master_id" text NOT NULL,
	"home_environment_id" text NOT NULL,
	"generation" integer NOT NULL,
	"repositories_json" text NOT NULL,
	CONSTRAINT "api_master_ownership_account_id_master_id_pk" PRIMARY KEY("account_id","master_id")
);

CREATE TABLE "api_model_connection_leases" (
	"account_id" text NOT NULL,
	"provider" text NOT NULL,
	"token" text NOT NULL,
	"expires_at_ms" bigint NOT NULL,
	CONSTRAINT "api_model_connection_leases_account_id_provider_pk" PRIMARY KEY("account_id","provider"),
	CONSTRAINT "model_connection_lease_provider" CHECK ("api_model_connection_leases"."provider" IN ('chatgpt','supergrok'))
);

CREATE TABLE "api_model_connections" (
	"account_id" text NOT NULL,
	"provider" text NOT NULL,
	"kind" text NOT NULL,
	"connection_id" text NOT NULL,
	"envelope" text NOT NULL,
	CONSTRAINT "api_model_connections_account_id_provider_kind_connection_id_pk" PRIMARY KEY("account_id","provider","kind","connection_id"),
	CONSTRAINT "model_connection_provider" CHECK ("api_model_connections"."provider" IN ('chatgpt','supergrok')),
	CONSTRAINT "model_connection_kind" CHECK ("api_model_connections"."kind" IN ('active','pending'))
);

CREATE TABLE "api_organization_domain_enrollments" (
	"organization_id" text NOT NULL,
	"account_id" text NOT NULL,
	"data" jsonb NOT NULL,
	CONSTRAINT "api_organization_domain_enrollments_organization_id_account_id_pk" PRIMARY KEY("organization_id","account_id")
);

CREATE TABLE "api_organization_domains" (
	"domain" text PRIMARY KEY NOT NULL,
	"organization_id" text NOT NULL,
	"data" jsonb NOT NULL
);

CREATE TABLE "api_platform_costs" (
	"cost_id" text PRIMARY KEY NOT NULL,
	"vendor" text NOT NULL,
	"kind" text NOT NULL,
	"amount_micros" bigint NOT NULL,
	"currency" text DEFAULT 'USD' NOT NULL,
	"external_id" text,
	"period_start" bigint NOT NULL,
	"period_end" bigint NOT NULL,
	"metadata" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"created_at" bigint NOT NULL
);

CREATE TABLE "api_provider_event_deliveries" (
	"provider" text NOT NULL,
	"delivery_id" text NOT NULL,
	"event_id" text NOT NULL,
	"source" text NOT NULL,
	"status" text NOT NULL,
	"received_at" bigint NOT NULL,
	CONSTRAINT "api_provider_event_deliveries_provider_delivery_id_pk" PRIMARY KEY("provider","delivery_id")
);

CREATE TABLE "api_provider_event_finalizations" (
	"provider" text NOT NULL,
	"event_id" text NOT NULL,
	"provider_execution_id" text,
	"finalized_at" bigint NOT NULL,
	"expires_at" bigint NOT NULL,
	CONSTRAINT "api_provider_event_finalizations_provider_event_id_pk" PRIMARY KEY("provider","event_id")
);

CREATE TABLE "api_provider_lifecycle_pairs" (
	"provider" text NOT NULL,
	"closing_event_id" text NOT NULL,
	"opening_event_id" text NOT NULL,
	"started_at" bigint NOT NULL,
	CONSTRAINT "api_provider_lifecycle_pairs_provider_closing_event_id_pk" PRIMARY KEY("provider","closing_event_id")
);

CREATE TABLE "api_provider_price_schedule" (
	"provider" text NOT NULL,
	"version" text NOT NULL,
	"effective_at" bigint NOT NULL,
	"base_nano_usd_per_second" bigint DEFAULT 0 NOT NULL,
	"cpu_nano_usd_per_second" bigint NOT NULL,
	"memory_nano_usd_per_gib_second" bigint NOT NULL,
	"storage_nano_usd_per_gib_second" bigint DEFAULT 0 NOT NULL,
	"created_at" bigint NOT NULL,
	CONSTRAINT "api_provider_price_schedule_provider_version_pk" PRIMARY KEY("provider","version")
);

CREATE TABLE "api_provider_statement_totals" (
	"statement_id" text PRIMARY KEY NOT NULL,
	"provider" text NOT NULL,
	"external_id" text NOT NULL,
	"amount_micros" bigint NOT NULL,
	"currency" text DEFAULT 'USD' NOT NULL,
	"period_start" bigint NOT NULL,
	"period_end" bigint NOT NULL,
	"metadata" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"created_at" bigint NOT NULL
);

CREATE TABLE "api_provider_usage_events" (
	"event_id" text NOT NULL,
	"provider" text NOT NULL,
	"type" text NOT NULL,
	"provider_resource_id" text,
	"occurred_at" bigint,
	"payload" jsonb NOT NULL,
	"received_at" bigint NOT NULL,
	"expires_at" bigint NOT NULL,
	CONSTRAINT "api_provider_usage_events_provider_event_id_pk" PRIMARY KEY("provider","event_id")
);

CREATE TABLE "api_revoked_dpop_keys" (
	"thumbprint" text PRIMARY KEY NOT NULL,
	"revoked_at" bigint NOT NULL
);

CREATE TABLE "api_slack_installations" (
	"team_id" text PRIMARY KEY NOT NULL,
	"owner_id" text NOT NULL,
	"generation" text NOT NULL,
	"account_id" text,
	"revision" integer DEFAULT 0 NOT NULL,
	"sealed" text NOT NULL,
	CONSTRAINT "api_slack_installations_team_id_generation_unique" UNIQUE("team_id","generation")
);

CREATE TABLE "api_slack_members" (
	"team_id" text NOT NULL,
	"generation" text NOT NULL,
	"user_id" text NOT NULL,
	"account_id" text,
	"revision" integer DEFAULT 0 NOT NULL,
	"sealed" text NOT NULL,
	CONSTRAINT "api_slack_members_team_id_generation_user_id_pk" PRIMARY KEY("team_id","generation","user_id")
);

CREATE TABLE "api_slack_sessions" (
	"token_hash" text PRIMARY KEY NOT NULL,
	"kind" text NOT NULL,
	"team_id" text NOT NULL,
	"owner_id" text NOT NULL,
	"generation" text NOT NULL,
	"expires_at" bigint NOT NULL,
	"payload" text NOT NULL,
	CONSTRAINT "api_slack_sessions_kind_check" CHECK ("api_slack_sessions"."kind" IN ('oauth', 'login', 'settings', 'workos'))
);

CREATE TABLE "api_slack_state" (
	"team_id" text NOT NULL,
	"generation" text NOT NULL,
	"key" text NOT NULL,
	"value" text NOT NULL,
	"expires_at" bigint NOT NULL,
	CONSTRAINT "api_slack_state_team_id_generation_key_pk" PRIMARY KEY("team_id","generation","key")
);

CREATE TABLE "api_snapshot_price_schedule" (
	"provider" text NOT NULL,
	"version" text NOT NULL,
	"monthly_micros" bigint NOT NULL,
	"duration_ms" bigint NOT NULL,
	CONSTRAINT "api_snapshot_price_schedule_provider_version_pk" PRIMARY KEY("provider","version"),
	CONSTRAINT "api_snapshot_price_schedule_monthly_micros_check" CHECK ("api_snapshot_price_schedule"."monthly_micros" >= 0),
	CONSTRAINT "api_snapshot_price_schedule_duration_ms_check" CHECK ("api_snapshot_price_schedule"."duration_ms" = 2592000000)
);

CREATE TABLE "api_stripe_customers" (
	"recovery_cursor" text,
	"recovery_matches" jsonb DEFAULT '[]'::jsonb NOT NULL,
	"recovery_complete" boolean DEFAULT false NOT NULL,
	"recovery_attempted_at" bigint,
	"recovery_lease_until" bigint DEFAULT 0 NOT NULL,
	"generation" integer DEFAULT 0 NOT NULL,
	"reservation_created_at" bigint,
	"account_id" text PRIMARY KEY NOT NULL,
	"customer_id" text,
	"created_at" bigint NOT NULL,
	CONSTRAINT "api_stripe_customers_customer_id_unique" UNIQUE("customer_id")
);

CREATE TABLE "api_stripe_meter_deliveries" (
	"delivery_key" text PRIMARY KEY NOT NULL,
	"payload" text NOT NULL,
	"first_attempt_at" bigint NOT NULL,
	"lease_until" bigint NOT NULL,
	"sent_at" bigint
);

CREATE TABLE "api_stripe_subscription_migrations" (
	"polar_subscription_id" text PRIMARY KEY NOT NULL,
	"account_id" text NOT NULL,
	"customer_id" text NOT NULL,
	"payload" text NOT NULL,
	"first_attempt_at" bigint NOT NULL,
	"schedule_id" text,
	CONSTRAINT "api_stripe_subscription_migrations_account_id_unique" UNIQUE("account_id"),
	CONSTRAINT "api_stripe_subscription_migrations_customer_id_unique" UNIQUE("customer_id"),
	CONSTRAINT "api_stripe_subscription_migrations_schedule_id_unique" UNIQUE("schedule_id")
);

CREATE TABLE "api_workspace_settings" (
	"owner_id" text PRIMARY KEY NOT NULL,
	"revision" integer NOT NULL,
	"values" jsonb NOT NULL,
	CONSTRAINT "api_workspace_settings_revision_positive" CHECK ("api_workspace_settings"."revision" > 0),
	CONSTRAINT "api_workspace_settings_values_object" CHECK (jsonb_typeof("api_workspace_settings"."values") = 'object')
);

ALTER TABLE "api_api_webhook_deliveries" ADD CONSTRAINT "api_api_webhook_deliveries_webhook_id_api_api_webhooks_webhook_id_fk" FOREIGN KEY ("webhook_id") REFERENCES "api_api_webhooks"("webhook_id") ON DELETE cascade ON UPDATE no action;
ALTER TABLE "api_cloud_project_builds" ADD CONSTRAINT "api_cloud_project_builds_project_id_api_cloud_projects_project_id_fk" FOREIGN KEY ("project_id") REFERENCES "api_cloud_projects"("project_id") ON DELETE cascade ON UPDATE no action;
ALTER TABLE "api_cloud_transcript_checkpoints" ADD CONSTRAINT "api_cloud_transcript_checkpoints_workspace_id_api_cloud_workspaces_workspace_id_fk" FOREIGN KEY ("workspace_id") REFERENCES "api_cloud_workspaces"("workspace_id") ON DELETE cascade ON UPDATE no action;
ALTER TABLE "api_cloud_workspace_api_messages" ADD CONSTRAINT "api_cloud_workspace_api_messages_workspace_id_api_cloud_workspaces_workspace_id_fk" FOREIGN KEY ("workspace_id") REFERENCES "api_cloud_workspaces"("workspace_id") ON DELETE cascade ON UPDATE no action;
ALTER TABLE "api_cloud_workspace_api_turn_receipts" ADD CONSTRAINT "api_cloud_workspace_api_turn_receipts_workspace_id_api_cloud_workspaces_workspace_id_fk" FOREIGN KEY ("workspace_id") REFERENCES "api_cloud_workspaces"("workspace_id") ON DELETE cascade ON UPDATE no action;
ALTER TABLE "api_cloud_workspace_command_receipts" ADD CONSTRAINT "api_cloud_workspace_command_receipts_workspace_id_api_cloud_workspaces_workspace_id_fk" FOREIGN KEY ("workspace_id") REFERENCES "api_cloud_workspaces"("workspace_id") ON DELETE cascade ON UPDATE no action;
ALTER TABLE "api_cloud_workspace_launch_intents" ADD CONSTRAINT "api_cloud_workspace_launch_intents_workspace_id_api_cloud_workspaces_workspace_id_fk" FOREIGN KEY ("workspace_id") REFERENCES "api_cloud_workspaces"("workspace_id") ON DELETE cascade ON UPDATE no action;
ALTER TABLE "api_cloud_workspace_runtime_summaries" ADD CONSTRAINT "api_cloud_workspace_runtime_summaries_workspace_id_api_cloud_workspaces_workspace_id_fk" FOREIGN KEY ("workspace_id") REFERENCES "api_cloud_workspaces"("workspace_id") ON DELETE cascade ON UPDATE no action;
ALTER TABLE "api_cloud_workspace_usage" ADD CONSTRAINT "api_cloud_workspace_usage_workspace_id_api_cloud_workspaces_workspace_id_fk" FOREIGN KEY ("workspace_id") REFERENCES "api_cloud_workspaces"("workspace_id") ON DELETE no action ON UPDATE no action;
ALTER TABLE "api_cloud_workspaces" ADD CONSTRAINT "api_cloud_workspaces_project_id_api_cloud_projects_project_id_fk" FOREIGN KEY ("project_id") REFERENCES "api_cloud_projects"("project_id") ON DELETE no action ON UPDATE no action;
ALTER TABLE "api_cloud_workspaces" ADD CONSTRAINT "api_cloud_workspaces_build_id_api_cloud_project_builds_build_id_fk" FOREIGN KEY ("build_id") REFERENCES "api_cloud_project_builds"("build_id") ON DELETE no action ON UPDATE no action;
ALTER TABLE "api_environment_credentials" ADD CONSTRAINT "api_environment_credentials_environment_id_api_environments_environment_id_fk" FOREIGN KEY ("environment_id") REFERENCES "api_environments"("environment_id") ON DELETE cascade ON UPDATE no action;
ALTER TABLE "api_machines" ADD CONSTRAINT "api_machines_entitlement_id_api_entitlements_entitlement_id_fk" FOREIGN KEY ("entitlement_id") REFERENCES "api_entitlements"("entitlement_id") ON DELETE no action ON UPDATE no action;
ALTER TABLE "api_provider_lifecycle_pairs" ADD CONSTRAINT "api_provider_lifecycle_pairs_provider_closing_event_id_api_provider_usage_events_provider_event_id_fk" FOREIGN KEY ("provider","closing_event_id") REFERENCES "api_provider_usage_events"("provider","event_id") ON DELETE cascade ON UPDATE no action;
ALTER TABLE "api_provider_lifecycle_pairs" ADD CONSTRAINT "api_provider_lifecycle_pairs_provider_opening_event_id_api_provider_usage_events_provider_event_id_fk" FOREIGN KEY ("provider","opening_event_id") REFERENCES "api_provider_usage_events"("provider","event_id") ON DELETE cascade ON UPDATE no action;
ALTER TABLE "api_slack_members" ADD CONSTRAINT "api_slack_members_team_id_generation_api_slack_installations_team_id_generation_fk" FOREIGN KEY ("team_id","generation") REFERENCES "api_slack_installations"("team_id","generation") ON DELETE cascade ON UPDATE no action;
ALTER TABLE "api_slack_state" ADD CONSTRAINT "api_slack_state_team_id_generation_api_slack_installations_team_id_generation_fk" FOREIGN KEY ("team_id","generation") REFERENCES "api_slack_installations"("team_id","generation") ON DELETE cascade ON UPDATE no action;
CREATE INDEX "api_agent_activity_env_idx" ON "api_agent_activity" USING btree ("environment_id","occurred_at");
CREATE INDEX "api_api_keys_account_idx" ON "api_api_keys" USING btree ("account_id");
CREATE UNIQUE INDEX "api_api_keys_secret_hash_idx" ON "api_api_keys" USING btree ("secret_hash");
CREATE UNIQUE INDEX "api_api_webhook_deliveries_event_idx" ON "api_api_webhook_deliveries" USING btree ("webhook_id","event_id");
CREATE INDEX "api_api_webhook_deliveries_due_idx" ON "api_api_webhook_deliveries" USING btree ("status","next_attempt_at");
CREATE INDEX "api_api_webhooks_account_idx" ON "api_api_webhooks" USING btree ("account_id");
CREATE INDEX "api_cloud_auth_authorities_state_idx" ON "api_cloud_auth_authorities" USING btree ("state","provisioning_lease_expires_at");
CREATE UNIQUE INDEX "api_cloud_billing_ledger_source_idx" ON "api_cloud_billing_ledger" USING btree ("kind","source_id");
CREATE INDEX "api_cloud_billing_ledger_period_idx" ON "api_cloud_billing_ledger" USING btree ("period_id","occurred_at");
CREATE INDEX "api_cloud_billing_meter_reconciliation_period_idx" ON "api_cloud_billing_meter_reconciliations" USING btree ("period_id","created_at");
CREATE UNIQUE INDEX "api_cloud_billing_outbox_idempotency_idx" ON "api_cloud_billing_outbox" USING btree ("provider","idempotency_key");
CREATE UNIQUE INDEX "api_cloud_billing_period_account_start_idx" ON "api_cloud_billing_periods" USING btree ("account_id","period_start");
CREATE INDEX "api_cloud_billing_period_active_idx" ON "api_cloud_billing_periods" USING btree ("account_id","period_end");
CREATE INDEX "api_cloud_billing_reservations_expiry_idx" ON "api_cloud_billing_reservations" USING btree ("expires_at");
CREATE UNIQUE INDEX "api_cloud_billing_usage_provider_event_idx" ON "api_cloud_billing_usage" USING btree ("provider","provider_event_id");
CREATE UNIQUE INDEX "api_cloud_billing_usage_provider_execution_idx" ON "api_cloud_billing_usage" USING btree ("provider","provider_execution_id","period_id");
CREATE INDEX "api_cloud_billing_usage_period_idx" ON "api_cloud_billing_usage" USING btree ("period_id","started_at");
CREATE INDEX "api_cloud_catalog_changes_cursor_idx" ON "api_cloud_catalog_changes" USING btree ("account_id","revision");
CREATE INDEX "api_cloud_github_installations_account_idx" ON "api_cloud_github_installations" USING btree ("account_id");
CREATE UNIQUE INDEX "api_cloud_imported_builds_idempotency_idx" ON "api_cloud_project_builds" USING btree ("account_id","provider","idempotency_key") WHERE "api_cloud_project_builds"."project_id" IS NULL;
CREATE UNIQUE INDEX "api_cloud_builds_project_provider_idempotency_idx" ON "api_cloud_project_builds" USING btree ("project_id","provider","idempotency_key");
CREATE INDEX "api_cloud_builds_active_idx" ON "api_cloud_project_builds" USING btree ("project_id","provider","state","updated_at");
CREATE INDEX "api_cloud_builds_reconcile_idx" ON "api_cloud_project_builds" USING btree ("state","next_action_at","lease_expires_at");
CREATE UNIQUE INDEX "api_cloud_projects_account_repo_idx" ON "api_cloud_projects" USING btree ("account_id","repository_identity");
CREATE UNIQUE INDEX "api_cloud_projects_account_idempotency_idx" ON "api_cloud_projects" USING btree ("account_id","idempotency_key");
CREATE INDEX "api_cloud_projects_account_idx" ON "api_cloud_projects" USING btree ("account_id");
CREATE UNIQUE INDEX "api_cloud_provider_connections_active" ON "api_cloud_provider_connections" USING btree ("account_id","provider") WHERE "api_cloud_provider_connections"."active";
CREATE INDEX "api_cloud_provider_connections_owner" ON "api_cloud_provider_connections" USING btree ("account_id");
CREATE INDEX "api_cloud_snapshots_account_idx" ON "api_cloud_snapshots" USING btree ("account_id","state");
CREATE UNIQUE INDEX "api_cloud_snapshots_retained_idx" ON "api_cloud_snapshots" USING btree ("account_id","provider") WHERE "api_cloud_snapshots"."state" = 'retained';
CREATE INDEX "api_cloud_snapshots_live_idx" ON "api_cloud_snapshots" USING btree ("state") WHERE "api_cloud_snapshots"."state" != 'deleted';
CREATE INDEX "api_cloud_snapshots_unsettled_idx" ON "api_cloud_snapshots" USING btree ("account_id") WHERE "api_cloud_snapshots"."state" = 'deleted' AND ("api_cloud_snapshots"."record"->>'retainedAtMs')::bigint IS NOT NULL AND COALESCE(("api_cloud_snapshots"."record"->>'checkpointAtMs')::bigint, ("api_cloud_snapshots"."record"->>'retainedAtMs')::bigint) < ("api_cloud_snapshots"."record"->>'stoppedAtMs')::bigint;
CREATE INDEX "api_cloud_transcript_checkpoint_created_idx" ON "api_cloud_transcript_checkpoints" USING btree ("created_at");
CREATE INDEX "api_cloud_usage_outbox_pending_idx" ON "api_cloud_usage_outbox" USING btree ("next_attempt_at","event_id") WHERE "api_cloud_usage_outbox"."acknowledged_at" IS NULL;
CREATE UNIQUE INDEX "api_cloud_workspace_api_messages_seq_idx" ON "api_cloud_workspace_api_messages" USING btree ("workspace_id","seq");
CREATE UNIQUE INDEX "api_cloud_workspace_api_messages_turn_idx" ON "api_cloud_workspace_api_messages" USING btree ("workspace_id","turn_id") WHERE "api_cloud_workspace_api_messages"."role" = 'assistant' AND "api_cloud_workspace_api_messages"."turn_id" IS NOT NULL;
CREATE INDEX "api_cloud_workspace_api_messages_pending_idx" ON "api_cloud_workspace_api_messages" USING btree ("workspace_id","status");
CREATE INDEX "api_cloud_workspace_api_messages_pending_expiry_idx" ON "api_cloud_workspace_api_messages" USING btree ("expires_at") WHERE "api_cloud_workspace_api_messages"."role" = 'user' AND "api_cloud_workspace_api_messages"."status" IN ('pending', 'delivered') AND "api_cloud_workspace_api_messages"."expires_at" IS NOT NULL;
CREATE INDEX "api_cloud_workspace_api_messages_pending_stale_idx" ON "api_cloud_workspace_api_messages" USING btree ("created_at","workspace_id") WHERE "api_cloud_workspace_api_messages"."role" = 'user' AND "api_cloud_workspace_api_messages"."status" = 'pending';
CREATE INDEX "api_cloud_workspace_launch_intents_expiry_idx" ON "api_cloud_workspace_launch_intents" USING btree ("expires_at");
CREATE INDEX "api_cloud_runtime_summaries_activity_idx" ON "api_cloud_workspace_runtime_summaries" USING btree ("last_activity_at");
CREATE UNIQUE INDEX "api_cloud_usage_provider_event_idx" ON "api_cloud_workspace_usage" USING btree ("provider","provider_event_id");
CREATE INDEX "api_cloud_usage_workspace_idx" ON "api_cloud_workspace_usage" USING btree ("workspace_id","occurred_at");
CREATE UNIQUE INDEX "api_cloud_workspaces_account_idempotency_idx" ON "api_cloud_workspaces" USING btree ("account_id","idempotency_key");
CREATE UNIQUE INDEX "api_cloud_workspaces_provider_sandbox_idx" ON "api_cloud_workspaces" USING btree ("provider","provider_sandbox_id");
CREATE UNIQUE INDEX "api_cloud_workspaces_chat_idx" ON "api_cloud_workspaces" USING btree ("chat_id");
CREATE INDEX "api_cloud_workspaces_project_idx" ON "api_cloud_workspaces" USING btree ("project_id","updated_at");
CREATE INDEX "api_cloud_workspaces_branch_lease_idx" ON "api_cloud_workspaces" USING btree ("project_id","branch","state");
CREATE UNIQUE INDEX "api_cloud_workspaces_active_branch_idx" ON "api_cloud_workspaces" USING btree ("project_id","branch") WHERE "api_cloud_workspaces"."state" <> 'deleted';
CREATE INDEX "api_cloud_workspaces_reconcile_idx" ON "api_cloud_workspaces" USING btree ("desired_state","next_action_at","lease_expires_at");
CREATE INDEX "api_devices_account_idx" ON "api_devices" USING btree ("account_id");
CREATE INDEX "api_dpop_proofs_expiry_idx" ON "api_dpop_proofs" USING btree ("expires_at");
CREATE INDEX "api_entitlements_account_idx" ON "api_entitlements" USING btree ("account_id");
CREATE INDEX "api_entitlements_subscription_idx" ON "api_entitlements" USING btree ("provider","provider_subscription_id");
CREATE INDEX "api_environment_credentials_env_idx" ON "api_environment_credentials" USING btree ("environment_id");
CREATE INDEX "api_environments_account_idx" ON "api_environments" USING btree ("account_id");
CREATE INDEX "api_environments_sharing_idx" ON "api_environments" USING gin ("sharing_audience");
CREATE INDEX "api_github_enrollments_installation" ON "api_github_enrollments" USING btree ("installation_id");
CREATE INDEX "api_github_join_policies_org" ON "api_github_join_policies" USING btree ("github_org_id");
CREATE INDEX "api_github_join_policies_installation" ON "api_github_join_policies" USING btree ("installation_id");
CREATE INDEX "api_github_org_members_user" ON "api_github_org_members" USING btree ("github_user_id");
CREATE INDEX "api_link_challenges_account_idx" ON "api_link_challenges" USING btree ("account_id");
CREATE INDEX "api_machines_account_idx" ON "api_machines" USING btree ("account_id");
CREATE UNIQUE INDEX "api_machines_account_idempotency_idx" ON "api_machines" USING btree ("account_id","idempotency_key");
CREATE UNIQUE INDEX "api_machines_enroll_token_idx" ON "api_machines" USING btree ("enrollment_token_hash");
CREATE UNIQUE INDEX "api_machines_environment_idx" ON "api_machines" USING btree ("environment_id");
CREATE UNIQUE INDEX "api_machines_provider_server_idx" ON "api_machines" USING btree ("provider","provider_server_id");
CREATE INDEX "api_machines_reconcile_idx" ON "api_machines" USING btree ("desired_state","next_action_at","lease_expires_at");
CREATE INDEX "api_organization_domains_org" ON "api_organization_domains" USING btree ("organization_id");
CREATE UNIQUE INDEX "api_platform_cost_vendor_external_idx" ON "api_platform_costs" USING btree ("vendor","external_id");
CREATE INDEX "api_provider_event_deliveries_event_idx" ON "api_provider_event_deliveries" USING btree ("provider","event_id");
CREATE INDEX "api_provider_event_finalizations_expiry_idx" ON "api_provider_event_finalizations" USING btree ("expires_at");
CREATE UNIQUE INDEX "api_provider_event_finalizations_execution_idx" ON "api_provider_event_finalizations" USING btree ("provider","provider_execution_id");
CREATE INDEX "api_provider_lifecycle_pairs_opening_idx" ON "api_provider_lifecycle_pairs" USING btree ("provider","opening_event_id");
CREATE UNIQUE INDEX "api_provider_price_schedule_effective_idx" ON "api_provider_price_schedule" USING btree ("provider","effective_at");
CREATE UNIQUE INDEX "api_provider_statement_external_idx" ON "api_provider_statement_totals" USING btree ("provider","external_id");
CREATE INDEX "api_provider_usage_events_expiry_idx" ON "api_provider_usage_events" USING btree ("expires_at");
CREATE INDEX "api_provider_usage_events_resource_time_idx" ON "api_provider_usage_events" USING btree ("provider","provider_resource_id","type","occurred_at");
CREATE INDEX "api_slack_installations_account_idx" ON "api_slack_installations" USING btree ("account_id");
CREATE INDEX "api_slack_members_account_idx" ON "api_slack_members" USING btree ("account_id");
CREATE INDEX "api_slack_sessions_expiry_idx" ON "api_slack_sessions" USING btree ("expires_at");
CREATE INDEX "api_slack_state_expiry_idx" ON "api_slack_state" USING btree ("expires_at");
CREATE INDEX "api_stripe_customer_recovery_pending_idx" ON "api_stripe_customers" USING btree (COALESCE("recovery_attempted_at", 0),"account_id") WHERE "api_stripe_customers"."customer_id" IS NULL AND NOT "api_stripe_customers"."recovery_complete";
