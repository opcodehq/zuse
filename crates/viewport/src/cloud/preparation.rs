//! Authoritative preparation progress and actionable setup failures.
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Running,
    Completed,
    Failed,
}
impl StepState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Waiting",
            Self::Running => "In progress",
            Self::Completed => "Done",
            Self::Failed => "Needs attention",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub key: &'static str,
    pub title: &'static str,
    pub state: StepState,
}
#[derive(Debug)]
pub struct Progress {
    pub steps: Vec<Step>,
    pub visible: bool,
    pub busy: bool,
    pub title: String,
    pub next: &'static str,
}
/// Connection diagnostics cannot replace the authoritative task lifecycle or
/// preparation receipts. They describe a different connection.
pub(crate) fn merge_status(task: &Value, connection: &Value) -> Value {
    let mut state = task.as_object().cloned().unwrap_or_default();
    if let Some(fields) = connection.as_object() {
        for (key, value) in fields {
            if matches!(
                key.as_str(),
                "state"
                    | "statusCode"
                    | "preparation"
                    | "startupPhase"
                    | "startupTimings"
                    | "runtimeIngressReady"
            ) && state.contains_key(key)
            {
                continue;
            }
            state.insert(key.clone(), value.clone());
        }
    }
    state.insert("connectionState".into(), connection["state"].clone());
    Value::Object(state)
}
/// Empty assistant placeholders and invisible dispatch metadata are not a
/// response. Correlate with this exact user message, never older history.
pub fn response_started(messages: &[zuse_model::SessionMessageEntry], message: &str) -> bool {
    let Some(index) = messages.iter().position(|entry| entry.id == message) else {
        return false;
    };
    messages[index + 1..]
        .iter()
        .take_while(|entry| entry.role != zuse_model::MessageRole::User)
        .any(|entry| {
            entry.role == zuse_model::MessageRole::Assistant
                && entry.parts.iter().any(|part| match part {
                    zuse_model::MessagePart::Text { text, .. }
                    | zuse_model::MessagePart::Reasoning { text, .. } => !text.trim().is_empty(),
                    zuse_model::MessagePart::AgentContext { .. } => false,
                    _ => true,
                })
        })
}
pub(crate) fn awaiting_response(
    session: &str,
    admissions: &[Value],
    messages: &[zuse_model::SessionMessageEntry],
    bridge: Option<&Value>,
) -> bool {
    admissions
        .iter()
        .chain(bridge.filter(|bridge| {
            !admissions
                .iter()
                .any(|row| row["commandId"] == bridge["commandId"])
        }))
        .any(|command| {
            command["sessionId"] == session
                && matches!(
                    command["executionState"].as_str(),
                    Some("accepted" | "queued" | "starting" | "running")
                )
                && command["messageId"]
                    .as_str()
                    .is_some_and(|message| !response_started(messages, message))
        })
}
/// Project only observed stage states. Unreported work remains pending, and
/// concurrent receipts stay concurrent rather than becoming a fictional order.
pub fn progress(status: &Value, awaiting_response: bool) -> Progress {
    let lifecycle = status["state"].as_str().unwrap_or_default();
    let phase = status["startupPhase"].as_str().unwrap_or_default();
    let inactive = matches!(
        lifecycle,
        "paused" | "pausing" | "archived" | "archiving" | "deleting" | "deleted"
    );
    let computer = if matches!(
        phase,
        "syncing-repository" | "repository-syncing" | "starting-agent" | "running"
    ) || lifecycle == "ready"
        || status["runtimeIngressReady"] == true
    {
        StepState::Completed
    } else if lifecycle == "failed" {
        StepState::Failed
    } else if !inactive {
        StepState::Running
    } else {
        StepState::Pending
    };
    let mut steps = vec![Step {
        key: "computer",
        title: if phase == "resuming" {
            "Resuming computer"
        } else {
            "Starting computer"
        },
        state: computer,
    }];
    for (key, title) in [
        ("repository", "Syncing repository"),
        ("environment", "Preparing environment"),
        ("credentials", "Preparing agent access"),
        ("agent", "Starting agent"),
    ] {
        let state = match status["preparation"][key]["state"].as_str() {
            Some("running") if lifecycle == "ready" => StepState::Pending,
            Some("running") => StepState::Running,
            Some("completed") => StepState::Completed,
            Some("failed") => StepState::Failed,
            _ if key == "repository"
                && status["startupTimings"]["repositoryReadyAt"].is_number() =>
            {
                StepState::Completed
            }
            _ if key == "repository"
                && lifecycle != "ready"
                && matches!(phase, "syncing-repository" | "repository-syncing") =>
            {
                StepState::Running
            }
            _ => StepState::Pending,
        };
        steps.push(Step { key, title, state });
    }
    let failure = issue(status["statusCode"].as_str().unwrap_or_default());
    let failed = lifecycle == "failed"
        || failure.is_some()
        || steps.iter().any(|s| s.state == StepState::Failed);
    let running: Vec<_> = steps
        .iter()
        .filter(|s| s.state == StepState::Running)
        .map(|s| s.title)
        .collect();
    let transport = if lifecycle == "ready" {
        status["connectionState"]
            .as_str()
            .filter(|state| matches!(*state, "connecting" | "reconnecting" | "blocked" | "failed"))
    } else {
        None
    };
    let visible = !inactive
        && (lifecycle != "ready"
            || failed
            || !running.is_empty()
            || awaiting_response
            || transport.is_some());
    let transport_failed = matches!(transport, Some("blocked" | "failed"));
    let busy = visible && !failed && lifecycle != "blocked" && !transport_failed;
    let title = if let Some(failure) = failure {
        failure.title.into()
    } else if failed {
        "Computer preparation needs attention".into()
    } else if transport_failed {
        "Computer connection needs attention".into()
    } else if transport.is_some() {
        "Reconnecting to computer".into()
    } else if !running.is_empty() {
        running.join(" · ")
    } else if awaiting_response {
        "Waiting for agent response".into()
    } else {
        "Waiting for computer preparation".into()
    };
    let next = if failed {
        "Review the failed step below. Your saved message will not be resent."
    } else if transport_failed {
        "Your work is saved. Review the connection details; no message has been resent."
    } else if transport.is_some() {
        "Your work is saved. Restoring the connection without resending your message."
    } else if lifecycle == "ready" && awaiting_response {
        "Your message is saved. Waiting for the agent to begin responding."
    } else {
        match steps
            .iter()
            .find(|step| step.key != "computer" && step.state == StepState::Pending)
            .map(|step| step.key)
        {
            Some("repository") => "Next: repository sync",
            Some("environment") => "Next: environment setup",
            Some("credentials") => "Next: prepare agent access",
            Some("agent") => "Next: start the coding agent",
            _ => "Waiting for the current preparation steps to finish",
        }
    };
    Progress {
        steps,
        visible,
        busy,
        title,
        next,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Recovery {
    Billing,
    Github,
    Agent,
    Environment,
    Operator,
}
impl Recovery {
    pub(crate) fn label(self) -> Option<&'static str> {
        match self {
            Self::Billing => Some("Review compute access"),
            Self::Github => Some("Review GitHub access"),
            Self::Agent => Some("Reconnect coding agent"),
            Self::Environment => Some("Review environment"),
            Self::Operator => None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Issue {
    pub stage: &'static str,
    pub title: &'static str,
    pub detail: &'static str,
    pub recovery: Recovery,
}
impl Issue {
    pub(crate) fn detail_for_admission(self, admitted: bool) -> &'static str {
        if admitted {
            return self.detail;
        }
        match self.recovery {
            Recovery::Billing => {
                "Starting a computer requires active compute access or an explicit staging allowance. Your draft remains in this app; a task has not been confirmed."
            }
            Recovery::Github => {
                "Review GitHub authorization, repository access and the selected ref. Your draft remains in this app; a task has not been confirmed."
            }
            Recovery::Agent => {
                "Reconnect the selected coding agent in Cloud settings. Your draft remains in this app; a task has not been confirmed."
            }
            Recovery::Environment => {
                "Review the environment recipe and command output. Your draft remains in this app; a task has not been confirmed."
            }
            Recovery::Operator => {
                "An operator must correct the server configuration or staging allowance. Your draft remains in this app; a task has not been confirmed. Retrying a connection cannot repair missing configuration."
            }
        }
    }
}
pub(crate) fn issue(code: &str) -> Option<Issue> {
    let (stage, title, detail, recovery) = match code {
        "cloud_provider_rejected" => (
            "Computer starting",
            "Computer start was rejected",
            "Your task and message are saved. Open Computer to review access and retry the original start when authorized. No replacement computer or message was created.",
            Recovery::Operator,
        ),
        "agent_connection_refresh_uncertain" => (
            "Agent connecting",
            "Coding agent credential needs review",
            "The selected account's credential renewal could not be confirmed. Review that exact connection in Cloud settings before restarting the runtime. No message was replayed.",
            Recovery::Agent,
        ),
        "codex_memory_transport_recovery_required" => (
            "Agent reconnecting",
            "Coding agent needs recovery",
            "The retained agent connection could not be safely renewed. Your work is saved and no message was replayed. Open Computer and use Restart runtime when available; it restarts agent and terminal processes while keeping the repository.",
            Recovery::Operator,
        ),
        "cloud_entitlement_required" | "cloud_subscription_required" => (
            "Computer starting",
            "Compute access required",
            "This task is saved. Starting its computer requires an active compute subscription or an explicit staging allowance.",
            Recovery::Billing,
        ),
        "credential_preparation_failed"
        | "agent_connection_required"
        | "agent_connection_reconnect_required"
        | "codex-auth-reconnect-required"
        | "claude-auth-reconnect-required"
        | "grok-auth-reconnect-required" => (
            "Agent connecting",
            "Coding agent needs authorization",
            "Reconnect the account selected for this task in Cloud settings. The saved message will not be sent again by opening settings.",
            Recovery::Agent,
        ),
        "github_approval_required"
        | "github_organization_approval_required"
        | "github_user_authorization_required"
        | "github_authentication_required"
        | "repository_preparation_failed" => (
            "Repository sync",
            "Repository setup needs attention",
            "Review GitHub authorization, repository access and the selected ref. Your task and first message are saved.",
            Recovery::Github,
        ),
        "environment_setup_failed" => (
            "Environment setup",
            "Environment setup failed",
            "Review the failed command and its output, then correct the environment recipe or timeout before retrying preparation.",
            Recovery::Environment,
        ),
        "cloud_retained_machine_missing" => (
            "Computer starting",
            "This computer no longer exists",
            "Its provider no longer has this computer, so it can't wake. Recreate it from the computer menu, or start a new chat. This conversation is saved.",
            Recovery::Operator,
        ),
        "staging_candidate_computer_limit" => (
            "Computer starting",
            "Staging computer limit reached",
            "Delete the existing staging test computer before starting another, or ask an operator to review the limit. This task is saved.",
            Recovery::Operator,
        ),
        "managed_base_unavailable"
        | "managed_base_unavailable_for_agent"
        | "cloud_restore_managed_base_required"
        | "managed_base_organization_runtime_required"
        | "managed_base_runtime_supervisor_required"
        | "staging_managed_base_candidate_expired"
        | "staging_managed_base_candidate_fenced" => (
            "Computer starting",
            "Server environment setup required",
            "An operator must publish or renew a compatible managed environment for this provider and agent. Reconnecting cannot repair this configuration. Your task is saved.",
            Recovery::Operator,
        ),
        "cloud_provider_not_configured"
        | "cloud_provider_credentials_required"
        | "runtime_provider_auth_adapter_required"
        | "github_runtime_credentials_required"
        | "cloud_auth_authority_migration_required" => (
            "Computer starting",
            "Server configuration required",
            "An operator must configure the provider or credential integration before this computer can start. Your task is saved; retrying the connection will not fix missing configuration.",
            Recovery::Operator,
        ),
        "workspace_runtime_update_required"
        | "environment_runtime_update_required"
        | "cloud_project_runtime_update_required"
        | "workload_secret_runtime_update_required"
        | "cloud_launch_attachments_runtime_update_required"
        | "cloud_model_configuration_runtime_update_required"
        | "runtime_agent_unsupported" => (
            "Computer starting",
            "Managed runtime update required",
            "An operator must publish a compatible runtime for this task's selected features. Your task is saved. Reconnecting will not update its managed environment.",
            Recovery::Operator,
        ),
        _ => return None,
    };
    Some(Issue {
        stage,
        title,
        detail,
        recovery,
    })
}
#[cfg(test)]
mod tests {
    #[test]
    fn repository_ready_phase_cannot_claim_an_unobserved_agent_handshake() {
        let status = serde_json::json!({"state":"setup","startupPhase":"starting-agent","startupTimings":{"repositoryReadyAt":10},"preparation":{"repository":{"state":"completed"},"environment":{"state":"running"},"credentials":{"state":"completed"}}});
        let observed = super::progress(&status, false);
        assert_eq!(
            observed
                .steps
                .iter()
                .find(|step| step.key == "agent")
                .unwrap()
                .state,
            super::StepState::Pending,
            "Repository readiness is not an agent process/handshake receipt"
        );
        assert_eq!(observed.title, "Preparing environment");
        let mut explicit = status;
        explicit["preparation"]["agent"] = serde_json::json!({"state":"running"});
        let observed = super::progress(&explicit, false);
        assert_eq!(
            observed
                .steps
                .iter()
                .find(|step| step.key == "agent")
                .unwrap()
                .state,
            super::StepState::Running
        );
        assert!(observed.title.contains("Starting agent"));
    }
    use super::*;
    #[test]
    fn concurrent_preparation_preserves_each_observed_state_and_failure_wins() {
        let mut status = serde_json::json!({"state":"setup","runtimeIngressReady":true,"preparation":{"repository":{"state":"running"},"credentials":{"state":"running"},"environment":{"state":"completed"}}});
        let view = progress(&status, false);
        assert!(view.busy && view.visible);
        assert_eq!(view.title, "Syncing repository · Preparing agent access");
        assert_eq!(view.next, "Next: start the coding agent");
        assert_eq!(view.steps[2].state, StepState::Completed);
        assert_eq!(
            view.steps[4].state,
            StepState::Pending,
            "credential work is not an agent handshake"
        );
        status["preparation"]["environment"]["state"] = serde_json::json!("failed");
        let view = progress(&status, false);
        assert!(!view.busy && view.visible);
        assert!(view.title.contains("needs attention"));
        assert_eq!(view.steps[1].state, StepState::Running);
    }
    #[test]
    fn ready_and_sleep_override_old_activity_without_fabricating_completion() {
        let mut status = serde_json::json!({"state":"ready","startupPhase":"running","preparation":{"repository":{"state":"running"}}});
        assert!(!progress(&status, false).visible);
        let waiting = progress(&status, true);
        assert!(waiting.busy);
        assert_eq!(waiting.title, "Waiting for agent response");
        assert_eq!(waiting.steps[1].state, StepState::Pending);
        status["preparation"]["repository"]["state"] = serde_json::json!("failed");
        assert!(
            progress(&status, false).visible,
            "failed receipts must remain actionable"
        );
        assert!(
            !progress(
                &serde_json::json!({"state":"failed","statusCode":"unknown-failure"}),
                false
            )
            .busy,
            "unknown failure must not animate forever"
        );
        let failed = progress(
            &serde_json::json!({"state":"failed","statusCode":"unknown-failure"}),
            false,
        );
        assert_eq!(
            failed.steps[0].state,
            StepState::Failed,
            "a failed start without allocation proof cannot remain in progress"
        );
        let downstream = progress(
            &serde_json::json!({"state":"failed","runtimeIngressReady":true,"preparation":{"environment":{"state":"failed"}}}),
            false,
        );
        assert_eq!(
            downstream.steps[0].state,
            StepState::Completed,
            "a downstream setup failure must not erase proven computer readiness"
        );
        for state in ["paused", "deleting", "deleted"] {
            status["state"] = serde_json::json!(state);
            assert!(!progress(&status, true).visible);
            assert!(!progress(&status, true).busy);
        }
    }
    #[test]
    fn ready_task_reconnect_keeps_completed_preparation_and_never_resumes_sleep() {
        let mut status = serde_json::json!({"state":"ready","preparation":{"repository":{"state":"completed"},"environment":{"state":"completed"}}});
        for connection in ["connecting", "reconnecting", "blocked", "failed"] {
            status["connectionState"] = serde_json::json!(connection);
            let view = progress(&status, true);
            assert!(view.visible);
            assert_eq!(
                view.busy,
                matches!(connection, "connecting" | "reconnecting")
            );
            assert_eq!(view.steps[1].state, StepState::Completed);
            assert_eq!(view.steps[2].state, StepState::Completed);
            assert!(view.title.contains("connection") || view.title.contains("Reconnecting"));
        }
        status["connectionState"] = serde_json::json!("ready");
        assert!(!progress(&status, false).visible);
        for lifecycle in ["paused", "pausing", "deleting", "deleted"] {
            status["state"] = serde_json::json!(lifecycle);
            status["connectionState"] = serde_json::json!("reconnecting");
            assert!(!progress(&status, true).visible);
        }
    }
    #[test]
    fn diagnostic_connection_cannot_erase_task_progress_or_capabilities() {
        let task = serde_json::json!({"state":"setup","statusCode":"environment-setup","preparation":{"environment":{"state":"running"}},"startupPhase":"booting","commandAdmissionVersion":2});
        let merged = merge_status(
            &task,
            &serde_json::json!({"state":"ready","preparation":{},"startupPhase":"running","epoch":9}),
        );
        assert_eq!(merged["state"], "setup");
        assert_eq!(merged["connectionState"], "ready");
        assert_eq!(merged["preparation"], task["preparation"]);
        assert_eq!(merged["commandAdmissionVersion"], 2);
        assert_eq!(merged["epoch"], 9);
    }
    #[test]
    fn response_wait_ignores_placeholders_old_history_controls_and_other_sessions() {
        let mut messages: Vec<zuse_model::SessionMessageEntry> = serde_json::from_value(serde_json::json!([
            {"id":"old","role":"assistant","parts":[{"kind":"text","id":"a","text":"Old response"}],"createdAt":1,"deviceId":"runtime"},
            {"id":"prompt","role":"user","parts":[],"createdAt":2,"deviceId":"runtime"},
            {"id":"placeholder","role":"assistant","parts":[{"kind":"text","id":"b","text":""}],"createdAt":3,"deviceId":"runtime"}
        ])).unwrap();
        let command = serde_json::json!({"commandId":"one","sessionId":"session","messageId":"prompt","executionState":"starting"});
        assert!(awaiting_response(
            "session",
            &[command.clone()],
            &messages,
            None
        ));
        assert!(!awaiting_response(
            "other",
            &[command.clone()],
            &messages,
            None
        ));
        let mut control = command.clone();
        control["messageId"] = Value::Null;
        assert!(!awaiting_response("session", &[control], &messages, None));
        let mut done = command.clone();
        done["executionState"] = serde_json::json!("completed");
        assert!(
            !awaiting_response("session", &[done], &messages, Some(&command)),
            "terminal receipt overrides local admission bridge"
        );
        let later_turn: Vec<zuse_model::SessionMessageEntry> = serde_json::from_value(serde_json::json!([
            {"id":"prompt","role":"user","parts":[],"createdAt":1,"deviceId":"runtime"},
            {"id":"later-user","role":"user","parts":[],"createdAt":2,"deviceId":"runtime"},
            {"id":"later-answer","role":"assistant","parts":[{"kind":"text","id":"later-text","text":"Response to the later user"}],"createdAt":3,"deviceId":"runtime"}
        ])).unwrap();
        assert!(
            !response_started(&later_turn, "prompt"),
            "a later user's response must not complete the earlier admission"
        );
        assert!(response_started(&later_turn, "later-user"));
        messages[2].parts = vec![zuse_model::MessagePart::Text {
            id: "b".into(),
            text: "Actual response".into(),
        }];
        assert!(!awaiting_response("session", &[command], &messages, None));
    }
    #[test]
    fn entitlement_failure_is_actionable_even_while_provider_is_allocating() {
        let failure = issue("cloud_entitlement_required").unwrap();
        assert_eq!(failure.stage, "Computer starting");
        assert_eq!(failure.recovery, Recovery::Billing);
        assert_eq!(failure.recovery.label(), Some("Review compute access"));
    }
    #[test]
    fn preadmission_errors_only_claim_a_local_draft() {
        for code in [
            "cloud_entitlement_required",
            "agent_connection_required",
            "managed_base_unavailable",
        ] {
            let detail = issue(code).unwrap().detail_for_admission(false);
            assert!(detail.contains("draft remains in this app"));
            assert!(detail.contains("task has not been confirmed"));
            assert!(!detail.contains("task is saved"));
        }
    }
    #[test]
    fn missing_server_configuration_never_offers_connection_retry() {
        for code in [
            "managed_base_unavailable",
            "staging_managed_base_candidate_expired",
            "github_runtime_credentials_required",
            "runtime_provider_auth_adapter_required",
            "environment_runtime_update_required",
        ] {
            let failure = issue(code).unwrap();
            assert_eq!(failure.recovery, Recovery::Operator);
            assert_eq!(failure.recovery.label(), None);
        }
        assert!(issue("allocating").is_none());
        assert!(
            issue("cloud_retained_machine_missing").is_some(),
            "a computer its provider no longer has says so instead of starting forever"
        );
        assert!(
            issue("cloud_allocation_unavailable").is_none(),
            "transport failure is not classified as missing configuration"
        );
    }
}
