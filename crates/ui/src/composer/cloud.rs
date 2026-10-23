//! Native cloud launch uses the existing attached tray and retains the accepted request on failure.
use super::*;
use serde_json::{Value, json};
#[derive(Default)]
pub(super) struct Launch {
    pub task: Option<Task<()>>,
    pub message: Option<String>,
    owner: Option<String>,
    request: Option<Value>,
    accepted: Option<Value>,
}
impl Composer {
    pub(super) fn send_cloud(&mut self, text: String, cx: &mut Context<Self>) -> bool {
        let state = self.state.read(cx);
        if state.selected_chat.is_some() {
            return false;
        }
        let Some(project) = state.cloud_project.clone() else {
            return false;
        };
        if self.cloud.task.is_some() {
            return true;
        }
        if !self.staged().is_empty() || !self.staged_appshots().is_empty() {
            self.failure = Some(
                "Start the cloud thread first, then attach files. Your draft is preserved.".into(),
            );
            self.failure_key = None;
            cx.notify();
            return true;
        }
        let Some(controller) = state.engine().map(|e| e.controller()) else {
            return true;
        };
        let Some(subject) = state.auth_user().map(|user| user.id.clone()) else {
            return true;
        };
        if self.cloud.owner.as_deref() != Some(subject.as_str()) {
            self.cloud = Launch {
                owner: Some(subject.clone()),
                ..Default::default()
            };
        }
        if matches!(
            &state.auth,
            Some(zuse_protocol::AuthState::SignedIn {
                org_id: Some(_),
                ..
            })
        ) {
            self.failure = Some(
                "Switch to your personal account before starting this cloud workspace.".into(),
            );
            self.failure_key = None;
            cx.notify();
            return true;
        }
        if project["accountId"].as_str() != Some(subject.as_str()) {
            self.failure = Some("The account changed. Select the cloud destination again.".into());
            self.failure_key = None;
            cx.notify();
            return true;
        }
        if self.cloud.request.is_none() {
            let config = self.pickers.read(cx).resolved(cx);
            let runtime_mode = match config.sandbox {
                SandboxLevel::WorkspaceWrite => "approval-required",
                SandboxLevel::Auto => "auto",
                SandboxLevel::DangerFullAccess => "full-access",
                SandboxLevel::ReadOnly => {
                    self.failure = Some(
                        "Read-only cloud launch is unavailable. Choose Ask before starting.".into(),
                    );
                    self.failure_key = None;
                    cx.notify();
                    return true;
                }
            };
            self.cloud.request = Some(
                json!({"runtimeMode":runtime_mode,"projectId":project["projectId"],"providerId":project["providerId"],"baseRef":project["defaultBranch"],"agent":config.harness.unwrap_or(HarnessId::ClaudeCode),"model":config.model.unwrap_or_else(||"sonnet".into()),"idempotencyKey":uuid::Uuid::new_v4().to_string(),"firstMessage":text}),
            );
        }
        let key = self.current_key.clone();
        let request = self.cloud.request.clone().unwrap();
        let accepted = self.cloud.accepted.clone();
        self.cloud.message = Some("Accepting cloud message…".into());
        self.failure = None;
        let launch_controller = controller.clone();
        let create = gpui_tokio::Tokio::spawn(cx, async move {
            match accepted {
                Some(value) => Ok(value),
                None => {
                    let response = launch_controller
                        .client()
                        .call_scoped(methods::CLOUD_CREATE, request)
                        .await?;
                    let value = &response["workspace"];
                    Ok::<Value, zuse_rpc::RpcError>(value.get("workspace").unwrap_or(value).clone())
                }
            }
        });
        self.cloud.task = Some(cx.spawn(async move |this, cx| {
            let result = create.await;
            let workspace = match result {
                Ok(Ok(value)) => value,
                _ => {
                    let message = match result {
                        Ok(Err(error)) => error.to_string(),
                        _ => {
                            "Cloud request interrupted. Retry the same request to check acceptance."
                                .into()
                        }
                    };
                    let _ = this.update(cx, |this, cx| this.cloud_failure(message, cx));
                    return;
                }
            };
            let id = workspace["workspaceId"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let initial = workspace["initialSessionId"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            if id.is_empty() || initial.is_empty() {
                let _ = this.update(cx, |this, cx| {
                    this.cloud_failure("Invalid cloud launch response".into(), cx)
                });
                return;
            }
            if this
                .update(cx, |this, cx| {
                    this.cloud.accepted = Some(workspace);
                    this.cloud.message = Some("Starting cloud workspace…".into());
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            let root = controller.clone();
            let workspace_id = id.clone();
            let open = gpui_tokio::Tokio::spawn(cx, async move {
                root.client()
                    .subscribe_scoped(
                        methods::CLOUD_ENVIRONMENT,
                        json!({"workspaceId":workspace_id}),
                    )
                    .await
            });
            let mut updates = match open.await {
                Ok(Ok(updates)) => updates,
                _ => {
                    let _ = this.update(cx, |this, cx| {
                        this.cloud_failure(
                            "Cloud connection interrupted. Retry to reopen the accepted workspace."
                                .into(),
                            cx,
                        )
                    });
                    return;
                }
            };
            while let Some(value) = updates.recv().await {
                if value["state"] == "ready" {
                    let target = crate::state::CloudTarget {
                        controller,
                        subject,
                        workspace: id,
                        initial_session: initial,
                    };
                    let _ = this.update(cx, |this, cx| {
                        if this.current_key == key && this.input.read(cx).text() == text {
                            this.input.update(cx, |input, cx| input.set_text("", cx));
                        }
                        this.cloud.message = None;
                        this.cloud.request = None;
                        this.cloud.accepted = None;
                        this.cloud.task = None;
                        crate::open_cloud_window(target, cx);
                        cx.notify();
                    });
                    return;
                }
                if matches!(value["state"].as_str(), Some("blocked" | "failed")) {
                    let _ = this.update(cx, |this, cx| {
                        this.cloud_failure(
                            value["message"]
                                .as_str()
                                .unwrap_or("Cloud startup failed")
                                .into(),
                            cx,
                        )
                    });
                    return;
                }
                if this
                    .update(cx, |this, cx| {
                        this.cloud.message = value["message"].as_str().map(str::to_owned);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.cloud_failure(
                    "Cloud startup connection closed. Retry to reopen the accepted workspace."
                        .into(),
                    cx,
                )
            });
        }));
        cx.notify();
        true
    }
    fn cloud_failure(&mut self, message: String, cx: &mut Context<Self>) {
        self.cloud.task = None;
        self.cloud.message = None;
        self.failure = Some(message.into());
        self.failure_key = None;
        cx.notify();
    }
}
