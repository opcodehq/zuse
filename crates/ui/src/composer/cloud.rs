//! Native cloud launch uses the existing attached tray and retains the accepted request on failure.
use super::*;
use crate::localization::message as tr;
use serde_json::{Value, json};
use zuse_i18n::messages as msg;
mod wake;
#[derive(Default)]
pub(super) struct Launch {
    pub task: Option<Task<()>>,
    pub message: Option<String>,
    cached: Option<bool>,
    owner: Option<String>,
    organization: Option<String>,
    pub(super) request: Option<Value>,
    accepted: Option<Value>,
    issue: Option<StagedAttachment>,
    staged: Vec<StagedAttachment>,
    pub pause_task: Option<Task<()>>,
    pub pause_error: Option<String>,
    native: Option<NativeTask>,
    pending_native: Option<Value>,
    pending_native_text: Option<String>,
    // Retain receipt identity through the gap before execution/history arrives.
    admitted_native: Option<Value>,
    native_staged: Vec<StagedAttachment>,
    rejected_drafts: Vec<(String, Vec<StagedAttachment>)>,
    pause_command: Option<String>,
    /// The cloud computer's stage, set by the conversation that owns this
    /// composer and shown in the tray's single status line.
    status: Option<(String, bool)>,
}
impl Launch {
    pub(super) fn admitted(&self) -> bool {
        self.accepted.is_some()
    }
}
struct NativeTask {
    target: crate::state::CloudTarget,
    wake: bool,
    queue_selection_version: u32,
}
impl Composer {
    pub(crate) fn set_cloud_task(
        &mut self,
        target: crate::state::CloudTarget,
        config: Value,
        cx: &mut Context<Self>,
    ) {
        let wake = self.cloud.native.as_ref().is_some_and(|previous| {
            previous.target.subject == target.subject
                && previous.target.organization == target.organization
                && previous.target.workspace == target.workspace
                && previous.target.initial_session == target.initial_session
                && previous.wake
        });
        let seed = serde_json::from_value::<HarnessId>(config["agent"].clone())
            .ok()
            .map(|harness| zuse_protocol::ChatConfig {
                harness,
                plan: false,
                model: config["model"].as_str().map(str::to_owned),
                reasoning: serde_json::from_value(config["reasoning"].clone())
                    .ok()
                    .flatten(),
                model_options: serde_json::from_value(
                    config
                        .get("modelOptions")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                )
                .unwrap_or_default(),
                sandbox: serde_json::from_value(config["sandbox"].clone()).unwrap_or_else(|_| {
                    match config["runtimeMode"].as_str() {
                        Some("full-access") => SandboxLevel::DangerFullAccess,
                        Some("auto") => SandboxLevel::Auto,
                        _ => SandboxLevel::WorkspaceWrite,
                    }
                }),
            });
        self.pickers.update(cx, |pickers, cx| {
            pickers.bind_cloud_task(
                target.clone(),
                config["providerId"].as_str().unwrap_or("boxd").to_owned(),
                seed,
                cx,
            )
        });
        self.cloud.native = Some(NativeTask {
            target,
            wake,
            queue_selection_version: config["queueRunSelectionVersion"].as_u64().unwrap_or(0)
                as u32,
        });
        self.cloud.cached = None;
        cx.notify();
    }
    pub(crate) fn clear_cloud_task(&mut self, cx: &mut Context<Self>) {
        self.cloud.task = None;
        self.cloud.native = None;
        self.cloud.pending_native = None;
        self.cloud.pending_native_text = None;
        self.cloud.admitted_native = None;
        self.cloud.native_staged.clear();
        self.pickers
            .update(cx, |pickers, cx| pickers.clear_cloud_task_binding(cx));
        self.input.update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }
    pub(crate) fn set_cloud_task_sleeping(&mut self, sleeping: bool) {
        if let Some(task) = &mut self.cloud.native {
            task.wake = sleeping;
        }
    }
    pub(super) fn send_cloud_task(
        &mut self,
        text: String,
        queue: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(task) = &self.cloud.native else {
            return false;
        };
        if self.cloud.task.is_some() {
            return true;
        }
        if queue
            && task.queue_selection_version
                != zuse_protocol::cloud_task_commands::QUEUE_RUN_SELECTION_VERSION
        {
            self.failure=Some("This server cannot save the selected agent with queued messages yet. Your draft is preserved; wait for this turn to finish, then Send.".into());
            cx.notify();
            return true;
        }
        if !self.staged_appshots().is_empty() {
            self.failure = Some("Attach the capture as a file for this cloud conversation. Your draft is preserved.".into());
            cx.notify();
            return true;
        }
        if self
            .cloud
            .pending_native_text
            .as_deref()
            .is_some_and(|previous| previous != text)
        {
            self.failure = Some("The previous message has an uncertain delivery. Retry its original text before sending different work.".into());
            cx.notify();
            return true;
        }
        if self.staged().len() > zuse_protocol::cloud_assets::MAX_ASSETS {
            self.failure = Some(
                "Attach at most eight files to one cloud message. Your draft is preserved.".into(),
            );
            cx.notify();
            return true;
        }
        if self.cloud.pending_native.is_none() {
            let command = uuid::Uuid::new_v4().to_string();
            let message = uuid::Uuid::new_v4().to_string();
            let Some(resolved) = self.pickers.read(cx).cloud_task_choice() else {
                self.failure=Some("This task's saved agent settings are not available yet. Your draft is preserved.".into());
                cx.notify();
                return true;
            };
            let harness = resolved.harness;
            let model = resolved.model.clone();
            if harness.is_none() || model.is_none() {
                self.failure = Some(
                    "Choose an available agent and model for this task. Your draft is preserved."
                        .into(),
                );
                cx.notify();
                return true;
            }
            let sandbox = resolved.sandbox;
            let action = if queue {
                json!({"type":"queue","queueId":command,"messageId":message,"input":{"text":text,"runSelection":{"harness":harness,"model":model,"reasoning":resolved.reasoning,"modelOptions":resolved.model_options,"sandbox":sandbox}},"holdForTurnEnd":true})
            } else {
                json!({"type":"submit","messageId":message,"payload":{"kind":"run","messageId":message,"request":{
                    "prompt":text,"harness":harness,"model":model,"reasoning":resolved.reasoning,
                    "modelOptions":resolved.model_options,"cwd":".","sandbox":sandbox,
                    "autoApprove":sandbox==SandboxLevel::DangerFullAccess,"resume":null,"attachments":[]
                }}})
            };
            self.cloud.pending_native = Some(
                json!({"commandId":command,"sessionId":task.target.initial_session,"action":action,
                "wake":wake::intent(queue, task.wake)}),
            );
            self.cloud.pending_native_text = Some(text.clone());
            self.cloud.native_staged = self.staged().to_vec();
        }
        self.dispatch_cloud_task(cx);
        true
    }
    pub(super) fn interrupt_cloud_task(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(task) = &self.cloud.native else {
            return false;
        };
        if self.cloud.task.is_some() || self.cloud.pending_native.is_some() {
            return true;
        }
        self.cloud.pending_native = Some(
            json!({"commandId":uuid::Uuid::new_v4().to_string(),"sessionId":task.target.initial_session,
            "action":{"type":"submit","messageId":uuid::Uuid::new_v4().to_string(),"payload":{"kind":"interrupt"}}}),
        );
        self.dispatch_cloud_task(cx);
        true
    }
    fn dispatch_cloud_task(&mut self, cx: &mut Context<Self>) {
        let Some(task) = &self.cloud.native else {
            return;
        };
        let target = task.target.clone();
        let Some(mut request) = self.cloud.pending_native.clone() else {
            return;
        };
        let staged = self.cloud.native_staged.clone();
        self.failure = None;
        self.cloud.task = Some(cx.spawn(async move |this, cx| {
            let field=if request["action"]["type"]=="queue" {"input"} else {""};
            let assets=if field=="input" {&request["action"]["input"]["attachments"]} else {&request["action"]["attachments"]};
            if !staged.is_empty() && assets.as_array().map_or(true,|assets|assets.len()!=staged.len()) {
                let mut assets=Vec::new();
                for attachment in &staged {
                    match upload_native_asset(&target,attachment).await {
                        Ok(asset)=>assets.push(asset),
                        Err(error)=>{let _=this.update(cx,|this,cx|this.cloud_failure(error,cx));return;}
                    }
                }
                if field=="input" {request["action"]["input"]["attachments"]=json!(assets);} else {request["action"]["attachments"]=json!(assets);}
                // Retain the exact asset receipts and command before dispatch.
                let _=this.update(cx,|this,_|this.cloud.pending_native=Some(request.clone()));
            }
            let result = target.controller.client().call_scoped(methods::CLOUD_TASK_COMMAND,
                json!({"workspaceId":target.workspace,"expectedAccountId":target.subject,
                    "expectedOrganizationId":target.organization,"request":request})).await;
            let _ = this.update(cx, |this, cx| {
                this.cloud.task = None;
                match result {
                    Ok(value) if value["accountId"]==target.subject && value["organizationId"].as_str()==target.organization.as_deref()
                        && value["data"]["commandId"]==request["commandId"] && value["data"]["durablyAdmitted"]==true => {
                        let foreground=request["action"]["type"]=="submit" && request["action"]["payload"]["kind"]=="run";
                        if foreground {
                            this.cloud.admitted_native=Some(json!({"commandId":request["commandId"],"messageId":request["action"]["messageId"]}));
                        }
                        this.cloud.pending_native = None;
                        let sent=std::mem::take(&mut this.cloud.native_staged);
                        if let Some(files)=this.attachments.get_mut(&this.current_key){files.retain(|file|!sent.iter().any(|sent|sent.id==file.id));}
                        this.sync_attachment_chips(cx);
                        if let Some(text)=this.cloud.pending_native_text.take() {
                            if this.input.read(cx).text()==text {this.input.update(cx, |input,cx| input.set_text("",cx));}
                            if request["action"]["type"]=="submit" {
                                if let Some(message)=request["action"]["messageId"].as_str() {
                                    let message=message.to_owned();let session=target.initial_session.clone();
                                    this.state.update(cx,|state,cx| {
                                        state.push_echo(&session,zuse_model::SessionMessageEntry {
                                            id:message.clone(),role:zuse_model::MessageRole::User,
                                            parts:vec![zuse_model::MessagePart::Text{id:format!("text:{message}"),text}],
                                            created_at:chrono::Utc::now().timestamp_millis(),device_id:state.selected_chat_row().map(|chat|chat.device_id.clone()).unwrap_or_default(),
                                            status:None,continuation_of:None,duration_ms:None,
                                        });cx.notify();
                                    });
                                    cx.emit(ComposerEvent::Sent{chat_id:session,message_id:message});
                                }
                            }
                        }
                        if foreground {cx.emit(ComposerEvent::CloudForegroundAdmitted);}
                        cx.emit(ComposerEvent::CloudLaunchChanged);
                    }
                    Ok(_) => this.failure = Some("Message admission was not verified. Retry the original message to check its saved identity.".into()),
                    Err(error) => this.failure = Some(format!("Message admission interrupted: {error}. Retry preserves the same message identity.").into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Show the cloud computer's stage above the composer (`busy` spins).
    pub(crate) fn set_cloud_status(
        &mut self,
        status: Option<(String, bool)>,
        cx: &mut Context<Self>,
    ) {
        if self.cloud.status != status {
            self.cloud.status = status;
            cx.notify();
        }
    }
    #[cfg(test)]
    pub(crate) fn cloud_status(&self) -> Option<&(String, bool)> {
        self.cloud.status.as_ref()
    }
    /// The one status line above the composer while a cloud computer starts:
    /// a new chat's launch, then the conversation's computer stage.
    pub(super) fn cloud_status_row(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<super::tray::TrayPill> {
        let (label, busy) = match self.cloud_creation_activity(cx) {
            Some((label, busy)) => (label.to_owned(), busy),
            None => self.cloud.status.clone()?,
        };
        let pill = super::tray::TrayPill::new("native-cloud-status", label);
        Some(if busy {
            pill.icon_element(crate::loaders::mini_mono_spinner(
                "native-cloud-status-spinner",
                2.,
                theme.text_muted,
                cx.entity_id(),
                cx,
            ))
        } else {
            pill.glyph(crate::icons::DITHER_CLOUD)
        })
    }
    pub(super) fn cloud_creation_activity(&self, cx: &App) -> Option<(&str, bool)> {
        if self.cloud.request.is_none() || self.state.read(cx).selected_chat.is_some() {
            return None;
        }
        Some((self.cloud.message.as_deref().unwrap_or(if self.cloud.task.is_some() {"Starting computer"} else if self.cloud.admitted() {"Cloud startup needs attention. Your message is queued."} else {"Cloud startup needs attention. Your message is saved here; delivery is unconfirmed."}), self.cloud.task.is_some()))
    }
    pub(crate) fn cloud_activity(&self) -> Option<(&str, bool)> {
        if self.cloud.task.is_some() {
            return Some((
                match self
                    .cloud
                    .pending_native
                    .as_ref()
                    .and_then(|v| v["action"]["type"].as_str())
                {
                    Some("queue") => "Saving queued message…",
                    _ if self
                        .cloud
                        .pending_native
                        .as_ref()
                        .is_some_and(|v| v["action"]["payload"]["kind"] == "interrupt") =>
                    {
                        "Stopping agent…"
                    }
                    _ => "Sending message…",
                },
                true,
            ));
        }
        self.cloud
            .admitted_native
            .as_ref()
            .map(|_| ("Waiting for agent…", true))
    }
    pub(crate) fn apply_cloud_response_activity(
        &mut self,
        messages: &[zuse_model::SessionMessageEntry],
        cx: &mut Context<Self>,
    ) {
        if self
            .cloud
            .admitted_native
            .as_ref()
            .and_then(|pending| pending["messageId"].as_str())
            .is_some_and(|message| zuse_viewport::cloud_response_started(messages, message))
        {
            self.cloud.admitted_native = None;
            cx.notify();
        }
    }
    pub(crate) fn apply_cloud_admission_activity(
        &mut self,
        admissions: &Value,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.cloud.admitted_native.as_ref() else {
            return;
        };
        // The authoritative command row takes over even when it is queued.
        if admissions.as_array().is_some_and(|rows| {
            rows.iter()
                .any(|row| row["commandId"] == pending["commandId"])
        }) {
            self.cloud.admitted_native = None;
            cx.notify();
        }
    }

    /// The first message of a cloud chat that is still being created.
    pub(crate) fn cloud_launch_prompt(&self) -> Option<String> {
        self.cloud.request.as_ref()?["firstMessage"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
    }
    pub(crate) fn cloud_launch_pending(&self, cx: &App) -> bool {
        self.cloud.request.is_some() && self.state.read(cx).selected_chat.is_none()
    }
    pub(super) fn cached_cloud(&self) -> bool {
        self.cloud.cached.is_some()
    }
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
        if !self.cloud.rejected_drafts.is_empty() {
            return Some(super::tray::TrayPill::new("rejected-cloud-draft","Previous message was not queued")
                .subtitle("Your previous draft and attachments are preserved.")
                .action(super::tray::tray_text_action("restore-rejected-cloud-draft","Restore previous draft",None,theme)
                    .on_click(cx.listener(|this,_,_,cx| {
                        if !this.input.read(cx).text().is_empty() {
                            this.failure=Some("Clear or save the current draft before restoring the previous message.".into());cx.notify();return;
                        }
                        if !this.cloud.rejected_drafts.is_empty() {
                            let (text,staged)=this.cloud.rejected_drafts.remove(0);
                            this.restore_rejected_draft(text,staged,cx);
                            this.failure=Some("Draft restored. Check the selected repository before sending.".into());cx.notify();
                        }
                    }))));
        }
        if let Some(paused) = self.cloud.cached {
            return Some(
                super::tray::TrayPill::new(
                    "cached-cloud-connection",
                    if paused {
                        "Cloud workspace paused"
                    } else {
                        "Reconnect to cloud workspace"
                    },
                )
                .glyph(crate::icons::DITHER_CLOUD)
                .subtitle("Sending a message will resume this workspace.")
                .action(
                    super::tray::tray_text_action(
                        "resume-cloud-workspace",
                        if paused { "Resume" } else { "Reconnect" },
                        None,
                        theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.emit(ComposerEvent::ResumeCloud {
                            text: this.input.read(cx).text().to_owned(),
                            send: false,
                        });
                    })),
                ),
            );
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
        self.cloud.request.is_some()
            || self.has_draft(cx)
            || self
                .drafts
                .iter()
                .any(|(key, draft)| key != &self.current_key && !draft.trim().is_empty())
            || self.attachments.values().any(|files| !files.is_empty())
            || self.appshots.values().any(|captures| !captures.is_empty())
            || self.queue_edit_draft.is_some()
    }
    pub(crate) fn pause_cloud(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloud.native.is_some() {
            cx.emit(ComposerEvent::CloudSleepRequested);
            return;
        }
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
            organization: state.cloud_organization_id().map(str::to_owned),
            workspace,
            initial_session: state.selected_chat.clone().unwrap_or_default(),
        };
        let command = self
            .cloud
            .pause_command
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        let root = target.controller.clone();
        let request = json!({"workspaceId":target.workspace,"expectedAccountId":target.subject,"expectedOrganizationId":target.organization,"commandId":command});
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
        let Some(mut project) = state.cloud_project.clone() else {
            return false;
        };
        if self
            .cloud
            .request
            .as_ref()
            .is_some_and(|request| request["projectId"] != project["projectId"])
        {
            self.failure=Some("A previous cloud request belongs to another project. Check that computer or retry from its original project before starting different work. Your new draft is preserved.".into());
            cx.notify();
            return true;
        }
        if self.cloud.task.is_some() {
            return true;
        }
        if self.cloud.request.is_none() {
            let refreshed = state.cloud_sidebar_projects().into_iter().find(|row| {
                row["projectId"] == project["projectId"] && row["accountId"] == project["accountId"]
            });
            let access = refreshed.as_ref().unwrap_or(&project);
            if access["canCreate"] == false {
                let reason = access["creationBlockedReason"].as_str().unwrap_or(
                    "Select a repository available through your connected GitHub installation.",
                );
                self.failure = Some(format!("Draft saved · not queued. {reason}").into());
                cx.notify();
                return true;
            }
        }
        if self.cloud.request.is_none() {
            match self.pickers.read(cx).cloud_launch_provider(cx) {
                Ok(provider) => project["providerId"] = json!(provider),
                Err(reason) => {
                    self.failure = Some(format!("Draft saved · not queued. {reason}").into());
                    cx.notify();
                    return true;
                }
            }
        }
        if self.cloud.request.is_none() && !self.pickers.read(cx).cloud_model_ready(cx) {
            self.failure = Some(
                "Choose an available Cloud model before starting. Your draft is preserved.".into(),
            );
            self.failure_key = None;
            cx.notify();
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
        let Some(subject) = state.cloud_user_id().map(str::to_owned) else {
            return true;
        };
        let organization = state.cloud_organization_id().map(str::to_owned);
        if self.cloud.owner.as_deref() != Some(subject.as_str())
            || self.cloud.organization != organization
        {
            self.cloud = Launch {
                owner: Some(subject.clone()),
                organization: organization.clone(),
                ..Default::default()
            };
        }
        let expected_owner = organization
            .as_ref()
            .map(|org| format!("organization:{org}"))
            .unwrap_or_else(|| subject.clone());
        if project["accountId"].as_str() != Some(expected_owner.as_str()) {
            self.failure = Some(
                "The account or organization changed. Select the cloud destination again.".into(),
            );
            self.failure_key = None;
            cx.notify();
            return true;
        }
        if self.cloud.request.is_none() {
            self.cloud.issue = self
                .pickers
                .read(cx)
                .source_issue(cx)
                .map(|(number, markdown)| {
                    StagedAttachment::from_file(
                        &format!("issue-{number}.md"),
                        markdown.into_bytes(),
                    )
                });
        }
        let mut staged = if self.cloud.request.is_some() {
            self.cloud.staged.clone()
        } else {
            self.staged().to_vec()
        };
        if self.cloud.request.is_none() {
            staged.extend(self.cloud.issue.iter().cloned());
        }
        if self.cloud.request.is_none() {
            let (base_ref, branch) = self
                .pickers
                .read(cx)
                .source_cloud_ref(project["defaultBranch"].as_str().unwrap_or("main"), cx);
            let config = self.pickers.read(cx).resolved(cx);
            let (Some(agent), Some(model)) = (config.harness, config.model) else {
                return true;
            };
            if config.plan {
                self.failure = Some(tr(cx, msg::native::composer_cloud_plan_unavailable).into());
                self.failure_key = None;
                cx.notify();
                return true;
            }
            let size = match self
                .pickers
                .read(cx)
                .cloud_launch_size(project["providerId"].as_str().unwrap_or_default(), cx)
            {
                Ok(size) => size,
                Err(reason) => {
                    self.failure = Some(format!("Draft saved · not queued. {reason}").into());
                    self.failure_key = None;
                    cx.notify();
                    return true;
                }
            };
            let Some(connection) = self.pickers.read(cx).cloud_connection_for(agent) else {
                self.failure=Some("This agent needs one usable connection. Check its connection in Settings → Coding agents. Your draft is preserved.".into());
                self.failure_key = None;
                cx.notify();
                return true;
            };
            let runtime_mode = match config.sandbox {
                SandboxLevel::WorkspaceWrite => "approval-required",
                SandboxLevel::Auto => "auto",
                SandboxLevel::DangerFullAccess => "full-access",
            };
            self.cloud.request = Some(
                json!({"runtimeMode":runtime_mode,"agentConnectionId":connection,"projectId":project["projectId"],"providerId":project["providerId"],"sizeId":size,"baseRef":base_ref,"branch":branch,"agent":agent,"model":model,"reasoning":config.reasoning,"modelOptions":config.model_options,"idempotencyKey":uuid::Uuid::new_v4().to_string(),"firstMessage":text,"expectedAccountId":subject,"expectedOrganizationId":organization,"attachmentTransfers":staged.iter().map(|a|json!({"uploadId":a.id,"fileName":a.name})).collect::<Vec<_>>()}),
            );
            if self
                .pickers
                .read(cx)
                .cloud_v2_available(project["providerId"].as_str().unwrap_or("boxd"))
            {
                self.cloud.request.as_mut().unwrap()["initialMessageDelivery"] =
                    json!("server-admitted-v2");
            }
            self.cloud.staged = staged.clone();
            self.input.update(cx, |input, cx| input.set_text("", cx));
            let key = self.current_key.clone();
            if let Some(files) = self.attachments.get_mut(&key) {
                files.retain(|file| !staged.iter().any(|sent| sent.id == file.id));
            }
            self.sync_attachment_chips(cx);
            cx.emit(ComposerEvent::CloudLaunchChanged);
        }
        self.pickers
            .update(cx, |pickers, cx| pickers.freeze_cloud_launch(true, cx));
        let request = self.cloud.request.clone().unwrap();
        let accepted = self.cloud.accepted.clone();
        self.cloud.message = Some(
            if staged.is_empty() {
                "Starting computer"
            } else {
                "Uploading attachments…"
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
                    let _ = this.update(cx, |this, cx| this.cloud_create_failure(message, cx));
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
            if workspace["commandDeliveryMode"]=="server-admitted-v2"&&workspace["commandAdmissionVersion"]==2{
                let _=this.update(cx,|this,cx|{
                    let launch = this.cloud.request.take().unwrap_or_default();
                    this.cloud.accepted=None;this.cloud.staged.clear();this.cloud.issue=None;this.cloud.task=None;this.cloud.message=None;
                    this.pickers.update(cx,|p,cx|p.finish_cloud_launch(cx));
                    cx.emit(ComposerEvent::CloudAdmitted { workspace, account: subject, organization, launch });
                    cx.emit(ComposerEvent::CloudLaunchChanged);cx.notify();
                });return;
            }
            if this
                .update(cx, |this, cx| {
                    this.cloud.accepted = Some(workspace);
                    this.cloud.message = Some("Starting computer".into());
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            let root = controller.clone();
            let workspace_id = id.clone();
            let open_subject=subject.clone();let open_organization=organization.clone();
            let open = gpui_tokio::Tokio::spawn(cx, async move {
                root.client()
                    .subscribe_scoped(
                        methods::CLOUD_ENVIRONMENT,
                        json!({"workspaceId":workspace_id,"expectedAccountId":open_subject,"expectedOrganizationId":open_organization}),
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
                        organization,
                        workspace: id,
                        initial_session: initial,
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.cloud.message = None;
                        let launch = this.cloud.request.take().unwrap_or_default();
                        let workspace = this.cloud.accepted.take().unwrap_or_default();
                        this.cloud.issue = None;
                        this.cloud.staged.clear();
                        this.cloud.task = None;
                        this.pickers
                            .update(cx, |pickers, cx| pickers.finish_cloud_launch(cx));
                        cx.emit(ComposerEvent::CloudLaunchChanged);
                        cx.emit(ComposerEvent::CloudAdmitted {
                            workspace,
                            account: target.subject,
                            organization: target.organization,
                            launch,
                        });
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
    fn cloud_create_failure(&mut self, message: String, cx: &mut Context<Self>) {
        if self.cloud.accepted.is_none()
            && zuse_engine::cloud::definitive_create_rejection(&message).is_some()
        {
            if let Some(request) = self.cloud.request.take() {
                let text = request["firstMessage"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let staged = std::mem::take(&mut self.cloud.staged);
                let same_project = self
                    .state
                    .read(cx)
                    .cloud_project
                    .as_ref()
                    .is_some_and(|project| project["projectId"] == request["projectId"]);
                if same_project && self.input.read(cx).text().is_empty() {
                    self.restore_rejected_draft(text, staged, cx);
                } else {
                    self.cloud.rejected_drafts.push((text, staged));
                }
            }
            self.cloud.issue = None;
            self.cloud.task = None;
            self.cloud.message = None;
            self.pickers
                .update(cx, |pickers, cx| pickers.finish_cloud_launch(cx));
            self.failure = Some(format!("Draft saved · not queued. {message}").into());
            self.failure_key = None;
            cx.emit(ComposerEvent::CloudLaunchChanged);
            cx.notify();
        } else {
            self.cloud_failure(message, cx);
        }
    }
    fn restore_rejected_draft(
        &mut self,
        text: String,
        staged: Vec<StagedAttachment>,
        cx: &mut Context<Self>,
    ) {
        self.input.update(cx, |input, cx| input.set_text(text, cx));
        let files = self
            .attachments
            .entry(self.current_key.clone())
            .or_default();
        for attachment in staged {
            if !files.iter().any(|file| file.id == attachment.id) {
                files.push(attachment);
            }
        }
        self.sync_attachment_chips(cx);
    }
    fn cloud_failure(&mut self, message: String, cx: &mut Context<Self>) {
        self.cloud.task = None;
        self.cloud.message = None;
        self.failure = Some(message.into());
        self.failure_key = None;
        cx.notify();
    }
}

async fn upload_native_asset(
    target: &crate::state::CloudTarget,
    attachment: &StagedAttachment,
) -> Result<Value, String> {
    use base64::Engine as _;
    use sha2::Digest as _;
    if attachment.bytes().is_empty()
        || attachment.bytes().len() > zuse_protocol::cloud_assets::MAX_BYTES
    {
        return Err("Cloud attachments must be non-empty and at most 20 MB.".into());
    }
    for (seq, bytes) in attachment
        .bytes()
        .chunks(zuse_protocol::cloud_assets::CHUNK_BYTES)
        .enumerate()
    {
        target.controller.client().call_scoped(methods::CLOUD_TASK_ASSET,json!({
            "workspaceId":target.workspace,"expectedAccountId":target.subject,"expectedOrganizationId":target.organization,
            "method":methods::UPLOAD_CHUNK,"params":{"uploadId":attachment.id,"seq":seq,"data":base64::engine::general_purpose::STANDARD.encode(bytes)}
        })).await.map_err(|error|error.to_string())?;
    }
    let value=target.controller.client().call_scoped(methods::CLOUD_TASK_ASSET,json!({
        "workspaceId":target.workspace,"expectedAccountId":target.subject,"expectedOrganizationId":target.organization,
        "method":methods::UPLOAD_COMMIT,"params":{"uploadId":attachment.id,"fileName":attachment.name}
    })).await.map_err(|error|error.to_string())?;
    if value["accountId"] != target.subject
        || value["organizationId"].as_str() != target.organization.as_deref()
    {
        return Err("Cloud attachment access changed.".into());
    }
    let asset = &value["data"]["asset"];
    let hash = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(attachment.bytes()));
    if asset["sizeBytes"].as_u64() != Some(attachment.bytes().len() as u64)
        || asset["sha256"] != hash
        || asset["originalName"] != attachment.name
    {
        return Err("The cloud attachment receipt did not match its file.".into());
    }
    Ok(asset.clone())
}

#[cfg(test)]
mod cached_tests {
    use super::*;

    struct AdmissionReceiptService;
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for AdmissionReceiptService {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            assert_eq!(method, methods::CLOUD_TASK_COMMAND);
            assert_eq!(params["request"]["commandId"], "sent-once");
            Ok(zuse_rpc::RpcReply::Value(
                json!({"accountId":"owner","organizationId":null,"data":{"commandId":"sent-once","durablyAdmitted":true}}),
            ))
        }
    }
    #[gpui::test]
    fn admitted_followup_keeps_activity_until_its_authoritative_receipt(
        cx: &mut gpui::TestAppContext,
    ) {
        // Drive both executors explicitly; no background Tokio thread can wake
        // the GPUI deterministic scheduler after teardown.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(AdmissionReceiptService),
        ));
        let state = cx.new(|_| AppState::new());
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx,|composer,cx| {
            composer.cloud.native=Some(NativeTask{target:crate::state::CloudTarget{controller,subject:"owner".into(),organization:None,workspace:"task".into(),initial_session:"session".into()},wake:false,queue_selection_version:1});
            composer.cloud.pending_native=Some(json!({"commandId":"sent-once","sessionId":"session","action":{"type":"submit","messageId":"message","payload":{"kind":"run","request":{"prompt":"Do this"}}}}));
            composer.cloud.pending_native_text=Some("Do this".into());
            composer.dispatch_cloud_task(cx);
            assert_eq!(composer.cloud_activity(),Some(("Sending message…",true)));
        });
        for _ in 0..8 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        cx.run_until_parked();
        composer.update(cx, |composer, cx| {
            assert!(
                composer.cloud.task.is_none(),
                "actual memory RPC receipt completed"
            );
            assert!(composer.cloud.pending_native.is_none());
            assert_eq!(
                composer.cloud_activity(),
                Some(("Waiting for agent…", true)),
                "receipt must not clear pre-response loader"
            );
            let receipt = composer.cloud.admitted_native.clone();
            let entry = |id: &str, role, text: &str| zuse_model::SessionMessageEntry {
                id: id.into(),
                role,
                parts: vec![zuse_model::MessagePart::Text {
                    id: format!("text:{id}"),
                    text: text.into(),
                }],
                created_at: 0,
                device_id: String::new(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            };
            let mut transcript = vec![
                entry("message", zuse_model::MessageRole::User, "Do this"),
                entry("answer", zuse_model::MessageRole::Assistant, ""),
            ];
            composer.apply_cloud_response_activity(&transcript, cx);
            assert!(
                composer.cloud_activity().is_some(),
                "empty assistant placeholder is not a response"
            );
            transcript[1] = entry(
                "answer",
                zuse_model::MessageRole::Assistant,
                "Working on it",
            );
            composer.apply_cloud_response_activity(&transcript, cx);
            assert!(
                composer.cloud_activity().is_none(),
                "actual exact-message response clears only waiting bridge"
            );
            composer.cloud.admitted_native = receipt;
            composer.apply_cloud_admission_activity(
                &json!([{"commandId":"other","executionState":"completed"}]),
                cx,
            );
            assert!(
                composer.cloud_activity().is_some(),
                "unrelated receipt cannot clear pending send"
            );
            composer.apply_cloud_admission_activity(
                &json!([{"commandId":"sent-once","executionState":"completed"}]),
                cx,
            );
            assert!(composer.cloud_activity().is_none());
            composer.cloud.pending_native = Some(json!({"action":{"type":"queue"}}));
            assert!(
                composer.cloud_activity().is_none(),
                "retained queue request alone is not running work"
            );
        });
    }
    #[gpui::test]
    fn cloud_queue_preserves_selected_run_config_and_old_server_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        for version in [0, 1] {
            let (send, receive) = tokio::sync::mpsc::channel(1);
            let (_replies, replies) = tokio::sync::mpsc::channel(1);
            let controller = crate::state::EngineHandle::from_test_client(
                zuse_rpc::RpcClient::new(send, replies),
            );
            let state = cx.new(|_| {
                let mut state = crate::state::AppState::new();
                state.apply_cloud_account(json!({"user":{"id":"queue-owner"},"orgId":null}));
                state
            });
            let composer = cx.new(|cx| Composer::new(state, cx));
            composer.update(cx,|c,cx| {
                let target=crate::state::CloudTarget{controller,subject:"queue-owner".into(),organization:None,workspace:"task".into(),initial_session:"session".into()};
                c.set_cloud_task(target,json!({"agent":"grok","model":"selected-grok","reasoning":"high","modelOptions":{"speed":"fast"},"sandbox":"workspace-write","queueRunSelectionVersion":version}),cx);
                c.input.update(cx,|input,cx|input.set_text("Keep this queued draft",cx));
                c.attachments.insert(c.current_key.clone(),vec![StagedAttachment::from_file("notes.txt",b"retained bytes".to_vec())]);
                assert!(c.send_cloud_task("Keep this queued draft".into(),true,cx));
                assert_eq!(c.input.read(cx).text(),"Keep this queued draft");assert_eq!(c.staged().len(),1);
                if version==0 {assert!(c.cloud.pending_native.is_none());assert!(c.failure.as_deref().unwrap().contains("draft is preserved"));}
                else {let request=c.cloud.pending_native.as_ref().unwrap();assert_eq!(request["action"]["input"]["runSelection"],json!({"harness":"grok","model":"selected-grok","reasoning":"high","modelOptions":{"speed":"fast"},"sandbox":"workspace-write"}));assert_eq!(request["action"]["input"]["text"],"Keep this queued draft");assert_eq!(c.cloud.native_staged.len(),1);c.cloud.task=None;}
            });
            drop(receive);
        }
    }
    #[gpui::test]
    fn new_cloud_request_uses_selected_provider_and_its_model_before_dispatch(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        let (_replies, replies) = tokio::sync::mpsc::channel(1);
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, replies));
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.apply_cloud_account(json!({"user":{"id":"fixture-user"},"orgId":null}));
            state.set_test_engine(controller);
            state
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx,|composer,cx| {
            let context=json!({"accountId":"fixture-user","ownerId":"fixture-user","organizationId":null,"commandAdmissionVersion":2,"computerCapabilities":{"providers":[{"id":"e2b","configured":true,"managedBaseReady":true,"commandAdmissionVersion":2,"preservesDisk":true,"preservesMemory":true,"supportsSnapshots":true,"supportsDocker":false,"sizes":[{"id":"default","label":"Standard","vcpus":2,"memoryMib":2048,"diskGib":8}],"unavailableReason":null}]},"providerAgentCatalogs":{"e2b":{"providers":[{"id":"codex","name":"Codex","defaultModel":"e2b-model","models":[{"id":"e2b-model","label":"E2B model"}]}]}},"agentConnections":[{"agent":"codex","state":"connected","connectionId":"codex-selected"}]});
            composer.pickers.update(cx,|p,cx|p.seed_cloud_creation_for_test(context,json!({"projectId":"repository","accountId":"fixture-user","providerId":"boxd","defaultBranch":"main","canCreate":true}),cx));
            assert!(composer.send_cloud("First E2B task".into(),cx));
            let request=composer.cloud.request.as_ref().expect("actual production send constructed durable request");assert_eq!(request["providerId"],"e2b");assert_eq!(request["projectId"],"repository");assert_eq!(request["model"],"e2b-model");assert_eq!(request["agentConnectionId"],"codex-selected");assert_eq!(request["initialMessageDelivery"],"server-admitted-v2");assert_eq!(request["sizeId"],"default","actual advertised E2B size must be explicit in the request");
            assert!(composer.cloud_creation_activity(cx).unwrap().1,"actual initial Send must animate existing creation tray before create returns");
            let pending=request.clone();
            composer.cloud_failure("Delivery interrupted".into(),cx);
            assert!(!composer.cloud_creation_activity(cx).unwrap().1,"failure stops spinner");
            assert_eq!(composer.cloud.request.as_ref(),Some(&pending),"uncertain failure retains exact request identity and prompt");
        });
        drop(receive);
    }
    #[gpui::test]
    fn new_cloud_request_recomputes_boat_and_e2b_sizes_without_account_reselection(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        let (_replies, replies) = tokio::sync::mpsc::channel(1);
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, replies));
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.apply_cloud_account(json!({"user":{"id":"fixture-user"},"orgId":null}));
            state.set_test_engine(controller);
            state
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx, |composer, cx| {
            let mut caps=vec![];
            let mut catalogs=serde_json::Map::new();
            for (provider,size,memory) in [("boat","small",4096),("e2b","default",2048)] {
                caps.push(json!({"id":provider,"configured":true,"managedBaseReady":true,"commandAdmissionVersion":2,"preservesDisk":true,"preservesMemory":provider=="e2b","supportsSnapshots":true,"supportsDocker":false,"sizes":[{"id":size,"label":"Standard","vcpus":2,"memoryMib":memory,"diskGib":8}],"unavailableReason":null}));
                catalogs.insert(provider.into(),json!({"providers":[{"id":"codex","name":"Codex","defaultModel":format!("{provider}-model"),"models":[{"id":format!("{provider}-model"),"label":provider}]}]}));
            }
            let context=json!({"accountId":"fixture-user","ownerId":"fixture-user","organizationId":null,"commandAdmissionVersion":2,"computerCapabilities":{"providers":caps},"providerAgentCatalogs":catalogs,"agentConnections":[{"agent":"codex","state":"connected","connectionId":"same-codex-account"}]});
            for (provider,size) in [("boat","small"),("e2b","default"),("boat","small")] {
                // Model a new draft on this same composer, retaining its account
                // and provider catalogs while the computer choice changes.
                composer.cloud.task=None;
                composer.cloud.request=None;
                composer.pickers.update(cx,|p,cx|{p.finish_cloud_launch(cx);p.seed_cloud_creation_for_test(context.clone(),json!({"projectId":"repository","accountId":"fixture-user","providerId":provider,"sizeId":"small","defaultBranch":"main","canCreate":true}),cx)});
                assert!(composer.send_cloud("First task on selected computer".into(),cx));
                let request=composer.cloud.request.as_ref().expect("actual Send must construct request");
                assert_eq!(request["providerId"],provider);
                assert_eq!(request["sizeId"],size,"the selected provider's advertised size must be recomputed");
                assert_eq!(request["agentConnectionId"],"same-codex-account");
                assert_eq!(request["model"],format!("{provider}-model"));
            }
            composer.cloud.task=None;
            composer.cloud.request=None;
            let mut unavailable=context.clone();
            unavailable["computerCapabilities"]["providers"]=json!([{"id":"boat","configured":true,"managedBaseReady":true,"commandAdmissionVersion":2,"preservesDisk":true,"preservesMemory":false,"supportsSnapshots":true,"supportsDocker":false,"sizes":[],"unavailableReason":null}]);
            composer.pickers.update(cx,|p,cx|{p.finish_cloud_launch(cx);p.seed_cloud_creation_for_test(unavailable,json!({"projectId":"repository","accountId":"fixture-user","providerId":"boat","defaultBranch":"main","canCreate":true}),cx)});
            composer.input.update(cx,|input,cx|input.set_text("Keep this draft",cx));
            assert!(composer.send_cloud("Keep this draft".into(),cx));
            assert!(composer.cloud.request.is_none());
            assert_eq!(composer.input.read(cx).text(),"Keep this draft");
            assert!(composer.failure.as_deref().unwrap().contains("not queued"));
        });
        drop(receive);
    }
    #[gpui::test]
    fn definitive_create_rejection_restores_draft_but_uncertain_failure_retains_identity(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.cloud_project = Some(json!({"projectId":"project"}));
            state
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx,|composer,cx| {
            let request=json!({"projectId":"project","firstMessage":"Original first message","idempotencyKey":"original-request"});
            composer.cloud.request=Some(request.clone());
            composer.cloud.staged=vec![StagedAttachment::from_file("notes.txt",b"retained bytes".to_vec())];
            composer.cloud_create_failure("This staging account has reached its computer limit. Your new message has not been queued.".into(),cx);
            assert!(composer.cloud.request.is_none());
            assert_eq!(composer.input.read(cx).text(),"Original first message");
            assert_eq!(composer.staged().len(),1);
            assert!(composer.failure.as_ref().unwrap().contains("Draft saved · not queued"));
            composer.cloud.request=Some(request);
            composer.cloud_create_failure("The workspace is busy. Wait for its current operation to finish.".into(),cx);
            assert_eq!(composer.cloud.request.as_ref().unwrap()["idempotencyKey"],"original-request");
            assert_eq!(composer.input.read(cx).text(),"Original first message");
        });
    }

    #[gpui::test]
    fn changing_project_never_retries_a_different_projects_retained_request(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.cloud_project = Some(json!({"projectId":"new-project","accountId":"owner"}));
            state
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx,|composer,cx| {
            composer.cloud.request=Some(json!({"projectId":"old-project","firstMessage":"Original retained message","idempotencyKey":"original-request"}));
            composer.input.update(cx,|input,cx|input.set_text("New project message",cx));
            assert!(composer.send_cloud("New project message".into(),cx));
            assert_eq!(composer.cloud.request.as_ref().unwrap()["idempotencyKey"],"original-request");
            assert_eq!(composer.cloud.request.as_ref().unwrap()["projectId"],"old-project");
            assert_eq!(composer.input.read(cx).text(),"New project message");
            assert!(composer.cloud.task.is_none());
            assert!(composer.failure.as_ref().unwrap().contains("another project"));
        });
    }

    #[gpui::test]
    fn cloud_launch_does_not_block_a_selected_side_chat(cx: &mut gpui::TestAppContext) {
        let parent = cx.new(|_| crate::state::AppState::new());
        let chat = serde_json::from_value(json!({
            "id":"local-chat", "deviceId":"local",
            "createdAt":"2026-01-01T00:00:00Z", "archived":false
        }))
        .unwrap();
        let state = cx.new(|cx| crate::state::AppState::side_chat_state(&parent, chat, true, cx));
        let composer = cx.new(|cx| Composer::new(state.clone(), cx));
        composer.update(cx, |composer, cx| {
            composer.cloud.request = Some(json!({"firstMessage":"Pinned Cloud prompt"}));
            assert!(!composer.cloud_launch_pending(cx));
            assert!(!composer.send_blocked(cx));
        });
        state.update(cx, |state, _| state.selected_chat = None);
        composer.update(cx, |composer, cx| {
            assert!(composer.cloud_launch_pending(cx));
            assert!(composer.send_blocked(cx));
        });
    }

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
        let _subscription = cx.update(|cx| {
            cx.subscribe(&composer, move |_, event, _| {
                if let ComposerEvent::ResumeCloud { text, send } = event {
                    captured.borrow_mut().push((text.clone(), *send));
                }
            })
        });
        composer.update(cx, |composer, cx| {
            composer.set_cached_cloud(true, cx);
            composer.input.update(cx, |input, cx| {
                input.set_text("Continue the retained conversation", cx)
            });
            composer.on_submit(cx);
            assert_eq!(
                composer.input.read(cx).text(),
                "Continue the retained conversation"
            );
            assert!(composer.failure.is_none());
        });
        assert_eq!(
            &*actions.borrow(),
            &[("Continue the retained conversation".to_owned(), true)]
        );
    }
}
