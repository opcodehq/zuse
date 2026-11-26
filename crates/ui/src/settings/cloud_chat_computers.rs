//! Settings → Cloud computer → Your computers: whether cloud chats may run
//! commands on this computer, and the account's enrolled computers. Names
//! come only from each computer's enrollment, and every command asks first.
use crate::composer::computers::{BridgeStatus, Computer};
use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::popover;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;
use crate::tooltip::TooltipExt as _;
use gpui::{
    AnyElement, Context, Entity, Focusable as _, Render, SharedString, Subscription, Task, Window,
    div, prelude::*, px,
};
use std::time::Duration;

pub struct CloudChatComputers {
    state: Entity<AppState>,
    /// The account's enrolled computers, for the account that loaded them.
    computers: Vec<Computer>,
    owner: Option<String>,
    /// This computer's connector.
    status: Option<BridgeStatus>,
    /// A connector or catalog action is in flight; others wait for it.
    busy: bool,
    error: Option<SharedString>,
    /// The last status read failed; the last known status stays shown.
    stale: bool,
    task: Option<Task<()>>,
    /// A background read in flight; the next tick waits for it.
    refresh: Option<Task<()>>,
    refreshing: bool,
    /// The computer being renamed and its name field.
    rename: Option<(String, Entity<ComposerInput>, Subscription)>,
    /// The new name the next rename action sends.
    new_name: Option<String>,
    _poll: Task<()>,
    _observe: Subscription,
}

impl CloudChatComputers {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            computers: Vec::new(),
            owner: None,
            status: None,
            busy: false,
            error: None,
            stale: false,
            task: None,
            refresh: None,
            refreshing: false,
            rename: None,
            new_name: None,
            _observe: observe,
            // Refreshed while Settings shows it; the list less often.
            _poll: cx.spawn(async move |this, cx| {
                let mut tick = 0u32;
                loop {
                    let alive = this
                        .update(cx, |page, cx| page.poll_bridge(tick.is_multiple_of(5), cx))
                        .is_ok();
                    if !alive {
                        return;
                    }
                    tick = tick.wrapping_add(1);
                    cx.background_executor().timer(Duration::from_secs(3)).await;
                }
            }),
        }
    }

    /// This computer's connector, and every few rounds the account's computers.
    fn poll_bridge(&mut self, computers: bool, cx: &mut Context<Self>) {
        // A read never replaces another read or a user action.
        if self.busy || self.refreshing {
            return;
        }
        self.bridge_action("refresh", None, computers, cx);
    }

    /// Run one connector or catalog action, then read the status and list again.
    fn bridge_action(
        &mut self,
        action: &str,
        id: Option<String>,
        computers: bool,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.read(cx);
        let Some(engine) = state.engine().map(|e| e.controller()) else {
            return;
        };
        let account = state.cloud_user_id().map(str::to_owned);
        let action = action.to_owned();
        let user_action = action != "refresh";
        let name = self.new_name.take();
        if self.busy {
            // One action at a time, so none of their outcomes is lost.
            return;
        }
        if user_action {
            self.busy = true;
            // A status read started earlier must not land over this action's result.
            self.refresh = None;
            self.refreshing = false;
            cx.notify();
        }
        let task = cx.spawn(async move |this, cx| {
            let client = engine.client();
            let acted = match action.as_str() {
                "start" => client
                    // The engine names it after the computer; it can be renamed here.
                    .call("StartComputerBridge", serde_json::json!({}))
                    .await
                    .map(drop),
                "stop" => client
                    .call("StopComputerBridge", serde_json::json!({}))
                    .await
                    .map(drop),
                "refresh" => Ok(()),
                _ => client
                    .call(
                        "CloudComputerControl",
                        serde_json::json!({"expectedAccountId":account,"action":action,
                            "deviceId":id,"name":name}),
                    )
                    .await
                    .map(drop),
            };
            // `None` when the read failed: keep showing the last known state.
            let status = client
                .call("ComputerBridgeStatus", serde_json::json!({}))
                .await
                .ok()
                .and_then(|value| serde_json::from_value::<BridgeStatus>(value).ok());
            let list = match &account {
                Some(account) if computers || user_action => Some(
                    client
                        .call(
                            "CloudComputerControl",
                            serde_json::json!({"expectedAccountId":account,"action":"list"}),
                        )
                        .await,
                ),
                _ => None,
            };
            this.update(cx, |page, cx| {
                if user_action {
                    page.busy = false;
                } else {
                    page.refreshing = false;
                }
                if let Err(error) = acted {
                    page.error = Some(error.to_string().into());
                }
                page.stale = status.is_none();
                if let Some(status) = status {
                    page.status = Some(status);
                }
                if page.state.read(cx).cloud_user_id() != account.as_deref() {
                    page.computers.clear();
                    page.owner = None;
                } else if let Some(list) = list {
                    match list {
                        Ok(rows) => {
                            page.computers = serde_json::from_value(rows).unwrap_or_default();
                            page.owner = account;
                        }
                        Err(error) if user_action => {
                            page.error = Some(error.to_string().into());
                        }
                        Err(_) => {}
                    }
                }
                cx.notify();
            })
            .ok();
        });
        if user_action {
            self.task = Some(task);
        } else {
            self.refreshing = true;
            self.refresh = Some(task);
        }
    }

    fn open_rename(
        &mut self,
        device: String,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| ComposerInput::new("Computer name", cx));
        input.update(cx, |input, cx| input.set_text(current, cx));
        window.focus(&input.focus_handle(cx), cx);
        let events = cx.subscribe(&input, |this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.submit_rename(cx);
            }
        });
        self.rename = Some((device, input, events));
        cx.notify();
    }

    /// Save the new name: shown at once, confirmed by the next list read.
    fn submit_rename(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some((device, input, _)) = self.rename.take() else {
            return;
        };
        let name = input.read(cx).text().trim().to_owned();
        if name.is_empty() || name.chars().count() > 128 {
            cx.notify();
            return;
        }
        if let Some(computer) = self.computers.iter_mut().find(|c| c.device_id == device) {
            computer.name = name.clone();
        }
        self.new_name = Some(name);
        self.bridge_action("rename", Some(device), true, cx);
    }

    fn render_card(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let status = self.status.clone().unwrap_or_default();
        let on = !matches!(status.state.as_str(), "" | "off");
        let (label, tone) = match status.state.as_str() {
            "online" if status.running > 0 => (
                format!("Online · {} running", status.running),
                theme.success_muted,
            ),
            "online" => ("Online".to_owned(), theme.success_muted),
            "starting" => ("Connecting…".to_owned(), theme.text_muted),
            "reconnecting" => ("Reconnecting…".to_owned(), theme.warning),
            "paused" => ("Paused".to_owned(), theme.warning),
            _ => ("Off".to_owned(), theme.text_muted),
        };
        let mut meta = vec![div().text_color(tone).child(label).into_any_element()];
        if let Some(error) = status.error.clone().filter(|_| on) {
            meta.push(div().child(error).into_any_element());
        }
        if self.stale {
            meta.push(
                div()
                    .text_color(theme.warning)
                    .child("Couldn't refresh")
                    .into_any_element(),
            );
        }
        let busy = self.busy;
        let action = if on {
            widgets::text_action(theme, widgets::ActionTone::Quiet, "Stop")
                .id("bridge-stop")
                .when(busy, |el| el.opacity(0.5))
                .role(gpui::Role::Button)
                .tab_index(0)
                .on_click(cx.listener(|page, _, _, cx| page.bridge_action("stop", None, true, cx)))
                .into_any_element()
        } else {
            popover::btn_primary(theme, if busy { "Allowing…" } else { "Allow" })
                .id("bridge-start")
                .role(gpui::Role::Button)
                .tab_index(0)
                .text_size(crate::typography::text_xs())
                .on_click(cx.listener(move |page, _, _, cx| {
                    if !page.busy {
                        page.bridge_action("start", None, true, cx)
                    }
                }))
                .into_any_element()
        };
        let mut card = widgets::section_card(theme).mt_0().child(
            widgets::card_row(theme, true)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(160.0))
                        .child(widgets::row_title(
                            theme,
                            "Use this computer from cloud chats",
                        ))
                        .child(widgets::meta_line(theme, meta)),
                )
                .child(
                    div()
                        .id("bridge-help")
                        .flex_none()
                        .tip("Your cloud chats can run commands here. Each one asks you first.")
                        .child(action),
                ),
        );
        let local = status.device_id.clone();
        let mut computers = self.computers.clone();
        computers.sort_by_key(|c| (Some(&c.device_id) != local.as_ref(), c.name.to_lowercase()));
        for (index, computer) in computers.into_iter().enumerate() {
            let this_computer = Some(&computer.device_id) == local.as_ref();
            let mut meta: Vec<AnyElement> = Vec::new();
            if this_computer {
                meta.push(div().child("This computer").into_any_element());
            }
            meta.push(
                div()
                    .child(platform_name(&computer.platform).to_owned())
                    .into_any_element(),
            );
            meta.push(if computer.connected {
                div()
                    .text_color(theme.success_muted)
                    .child("Online")
                    .into_any_element()
            } else {
                div().child("Offline").into_any_element()
            });
            if computer.always_allowed {
                meta.push(div().child("Always allowed").into_any_element());
            }
            let revoke = computer.device_id.clone();
            let forget = computer.device_id.clone();
            let rename = (computer.device_id.clone(), computer.name.clone());
            let editing = self
                .rename
                .as_ref()
                .filter(|(device, _, _)| *device == computer.device_id)
                .map(|(_, input, _)| input.clone());
            let title = match &editing {
                Some(input) => div()
                    .h(px(28.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(theme.accent.opacity(0.6))
                    .bg(theme.muted_fill().opacity(0.4))
                    .text_size(crate::typography::text_sm())
                    .child(input.clone())
                    .into_any_element(),
                None => widgets::row_title(theme, computer.name.clone()).into_any_element(),
            };
            card = card.child(
                widgets::card_row(theme, false)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .child(title)
                            .child(widgets::meta_line(theme, meta)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .when(editing.is_some(), |row| {
                                row.child(
                                    widgets::text_action(
                                        theme,
                                        widgets::ActionTone::Quiet,
                                        "Cancel",
                                    )
                                    .id(("bridge-rename-cancel", index))
                                    .role(gpui::Role::Button)
                                    .on_click(cx.listener(
                                        |page, _, _, cx| {
                                            page.rename = None;
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    widgets::text_action(
                                        theme,
                                        widgets::ActionTone::Filled,
                                        "Save",
                                    )
                                    .id(("bridge-rename-save", index))
                                    .role(gpui::Role::Button)
                                    .when(busy, |el| el.opacity(0.5))
                                    .on_click(cx.listener(|page, _, _, cx| page.submit_rename(cx))),
                                )
                            })
                            .when(editing.is_none(), |row| {
                                row.child(
                                    widgets::text_action(
                                        theme,
                                        widgets::ActionTone::Quiet,
                                        "Rename",
                                    )
                                    .id(("bridge-rename", index))
                                    .role(gpui::Role::Button)
                                    .on_click(cx.listener(
                                        move |page, _, window, cx| {
                                            page.open_rename(
                                                rename.0.clone(),
                                                rename.1.clone(),
                                                window,
                                                cx,
                                            )
                                        },
                                    )),
                                )
                            })
                            .when(computer.always_allowed, |row| {
                                row.child(
                                    widgets::text_action(
                                        theme,
                                        widgets::ActionTone::Quiet,
                                        "Revoke always",
                                    )
                                    .id(("bridge-revoke", index))
                                    .when(busy, |el| el.opacity(0.5))
                                    .role(gpui::Role::Button)
                                    .on_click(cx.listener(
                                        move |page, _, _, cx| {
                                            page.bridge_action(
                                                "revoke",
                                                Some(revoke.clone()),
                                                true,
                                                cx,
                                            )
                                        },
                                    )),
                                )
                            })
                            .child(
                                widgets::text_action(theme, widgets::ActionTone::Quiet, "Remove")
                                    .id(("bridge-forget", index))
                                    .when(busy, |el| el.opacity(0.5))
                                    .role(gpui::Role::Button)
                                    .on_click(cx.listener(move |page, _, _, cx| {
                                        page.bridge_action("forget", Some(forget.clone()), true, cx)
                                    })),
                            ),
                    ),
            );
        }
        card.into_any_element()
    }
}

fn platform_name(platform: &str) -> &str {
    match platform {
        "macos" => "macOS",
        "linux" => "Linux",
        "windows" => "Windows",
        other => other,
    }
}

impl Render for CloudChatComputers {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::toast::take_error(&mut self.error, cx);
        let theme = Theme::of(cx).for_settings_surface();
        if self.owner.as_deref() != self.state.read(cx).cloud_user_id() {
            self.computers.clear();
            self.owner = None;
        }
        div().w_full().child(self.render_card(&theme, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Background reads never stack up or replace one in flight, and a
    /// failed status read keeps the last known state.
    #[gpui::test]
    fn polls_wait_for_the_read_in_flight_and_keep_the_last_status(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let (send, mut requests) = tokio::sync::mpsc::channel::<String>(64);
        let (replies, receive) = tokio::sync::mpsc::channel::<String>(64);
        let handle =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, receive));
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.set_test_engine(handle);
            state
        });
        let page = cx.new(|cx| CloudChatComputers::new(state, cx));
        let settle = |cx: &mut gpui::TestAppContext| {
            for _ in 0..6 {
                cx.run_until_parked();
                runtime.block_on(async { tokio::task::yield_now().await });
            }
        };
        let drain = |requests: &mut tokio::sync::mpsc::Receiver<String>| {
            let mut seen = Vec::new();
            while let Ok(line) = requests.try_recv() {
                seen.push(serde_json::from_str::<serde_json::Value>(line.trim()).unwrap());
            }
            seen
        };
        settle(cx);
        let first = drain(&mut requests);
        assert_eq!(first.len(), 1, "one read in flight: {first:?}");
        // Further ticks while it is unanswered start nothing new.
        page.update(cx, |page, cx| page.poll_bridge(true, cx));
        page.update(cx, |page, cx| page.poll_bridge(true, cx));
        settle(cx);
        assert!(drain(&mut requests).is_empty());
        runtime
            .block_on(replies.send(format!(
                "{}\n",
                serde_json::json!({"id":first[0]["id"],"ok":{"state":"online","deviceId":"mac","running":0}})
            )))
            .unwrap();
        settle(cx);
        page.read_with(cx, |page, _| {
            assert_eq!(
                page.status.as_ref().map(|s| s.state.as_str()),
                Some("online")
            );
            assert!(!page.refreshing);
        });
        // The next read fails: Stop must stay available.
        page.update(cx, |page, cx| page.poll_bridge(false, cx));
        settle(cx);
        let next = drain(&mut requests);
        assert_eq!(next.len(), 1);
        runtime
            .block_on(replies.send(format!(
                "{}\n",
                serde_json::json!({"id":next[0]["id"],"err":"engine busy"})
            )))
            .unwrap();
        settle(cx);
        page.read_with(cx, |page, _| {
            assert_eq!(
                page.status.as_ref().map(|s| s.state.as_str()),
                Some("online")
            );
            assert!(page.stale);
        });
    }

    /// Renaming shows the new name at once and sends it for that computer.
    #[gpui::test]
    fn rename_sends_the_new_name_for_that_computer(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let (send, mut requests) = tokio::sync::mpsc::channel::<String>(64);
        let (_replies, receive) = tokio::sync::mpsc::channel::<String>(64);
        let handle =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, receive));
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.apply_cloud_account(serde_json::json!({"user":{"id":"owner"},"orgId":null}));
            state.set_test_engine(handle);
            state
        });
        let page = cx.new(|cx| CloudChatComputers::new(state, cx));
        let settle = |cx: &mut gpui::TestAppContext| {
            for _ in 0..6 {
                cx.run_until_parked();
                runtime.block_on(async { tokio::task::yield_now().await });
            }
        };
        settle(cx);
        while requests.try_recv().is_ok() {}
        page.update(cx, |page, cx| {
            // The background read is still unanswered; a rename is not blocked by it.
            page.refresh = None;
            page.refreshing = false;
            page.computers = vec![Computer {
                device_id: "mac".into(),
                name: "MacBook".into(),
                platform: "macos".into(),
                ..Default::default()
            }];
            let input = cx.new(|cx| ComposerInput::new("Computer name", cx));
            input.update(cx, |input, cx| input.set_text("Studio Mac", cx));
            let events = cx.subscribe(
                &input,
                |_: &mut CloudChatComputers, _, _: &ComposerInputEvent, _| {},
            );
            page.rename = Some(("mac".into(), input, events));
            page.submit_rename(cx);
            assert_eq!(page.computers[0].name, "Studio Mac");
            assert!(page.rename.is_none());
        });
        settle(cx);
        let mut sent = Vec::new();
        while let Ok(line) = requests.try_recv() {
            sent.push(serde_json::from_str::<serde_json::Value>(line.trim()).unwrap());
        }
        let rename = sent
            .iter()
            .find(|r| r["params"]["action"] == "rename")
            .expect("rename sent");
        assert_eq!(rename["method"], "CloudComputerControl");
        assert_eq!(rename["params"]["deviceId"], "mac");
        assert_eq!(rename["params"]["name"], "Studio Mac");
        assert_eq!(rename["params"]["expectedAccountId"], "owner");
    }
}
