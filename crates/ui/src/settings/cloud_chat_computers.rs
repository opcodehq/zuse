//! Settings → Cloud computer → Your computers: whether cloud chats may run
//! commands on this computer, and the account's enrolled computers. Names
//! come only from each computer's enrollment, and every command asks first.
use crate::composer::computers::{BridgeStatus, Computer};
use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;
use crate::tooltip::TooltipExt as _;
use gpui::{
    AnyElement, Context, Entity, Focusable as _, Render, SharedString, Subscription, Task, Window,
    div, prelude::*, px,
};
use std::time::Duration;
use zuse_viewport::cloud_settings::kit;
use zuse_viewport::settings_style as s;

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
    /// The computer whose "…" menu is open.
    menu: Option<String>,
    /// Background refreshes, only while Settings shows this page.
    poll: Option<Task<()>>,
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
            menu: None,
            _observe: observe,
            poll: None,
        }
    }

    /// Refresh while the page is on screen and stop when it isn't. An action
    /// the owner already started still finishes.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if visible == self.poll.is_some() {
            return;
        }
        if !visible {
            self.poll = None;
            self.refresh = None;
            self.refreshing = false;
            return;
        }
        // The list less often than this computer's status.
        self.poll = Some(cx.spawn(async move |this, cx| {
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
        }));
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

    /// The trailing "…" button of a row and, while open, its menu.
    fn row_menu(
        &self,
        device: &str,
        name: &str,
        always_allowed: bool,
        local: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.menu.as_deref() == Some(device);
        let toggle = device.to_owned();
        let mut trigger = div().relative().child(
            kit::icon_button(
                SharedString::from(format!("bridge-more-{device}")),
                "more-horizontal",
                "More actions",
                self.busy,
            )
            .on_click(cx.listener(move |page, _, _, cx| {
                page.menu = if page.menu.as_deref() == Some(toggle.as_str()) {
                    None
                } else {
                    Some(toggle.clone())
                };
                cx.notify();
            })),
        );
        if !open {
            return trigger.into_any_element();
        }
        let rename = (device.to_owned(), name.to_owned());
        let mut menu = kit::menu_surface()
            .id(SharedString::from(format!("bridge-menu-{device}")))
            .role(gpui::Role::Menu)
            .on_mouse_down_out(cx.listener(|page, _, _, cx| {
                page.menu = None;
                cx.notify();
            }))
            .child(
                kit::menu_item(
                    SharedString::from(format!("bridge-rename-{device}")),
                    "pen",
                    "Rename",
                    false,
                )
                .on_click(cx.listener(move |page, _, window, cx| {
                    page.menu = None;
                    page.open_rename(rename.0.clone(), rename.1.clone(), window, cx);
                })),
            );
        if always_allowed {
            let revoke = device.to_owned();
            menu = menu.child(
                kit::menu_item(
                    SharedString::from(format!("bridge-revoke-{device}")),
                    "restart",
                    "Stop running without asking",
                    false,
                )
                .on_click(cx.listener(move |page, _, _, cx| {
                    page.menu = None;
                    page.bridge_action("revoke", Some(revoke.clone()), true, cx);
                })),
            );
        }
        if !local {
            let forget = device.to_owned();
            menu = menu.child(
                kit::menu_item(
                    SharedString::from(format!("bridge-forget-{device}")),
                    "trash-bin-minimalistic",
                    "Remove",
                    true,
                )
                .on_click(cx.listener(move |page, _, _, cx| {
                    page.menu = None;
                    page.bridge_action("forget", Some(forget.clone()), true, cx);
                })),
            );
        }
        trigger = trigger.child(
            div()
                .absolute()
                .right(px(0.))
                .top(gpui::relative(1.))
                .child(gpui::deferred(
                    gpui::anchored()
                        .anchor(gpui::Anchor::TopRight)
                        .offset(gpui::point(px(0.), px(4.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(menu),
                )),
        );
        trigger.into_any_element()
    }

    /// A row's title, or its name field while it is being renamed.
    fn row_title(&self, device: &str, name: &str, theme: &Theme) -> (AnyElement, bool) {
        match self.rename.as_ref().filter(|(id, _, _)| id == device) {
            Some((_, input, _)) => (
                div()
                    .h(px(28.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(theme.accent.opacity(0.6))
                    .text_size(px(s::TITLE_SIZE))
                    .child(input.clone())
                    .into_any_element(),
                true,
            ),
            None => (
                s::title_text(name.to_owned()).truncate().into_any_element(),
                false,
            ),
        }
    }

    /// Save and Cancel for the name field.
    fn rename_actions(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                widgets::text_action(theme, widgets::ActionTone::Quiet, "Cancel")
                    .id("bridge-rename-cancel")
                    .tab_index(0)
                    .role(gpui::Role::Button)
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.rename = None;
                        cx.notify();
                    })),
            )
            .child(
                widgets::text_action(theme, widgets::ActionTone::Filled, "Save")
                    .id("bridge-rename-save")
                    .tab_index(0)
                    .role(gpui::Role::Button)
                    .when(self.busy, |el| el.opacity(0.5))
                    .on_click(cx.listener(|page, _, _, cx| page.submit_rename(cx))),
            )
            .into_any_element()
    }

    /// One row: platform tile, name over a muted line, then trailing controls.
    fn computer_row(
        first: bool,
        platform: &str,
        title: AnyElement,
        subtitle: AnyElement,
        trailing: Vec<AnyElement>,
    ) -> gpui::Div {
        s::row(first)
            .flex_nowrap()
            // The tile takes a bare glyph name: a laptop for macOS, a desktop otherwise.
            .child(kit::icon_tile(
                if platform == "macos" {
                    "laptop"
                } else {
                    "monitor"
                },
                28.,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(title)
                    .child(subtitle),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .children(trailing),
            )
    }

    /// "This computer": the switch that lets cloud chats use it, and its state.
    fn this_computer(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let status = self.status.clone().unwrap_or_default();
        let on = !matches!(status.state.as_str(), "" | "off");
        let (label, online) = match status.state.as_str() {
            "online" if status.running > 0 => {
                (format!("Online · {} running", status.running), true)
            }
            "online" => ("Online".to_owned(), true),
            "starting" => ("Connecting…".to_owned(), false),
            "reconnecting" => ("Reconnecting…".to_owned(), false),
            "paused" => (
                "Paused while another account is signed in".to_owned(),
                false,
            ),
            _ if self.busy => ("Turning on…".to_owned(), false),
            _ => (
                "Off · cloud chats can't use this computer".to_owned(),
                false,
            ),
        };
        let mut line: Vec<AnyElement> = vec![div().child(label).into_any_element()];
        if let Some(error) = status.error.clone().filter(|_| on && !online) {
            line.push(
                div()
                    .id("bridge-error")
                    .text_color(theme.warning)
                    .child("Couldn't connect")
                    .tip(error)
                    .into_any_element(),
            );
        }
        if self.stale {
            line.push(
                div()
                    .text_color(theme.warning)
                    .child("Couldn't refresh")
                    .into_any_element(),
            );
        }
        let local = status.device_id.clone();
        let entry = local
            .as_ref()
            .and_then(|id| self.computers.iter().find(|c| &c.device_id == id));
        let name = entry
            .map(|c| c.name.clone())
            .or_else(|| status.name.clone())
            .unwrap_or_else(|| "This computer".to_owned());
        let platform = entry
            .map(|c| c.platform.clone())
            .unwrap_or_else(|| std::env::consts::OS.to_owned());
        if entry.is_some_and(|c| c.always_allowed) {
            line.push(div().child("Runs without asking").into_any_element());
        }
        let busy = self.busy;
        let switch = s::toggle("bridge-switch", on, "Use this computer from cloud chats")
            .tab_index(0)
            .when(busy, |el| el.opacity(0.5))
            .on_click(cx.listener(move |page, _, _, cx| {
                if !page.busy {
                    page.bridge_action(if on { "stop" } else { "start" }, None, true, cx)
                }
            }))
            .into_any_element();
        let (title, editing) = match &local {
            Some(id) => self.row_title(id, &name, theme),
            None => (
                s::title_text(name.clone()).truncate().into_any_element(),
                false,
            ),
        };
        let mut trailing = Vec::new();
        if editing {
            trailing.push(self.rename_actions(theme, cx));
        } else if let Some(id) = &local {
            let always = entry.is_some_and(|c| c.always_allowed);
            trailing.push(self.row_menu(id, &name, always, true, cx));
        }
        trailing.push(switch);
        kit::section(
            "This computer",
            "Cloud chats can run commands here while this is on. Each command asks you first in the chat, unless you allowed that chat or this computer.",
            None,
            Self::computer_row(
                true,
                &platform,
                title,
                meta(theme, on.then_some(online), line),
                trailing,
            ),
        )
    }

    /// The account's other enrolled computers.
    fn other_computers(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let local = self.status.as_ref().and_then(|s| s.device_id.clone());
        let mut others: Vec<Computer> = self
            .computers
            .iter()
            .filter(|c| Some(&c.device_id) != local.as_ref())
            .cloned()
            .collect();
        others.sort_by_key(|c| c.name.to_lowercase());
        let mut body = div().flex().flex_col();
        if others.is_empty() {
            body = body.child(s::row(true).child(s::description_text(
                "Turn this on in Zuse on your other computers to use them too.",
            )));
        }
        for (index, computer) in others.iter().enumerate() {
            let (title, editing) = self.row_title(&computer.device_id, &computer.name, theme);
            let mut line = vec![
                div()
                    .child(if computer.connected {
                        "Online"
                    } else {
                        "Offline"
                    })
                    .into_any_element(),
                div()
                    .child(platform_name(&computer.platform).to_owned())
                    .into_any_element(),
            ];
            if computer.always_allowed {
                line.push(div().child("Runs without asking").into_any_element());
            }
            let trailing = vec![if editing {
                self.rename_actions(theme, cx)
            } else {
                self.row_menu(
                    &computer.device_id,
                    &computer.name,
                    computer.always_allowed,
                    false,
                    cx,
                )
            }];
            body = body.child(Self::computer_row(
                index == 0,
                &computer.platform,
                title,
                meta(theme, Some(computer.connected), line),
                trailing,
            ));
        }
        kit::section(
            "Other computers",
            "Your other computers that allowed cloud chats. The agent uses one only when you ask for it by name.",
            None,
            body,
        )
    }
}

/// A small online/offline dot.
fn status_dot(theme: &Theme, online: bool) -> gpui::Div {
    div().flex_none().size(px(6.)).rounded_full().bg(if online {
        theme.success_muted
    } else {
        theme.text_muted.opacity(0.5)
    })
}

/// A row's muted second line: an optional status dot, then fragments split
/// by middle dots.
fn meta(theme: &Theme, dot: Option<bool>, fragments: Vec<AnyElement>) -> AnyElement {
    let mut line = s::description_text(SharedString::default())
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(6.))
        .when_some(dot, |line, online| line.child(status_dot(theme, online)));
    for (index, fragment) in fragments.into_iter().enumerate() {
        if index > 0 {
            line = line.child(div().child("·"));
        }
        line = line.child(fragment);
    }
    line.into_any_element()
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
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(kit::SECTION_GAP))
            .child(self.this_computer(&theme, cx))
            .child(self.other_computers(&theme, cx))
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
        let page = cx.new(|cx| {
            let mut page = CloudChatComputers::new(state, cx);
            page.set_visible(true, cx);
            page
        });
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
        let page = cx.new(|cx| {
            let mut page = CloudChatComputers::new(state, cx);
            page.set_visible(true, cx);
            page
        });
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

    /// Refreshes stop when the page leaves the screen; an action already
    /// started still finishes.
    #[gpui::test]
    fn refreshes_run_only_while_the_page_is_shown(cx: &mut gpui::TestAppContext) {
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
        assert!(requests.try_recv().is_err(), "nothing reads while hidden");
        page.update(cx, |page, cx| page.set_visible(true, cx));
        settle(cx);
        assert!(requests.try_recv().is_ok(), "shown: it reads");
        page.update(cx, |page, cx| {
            page.bridge_action("stop", None, false, cx);
            page.set_visible(false, cx);
            assert!(page.poll.is_none() && page.refresh.is_none());
            assert!(page.task.is_some() && page.busy, "the started action stays");
        });
    }
}
