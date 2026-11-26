//! Settings: whether cloud chats may run commands on this computer, and the
//! account's enrolled computers. Separate from remote access and pairing:
//! a cloud chat reaches this computer only through its own enrolled
//! connector, and every command asks first.
use crate::composer::computers::{BridgeStatus, Computer};
use crate::popover;
use crate::settings::devices::platform_label;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;
use crate::tooltip::TooltipExt as _;
use gpui::{
    AnyElement, Context, Entity, Render, SharedString, Subscription, Task, Window, div, prelude::*,
    px,
};
use std::time::Duration;

pub struct CloudChatComputers {
    state: Entity<AppState>,
    /// The account's enrolled computers, for the account that loaded them.
    computers: Vec<Computer>,
    owner: Option<String>,
    /// This computer's connector.
    status: Option<BridgeStatus>,
    busy: bool,
    error: Option<SharedString>,
    task: Option<Task<()>>,
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
            task: None,
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
        if self.busy {
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
        let label = state
            .devices
            .iter()
            .find(|d| Some(d.id.as_str()) == state.local_device_id.as_deref())
            .map(|d| d.name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "This computer".into());
        let action = action.to_owned();
        let user_action = action != "refresh";
        if user_action {
            self.busy = true;
            cx.notify();
        }
        self.task = Some(cx.spawn(async move |this, cx| {
            let client = engine.client();
            let acted = match action.as_str() {
                "start" => client
                    .call("StartComputerBridge", serde_json::json!({ "label": label }))
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
                        serde_json::json!({"expectedAccountId":account,"action":action,"deviceId":id}),
                    )
                    .await
                    .map(drop),
            };
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
                page.busy = false;
                if let Err(error) = acted {
                    page.error = Some(error.to_string().into());
                }
                page.status = status;
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
        }));
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
        let busy = self.busy;
        let action = if on {
            widgets::text_action(theme, widgets::ActionTone::Quiet, "Stop")
                .id("bridge-stop")
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
                        .tip("Attach it in a cloud chat's composer. Every command asks you first.")
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
                    .child(platform_label(&computer.platform).to_string())
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
            card = card.child(
                widgets::card_row(theme, false)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .child(widgets::row_title(theme, computer.name.clone()))
                            .child(widgets::meta_line(theme, meta)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .when(computer.always_allowed, |row| {
                                row.child(
                                    widgets::text_action(
                                        theme,
                                        widgets::ActionTone::Quiet,
                                        "Revoke always",
                                    )
                                    .id(("bridge-revoke", index))
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
