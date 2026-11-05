//! Compact attached composer notices and explicit recovery actions.
use super::*;
use crate::{
    assets::icon,
    shell::{BORDER, FAINT, MUTED, TEXT, column, control, icon_button, label, row},
};

fn show_preparation_step(status: &Value, step: &crate::cloud::preparation::Step) -> bool {
    !(status["state"] == "ready"
        && step.state == crate::cloud::preparation::StepState::Pending
        && status["preparation"][step.key]["state"].is_null())
}

impl WorkspaceView {
    fn latest_provider_alert(&self) -> Option<(String, String)> {
        if self.is_working() {
            return None;
        }
        let entry = self
            .messages
            .iter()
            .rev()
            .find(|m| m.role == zuse_model::MessageRole::Assistant)?;
        entry.parts.iter().rev().find_map(|part| match part {
            zuse_model::MessagePart::Error { id, message } => {
                let key = format!("{}:{id}", entry.id);
                (!self.dismissed_alerts.contains(&key)).then(|| (key, message.clone()))
            }
            _ => None,
        })
    }
    fn restore_provider_message(
        &mut self,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.queue_edit.is_some()
            || !self.input.read(cx).value(cx).is_empty()
            || !self.attachments.is_empty()
        {
            self.error =
                Some("Keep or clear the current draft before restoring this message.".into());
            cx.notify();
            return;
        }
        let text = self
            .messages
            .iter()
            .rev()
            .find(|m| m.role == zuse_model::MessageRole::User)
            .map(|entry| {
                entry
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        zuse_model::MessagePart::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        if let Some(text) = text {
            let parsed = zuse_protocol::message_attachments::parse(&text);
            self.attachments = parsed
                .attachments
                .into_iter()
                .map(|a| json!({"path":a.path,"name":a.name}))
                .collect();
            self.input
                .update(cx, |input, cx| input.set_value(parsed.text, cx));
            self.dismissed_alerts.insert(key);
            self.sync_previews(true, cx);
            window.focus(&self.input.focus_handle(cx), cx);
            cx.notify();
        }
    }
    pub(crate) fn cloud_preparation_progress(&self) -> crate::cloud::preparation::Progress {
        let session = self.selected.as_ref().and_then(|chat| chat["id"].as_str());
        let waiting = session.is_some_and(|session| {
            crate::cloud::preparation::awaiting_response(
                session,
                &self.cloud.command_admissions,
                &self.messages,
                self.cloud.response_wait.as_ref(),
            ) || self.send_draft.iter().any(|(id, draft)| {
                self.pending
                    .get(id)
                    .is_some_and(|method| method == methods::QUEUE_COMMAND)
                    && draft.chat_id.as_deref() == Some(session)
            })
        });
        crate::cloud::preparation::progress(&self.cloud.guest_status, waiting)
    }
    pub(super) fn has_composer_notices(&self) -> bool {
        (!self.connected && !self.cloud.guest && !self.cloud.mode)
            || (self.cloud.guest
                && (self.cloud.guest_status["state"] != "ready"
                    || self.cloud_preparation_progress().visible))
            || self.transcript_state.notice.is_some()
            || self.error.is_some()
            || self.latest_provider_alert().is_some()
            || self.connection_notice.is_some()
            || !self.uncertain_drafts.is_empty()
            || self.is_working()
            || self.provider_update_notice()
            || self.has_cloud_creation_notice()
            || self.cloud_unreviewed_command().is_some()
    }
    pub(super) fn composer_notices(&self, cx: &mut Context<Self>) -> AnyElement {
        let alert = self
            .error
            .as_ref()
            .map(|error| (format!("local:{error}"), error.clone(), true))
            .or_else(|| {
                self.latest_provider_alert()
                    .map(|(id, text)| (id, text, false))
            });
        let mut view = column().w_full();
        if self.cloud_unreviewed_command().is_some() {
            let pending = self.unknown_review_request.is_some();
            view=view.child(row().min_h(px(38.)).px_3().gap_2()
                .child(column().flex_1().min_w_0().child(label("Check the earlier action before continuing"))
                    .child(label("It may have executed. Check files and running commands first. This review allows new messages; it never retries the earlier action.").text_size(px(11.))))
                .child(control("cloud-review-unknown",if pending{"Reviewing…"}else{"Allow new messages"})
                    .when(pending,|view|view.opacity(0.35))
                    .when(!pending,|view|view.on_click(cx.listener(|this,_,_,cx|this.review_unknown_command(cx))))));
        }

        if self.has_cloud_creation_notice() {
            view = view.child(self.cloud_notices(cx));
        }
        let preparation = self.cloud_preparation_progress();
        if self.cloud.guest && (self.cloud.guest_status["state"] != "ready" || preparation.visible)
        {
            let inactive = matches!(
                self.cloud.guest_status["state"].as_str(),
                Some("paused" | "pausing" | "archived" | "archiving" | "deleting" | "deleted")
            );
            let issue = if inactive {
                None
            } else {
                crate::cloud::preparation::issue(
                    self.cloud.guest_status["statusCode"]
                        .as_str()
                        .unwrap_or_default(),
                )
                .or_else(|| {
                    ["repository", "environment", "credentials", "agent"]
                        .into_iter()
                        .find_map(|key| {
                            let stage = &self.cloud.guest_status["preparation"][key];
                            (stage["state"] == "failed")
                                .then(|| {
                                    crate::cloud::preparation::issue(
                                        stage["errorCode"].as_str().unwrap_or_default(),
                                    )
                                    .or_else(|| match key {
                                        "repository" => crate::cloud::preparation::issue(
                                            "repository_preparation_failed",
                                        ),
                                        "environment" => crate::cloud::preparation::issue(
                                            "environment_setup_failed",
                                        ),
                                        _ => None,
                                    })
                                })
                                .flatten()
                        })
                })
            };
            let state = self.cloud.guest_status["state"]
                .as_str()
                .unwrap_or("connecting");
            let retry = issue.is_none() && matches!(state, "blocked" | "failed" | "paused");
            let title = if let Some(issue) = issue {
                issue.title
            } else if state == "paused" {
                "Computer sleeping"
            } else if state == "pausing" {
                "Putting computer to sleep"
            } else if matches!(state, "deleting" | "deleted") {
                "Computer deleted or being removed"
            } else if matches!(state, "archiving" | "archived") {
                "Computer archived or being archived"
            } else if retry {
                "Cloud connection needs attention"
            } else {
                preparation.title.as_str()
            };
            view = view.child(
                row()
                    .min_h(px(32.))
                    .px_3()
                    .gap_2()
                    .id("cloud-preparation-status")
                    .role(Role::Status)
                    .aria_label(title.to_owned())
                    .child(if preparation.busy {
                        crate::acp_catalog::spinner("cloud-preparation-spinner", 14.)
                    } else {
                        icon("cloud-dither").size(px(14.)).into_any_element()
                    })
                    .child(column().flex_1().min_w_0().child(label(title.to_owned())).when_some(
                        issue.map(|issue| issue.detail.to_owned()).or_else(|| {
                            self.cloud.guest_status["message"]
                                .as_str()
                                .map(str::to_owned)
                                .or_else(|| preparation.busy.then(|| preparation.next.to_owned()))
                        }),
                        |view, detail| view.child(label(detail).text_size(px(11.))),
                    ))
                    .child(
                        control(
                            "cloud-preparation-details",
                                        if self.cloud.preparation_open {
                                "Hide activity"
                            } else {
                                "Activity"
                            },
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cloud.preparation_open = !this.cloud.preparation_open;
                            if this.cloud.preparation_open {
                                let _ = this
                                    .requests
                                    .unbounded_send(json!({"ui":"cloudPreparationLogs"}));
                            }
                            cx.notify();
                        })),
                    )
                    .when_some(
                        issue.and_then(|issue| issue.recovery.label()),
                        |view, action| {
                            view.child(control("cloud-preparation-recovery", action).on_click(
                                cx.listener(move |this, _, _, _| {
                                    let agent = issue.is_some_and(|issue| issue.recovery == crate::cloud::preparation::Recovery::Agent);
                                    let _ = this.requests.unbounded_send(json!({"ui":if agent {"cloudAgentSetup"} else {"cloudSetup"}}));
                                }),
                            ))
                        },
                    )
                    .when(retry, |view| {
                        view.child(
                            control(
                                "retry-cloud-connection",
                                if state == "paused" {
                                    "Resume"
                                } else {
                                    "Retry connection"
                                },
                            )
                            .on_click(cx.listener(|this, _, _, _| {
                                let _ = this.requests.unbounded_send(json!({"ui":"resumeCloud"}));
                            })),
                        )
                    }),
            );
            if preparation.busy
                && self.cloud.guest_status["state"] != "ready"
                && !self.cloud.preparation_open
            {
                let completed = preparation
                    .steps
                    .iter()
                    .filter(|step| step.state == crate::cloud::preparation::StepState::Completed)
                    .count();
                let remaining = preparation
                    .steps
                    .iter()
                    .filter(|step| step.state != crate::cloud::preparation::StepState::Completed)
                    .map(|step| format!("{}: {}", step.title, step.state.label()))
                    .collect::<Vec<_>>()
                    .join(" · ");
                view = view.child(
                    div()
                        .id("cloud-preparation-steps")
                        .px_3()
                        .pb_2()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child(if completed == 0 {
                            remaining
                        } else if remaining.is_empty() {
                            format!("{completed} steps done")
                        } else {
                            format!("{completed} done · {remaining}")
                        }),
                );
            }
            if self.cloud.preparation_open {
                let status = &self.cloud.guest_status;
                let mut activity = column()
                    .px_3()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(BORDER));
                let mut failed_stage = false;
                for step in preparation
                    .steps
                    .iter()
                    .filter(|step| show_preparation_step(status, step))
                {
                    failed_stage |= step.state == crate::cloud::preparation::StepState::Failed;
                    activity = activity.child(
                        row()
                            .gap_2()
                            .child(label(step.title).text_size(px(11.)))
                            .child(label(step.state.label()).text_size(px(11.)).text_color(rgb(
                                if step.state == crate::cloud::preparation::StepState::Failed {
                                    0xeb8f8f
                                } else {
                                    MUTED
                                },
                            ))),
                    );
                    if let Some(error) = status["preparation"][step.key]["errorCode"].as_str() {
                        if let Some(step_issue) = crate::cloud::preparation::issue(error)
                            && issue.is_none_or(|issue| issue.detail != step_issue.detail)
                        {
                            activity = activity.child(
                                label(step_issue.detail)
                                    .text_size(px(11.))
                                    .text_color(rgb(0xeb8f8f)),
                            );
                        }
                    }
                }
                if failed_stage && issue.is_none() {
                    activity =
                        activity.child(control("cloud-prepare-settings", "Review setup").on_click(
                            cx.listener(|this, _, _, _| {
                                let _ = this.requests.unbounded_send(json!({"ui":"cloudSetup"}));
                            }),
                        ));
                }
                let log = &status["preparationLog"];
                activity = activity.child(
                    control("refresh-cloud-preparation-output", "Refresh output").on_click(
                        cx.listener(|this, _, _, _| {
                            let _ = this
                                .requests
                                .unbounded_send(json!({"ui":"cloudPreparationLogs"}));
                        }),
                    ),
                );
                if log.is_object() {
                    let output = format!(
                        "{}{}{}",
                        log["stdout"].as_str().unwrap_or_default(),
                        if log["stdout"].as_str().is_some_and(|s| !s.is_empty()) {
                            "\n"
                        } else {
                            ""
                        },
                        log["stderr"].as_str().unwrap_or_default()
                    );
                    activity = activity
                        .child(label("Initialization output").text_size(px(11.)))
                        .child(
                            column()
                                .id("cloud-preparation-output")
                                .max_h(px(180.))
                                .overflow_y_scroll()
                                .child(
                                    div()
                                        .font_family("Geist Mono")
                                        .text_size(px(11.))
                                        .child(output),
                                ),
                        );
                    if log["truncated"] == true {
                        activity = activity.child(
                            label("Output truncated.")
                                .text_size(px(11.))
                                .text_color(rgb(MUTED)),
                        );
                    }
                }
                if let Some(error) = status["preparationLogError"].as_str() {
                    activity = activity.child(
                        label(error.to_owned())
                            .text_size(px(11.))
                            .text_color(rgb(0xeb8f8f)),
                    );
                }
                view = view.child(activity);
            }
        }

        if let Some(notice) = &self.transcript_state.notice {
            view = view.child(
                row()
                    .id("transcript-refresh-notice")
                    .role(Role::Status)
                    .min_h(px(32.))
                    .px_3()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(icon("refresh").size(px(14.)).text_color(rgb(MUTED)))
                    .child(label(notice.clone()).flex_1().min_w_0().text_size(px(11.)))
                    .child(
                        control("retry-transcript", "Retry")
                            .tab_index(0)
                            .h(px(24.))
                            .on_click(cx.listener(|this, _, _, cx| this.retry_transcript(cx))),
                    ),
            );
        }

        if self.provider_update_notice() {
            view = view.child(
                row()
                    .id("provider-updates-notice")
                    .role(Role::Status)
                    .min_h(px(32.))
                    .px_3()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(icon("refresh").size(px(14.)).text_color(rgb(MUTED)))
                    .child(
                        label("Agent provider updates")
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.)),
                    )
                    .child(
                        control("review-provider-updates", "Review")
                            .h(px(24.))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.settings = true;
                                this.select_settings_section(
                                    crate::settings_view::Section::Providers,
                                    cx,
                                );
                                window.focus(&this.settings_nav_focus, cx);
                            })),
                    ),
            );
        }
        if !self.connected && !self.cloud.guest && !self.cloud.mode {
            view = view.child(
                row()
                    .id("connection-alert")
                    .role(Role::Status)
                    .min_h(px(32.))
                    .px_3()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(icon("globe").size(px(14.)).text_color(rgb(0xe2b876)))
                    .child(
                        label(self.status.clone())
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.)),
                    ),
            );
        }
        if let Some((id, message, local)) = alert {
            let lower = message.to_lowercase();
            let title = if lower.contains("auth")
                || lower.contains("sign in")
                || lower.contains("api key")
                || lower.contains("log in")
            {
                "Sign in to continue"
            } else if lower.contains("quota")
                || lower.contains("rate limit")
                || lower.contains("usage limit")
            {
                "Usage limit reached"
            } else if lower.contains("not installed") {
                "Agent unavailable"
            } else if lower.contains("file") || lower.contains("save") {
                "File needs attention"
            } else {
                "Something went wrong"
            };
            let expanded = self.expanded_alert.as_deref() == Some(&id);
            let restore = id.clone();
            let account_action = lower.contains("auth")
                || lower.contains("sign in")
                || lower.contains("api key")
                || lower.contains("log in")
                || lower.contains("not installed")
                || lower.contains("quota")
                || lower.contains("usage limit");
            let toggle = id.clone();
            let dismiss = id.clone();
            let copy = message.clone();
            view = view.child(
                column()
                    .id("composer-error")
                    .role(Role::Alert)
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        row()
                            .min_h(px(32.))
                            .px_3()
                            .py_1()
                            .gap_2()
                            .child(
                                icon("danger-triangle")
                                    .size(px(14.))
                                    .text_color(rgb(0xe2b876)),
                            )
                            .child(
                                row()
                                    .id("expand-composer-error")
                                    .cursor_pointer()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(11.))
                                            .text_color(rgb(MUTED))
                                            .text_ellipsis()
                                            .child(message.clone()),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.expanded_alert =
                                            if this.expanded_alert.as_deref() == Some(&toggle) {
                                                None
                                            } else {
                                                Some(toggle.clone())
                                            };
                                        cx.notify();
                                    })),
                            )
                            .child(
                                icon_button("copy-composer-error", "copy", "Copy error details")
                                    .size(px(24.))
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.clone(),
                                        ))
                                    }),
                            )
                            .when(!local, |d| {
                                d.child(
                                    crate::styling::dither_action(
                                        "restore-provider-message",
                                        "Restore message",
                                        true,
                                        true,
                                    )
                                    .h(px(26.))
                                    .text_size(px(11.))
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.restore_provider_message(
                                                restore.clone(),
                                                window,
                                                cx,
                                            )
                                        },
                                    )),
                                )
                            })
                            .when(account_action, |d| {
                                d.child(
                                    control("composer-error-settings", "Settings")
                                        .h(px(26.))
                                        .text_size(px(11.))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.settings = true;
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(
                                icon_button("dismiss-error", "close", "Dismiss error")
                                    .size(px(24.))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if local {
                                            this.error = None;
                                        } else {
                                            this.dismissed_alerts.insert(dismiss.clone());
                                        }
                                        this.expanded_alert = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when(expanded, |d| {
                        d.child(
                            div()
                                .id("composer-error-details")
                                .max_h(px(160.))
                                .overflow_y_scroll()
                                .px_3()
                                .py_2()
                                .border_t_1()
                                .border_color(rgb(BORDER))
                                .text_size(px(11.))
                                .line_height(px(18.))
                                .text_color(rgb(MUTED))
                                .child(message),
                        )
                    }),
            );
        }
        if self.connection_notice.is_some() || !self.uncertain_drafts.is_empty() {
            view = view.child(self.connection_recovery_view(cx));
        }
        if self.is_working() {
            view = view.child(
                row()
                    .id("agent-working-tray")
                    .role(Role::Status)
                    .min_h(px(32.))
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(div().size(px(5.)).rounded_full().bg(rgb(0x89ca21)))
                    .child(label("Working").text_color(rgb(TEXT)).text_size(px(11.)))
                    .child(label("New messages join the queue").text_size(px(10.)))
                    .child(div().flex_1()),
            );
        }
        view.into_any_element()
    }
    pub(super) fn access_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .id("access-menu")
            .role(Role::ListBox)
            .aria_label("Agent access")
            .occlude()
            .absolute()
            .bottom_full()
            .mb_2()
            .left(px(40.))
            .w(px((self.content_width - 80.).clamp(200., 256.)))
            .p_1()
            .gap(px(0.))
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(0x343434))
            .bg(rgb(0x20211e))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                label("AGENT ACCESS")
                    .text_size(px(10.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(MUTED))
                    .px_2()
                    .pt(px(6.))
                    .pb(px(4.)),
            )
            .children(
                zuse_protocol::presentation::ACCESS_LEVELS
                    .into_iter()
                    .enumerate()
                    .map(|(i, mode)| {
                        let details = zuse_protocol::presentation::access_details(mode);
                        row()
                            .id(("access-mode", i))
                            .role(Role::ListBoxOption)
                            .aria_label(details.label)
                            .aria_selected(self.sandbox == mode)
                            .cursor_pointer()
                            .px_2()
                            .py_2()
                            .gap_2()
                            .rounded(px(6.))
                            .when(self.access_active == i, |view| view.bg(rgb(0x2b2b28)))
                            .hover(|d| d.bg(rgb(0x2b2b28)))
                            .child(
                                column()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(4.))
                                    .child(
                                        row()
                                            .gap_2()
                                            .child(
                                                svg()
                                                    .path(details.icon)
                                                    .size(px(14.))
                                                    .flex_none()
                                                    .text_color(rgb(TEXT)),
                                            )
                                            .child(
                                                div()
                                                    .text_color(rgb(TEXT))
                                                    .text_size(px(12.))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(details.label),
                                            ),
                                    )
                                    .child(
                                        label(details.description)
                                            .text_size(px(11.))
                                            .line_height(px(16.)),
                                    ),
                            )
                            .when(self.sandbox == mode, |view| {
                                view.child(icon("check").size(px(14.)).text_color(rgb(TEXT)))
                            })
                            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                if this.access_active != i {
                                    this.access_active = i;
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.choose_access(mode, cx);
                                window.focus(&this.input.focus_handle(cx), cx);
                                cx.notify();
                            }))
                    }),
            )
            .child(
                label(if self.access_save.is_some() {
                    "Saving permission mode…"
                } else if self.selected.is_none() {
                    "Used when you start this chat."
                } else {
                    "Saved for this chat and its next turns."
                })
                .px_2()
                .py_2()
                .text_size(px(10.))
                .text_color(rgb(FAINT)),
            )
            .into_any_element()
    }
    pub(super) fn composer_add_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .id("composer-add-menu")
            .absolute()
            .bottom_full()
            .mb_2()
            .left_0()
            .w(px(220.))
            .p_1()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x343434))
            .bg(rgb(0x20211e))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                control("attach-files-menu", "Attach images or files")
                    .h(px(34.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_menu = false;
                        this.pick_attachments();
                        cx.notify();
                    })),
            )
            .child(
                control("composer-tools-settings", "Plugins & connections")
                    .h(px(34.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_menu = false;
                        this.settings = true;
                        this.plugin_action(json!({"action":"list"}));
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn ready_activity_hides_unreported_pending_stages_without_claiming_completion() {
        let ready = json!({"state":"ready","preparation":{"repository":{"state":"completed"},"environment":{"state":"failed"},"credentials":{"state":"pending"}}});
        let progress = crate::cloud::preparation::progress(&ready, true);
        let shown: Vec<_> = progress
            .steps
            .iter()
            .filter(|step| show_preparation_step(&ready, step))
            .map(|step| step.key)
            .collect();
        assert!(shown.contains(&"repository"));
        assert!(
            shown.contains(&"environment"),
            "failures must remain visible"
        );
        assert!(
            shown.contains(&"credentials"),
            "an explicit pending receipt is still meaningful"
        );
        assert!(
            !shown.contains(&"agent"),
            "legacy runtime never reported an agent stage"
        );
        assert_eq!(
            progress
                .steps
                .iter()
                .find(|step| step.key == "agent")
                .unwrap()
                .state,
            crate::cloud::preparation::StepState::Pending,
            "display filtering must not manufacture completion"
        );
        let preparing = json!({"state":"setup"});
        assert!(
            progress
                .steps
                .iter()
                .all(|step| show_preparation_step(&preparing, step)),
            "preparation still shows upcoming work"
        );
    }
}
