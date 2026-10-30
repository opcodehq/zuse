//! Compact chat/PR permission controls, backed by the owning engine's watch.
use crate::{state::AppState, theme::Theme};
use gpui::{Context, Entity, SharedString, Task, Window, div, prelude::*, px};
use std::time::Duration;
use zuse_protocol::{
    SandboxLevel,
    pr_watch::{MAX_REPAIR_ATTEMPTS, PrWatchRequest, PrWatchStatus, SetPrWatchRequest},
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
            .is_some_and(|c| {
                !c.archived
                    && c.config
                        .as_ref()
                        .is_some_and(|c| c.sandbox != SandboxLevel::ReadOnly)
            });
        let ready = self.connected && !self.busy;
        let error = self
            .error
            .clone()
            .or_else(|| status.and_then(|s| s.error.clone()));
        let ci = status
            .map(|s| s.ci.label())
            .unwrap_or("Connecting to CI watcher…");
        let watch_button = div()
            .id("pr-watch-toggle")
            .role(gpui::Role::Button)
            .tab_index(if ready { 0 } else { -1 })
            .aria_label(if watching {
                "Pause CI watch"
            } else {
                "Watch CI"
            })
            .rounded(px(5.0))
            .px(px(6.0))
            .py(px(3.0))
            .text_color(if watching {
                theme.accent
            } else {
                theme.text_muted
            })
            .focus_visible(move |el| el.border_1().border_color(theme.accent))
            .when(ready, |el| {
                el.cursor_pointer()
                    .hover(|el| el.bg(theme.element_hover))
                    .on_click(cx.listener(move |control, _, _, cx| {
                        cx.stop_propagation();
                        control.set(!watching, false, false, cx);
                    }))
                    .on_key_down(cx.listener(move |_, event: &gpui::KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            // GPUI activates the focused control on key-up.
                            cx.stop_propagation();
                        }
                    }))
            })
            .child(if watching { "Watching CI" } else { "Watch CI" });
        let fix_ready = ready && ((writable && status.is_some_and(|s| s.can_auto_fix)) || fixing);
        let fix_label = if fixing {
            format!("Auto-fix on · {attempts}/3")
        } else if exhausted {
            "Restart auto-fix · 3/3".into()
        } else {
            format!("Auto-fix · {attempts}/3")
        };
        let fix_button = div().id("pr-auto-fix-toggle").role(gpui::Role::Button).tab_index(if fix_ready { 0 } else { -1 })
            .flex().items_center().gap(px(4.0))
            .aria_label(fix_label.clone()).rounded(px(5.0)).px(px(6.0)).py(px(3.0))
            .text_color(if fixing { theme.accent } else { theme.text_muted })
            .when(!fix_ready, |el| el.opacity(0.45))
            .focus_visible(move |el| el.border_1().border_color(theme.accent))
            .tooltip(crate::settings::widgets::text_tooltip("Allow up to three CI repairs in this chat, including commits and normal pushes. Never force-push or merge. Restart grants a fresh budget after reviewing prior repairs."))
            .when(fix_ready, |el| el.cursor_pointer().hover(|el| el.bg(theme.element_hover)).on_click(cx.listener(move |control, _, _, cx| {
                cx.stop_propagation(); control.set(true, !fixing, exhausted && !fixing, cx);
            })).on_key_down(cx.listener(move |_, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") { cx.stop_propagation(); }
            })))
            .child(crate::icons::icon(crate::icons::PR_REPAIR).size(px(12.0)))
            .child(SharedString::from(fix_label));
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
                    .gap(px(4.0))
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
