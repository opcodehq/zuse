//! Compact chat/PR permission controls, backed by the owning engine's watch.
use crate::localization::message as tr;
use crate::{state::AppState, theme::Theme};
use gpui::{Context, Entity, SharedString, Task, Window, div, prelude::*, px};
use std::time::Duration;
use zuse_i18n::messages as msg;
use zuse_protocol::pr_watch::{
    MAX_REPAIR_ATTEMPTS, PrWatchRequest, PrWatchStatus, SetPrWatchRequest,
};
use zuse_rpc::methods;

pub(crate) struct PrWatchControl {
    state: Entity<AppState>,
    request: PrWatchRequest,
    target: Option<String>,
    compact: bool,
    status: Option<PrWatchStatus>,
    error: Option<String>,
    connected: bool,
    busy: bool,
    _watch: Option<Task<()>>,
    action: Option<Task<()>>,
}

impl PrWatchControl {
    pub(crate) fn new(
        state: Entity<AppState>,
        chat_id: String,
        url: String,
        target: Option<String>,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut control = Self {
            state,
            request: PrWatchRequest { chat_id, url },
            target,
            compact,
            status: None,
            error: None,
            connected: false,
            busy: false,
            _watch: None,
            action: None,
        };
        control.connect(cx);
        control
    }
    fn connect(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let request = self.request.clone();
        let target = self.target.clone();
        self._watch = Some(cx.spawn(async move |this, cx| {
            let mut delay = 1;
            loop {
                let params = serde_json::json!({"chatId": request.chat_id, "url": request.url, "targetDeviceId": target});
                let observed = engine.client().call(methods::OBSERVE_PR_WATCH, params).await;
                if let Err(error) = &observed {
                    let message = match error {
                        zuse_rpc::RpcError::UnknownMethod(_) => "Update Zuse on the chat's device to watch CI.".to_string(),
                        _ => error.to_string(),
                    };
                    if this.update(cx, |control, cx| { control.error = Some(message); cx.notify(); }).is_err() { return; }
                    if matches!(error, zuse_rpc::RpcError::UnknownMethod(_)) { return; }
                }
                if observed.is_ok() {
                    if let Ok(mut stream) = engine.client().subscribe_checked(methods::WATCH_PR_WATCHES, serde_json::json!({"targetDeviceId": target})).await {
                        while let Some(value) = stream.recv().await {
                            let Ok(statuses) = serde_json::from_value::<Vec<PrWatchStatus>>(value) else { break };
                            if this.update(cx, |control, cx| {
                                control.status = statuses.into_iter().find(|s| s.chat_id == request.chat_id && s.url == request.url);
                                control.error = None;
                                control.connected = true;
                                cx.notify();
                            }).is_err() { return; }
                            delay = 1;
                        }
                    }
                }
                if this.update(cx, |control, cx| { control.connected = false; cx.notify(); }).is_err() { return; }
                cx.background_executor().timer(Duration::from_secs(delay)).await;
                delay = (delay * 2).min(60);
            }
        }));
    }

    fn set(&mut self, watching: bool, auto_fix: bool, reset: bool, cx: &mut Context<Self>) {
        if self.busy || !self.connected {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let mut params = serde_json::to_value(SetPrWatchRequest {
            watch: self.request.clone(),
            enabled: watching,
            auto_fix,
            reset_attempts: reset,
        })
        .unwrap();
        if let Some(target) = &self.target {
            params["targetDeviceId"] = target.clone().into();
        }
        self.busy = true;
        self.action = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::SET_PR_WATCH, params).await;
            let _ = this.update(cx, |control, cx| {
                control.busy = false;
                match result {
                    Ok(value) => match serde_json::from_value::<PrWatchStatus>(value) {
                        Ok(_) => {
                            control.error = None;
                        }
                        Err(_) => control.error = Some(
                            "Watch update could not be confirmed. Reconnect before trying again."
                                .into(),
                        ),
                    },
                    Err(error) => control.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Render for PrWatchControl {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let status = self.status.as_ref();
        let watching = status.is_some_and(|s| s.enabled);
        let fixing = status.is_some_and(|s| s.auto_fix);
        let attempts = status.map_or(0, |s| s.attempts);
        let exhausted = attempts >= MAX_REPAIR_ATTEMPTS;
        let writable = self
            .state
            .read(cx)
            .chats
            .iter()
            .find(|c| c.id == self.request.chat_id)
            .is_some_and(|c| !c.archived && c.config.as_ref().is_some_and(|c| !c.plan));
        let ready = self.connected && !self.busy;
        let error = self
            .error
            .clone()
            .or_else(|| status.and_then(|s| s.error.clone()));
        let ci = status
            .map(|s| s.ci.label())
            .unwrap_or("Connecting to CI watcher…");
        // Each control is a labeled switch: on/off reads at a glance, where a
        // text button left its state to its color.
        let switch_row = |id: &'static str,
                          on: bool,
                          enabled: bool,
                          label: SharedString,
                          meta: Option<SharedString>| {
            div()
                .id(id)
                .role(gpui::Role::Switch)
                .aria_label(label.clone())
                .aria_toggled(if on {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .tab_index(if enabled { 0 } else { -1 })
                .h(px(24.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .px(px(4.0))
                .rounded(px(5.0))
                .text_color(if on { theme.text } else { theme.text_muted })
                .when(!enabled, |row| row.opacity(0.45))
                .focus_visible(move |row| row.bg(theme.element_hover))
                .when(enabled, |row| {
                    row.cursor_pointer()
                        .hover(|row| row.text_color(theme.text))
                        .on_key_down(|event: &gpui::KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                // GPUI activates the focused control on key-up.
                                cx.stop_propagation();
                            }
                        })
                })
                .child(crate::settings::widgets::toggle_switch(&theme, on, id))
                .child(label)
                .children(meta.map(|meta| div().text_color(theme.text_muted).child(meta)))
        };
        let watch_label: SharedString =
            crate::localization::text("projects-github_watch_ci", cx).into();
        let watch_button =
            switch_row("pr-watch-toggle", watching, ready, watch_label, None).when(ready, |row| {
                row.on_click(cx.listener(move |control, _, _, cx| {
                    cx.stop_propagation();
                    control.set(!watching, false, false, cx);
                }))
            });
        let fix_ready = ready && ((writable && status.is_some_and(|s| s.can_auto_fix)) || fixing);
        // The repair budget only shows once a repair has run; a used-up
        // budget turns the switch into a restart.
        let fix_meta = (attempts > 0).then(|| {
            SharedString::from(tr(cx, |l| {
                msg::native::pr_watch_repairs_used(l, attempts, MAX_REPAIR_ATTEMPTS)
            }))
        });
        let fix_button = switch_row(
            "pr-auto-fix-toggle",
            fixing,
            fix_ready,
            if exhausted && !fixing {
                tr(cx, msg::native::pr_watch_restart_auto_fix).into()
            } else {
                tr(cx, msg::native::pr_watch_auto_fix).into()
            },
            fix_meta,
        )
        .tooltip(crate::settings::widgets::text_tooltip(tr(
            cx,
            msg::native::pr_watch_auto_fix_help,
        )))
        .when(fix_ready, |row| {
            row.on_click(cx.listener(move |control, _, _, cx| {
                cx.stop_propagation();
                control.set(true, !fixing, exhausted && !fixing, cx);
            }))
        });
        let help = error.clone().unwrap_or_else(|| ci.to_string());
        div()
            .id("pr-watch-controls")
            .tooltip(crate::settings::widgets::text_tooltip(help))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .text_size(crate::typography::text_xs())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(watch_button)
                    .child(fix_button)
                    .when(!self.compact, |el| {
                        el.child(div().text_color(theme.text_muted).child(ci))
                    }),
            )
            .when_some(error.filter(|_| !self.compact), |el, error| {
                el.child(
                    div()
                        .text_color(theme.danger)
                        .child(SharedString::from(error)),
                )
            })
    }
}
