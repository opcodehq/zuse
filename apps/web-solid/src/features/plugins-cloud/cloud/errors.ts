// Human copy for cloud error codes. The server sends codes, not sentences (crates/cloud/src/error.rs), so the
// strings mirror desktop's (crates/viewport/src/cloud/preparation.rs `issue`, crates/engine/src/cloud.rs).

export type Recovery = "retry" | "agent" | "github" | "billing" | "environment" | "none";
export interface CloudIssue {
  title: string;
  detail: string;
  recovery: Recovery;
}

const AGENT = "Coding agent needs authorization";
const issues: [codes: string[], issue: CloudIssue][] = [
  [["cloud_provider_rejected"], { title: "Computer start was rejected", detail: "The cloud computer didn't start before the deadline, or the provider refused it. Your chat and first message are saved, so you can retry the original start.", recovery: "retry" }],
  [["cloud_runtime_launch_incompatible"], { title: "The computer couldn't restart after an update", detail: "Its runtime no longer matches the computer, and rolling back didn't help. Your chat is saved. Retrying may not fix this; start a new chat if it keeps happening.", recovery: "retry" }],
  [["cloud_lifecycle_timed_out", "cloud_retained_provider_failed"], { title: "The computer couldn't wake up", detail: "Waking this computer failed, so it went back to sleep and stopped using machine minutes. Your chat and saved messages are kept.", recovery: "retry" }],
  [["cloud_retained_machine_missing"], { title: "This computer no longer exists", detail: "The cloud no longer has this computer, so it can't wake. Your chat history is saved; start a new chat to keep working.", recovery: "none" }],
  [["cloud_restore_launch_timed_out", "cloud_restore_boot_expired", "cloud_restore_wake_failed", "cloud_archive_restore_failed"], { title: "Restoring the computer didn't finish", detail: "The restored computer didn't come online in time. Your files and chat are saved, so you can retry the restore.", recovery: "retry" }],
  [["cloud_guest_start_timeout", "cloud_guest_start_failed", "cloud_guest_start_lost", "cloud_guest_start_rejected", "cloud_allocation_boot_expired"], { title: "The computer didn't finish starting", detail: "The computer booted but its runtime didn't come online. Your chat and first message are saved.", recovery: "retry" }],
  [["cloud_provider_rate_limited", "cloud_provider_unavailable", "cloud_provider_conflict", "cloud_provider_invalid_response", "cloud_provider_outcome_unknown", "cloud_provider_allocation_outcome_unknown", "cloud_provider_machine_missing"], { title: "The cloud provider is busy", detail: "The provider didn't confirm the computer. Zuse retries on its own until the start deadline.", recovery: "retry" }],
  [["cloud_entitlement_required", "cloud_subscription_required"], { title: "Compute access required", detail: "Starting this computer needs an active Cloud plan. Your chat is saved; open Usage to continue.", recovery: "billing" }],
  [["cloud_usage_hold"], { title: "Cloud computers are paused", detail: "Cloud computers are paused because this month's machine minutes or pay-as-you-go cap are used up. Open Usage to continue.", recovery: "billing" }],
  [["cloud_size_requires_cloud_max"], { title: "Cloud Max required", detail: "Large computers (8 vCPU / 32 GB) need Cloud Max. Upgrade in Usage to use them.", recovery: "billing" }],
  [["cloud_trial_unavailable"], { title: "Trial unavailable", detail: "This card or account has already used a trial.", recovery: "billing" }],
  [["credential_preparation_failed", "agent_connection_required", "agent_connection_reconnect_required", "codex-auth-reconnect-required", "claude-auth-reconnect-required", "grok-auth-reconnect-required"], { title: AGENT, detail: "Reconnect the coding agent account this chat uses, then retry. Your saved message won't be sent twice.", recovery: "agent" }],
  [["agent_connection_refresh_uncertain"], { title: "Coding agent credential needs review", detail: "Renewing the agent account's sign-in couldn't be confirmed. Reconnect it, then retry. No message was resent.", recovery: "agent" }],
  [["codex_memory_transport_recovery_required"], { title: "Coding agent needs recovery", detail: "The agent's connection couldn't be renewed safely. Your work is saved and no message was resent.", recovery: "retry" }],
  [["github_approval_required", "github_organization_approval_required", "github_user_authorization_required", "github_authentication_required", "github_repository_access_denied", "github_installation_required", "github_repository_not_selected", "github_reconnect_required", "github_user_reconnect_required", "repository_preparation_failed"], { title: "Repository setup needs attention", detail: "Check GitHub access to the repository and the selected branch. Your chat and first message are saved.", recovery: "github" }],
  [["environment_setup_failed"], { title: "Environment setup failed", detail: "A setup command for this repository failed. Fix the environment setup in Cloud → Setup, then retry.", recovery: "environment" }],
  [["staging_candidate_computer_limit"], { title: "Staging computer limit reached", detail: "Delete the existing staging test computer before starting another. Your chat is saved.", recovery: "none" }],
  [["managed_base_unavailable", "managed_base_unavailable_for_agent", "cloud_project_not_ready", "cloud_restore_managed_base_required", "managed_base_organization_runtime_required", "managed_base_runtime_supervisor_required", "staging_managed_base_candidate_expired", "staging_managed_base_candidate_fenced"], { title: "No compatible computer environment", detail: "This service has no compatible computer environment for the repository and agent yet. Your chat is saved; retrying won't change this.", recovery: "none" }],
  [["cloud_provider_not_configured", "cloud_provider_credentials_required", "runtime_provider_auth_adapter_required", "github_runtime_credentials_required", "cloud_auth_authority_migration_required"], { title: "Cloud service isn't configured", detail: "The service needs its provider or credentials set up before this computer can start. Your chat is saved; retrying won't change this.", recovery: "none" }],
  [["workspace_runtime_update_required", "environment_runtime_update_required", "cloud_project_runtime_update_required", "workload_secret_runtime_update_required", "cloud_launch_attachments_runtime_update_required", "cloud_model_configuration_runtime_update_required", "runtime_agent_unsupported", "cloud_lifecycle_runtime_update_required", "cloud_delete_runtime_update_required"], { title: "Computer update required", detail: "This computer needs a runtime update before it can do that. Your chat is saved.", recovery: "none" }],
  [["cloud_lifecycle_in_progress", "cloud_lifecycle_quiescence_required"], { title: "The computer is busy", detail: "This computer is pausing, archiving or waking. Try again when it finishes.", recovery: "retry" }],
  [["computer_recreation_in_progress"], { title: "The computer is being recreated", detail: "This computer is being recreated. Try again when it finishes.", recovery: "retry" }],
  [["cloud_allocation_retry_unavailable", "cloud_allocation_retry_not_required", "cloud_allocation_retry_outcome_unknown"], { title: "Start can't be retried right now", detail: "This computer's start can't be retried right now. Reopen it to see its current state.", recovery: "none" }],
  [["cloud_launch_upload_expired"], { title: "Uploads expired", detail: "These cloud uploads expired. Start a new cloud chat and attach the files again.", recovery: "none" }],
  [["launch_asset_incomplete"], { title: "Upload incomplete", detail: "The cloud attachment upload is incomplete. Retry the same message.", recovery: "retry" }],
  [["launch_asset_released"], { title: "Attachment no longer available", detail: "This cloud attachment was already delivered or has expired. Attach the file again.", recovery: "none" }],
  [["launch_asset_integrity_failed", "launch_asset_manifest_conflict", "launch_asset_chunk_conflict"], { title: "Attachment couldn't be verified", detail: "The cloud attachment could not be verified. Keep the original file and start a new chat.", recovery: "none" }],
];
const byCode = new Map(issues.flatMap(([codes, issue]) => codes.map((code) => [code, issue] as const)));

/** Shown when a code has no copy of its own, so a raw code never reaches the user. */
export const GENERIC_ISSUE: CloudIssue = {
  title: "Something went wrong",
  detail: "The cloud computer hit an unexpected problem. Your chat is saved; retry, or start a new chat if it keeps happening.",
  recovery: "retry",
};

// snake_case with two or more underscores, or a kebab-case `*-auth-reconnect-required` code.
const CODE = /\b(?:[a-z][a-z0-9]*(?:_[a-z0-9]+){2,}|[a-z]+-auth-reconnect-required)\b/;

/** The issue for a code, or null for an unknown one. */
export const cloudIssue = (code: string | null | undefined): CloudIssue | null => (code ? byCode.get(code) ?? null : null);

/** The first error code inside a message, if any. */
export const codeIn = (message: string) => CODE.exec(message)?.[0] ?? null;

/** Readable text for a server or engine message: known codes get their copy, unknown codes the generic copy. */
export function cloudErrorText(message: string) {
  const code = codeIn(message);
  if (!code) return message;
  return (cloudIssue(code) ?? GENERIC_ISSUE).detail;
}
