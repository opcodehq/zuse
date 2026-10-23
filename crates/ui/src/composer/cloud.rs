//! Native cloud launch uses the existing attached tray and retains the accepted request on failure.
use super::*;
use serde_json::{Value, json};
#[derive(Default)]
pub(super) struct Launch {
    pub task: Option<Task<()>>,
    pub message: Option<String>,
    cached: Option<bool>,
    owner: Option<String>,
    request: Option<Value>,
    accepted: Option<Value>,
    pub pause_task: Option<Task<()>>,
    pub pause_error: Option<String>,
    pause_command: Option<String>,
}
impl Composer {
    pub(super) fn cached_cloud(&self) -> bool { self.cloud.cached.is_some() }
    pub(crate) fn set_cached_cloud(&mut self, paused: bool, cx: &mut Context<Self>) {
        if self.cloud.cached != Some(paused) {
            self.cloud.cached = Some(paused);
            cx.notify();
        }
    }
    pub(super) fn live_cloud_row(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<super::tray::TrayPill> {
        if let Some(paused) = self.cloud.cached {
            return Some(super::tray::TrayPill::new("cached-cloud-connection", if paused { "Cloud workspace paused" } else { "Reconnect to cloud workspace" })
                .glyph(crate::icons::DITHER_CLOUD)
                .subtitle("Sending a message will resume this workspace.")
                .action(super::tray::tray_text_action("resume-cloud-workspace", if paused { "Resume" } else { "Reconnect" }, None, theme)
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.emit(ComposerEvent::ResumeCloud { text: this.input.read(cx).text().to_owned(), send: false });
                    }))));
        }
        self.state.read(cx).engine()?.cloud_workspace()?;
        let busy = self.cloud.pause_task.is_some();
        if !busy && self.cloud.pause_error.is_none() {
            return None;
        }
        let mut row = super::tray::TrayPill::new(
            "live-cloud-workspace",
            if busy {
                "Pausing cloud workspace…"
            } else {
                "Couldn’t pause cloud workspace"
            },
        )
        .glyph(crate::icons::DITHER_CLOUD);
        if let Some(error) = &self.cloud.pause_error {
            row = row.subtitle(error.clone());
        }
        Some(
            row.action(
                super::tray::tray_text_action(
                    "pause-cloud-workspace",
                    if busy {
                        "Pausing…"
                    } else if self.cloud.pause_error.is_some() {
                        "Retry pause"
                    } else {
                        "Pause"
                    },
                    None,
                    theme,
                )
                .opacity(if busy { 0.5 } else { 1. })
                .on_click(cx.listener(|this, _, window, cx| this.pause_cloud(window, cx))),
            ),
        )
    }
    pub(crate) fn cloud_has_draft(&self, cx: &App) -> bool {
        self.has_draft(cx)
            || self
                .drafts
                .iter()
                .any(|(key, draft)| key != &self.current_key && !draft.trim().is_empty())
            || self.attachments.values().any(|files| !files.is_empty())
            || self.appshots.values().any(|captures| !captures.is_empty())
            || self.queue_edit_draft.is_some()
    }
    pub(crate) fn pause_cloud(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloud.pause_task.is_some() {
            return;
        }
        let unsaved_files = window
            .root::<crate::shell::Shell>()
            .flatten()
            .is_none_or(|shell| shell.read(cx).cloud_pause_has_unsaved_work(cx));
        if self.cloud_has_draft(cx) || unsaved_files {
            self.cloud.pause_error = Some("Send or clear drafts and save editor changes before pausing. Your work is preserved.".into());
            cx.notify();
            return;
        }
        let state = self.state.read(cx);
        let Some(engine) = state.engine() else {
            return;
        };
        let Some(workspace) = engine.cloud_workspace() else {
            return;
        };
        let Some(subject) = state.cloud_user_id().map(str::to_owned) else {
            return;
        };
        let target = crate::state::CloudTarget {
            controller: engine.controller(),
            subject,
            workspace,
            initial_session: state.selected_chat.clone().unwrap_or_default(),
        };
        let command = self
            .cloud
            .pause_command
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        let root = target.controller.clone();
        let request = json!({"workspaceId":target.workspace,"expectedAccountId":target.subject,"commandId":command});
        let handle = window.window_handle();
        self.cloud.pause_error = None;
        self.cloud.pause_task = Some(cx.spawn(async move |this, cx| {
            let result = root
                .client()
                .call_scoped(methods::CLOUD_PAUSE, request)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.cloud.pause_task = None;
                match result {
                    Ok(_) => {
                        gpui::App::defer(cx, move |cx| {
                            crate::cloud_history::saved::open(target, cx);
                            if let Some(handle) = handle.downcast::<crate::shell::Shell>() {
                                let _ = handle.update(cx, |shell, window, cx| {
                                    if shell.cloud_pause_can_close(cx)
                                        && shell.prepare_window_close(cx)
                                    {
                                        window.remove_window();
                                    }
                                });
                            }
                        });
                    }
                    Err(error) => this.cloud.pause_error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    pub(super) fn send_cloud(&mut self, text: String, cx: &mut Context<Self>) -> bool {
        if self.cloud.cached.is_some() {
            cx.emit(ComposerEvent::ResumeCloud { text, send: true });
            return true;
        }
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
        if !self.staged_appshots().is_empty() {
            self.failure = Some(
                "Start the cloud thread before attaching app captures. Your draft is preserved."
                    .into(),
            );
            self.failure_key = None;
            cx.notify();
            return true;
        }
        let Some(engine) = state.engine().cloned() else {
            return true;
        };
        let controller = engine.controller();
        let staged = self.staged().to_vec();
        let Some(subject) = state.cloud_user_id().map(str::to_owned) else {
            return true;
        };
        if self.cloud.owner.as_deref() != Some(subject.as_str()) {
            self.cloud = Launch {
                owner: Some(subject.clone()),
                ..Default::default()
            };
        }
        if state.cloud_account.as_ref().is_some_and(|value|value["orgId"].is_string()) {
            self.failure = Some("Switch to your personal account before starting this cloud workspace.".into());
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
                json!({"runtimeMode":runtime_mode,"projectId":project["projectId"],"providerId":project["providerId"],"baseRef":project["defaultBranch"],"agent":config.harness.unwrap_or(HarnessId::ClaudeCode),"model":config.model.unwrap_or_else(||"sonnet".into()),"idempotencyKey":uuid::Uuid::new_v4().to_string(),"firstMessage":text,"expectedAccountId":subject,"attachmentTransfers":staged.iter().map(|a|json!({"uploadId":a.id,"fileName":a.name})).collect::<Vec<_>>()}),
            );
        }
        let key = self.current_key.clone();
        let request = self.cloud.request.clone().unwrap();
        let accepted = self.cloud.accepted.clone();
        self.cloud.message = Some(
            if staged.is_empty() {
                "Accepting cloud message…"
            } else {
                "Uploading cloud attachments…"
            }
            .into(),
        );
        self.failure = None;
        let launch_controller = controller.clone();
        self.cloud.task = Some(cx.spawn(async move |this, cx| {
            if accepted.is_none() {
                for attachment in &staged {
                    if let Err(error) = attachments::upload_attachment(
                        &engine,
                        cx.background_executor(),
                        None,
                        &attachment.id,
                        attachment,
                        None,
                    )
                    .await
                    {
                        let _ = this.update(cx, |this, cx| this.cloud_failure(error, cx));
                        return;
                    }
                }
            }
            let create = gpui_tokio::Tokio::spawn(cx, async move {
                match accepted {
                    Some(value) => Ok(value),
                    None => {
                        let response = launch_controller
                            .client()
                            .call_scoped(methods::CLOUD_CREATE, request)
                            .await?;
                        let value = &response["workspace"];
                        Ok::<Value, zuse_rpc::RpcError>(
                            value.get("workspace").unwrap_or(value).clone(),
                        )
                    }
                }
            });
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
                            if let Some(current) = this.attachments.get_mut(&key) {
                                current.retain(|a| !staged.iter().any(|sent| sent.id == a.id));
                            }
                            this.sync_attachment_chips(cx);
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

#[cfg(test)]
mod cached_tests {
    use super::*;

    #[gpui::test]
    fn cached_cloud_submit_resumes_without_consuming_the_draft(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.selected_chat = Some("retained-session".into());
            state
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        let actions = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let captured = actions.clone();
        let _subscription = cx.update(|cx| cx.subscribe(&composer, move |_, event, _| {
            if let ComposerEvent::ResumeCloud { text, send } = event {
                captured.borrow_mut().push((text.clone(), *send));
            }
        }));
        composer.update(cx, |composer, cx| {
            composer.set_cached_cloud(true, cx);
            composer.input.update(cx, |input, cx| input.set_text("Continue the retained conversation", cx));
            composer.on_submit(cx);
            assert_eq!(composer.input.read(cx).text(), "Continue the retained conversation");
            assert!(composer.failure.is_none());
        });
        assert_eq!(&*actions.borrow(), &[("Continue the retained conversation".to_owned(), true)]);
    }
}
