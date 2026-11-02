//! Provider updates are owned by the connected computer and confirmed by its watch.
use super::*;
use crate::localization::message as tr;
use crate::{
    settings_style as s,
    shell::{BORDER, MUTED, column, row},
};
use zuse_i18n::messages as msg;
use zuse_protocol::{
    HarnessId, HarnessUpdatePhase as Phase, HarnessUpdatePolicy, HarnessUpdateStatus,
};

fn in_progress(phase: Phase) -> bool {
    matches!(
        phase,
        Phase::WaitingForIdle
            | Phase::Preparing
            | Phase::Downloading
            | Phase::Installing
            | Phase::Verifying
    )
}
impl WorkspaceView {
    pub(super) fn refresh_provider_updates(&mut self) {
        self.provider_updates_confirmed = false;
        if let Some(id) = self.provider_updates_watch.take() {
            self.cancel(id);
        }
        if self.connected {
            self.provider_updates_watch = Some(self.call(
                methods::WATCH_HARNESS_UPDATES,
                json!({"targetDeviceId":self.device_id}),
            ));
        }
    }
    fn update_action(
        &mut self,
        method: &'static str,
        harness: Option<HarnessId>,
        policy: Option<HarnessUpdatePolicy>,
        cx: &mut Context<Self>,
    ) {
        if !self.connected
            || !self.provider_updates_confirmed
            || self.provider_updates_cancel.is_some()
            || (self.provider_updates_action.is_some() && method != methods::CANCEL_HARNESS_UPDATE)
        {
            return;
        }
        if let Some(harness) = harness {
            let Some(status) = self
                .provider_updates
                .iter()
                .find(|status| status.harness == harness)
            else {
                return;
            };
            if method == methods::APPLY_HARNESS_UPDATE && !status.can_start_update() {
                return;
            }
            if method == methods::CANCEL_HARNESS_UPDATE && !in_progress(status.phase) {
                return;
            }
            if method == methods::SET_HARNESS_UPDATE_POLICY && policy.is_none() {
                return;
            }
        } else if method != methods::CHECK_HARNESS_UPDATES {
            return;
        }
        let id = self.call(
            method,
            json!({"targetDeviceId":self.device_id,"harness":harness,"policy":policy}),
        );
        if method == methods::CANCEL_HARNESS_UPDATE {
            self.provider_updates_cancel = Some(id);
        } else {
            self.provider_updates_action = Some((id, method));
        }
        self.provider_updates_error = None;
        cx.notify();
    }
    pub(super) fn receive_provider_updates(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let watching = self.provider_updates_watch == Some(id);
        let canceling = self.provider_updates_cancel == Some(id);
        let acting = self
            .provider_updates_action
            .is_some_and(|(request, _)| request == id);
        if !watching && !acting && !canceling {
            return false;
        }
        if let Some(error) = frame["err"].as_str() {
            let canceled_update = error == "update cancelled"
                && self
                    .provider_updates_action
                    .is_some_and(|(request, method)| {
                        request == id && method == methods::APPLY_HARNESS_UPDATE
                    });
            if !canceled_update {
                self.provider_updates_error = Some(error.to_owned());
            }
            self.pending.remove(&id);
            if watching {
                self.provider_updates_watch = None;
                self.provider_updates_confirmed = false;
            }
            if canceling {
                self.provider_updates_cancel = None;
                self.refresh_provider_updates();
            }
            if acting {
                self.provider_updates_action = None;
                self.refresh_provider_updates();
            }
        } else if watching {
            if let Some(value) = frame.get("item").or_else(|| frame.get("ok")) {
                match serde_json::from_value::<Vec<HarnessUpdateStatus>>(value.clone()) {
                    Ok(statuses) => {
                        self.provider_updates = statuses;
                        self.provider_updates_confirmed = true;
                    }
                    Err(_) => {
                        self.provider_updates_confirmed = false;
                        self.provider_updates_error = Some("This computer returned an invalid provider update status. Refresh before making changes.".into());
                    }
                }
            }
            if frame["done"] == true {
                self.pending.remove(&id);
                self.provider_updates_watch = None;
                self.provider_updates_confirmed = false;
            }
        } else if frame.get("ok").is_some() || frame["done"] == true {
            let method = if canceling {
                self.provider_updates_cancel = None;
                methods::CANCEL_HARNESS_UPDATE
            } else {
                self.provider_updates_action.take().unwrap().1
            };
            self.pending.remove(&id);
            let reply = &frame["ok"];
            let confirmed = match method {
                methods::CHECK_HARNESS_UPDATES => {
                    serde_json::from_value::<Vec<HarnessUpdateStatus>>(reply.clone()).is_ok()
                }
                methods::APPLY_HARNESS_UPDATE => reply["ok"] == true,
                methods::CANCEL_HARNESS_UPDATE => reply["cancelled"].is_boolean(),
                methods::SET_HARNESS_UPDATE_POLICY => {
                    serde_json::from_value::<HarnessUpdateStatus>(reply.clone()).is_ok()
                }
                _ => false,
            };
            if !confirmed {
                self.provider_updates_error = Some("The computer did not confirm this update action. Review its refreshed status before trying again.".into());
            }
            self.refresh_provider_updates();
        }
        cx.notify();
        true
    }
    pub(super) fn provider_update_notice(&self) -> bool {
        self.preferences.provider_update_notifications
            && self.connected
            && self.provider_updates_confirmed
            && self
                .provider_updates
                .iter()
                .any(HarnessUpdateStatus::show_update_notice)
    }
    pub(super) fn check_provider_updates(&mut self, cx: &mut Context<Self>) {
        if !self.provider_updates_confirmed {
            self.refresh_provider_updates();
        } else {
            self.update_action(methods::CHECK_HARNESS_UPDATES, None, None, cx);
        }
    }
    fn provider_update_status(&self, id: &str) -> Option<&HarnessUpdateStatus> {
        self.provider_updates.iter().find(|status| {
            serde_json::to_value(status.harness)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .as_deref()
                == Some(id)
        })
    }
    pub(super) fn provider_update_title(
        &self,
        id: &str,
        name: String,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut title = row()
            .gap(px(6.))
            .min_w_0()
            .child(s::title_text(name.clone()));
        if let Some(status) = self.provider_update_status(id) {
            if let Some(version) = &status.installed_version {
                title = title.child(
                    s::description_text(format!("v{version}"))
                        .text_size(px(10.))
                        .line_height(px(13.5))
                        .font_family("Geist Mono"),
                );
            }
            let action = if in_progress(status.phase) {
                Some((methods::CANCEL_HARNESS_UPDATE, "Updating", "refresh"))
            } else if status.can_start_update() {
                Some((methods::APPLY_HARNESS_UPDATE, "Update", "alt-arrow-up"))
            } else {
                None
            };
            if let Some((method, label, glyph)) = action {
                let harness = status.harness;
                let label = if method == methods::APPLY_HARNESS_UPDATE {
                    format!("v{}", status.latest_version.as_deref().unwrap_or_default())
                } else {
                    label.into()
                };
                let busy = !self.connected
                    || !self.provider_updates_confirmed
                    || self.provider_updates_cancel.is_some()
                    || (self.provider_updates_action.is_some()
                        && method != methods::CANCEL_HARNESS_UPDATE);
                title = title.child(
                    row()
                        .id(("provider-update", index))
                        .role(Role::Button)
                        .tab_index(0)
                        .aria_label(format!(
                            "{} {name} update",
                            if method == methods::CANCEL_HARNESS_UPDATE {
                                "Cancel"
                            } else {
                                "Install"
                            }
                        ))
                        .cursor_pointer()
                        .h(px(20.))
                        .px_1()
                        .gap_1()
                        .rounded(px(4.))
                        .text_color(crate::palette::rgb(
                            if method == methods::APPLY_HARNESS_UPDATE {
                                0xe2b876
                            } else {
                                MUTED
                            },
                        ))
                        .hover(|view| view.bg(s::frame_bg().opacity(0.6)))
                        .when(busy, |view| view.opacity(0.5))
                        .child(crate::assets::icon(glyph).size(px(14.)))
                        .child(div().text_size(px(10.)).child(label))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.update_action(method, Some(harness), None, cx);
                        }))
                        .on_key_down(|event: &KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                cx.stop_propagation();
                            }
                        }),
                );
            } else if matches!(
                status.phase,
                Phase::Updated | Phase::Checking | Phase::Failed
            ) {
                title = title.child(
                    s::description_text(match status.phase {
                        Phase::Updated => crate::localization::text("providers-updated", cx),
                        Phase::Checking => {
                            crate::localization::text("simulators-status_checking", cx)
                        }
                        _ => crate::localization::text("chat-update_banner_update_failed", cx),
                    })
                    .text_size(px(10.)),
                );
            }
        }
        title.into_any_element()
    }
    fn on_provider_policy_key(
        &mut self,
        harness: HarnessId,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        let policies = [
            HarnessUpdatePolicy::Notify,
            HarnessUpdatePolicy::AutoWhenIdle,
            HarnessUpdatePolicy::Off,
        ];
        let Some(status) = self
            .provider_updates
            .iter()
            .find(|status| status.harness == harness)
        else {
            return;
        };
        let index = policies
            .iter()
            .position(|policy| *policy == status.policy)
            .unwrap_or(0);
        let next = match event.keystroke.key.as_str() {
            "left" | "up" => (index + 2) % 3,
            "right" | "down" => (index + 1) % 3,
            "home" => 0,
            "end" => 2,
            "enter" | "space" if !event.is_held => index,
            _ => return,
        };
        self.update_action(
            methods::SET_HARNESS_UPDATE_POLICY,
            Some(harness),
            Some(policies[next]),
            cx,
        );
        cx.stop_propagation();
    }
    pub(super) fn provider_update_details(
        &self,
        id: &str,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut details = column().gap_2();
        let Some(status) = self.provider_update_status(id) else {
            return details.into_any_element();
        };
        let harness = status.harness;
        let busy = !self.connected
            || !self.provider_updates_confirmed
            || self.provider_updates_action.is_some()
            || self.provider_updates_cancel.is_some();
        let choices = [
            (HarnessUpdatePolicy::Notify, "Notify"),
            (HarnessUpdatePolicy::AutoWhenIdle, "Auto when idle"),
            (HarnessUpdatePolicy::Off, "Off"),
        ];
        details = details.child(
            row()
                .id("provider-update-policies")
                .role(Role::RadioGroup)
                .aria_label(tr(cx, msg::native::provider_update_policy))
                .track_focus(&self.provider_policy_focus)
                .tab_index(0)
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                    this.on_provider_policy_key(harness, event, cx)
                }))
                .gap_2()
                .flex_wrap()
                .child(s::description_text(tr(
                    cx,
                    msg::settings::update_channel_updates,
                )))
                .children(
                    choices
                        .into_iter()
                        .enumerate()
                        .map(|(choice, (policy, label))| {
                            s::button(
                                SharedString::from(format!(
                                    "provider-update-policy-{index}-{choice}"
                                )),
                                label,
                            )
                            .role(Role::RadioButton)
                            .tab_stop(false)
                            .aria_toggled(if policy == status.policy {
                                Toggled::True
                            } else {
                                Toggled::False
                            })
                            .when(policy == status.policy, |button| button.bg(s::frame_bg()))
                            .when(busy, |button| button.opacity(0.5))
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    window.focus(&this.provider_policy_focus, cx);
                                    this.update_action(
                                        methods::SET_HARNESS_UPDATE_POLICY,
                                        Some(harness),
                                        Some(policy),
                                        cx,
                                    )
                                },
                            ))
                        }),
                ),
        );
        if let Some(progress) = &status.progress {
            if let Some(message) = &progress.message {
                details = details.child(s::description_text(message.clone()));
            }
            if let (Some(done), Some(total)) = (
                progress.completed_bytes,
                progress.total_bytes.filter(|total| *total > 0),
            ) {
                details = details.child(
                    div()
                        .h(px(3.))
                        .rounded_full()
                        .bg(crate::palette::rgb(BORDER))
                        .child(
                            div()
                                .h_full()
                                .w(relative((done as f64 / total as f64).clamp(0., 1.) as f32))
                                .rounded_full()
                                .bg(crate::palette::rgb(0x89ca21)),
                        ),
                );
            }
        }
        if let Some(command) = &status.manual_command {
            let command = command.clone();
            details = details.child(
                row()
                    .gap_2()
                    .child(s::description_text(command.clone()).flex_1().min_w_0())
                    .child(
                        s::button(
                            ("copy-update-command", index),
                            tr(cx, msg::settings::snapshot_copy_command),
                        )
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(command.clone()))
                        }),
                    ),
            );
        }
        if let Some(error) = &status.error {
            details = details.child(s::description_text(error.message.clone()));
        }
        details.into_any_element()
    }
}
