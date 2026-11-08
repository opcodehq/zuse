//! Compact chat/PR permission controls, backed by the owning engine's watch.
use crate::{state::AppState, theme::Theme};
use gpui::{Context, Entity, SharedString, Subscription, Task, Window, div, prelude::*, px};
use std::time::Duration;
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
    engine: Option<crate::state::EngineHandle>,
    _state: Subscription,
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
        let observe = cx.observe(&state, |control, _, cx| {
            let engine = control.state.read(cx).engine().cloned();
            let same = match (&control.engine, &engine) {
                (Some(a), Some(b)) => a.same_connection(b),
                (None, None) => true,
                _ => false,
            };
            if !same {
                control._watch = None;
                control.action = None;
                control.connected = false;
                control.busy = false;
                control.status = None;
                control.error = None;
                control.engine = engine;
                control.connect(cx);
                cx.notify();
            }
        });
        let engine = state.read(cx).engine().cloned();
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
            engine,
            _state: observe,
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
                let observed = crate::pull_request_detail::read_request(&engine, methods::OBSERVE_PR_WATCH, params, cx.background_executor().clone()).await;
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
            let call = engine.client().call(methods::SET_PR_WATCH, params);
            let deadline = cx.background_executor().timer(Duration::from_secs(45));
            futures::pin_mut!(call, deadline);
            let result = match futures::future::select(call, deadline).await {
                futures::future::Either::Left((result, _)) => result,
                futures::future::Either::Right(_) => Err(zuse_rpc::RpcError::Failed("Couldn't confirm the automation setting. Reconnect to check its current state before trying again.".into())),
            };
            let _ = this.update(cx, |control, cx| {
                control.busy = false;
                match result {
                    Ok(value) => match serde_json::from_value::<PrWatchStatus>(value) {
                        Ok(status) => {
                            control.status = Some(status);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request_test_support::{self as fixture, ScriptedRpc};
    use futures::StreamExt;

    #[gpui::test]
    fn pull_request_watch_late_connection_and_setting_receipt_update_the_controls(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = fixture::runtime();
        let _runtime = runtime.enter();
        fixture::init(cx);
        let status = PrWatchStatus {
            chat_id: "chat".into(),
            url: "https://github.com/acme/app/pull/1".into(),
            branch: "feature".into(),
            enabled: true,
            auto_fix: false,
            attempts: 1,
            max_attempts: 3,
            ci: zuse_protocol::change_request_assessment::CiState::Passed,
            failure_sequence: 0,
            error: None,
            repairing: false,
            can_auto_fix: true,
        };
        let rpc = ScriptedRpc::new({
            let status = status.clone();
            move |method, _| {
                let mut status = status.clone();
                async move {
                    match method.as_str() {
                        methods::OBSERVE_PR_WATCH => zuse_rpc::RpcReply::value(&status),
                        methods::WATCH_PR_WATCHES => Ok(zuse_rpc::RpcReply::Stream(
                            futures::stream::once(async move {
                                serde_json::to_value(vec![status]).unwrap()
                            })
                            .chain(futures::stream::pending())
                            .boxed(),
                        )),
                        methods::SET_PR_WATCH => {
                            status.enabled = false;
                            zuse_rpc::RpcReply::value(&status)
                        }
                        _ => Err(zuse_rpc::RpcError::UnknownMethod(method)),
                    }
                }
            }
        });
        let (state, control) = cx.update(|cx| {
            let state = fixture::state(cx, None);
            let control = cx.new(|cx| {
                PrWatchControl::new(
                    state.clone(),
                    "chat".into(),
                    status.url.clone(),
                    None,
                    false,
                    cx,
                )
            });
            (state, control)
        });
        assert!(!control.read_with(cx, |control, _| control.connected));
        state.update(cx, |state, cx| {
            state.set_test_engine(crate::state::EngineHandle::from_test_client(rpc.client()));
            cx.notify();
        });
        rpc.settle(cx, &runtime, |cx| {
            control.read_with(cx, |control, _| control.connected)
        });
        assert!(control.read_with(cx, |control, _| control.status.as_ref().unwrap().enabled));
        control.update(cx, |control, cx| control.set(false, false, false, cx));
        rpc.settle(cx, &runtime, |cx| {
            control.read_with(cx, |control, _| !control.busy)
        });
        assert!(!control.read_with(cx, |control, _| control.status.as_ref().unwrap().enabled));
    }
}

impl Render for PrWatchControl {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let status = self.status.as_ref();
        let watching = status.is_some_and(|s| s.enabled);
        let fixing = status.is_some_and(|s| s.auto_fix);
        let exhausted = status.map_or(0, |s| s.attempts) >= MAX_REPAIR_ATTEMPTS;
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
        let fix_ready = ready && ((writable && status.is_some_and(|s| s.can_auto_fix)) || fixing);
        let toggle = div()
            .id("pr-auto-fix-toggle")
            .role(gpui::Role::Button)
            .tab_index(if fix_ready { 0 } else { -1 })
            .aria_label(format!(
                "Auto-fix CI: {}",
                if fixing { "on" } else { "off" }
            ))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(px(5.0))
            .when(!fix_ready, |el| el.opacity(0.45))
            .focus_visible(move |el| el.border_1().border_color(theme.accent))
            .tooltip(crate::settings::widgets::text_tooltip(
                if exhausted && !fixing {
                    "Repair limit reached. Turn on to start over."
                } else {
                    "Lets the agent commit and push up to 3 CI fixes. Never force-pushes or merges."
                },
            ))
            .when(fix_ready, |el| {
                el.cursor_pointer()
                    .on_click(cx.listener(move |control, _, _, cx| {
                        cx.stop_propagation();
                        control.set(!fixing, !fixing, exhausted && !fixing, cx);
                    }))
                    .on_key_down(cx.listener(move |_, event: &gpui::KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            // GPUI activates the focused control on key-up.
                            cx.stop_propagation();
                        }
                    }))
            })
            .when(self.compact, |el| el.child("Auto-fix CI"))
            .child(crate::settings::widgets::toggle_switch(
                &theme,
                fixing,
                format!("pr-auto-fix-{}", cx.entity_id()),
            ));
        div()
            .id("pr-watch-controls")
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(8.0))
            .text_size(crate::typography::text_xs())
            .text_color(theme.text_muted)
            .child(toggle)
            // A watch started without auto-fix has no switch of its own.
            .when(watching && !fixing && ready, |el| {
                el.child(
                    div()
                        .id("pr-watch-stop")
                        .role(gpui::Role::Button)
                        .tab_index(0)
                        .cursor_pointer()
                        .hover(|style| style.text_color(theme.text))
                        .child("Stop watching")
                        .on_click(cx.listener(|control, _, _, cx| {
                            cx.stop_propagation();
                            control.set(false, false, false, cx);
                        })),
                )
            })
            .when_some(error.filter(|_| !self.compact), |el, error| {
                el.child(
                    div()
                        .min_w_0()
                        .text_color(theme.danger)
                        .child(SharedString::from(error)),
                )
            })
    }
}
