//! Settings → Devices (feature-inventory §1.5): the device registry — name,
//! platform, last-seen, an Online/Offline badge, a "This device" badge, click-to-copy id,
//! and a Rename dialog (Mutate renameDevice).

use crate::localization::message as tr;
use chrono::{DateTime, Utc};
use gpui::{
    AnyElement, ClipboardItem, Context, Entity, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};
use std::time::Duration;
use zuse_i18n::messages as msg;

use zuse_protocol::WorkspaceScope;
use zuse_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::popover;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;
mod workspace_storage;
pub(crate) use super::managed_access::ComputerRemoved;
pub(crate) use workspace_storage::{StorageEvent, WorkspaceStorage};
impl gpui::EventEmitter<ComputerRemoved> for DevicesPage {}

/// A device that pinged within this window shows a presence dot (engines
/// heartbeat every 15s; 70s tolerates a couple of missed beats).
pub const DEVICE_ONLINE_WINDOW_SECS: i64 = 70;

/// Presence: last-seen within the online window (future timestamps count). Pure.
pub fn device_online(last_seen: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    last_seen
        .is_some_and(|at| now.signed_duration_since(at).num_seconds() <= DEVICE_ONLINE_WINDOW_SECS)
}

/// Compact last-seen line. Pure.
pub fn format_last_seen(last_seen: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(at) = last_seen else {
        return "never seen".to_string();
    };
    let secs = now.signed_duration_since(at).num_seconds();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// Scope-aware copy: a local registry describes only the active local
/// workspace and must not imply that account device metadata is already live.
pub fn devices_subtitle(scope: Option<WorkspaceScope>) -> &'static str {
    match scope {
        Some(WorkspaceScope::Local) => "Devices in this local workspace.",
        Some(WorkspaceScope::Synced) => "Devices synced to this workspace.",
        Some(WorkspaceScope::Development) | None => "Devices in this workspace.",
    }
}

struct RenameDialog {
    device_id: String,
    input: Entity<ComposerInput>,
    _events: Subscription,
}

pub struct DevicesPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    pairing: Entity<crate::settings::pairing::PairingCard>,
    managed: Entity<crate::settings::managed_access::ManagedAccessCard>,
    bridge: Vec<serde_json::Value>,
    bridge_owner: Option<String>,
    bridge_pending: Vec<serde_json::Value>,
    bridge_attached: Vec<String>,
    rename: Option<RenameDialog>,
    /// Device id whose id-chip shows "Copied" right now.
    copied: Option<String>,
    error: Option<SharedString>,
    task: Option<Task<()>>,
    copy_task: Option<Task<()>>,
    _observe: Subscription,
    _managed: Subscription,
    _managed_paint: Subscription,
    _pairing_paint: Subscription,
    workspace_storage: Option<WorkspaceStorage>,
    storage_focus: gpui::FocusHandle,
}

impl DevicesPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        let pairing = cx.new(|cx| crate::settings::pairing::PairingCard::new(state.clone(), cx));
        let managed =
            cx.new(|cx| crate::settings::managed_access::ManagedAccessCard::new(state.clone(), cx));
        cx.subscribe(&managed, |_: &mut Self, _, event: &ComputerRemoved, cx| {
            cx.emit(event.clone())
        })
        .detach();
        let managed_events = cx.subscribe(
            &managed,
            |this: &mut Self, _, _: &crate::settings::managed_access::AccessChanged, cx| {
                this.pairing.update(cx, |card, cx| card.refresh(cx));
            },
        );
        let managed_paint = cx.observe(&managed, |_, _, cx| cx.notify());
        let pairing_paint = cx.observe(&pairing, |_, _, cx| cx.notify());
        Self {
            _managed: managed_events,
            _managed_paint: managed_paint,
            _pairing_paint: pairing_paint,
            managed,
            pairing,
            state,
            scroll: widgets::PageScroll::default(),
            bridge: Vec::new(),
            bridge_owner: None,
            bridge_pending: Vec::new(),
            bridge_attached: Vec::new(),
            rename: None,
            copied: None,
            error: None,
            task: None,
            copy_task: None,
            _observe: observe,
            workspace_storage: None,
            storage_focus: cx.focus_handle().tab_stop(true),
        }
    }

    pub(crate) fn refresh_computers(&mut self, cx: &mut Context<Self>) {
        self.pairing.update(cx, |pairing, cx| pairing.refresh(cx));
        self.managed.update(cx, |managed, cx| managed.refresh(cx));
    }

    pub(crate) fn open_rename(
        &mut self,
        device_id: String,
        current: String,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| ComposerInput::new("Device name", cx));
        input.update(cx, |input, cx| input.set_text(current, cx));
        let events = cx.subscribe(&input, |this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.submit_rename(cx);
            }
        });
        self.rename = Some(RenameDialog {
            device_id,
            input,
            _events: events,
        });
        cx.notify();
    }

    /// Escape that reached Settings unclaimed closes the rename dialog first,
    /// so it never closes Settings under the dialog. Returns whether it did.
    pub(crate) fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self
            .pairing
            .update(cx, |card, cx| card.dismiss_on_escape(cx))
            || self
                .managed
                .update(cx, |card, cx| card.dismiss_on_escape(cx))
        {
            return true;
        }
        if self.rename.take().is_none() {
            return false;
        }
        cx.notify();
        true
    }

    fn submit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.rename.take() else {
            return;
        };
        let name = dialog.input.read(cx).text().trim().to_string();
        if name.is_empty() {
            cx.notify();
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = serde_json::json!({
            "op": "renameDevice",
            "deviceId": dialog.device_id,
            "name": name,
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::MUTATE, params).await;
            this.update(cx, |page, cx| {
                if let Err(err) = result {
                    page.error = Some(format!("Rename failed: {err}").into());
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn bridge_action(&mut self, action: &str, id: Option<String>, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let Some(engine) = state.engine().map(|e| e.controller()) else {
            return;
        };
        let Some(account) = state.cloud_user_id().map(str::to_owned) else {
            return;
        };
        let label = state
            .devices
            .iter()
            .find(|d| Some(d.id.as_str()) == state.local_device_id.as_deref())
            .map(|d| d.name.clone())
            .unwrap_or_else(|| "Computer".into());
        let action = action.to_owned();
        self.task=Some(cx.spawn(async move|this,cx|{
            let result=async{
                match action.as_str(){
                    "start"=>{engine.client().call("StartComputerBridge",serde_json::json!({"label":label})).await?;}
                    "stop"=>{engine.client().call("StopComputerBridge",serde_json::json!({})).await?;}
                    "list"=>{}
                    _=>{engine.client().call("CloudComputerControl",serde_json::json!({"expectedAccountId":account,"action":action,"deviceId":id})).await?;}
                }
                engine.client().call("CloudComputerControl",serde_json::json!({"expectedAccountId":account,"action":"list"})).await
            }.await;
            this.update(cx,|page,cx|{
                if page.state.read(cx).cloud_user_id()!=Some(account.as_str()){
                    page.bridge.clear();page.bridge_owner=None;cx.notify();return;
                }
                page.bridge_owner=Some(account);
                match result{
                    Ok(rows)=>page.bridge=serde_json::from_value(rows).unwrap_or_default(),
                    Err(e)=>{page.bridge.clear();page.error=Some(e.to_string().into());}
                }
                cx.notify();
            }).ok();
        }));
    }

    fn bridge_chat_action(
        &mut self,
        action: &str,
        id: Option<String>,
        decision: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.read(cx);
        let Some(target) = state.engine().and_then(|e| e.cloud_target()) else {
            return;
        };
        let Some(chat) = state.selected_chat.clone() else {
            return;
        };
        let action = action.to_owned();
        self.task=Some(cx.spawn(async move|this,cx|{
            let result=async{
                let mut params=serde_json::json!({"expectedAccountId":target.subject,"workspaceId":target.workspace,"chatId":chat,"action":action,"deviceId":id,"requestId":id,"decision":decision});
                if action!="refresh"{target.controller.client().call("CloudComputerControl",params.clone()).await?;}
                params["action"]=serde_json::json!("pending");
                let pending=target.controller.client().call("CloudComputerControl",params.clone()).await?;
                params["action"]=serde_json::json!("attached");
                let attached=target.controller.client().call("CloudComputerControl",params).await?;
                Ok::<_,zuse_rpc::RpcError>((pending,attached))
            }.await;
            this.update(cx,|page,cx|{
                let state=page.state.read(cx);
                if state.selected_chat.as_deref()!=Some(chat.as_str()) || state.engine().and_then(|e|e.cloud_workspace()).as_deref()!=Some(target.workspace.as_str()){
                    page.bridge_pending.clear();page.bridge_attached.clear();cx.notify();return;
                }
                match result{
                    Ok((pending,attached))=>{page.bridge_pending=serde_json::from_value(pending).unwrap_or_default();page.bridge_attached=serde_json::from_value(attached).unwrap_or_default();}
                    Err(e)=>{page.bridge_pending.clear();page.error=Some(e.to_string().into());}
                }
                cx.notify();
            }).ok();
        }));
    }

    fn copy_id(&mut self, device_id: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(device_id.clone()));
        self.copied = Some(device_id);
        self.copy_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            this.update(cx, |page, cx| {
                page.copied = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn render_rename_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let dialog = self.rename.as_ref()?;
        let input = dialog.input.clone();
        let card = popover::dialog_card(&theme)
            .id("rename-device-card")
            .role(gpui::Role::Dialog)
            .aria_label(tr(cx, msg::native::rename_device))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.rename = None;
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .child(popover::dialog_title(&theme, "Rename device"))
            .child(
                div()
                    .mt(px(12.0))
                    .child(popover::dialog_field(input.into_any_element())),
            )
            .child(
                popover::dialog_footer(&theme)
                    .child(
                        widgets::text_action(
                            &theme,
                            widgets::ActionTone::Quiet,
                            tr(cx, msg::settings::organizations_cancel),
                        )
                        .id("rename-cancel")
                        .tab_index(0)
                        .role(gpui::Role::Button)
                        .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.rename = None;
                            cx.notify();
                        })),
                    )
                    .child(
                        widgets::text_action(
                            &theme,
                            widgets::ActionTone::Solid,
                            tr(cx, msg::settings::model_connections_rename),
                        )
                        .id("rename-save")
                        .tab_index(0)
                        .role(gpui::Role::Button)
                        .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                        .on_click(cx.listener(|this, _, _, cx| this.submit_rename(cx))),
                    ),
            )
            .into_any_element();
        Some(popover::modal("rename-device-dialog", viewport, card))
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for DevicesPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

/// Human platform label (zuse settings.devices.tsx `platformLabel`).
pub fn platform_label(platform: &str) -> &str {
    match platform {
        "macos" | "darwin" => "macOS",
        "linux" => "Linux",
        "windows" => "Windows",
        "web" => "Web",
        "ios" => "iOS",
        "android" => "Android",
        other => other,
    }
}

/// Short device id for the click-to-copy chip (`abcd1234…wxyz`).
pub fn short_id(id: &str) -> String {
    if id.len() > 12 {
        format!("{}…{}", &id[..8], &id[id.len() - 4..])
    } else {
        id.to_string()
    }
}

impl Render for DevicesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::toast::take_error(&mut self.error, cx);
        let theme = Theme::of(cx).for_settings_surface();
        let now = Utc::now();
        let (devices, local_id, workspace_scope) = {
            let state = self.state.read(cx);
            (
                state.devices.clone(),
                state.local_device_id.clone(),
                state.workspace_scope,
            )
        };
        // Reaching this computer through the account is the relay under another name; the relay
        // row ("Coming soon") stands for both until it ships.
        let pairing = self.pairing.update(cx, |card, cx| card.parts(None, cx));
        let paired = pairing.device_rows.len();
        let computer_rows = self
            .managed
            .update(cx, |card, cx| card.computer_rows(paired, cx));
        let account_dialog = self.managed.update(cx, |card, cx| card.dialog(cx));
        let copied = self.copied.clone();
        let dialog = self.render_rename_dialog(window.viewport_size(), cx);
        // Split into this device and the rest; each renders as rows in one
        // block, like every other settings page.
        let (local, others): (Vec<_>, Vec<_>) = devices
            .into_iter()
            .enumerate()
            .partition(|(_, device)| local_id.as_deref() == Some(device.id.as_str()));
        let device_row = |ix: usize, device: zuse_protocol::Device, first: bool| {
            let online = device_online(device.last_seen_at, now);
            let is_local = local_id.as_deref() == Some(device.id.as_str());
            let id_copied = copied.as_deref() == Some(device.id.as_str());
            let copy_id = device.id.clone();
            let rename_id = device.id.clone();
            let rename_name = device.name.clone();
            let mut meta: Vec<AnyElement> = vec![
                div()
                    .child(SharedString::from(
                        platform_label(&device.platform).to_string(),
                    ))
                    .into_any_element(),
            ];
            if let Some(version) = device.version.as_deref().filter(|v| !v.is_empty()) {
                meta.push(
                    div()
                        .child(SharedString::from(format!("v{version}")))
                        .into_any_element(),
                );
            }
            // Presence only says something about other devices.
            if !is_local {
                meta.push(if online {
                    div()
                        .text_color(theme.success_muted)
                        .child(SharedString::from("Online"))
                        .into_any_element()
                } else {
                    div()
                        .child(SharedString::from(format!(
                            "Last seen {}",
                            format_last_seen(device.last_seen_at, now)
                        )))
                        .into_any_element()
                });
            }
            widgets::card_row(&theme, first)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(160.0))
                        .child(widgets::row_title(&theme, device.name.clone()))
                        .child(widgets::meta_line(&theme, meta)),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            widgets::text_action(
                                &theme,
                                widgets::ActionTone::Quiet,
                                if id_copied {
                                    crate::localization::text("settings-cloud_api_keys_copied", cx)
                                } else {
                                    ("Copy ID").to_string()
                                },
                            )
                            .id(("device-id", ix))
                            .tab_index(0)
                            .role(gpui::Role::Button)
                            .aria_label(format!("Copy device ID {}", device.id))
                            .focus_visible(|s| s.border_2().border_color(theme.accent))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.copy_id(copy_id.clone(), cx);
                                },
                            )),
                        )
                        .child(
                            widgets::text_action(
                                &theme,
                                widgets::ActionTone::Filled,
                                tr(cx, msg::settings::model_connections_rename),
                            )
                            .id(("device-rename", ix))
                            .tab_index(0)
                            .role(gpui::Role::Button)
                            .aria_label(format!("Rename {}", device.name))
                            .focus_visible(|s| s.border_2().border_color(theme.accent))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.open_rename(rename_id.clone(), rename_name.clone(), cx);
                                },
                            )),
                        ),
                )
                .into_any_element()
        };
        let local_block = (!local.is_empty()).then(|| {
            let mut block = widgets::section_card(&theme).mt_0();
            for (n, (ix, device)) in local.into_iter().enumerate() {
                block = block.child(device_row(ix, device, n == 0));
            }
            block
        });
        let has_others = !others.is_empty();
        let others_block = if others.is_empty() {
            widgets::section_card(&theme).mt_0().child(
                widgets::card_row(&theme, true).child(
                    div()
                        .text_size(crate::typography::text_sm())
                        .text_color(theme.text_muted)
                        .child(SharedString::from(
                            "Sign in on another device to see it here.",
                        )),
                ),
            )
        } else {
            let mut block = widgets::section_card(&theme).mt_0();
            for (n, (ix, device)) in others.into_iter().enumerate() {
                block = block.child(device_row(ix, device, n == 0));
            }
            block
        };
        let mut bridge = widgets::section_card(&theme).child(
            widgets::card_row(&theme, true)
                .child(widgets::row_title(&theme, "Cloud chat computers"))
                .child(
                    widgets::text_action(&theme, widgets::ActionTone::Quiet, "Load computers")
                        .id("bridge-load")
                        .on_click(
                            cx.listener(|page, _, _, cx| page.bridge_action("list", None, cx)),
                        ),
                ),
        );
        if self.bridge_owner.as_deref() != self.state.read(cx).cloud_user_id() {
            self.bridge.clear();
            self.bridge_owner = None;
        }
        bridge = bridge.child(
            widgets::card_row(&theme, false)
                .child(
                    widgets::text_action(
                        &theme,
                        widgets::ActionTone::Quiet,
                        "Allow cloud chats to use this computer",
                    )
                    .id("bridge-start")
                    .on_click(cx.listener(|page, _, _, cx| page.bridge_action("start", None, cx))),
                )
                .child(
                    widgets::text_action(
                        &theme,
                        widgets::ActionTone::Quiet,
                        "Stop computer connector",
                    )
                    .id("bridge-stop")
                    .on_click(cx.listener(|page, _, _, cx| page.bridge_action("stop", None, cx))),
                ),
        );
        let cloud_chat = self
            .state
            .read(cx)
            .engine()
            .and_then(|e| e.cloud_target())
            .is_some();
        if cloud_chat {
            bridge = bridge.child(
                widgets::card_row(&theme, false).child(
                    widgets::text_action(
                        &theme,
                        widgets::ActionTone::Quiet,
                        "Load this chat's computer approvals",
                    )
                    .id("bridge-chat-refresh")
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.bridge_chat_action("refresh", None, None, cx)
                    })),
                ),
            );
        }
        for (index, computer) in self.bridge.iter().enumerate() {
            let id = computer["deviceId"].as_str().unwrap_or("").to_owned();
            let forget = id.clone();
            let mut row = widgets::card_row(&theme, false).child(widgets::row_title(
                &theme,
                format!(
                    "{} · {}",
                    computer["name"].as_str().unwrap_or("Computer"),
                    computer["platform"].as_str().unwrap_or("")
                ),
            ));
            if cloud_chat {
                let device = id.clone();
                let attached = self.bridge_attached.contains(&device);
                row = row.child(
                    widgets::text_action(
                        &theme,
                        widgets::ActionTone::Quiet,
                        if attached {
                            "Detach from this chat"
                        } else {
                            "Attach to this chat"
                        },
                    )
                    .id(("bridge-attach", index))
                    .on_click(cx.listener(move |page, _, _, cx| {
                        page.bridge_chat_action(
                            if attached { "detach" } else { "attach" },
                            Some(device.clone()),
                            None,
                            cx,
                        )
                    })),
                );
            }
            if computer["alwaysAllowed"] == true {
                row = row.child(
                    widgets::text_action(&theme, widgets::ActionTone::Quiet, "Revoke always allow")
                        .id(("bridge-revoke", index))
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.bridge_action("revoke", Some(id.clone()), cx)
                        })),
                );
            }
            bridge = bridge.child(
                row.child(
                    widgets::text_action(&theme, widgets::ActionTone::Quiet, "Remove computer")
                        .id(("bridge-forget", index))
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.bridge_action("forget", Some(forget.clone()), cx)
                        })),
                ),
            );
        }
        if cloud_chat {
            for (index, request) in self.bridge_pending.iter().enumerate() {
                let title = format!(
                    "{} · {} · {}\n{}\n{}",
                    request["computerName"].as_str().unwrap_or("Computer"),
                    request["osUser"].as_str().unwrap_or(""),
                    request["shell"].as_str().unwrap_or(""),
                    request["cwd"].as_str().unwrap_or(""),
                    request["command"].as_str().unwrap_or("")
                );
                let mut row = div().flex().flex_col().child(title);
                for (decision, label) in [
                    ("once", "Approve once"),
                    ("session", "Approve for this session"),
                    ("always", "Approve always"),
                    ("deny", "Deny"),
                ] {
                    let id = request["id"].as_str().unwrap_or("").to_owned();
                    row = row.child(
                        widgets::text_action(&theme, widgets::ActionTone::Quiet, label)
                            .id((
                                SharedString::from(format!("bridge-decide-{decision}")),
                                index,
                            ))
                            .on_click(cx.listener(move |page, _, _, cx| {
                                page.bridge_chat_action(
                                    "decide",
                                    Some(id.clone()),
                                    Some(decision.into()),
                                    cx,
                                )
                            })),
                    );
                }
                bridge = bridge.child(widgets::card_row(&theme, false).child(row));
            }
        }
        let card = div()
            .child(bridge)
            .flex()
            .flex_col()
            .gap(px(16.))
            .when_some(local_block, |el, block| {
                el.child(
                    widgets::group(
                        &theme,
                        tr(cx, msg::settings::model_connections_local_scope),
                        "",
                    )
                    .child(block),
                )
            })
            // A local-only workspace never has other devices to list.
            // Account devices appear once there are some; an empty card only adds noise.
            .when(
                workspace_scope != Some(WorkspaceScope::Local) && has_others,
                |el| {
                    el.child(
                        widgets::group(&theme, tr(cx, msg::native::other_devices), "")
                            .child(others_block),
                    )
                },
            );

        let scrollbar = popover::rail(self, "devices-page-scrollbar", &theme, cx);
        div()
            .id("devices-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("devices-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::settings_page_column(window)
                                .child(widgets::section_title(
                                    &theme,
                                    &tr(cx, msg::settings::settings_page_remote_access),
                                    &tr(cx, msg::native::use_this_computer_from_your_phone_a_browser_or_another_computer),
                                ))
                                .child({
                                    use zuse_viewport::{remote_access_style as r, settings_style as st};
                                    let empty = pairing.device_rows.is_empty() && computer_rows.is_empty();
                                    let advanced_open = pairing.advanced.is_some();
                                    div().mt(px(20.)).flex().flex_col().gap(px(20.))
                                        .child(super::pairing::section(&theme, "This computer", None, pairing.hero))
                                        .child(super::pairing::section(
                                            &theme,
                                            "Devices",
                                            Some(pairing.add_computer),
                                            st::card()
                                                .when(empty, |v| v.child(r::empty("No devices yet. Use Connect device to pair your phone.")))
                                                .children(pairing.device_rows)
                                                .children(computer_rows),
                                        ))
                                        .child(div().flex().flex_col().gap(px(12.))
                                            .child(pairing.advanced_toggle)
                                            .when(advanced_open, |v| v
                                                .children(pairing.advanced)
                                                .child(card)
                                                .children(self.render_workspace_storage(cx))))
                                }),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
            .children(pairing.dialog)
            .children(account_dialog)
            .when_some(dialog, |el, dialog| el.child(dialog))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    #[test]
    fn presence_window() {
        let now = Utc::now();
        assert!(device_online(Some(now - TimeDelta::seconds(10)), now));
        assert!(device_online(Some(now - TimeDelta::seconds(70)), now));
        assert!(!device_online(Some(now - TimeDelta::seconds(71)), now));
        assert!(!device_online(None, now));
        // Clock skew (future) counts as online.
        assert!(device_online(Some(now + TimeDelta::seconds(30)), now));
    }

    #[test]
    fn last_seen_formatting() {
        let now = Utc::now();
        assert_eq!(format_last_seen(None, now), "never seen");
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::seconds(30)), now),
            "just now"
        );
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::minutes(5)), now),
            "5m ago"
        );
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::hours(3)), now),
            "3h ago"
        );
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::days(2)), now),
            "2d ago"
        );
    }
}
