//! Account remote controls share the connected profile and never store credentials.
use super::*;
use crate::{settings_style as s, shell::column};
#[derive(Debug, Clone, Copy, PartialEq)]
enum Read {
    Host,
    Computers,
    Routes,
    Devices,
}
#[derive(Default)]
pub(super) struct ManagedAccess {
    host: Value,
    computers: Vec<Value>,
    saved: Vec<Value>,
    devices: Vec<Value>,
    reads: HashMap<u64, Read>,
    action: Option<u64>,
    account_ready: bool,
    error: Option<String>,
}
impl WorkspaceView {
    pub(super) fn account_connection_error(&mut self, message: &str) {
        self.managed_access.error = Some(message.to_owned());
    }
    pub(super) fn refresh_managed_access(&mut self) {
        if !self.connected || self.managed_access.action.is_some() {
            return;
        }
        let mut reads = vec![(Read::Host, methods::WATCH_MANAGED_HOST)];
        if self.managed_access.account_ready && self.cloud_account["user"].is_object() {
            reads.extend([
                (Read::Computers, methods::LIST_ACCOUNT_ENVIRONMENTS),
                (Read::Routes, methods::LIST_ACCOUNT_ROUTES),
                (Read::Devices, methods::LIST_ACCOUNT_DEVICES),
            ]);
        }
        for (kind, method) in reads {
            if self
                .managed_access
                .reads
                .values()
                .any(|value| *value == kind)
            {
                continue;
            }
            let id = self.call(method, json!({}));
            self.managed_access.reads.insert(id, kind);
        }
    }
    pub(super) fn managed_account_updated(&mut self, changed: bool) {
        if changed || !self.managed_access.account_ready {
            let reads = std::mem::take(&mut self.managed_access.reads);
            for (id, _) in reads {
                self.cancel(id);
            }
            if let Some(id) = self.managed_access.action.take() {
                self.cancel(id);
                self.managed_access.error=Some("The account changed during an access action. Check its current state before trying again.".into());
            }
            self.managed_access.computers.clear();
            self.managed_access.saved.clear();
            self.managed_access.devices.clear();
            self.managed_access.host = Value::Null;
            self.managed_access.account_ready = true;
            if self.settings && self.settings_section == settings_view::Section::Devices {
                self.refresh_managed_access();
            }
        }
    }
    pub(super) fn interrupt_managed_access(&mut self) {
        self.managed_access.reads.clear();
        self.managed_access.action = None;
        self.managed_access.account_ready = false;
    }
    pub(super) fn managed_action(&mut self, method: &'static str, params: Value) {
        if !self.connected || self.managed_access.action.is_some() {
            return;
        }
        self.managed_access.error = None;
        // Retire catalog reads before writes so a queued old response cannot
        // replace the fresh post-action account state. Keep the live host watch.
        let stale: Vec<u64> = self
            .managed_access
            .reads
            .iter()
            .filter(|(_, kind)| **kind != Read::Host)
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            self.managed_access.reads.remove(&id);
            self.cancel(id);
        }
        self.managed_access.action = Some(self.call(method, params));
    }
    pub(super) fn receive_managed_access(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let action = self.managed_access.action == Some(id);
        let method = self.pending.get(&id).cloned().unwrap_or_default();
        let kind = self.managed_access.reads.get(&id).copied();
        if !action && kind.is_none() {
            return false;
        }
        if let Some(error) = frame["err"].as_str() {
            self.managed_access.error = Some(error.into());
        }
        if let Some(value) = frame.get("ok").or_else(|| frame.get("item")) {
            match kind {
                Some(Read::Host) => self.managed_access.host = value.clone(),
                Some(Read::Routes) => {
                    self.managed_access.saved =
                        value["computers"].as_array().cloned().unwrap_or_default()
                }
                Some(Read::Computers) => {
                    self.managed_access.computers = value["environments"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                }
                Some(Read::Devices) => {
                    self.managed_access.devices =
                        value["clients"].as_array().cloned().unwrap_or_default()
                }
                None if action && method == methods::FORGET_ACCOUNT_ROUTE => {
                    self.managed_access.saved =
                        value["computers"].as_array().cloned().unwrap_or_default()
                }
                None => {}
            }
        }
        if action || kind != Some(Read::Host) || frame.get("err").is_some() || frame["done"] == true
        {
            self.managed_access.reads.remove(&id);
            self.pending.remove(&id);
        }
        if action {
            if frame.get("ok").is_some()
                && matches!(
                    method.as_str(),
                    methods::LINK_MANAGED_HOST
                        | methods::UNLINK_MANAGED_HOST
                        | methods::REVOKE_ACCOUNT_DEVICE
                )
            {
                self.remote_ui.dialog = None;
            }
            self.managed_access.action = None;
            if method != methods::FORGET_ACCOUNT_ROUTE {
                self.refresh_managed_access();
            }
            if matches!(
                method.as_str(),
                methods::LINK_MANAGED_HOST | methods::UNLINK_MANAGED_HOST
            ) {
                self.refresh_pairings();
            }
        }
        cx.notify();
        true
    }
    pub(super) fn managed_access_busy(&self) -> bool {
        self.managed_access.action.is_some()
    }
    pub(super) fn managed_access_error(&self) -> Option<String> {
        self.managed_access
            .error
            .clone()
            .or_else(|| {
                self.managed_access.host["error"]
                    .as_str()
                    .map(str::to_owned)
            })
            .or_else(|| self.cloud_account["error"].as_str().map(str::to_owned))
    }
    pub(super) fn managed_access_method(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::remote_access_style as r;
        let host = &self.managed_access.host;
        let configured = host["configured"] == true;
        let busy = self.managed_access_busy();
        let description = if configured && host["tunnelRunning"] == true {
            format!(
                "Available anywhere · {}",
                host["endpoint"]["httpBaseUrl"]
                    .as_str()
                    .unwrap_or("Zuse account")
            )
        } else if configured && host["accountMatches"] != true {
            "Sign in to the linked account.".into()
        } else if configured {
            "Reconnecting to your account…".into()
        } else {
            "Access this computer from your signed-in devices.".into()
        };
        r::method_row("remote-server", "Zuse Serve", description, true)
            .child(
                r::method_button(
                    "managed-host-link",
                    if busy {
                        "Updating…"
                    } else if configured {
                        "Turn off"
                    } else {
                        "Set up"
                    },
                    cx,
                )
                .when(busy, |v| v.opacity(0.5))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_remote_dialog(
                        if configured {
                            super::pairing::RemoteDialog::Unlink
                        } else {
                            super::pairing::RemoteDialog::Setup
                        },
                        window,
                        cx,
                    )
                })),
            )
            .into_any_element()
    }
    pub(super) fn managed_setup_form(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::remote_access_style as r;
        let signed_in = self.cloud_account["user"].is_object();
        let busy = self.managed_access_busy();
        column()
            .gap(px(12.))
            .child(
                crate::shell::row()
                    .gap(px(10.))
                    .child(
                        r::details(
                            crate::localization::text("projects-projects_sidebar_zuse_account", cx),
                            self.cloud_account["user"]["email"]
                                .as_str()
                                .map(zuse_protocol::presentation::masked_email)
                                .unwrap_or_else(|| "Sign in to link this computer.".into()),
                        )
                        .flex_1(),
                    )
                    .child(
                        r::button(
                            "managed-account-sign-in",
                            if busy {
                                crate::localization::text("settings-model_connections_working", cx)
                            } else if signed_in {
                                crate::localization::text("common-signOut", cx)
                            } else {
                                crate::localization::text("common-signIn", cx)
                            },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.managed_action(
                                if signed_in {
                                    methods::CLOUD_ACCOUNT_SIGN_OUT
                                } else {
                                    methods::CLOUD_ACCOUNT_SIGN_IN
                                },
                                json!({}),
                            );
                            cx.notify();
                        })),
                    ),
            )
            .when_some(
                self.cloud_account["login"]["url"]
                    .as_str()
                    .filter(|url| url.starts_with("https://"))
                    .map(str::to_owned),
                |view, url| {
                    view.child(r::details(
                        "Complete sign-in",
                        self.cloud_account["login"]["userCode"]
                            .as_str()
                            .unwrap_or("Continue in your browser.")
                            .to_owned(),
                    ))
                    .child(
                        r::button("managed-open-sign-in", "Open sign-in")
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                },
            )
            .when(signed_in, |view| {
                view.child(r::details(
                    "Computer name",
                    "A name you recognize on your other devices.",
                ))
                .child(s::field().child(self.managed_name.clone()))
            })
            .into_any_element()
    }
    pub(super) fn managed_computers(&self, add: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        use crate::remote_access_style as r;
        let rows = zuse_protocol::managed_connection::computer_rows(
            &self.managed_access.computers,
            &self.managed_access.saved,
        );
        r::frame(crate::localization::text("settings-remote_other_computers", cx),crate::localization::text("settings-using_computers_card_projects_and_chats_from_connected_computers_stay_in_the_same_sidebar", cx),Some(add))
            .child(s::card().when(rows.is_empty(),|v|v.child(r::empty_computers(cx)))
                .children(rows.into_iter().enumerate().map(|(index,computer)|{
                    let id=computer["environmentId"].as_str().unwrap_or_default().to_owned();let environment=id.clone();
                    let remembered=self.managed_access.saved.iter().any(|saved|saved["environmentId"]==computer["environmentId"]);
                    let current=self.device_id.as_deref()==Some(id.as_str());let connect=!current&&zuse_protocol::managed_connection::supports_record(computer);
                    let label=computer["label"].as_str().unwrap_or("Linked computer").to_owned();let subject=self.cloud_account["user"]["id"].as_str().unwrap_or_default().to_owned();let connect_subject=subject.clone();
                    let description=if current {"This computer"}else if connect {computer["serviceState"].as_str().unwrap_or("Availability checked on connect")}else{"Update Zuse on this computer to connect."};
                    crate::shell::row().min_h(px(48.)).px(px(12.)).py(px(8.)).gap(px(10.)).when(index>0,|v|v.border_t_1().border_color(s::card_border().opacity(0.4)))
                        .child(crate::assets::icon("monitor").size(px(16.))).child(r::details(label.clone(),description.to_owned()).flex_1())
                        .when(connect,|v|v.child(r::button(("connect-account-computer",index),crate::localization::text("common-connect", cx)).on_click(cx.listener(move|this,_,_,_|{let _=this.requests.unbounded_send(json!({"ui":"openAccountComputer","environmentId":environment,"expectedSubject":connect_subject,"label":label}));}))))
                        .when(remembered,|v|v.child(r::button(("forget-account-computer",index),"Forget saved").on_click(cx.listener(move|this,_,_,cx|{this.managed_action(methods::FORGET_ACCOUNT_ROUTE,json!({"environmentId":id,"expectedSubject":subject}));cx.notify();}))))
                }))).into_any_element()
    }
    pub(super) fn managed_account_devices(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::remote_access_style as r;
        if !self.cloud_account["user"].is_object() {
            return None;
        }
        Some(
            r::frame(
                "Account devices",
                "Manage devices authorized through your account.",
                None,
            )
            .child(
                s::card()
                    .when(self.managed_access.devices.is_empty(), |v| {
                        v.child(r::empty("No account devices connected yet."))
                    })
                    .children(self.managed_access.devices.iter().enumerate().map(
                        |(index, device)| {
                            let id = device["clientId"].as_str().unwrap_or_default().to_owned();
                            let label = device["label"]
                                .as_str()
                                .unwrap_or("Account device")
                                .to_owned();
                            crate::shell::row()
                                .min_h(px(48.))
                                .px(px(12.))
                                .py(px(8.))
                                .gap(px(10.))
                                .when(index > 0, |v| {
                                    v.border_t_1().border_color(s::card_border().opacity(0.4))
                                })
                                .child(
                                    r::details(
                                        label.clone(),
                                        device["platform"].as_str().unwrap_or("Device").to_owned(),
                                    )
                                    .flex_1(),
                                )
                                .child(
                                    r::small_button(
                                        ("revoke-account-device", index),
                                        crate::localization::text(
                                            "chat-device_bridge_panel_revoke",
                                            cx,
                                        ),
                                    )
                                    .border_color(gpui::transparent_black())
                                    .text_color(s::destructive_text())
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.managed_action(
                                                methods::REVOKE_ACCOUNT_DEVICE,
                                                json!({"clientId":id}),
                                            );
                                            cx.notify();
                                        },
                                    )),
                                )
                        },
                    )),
            )
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn account_changes_retire_catalogs_and_uncertain_actions_are_not_replayed(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut calls) = futures::channel::mpsc::unbounded();
        let (_, events) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(Transport { requests, events }, window, cx)
        });
        window.update(cx,|view,_,cx| {
            view.connected=true;view.settings=true;view.settings_section=settings_view::Section::Devices;
            view.cloud_account=json!({"user":{"id":"account-a"}});view.managed_account_updated(true);
            view.refresh_managed_access();
            assert_eq!(view.managed_access.reads.len(),4);
            let host=*view.managed_access.reads.iter().find(|(_,kind)|**kind==Read::Host).unwrap().0;
            let stale=*view.managed_access.reads.iter().find(|(_,kind)|**kind==Read::Computers).unwrap().0;
            view.receive(json!({"id":host,"item":{"configured":true,"enabled":true,"accountMatches":true,"tunnelRunning":false}}),cx);
            view.receive(json!({"id":host,"item":{"configured":true,"enabled":true,"accountMatches":true,"tunnelRunning":true}}),cx);
            assert_eq!(view.managed_access.host["tunnelRunning"],true);
            assert_eq!(view.managed_access.reads.get(&host),Some(&Read::Host));
            view.managed_action(methods::UNLINK_MANAGED_HOST,json!({}));
            let action=view.managed_access.action.unwrap();
            view.managed_action(methods::UNLINK_MANAGED_HOST,json!({}));
            view.receive(json!({"id":stale,"ok":{"environments":[{"environmentId":"stale"}]}}),cx);
            assert!(view.managed_access.computers.is_empty());
            view.receive(json!({"id":action,"err":"Receipt lost","outcomeUnknown":true}),cx);
            assert!(view.managed_access.action.is_none());
            let old=*view.managed_access.reads.iter().find(|(_,kind)|**kind==Read::Computers).unwrap().0;
            view.cloud_account=json!({"user":{"id":"account-b"}});view.managed_account_updated(true);
            view.receive(json!({"id":old,"ok":{"environments":[{"environmentId":"account-a"}]}}),cx);
            assert!(view.managed_access.computers.is_empty());
        }).unwrap();
        let mut frames = Vec::new();
        while let Ok(Some(frame)) = calls.try_next() {
            frames.push(frame);
        }
        assert_eq!(
            frames
                .iter()
                .filter(|f| f["method"] == methods::UNLINK_MANAGED_HOST)
                .count(),
            1
        );
        // The second watch belongs to the new account epoch; no polling was introduced.
        assert_eq!(
            frames
                .iter()
                .filter(|f| f["method"] == methods::WATCH_MANAGED_HOST)
                .count(),
            2
        );
    }
}
