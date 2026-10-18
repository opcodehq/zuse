//! Account remote controls share the connected profile and never store credentials.
use super::*;
use crate::{settings_style as s, shell::column};
#[derive(Debug, Clone, Copy, PartialEq)]
enum Read {
    Host,
    Computers,
    Devices,
}
#[derive(Default)]
pub(super) struct ManagedAccess {
    host: Value,
    computers: Vec<Value>,
    devices: Vec<Value>,
    reads: HashMap<u64, Read>,
    action: Option<u64>,
    account_ready: bool,
    error: Option<String>,
}
impl WorkspaceView {
    pub(super) fn refresh_managed_access(&mut self) {
        if !self.connected || self.managed_access.action.is_some() {
            return;
        }
        let mut reads = vec![(Read::Host, methods::WATCH_MANAGED_HOST)];
        if self.managed_access.account_ready && self.cloud_account["user"].is_object() {
            reads.extend([
                (Read::Computers, methods::LIST_ACCOUNT_ENVIRONMENTS),
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
    fn managed_action(&mut self, method: &'static str, params: Value) {
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
                None => {}
            }
        }
        if action || kind != Some(Read::Host) || frame.get("err").is_some() || frame["done"] == true
        {
            self.managed_access.reads.remove(&id);
            self.pending.remove(&id);
        }
        if action {
            self.managed_access.action = None;
            self.refresh_managed_access();
            self.refresh_pairings();
        }
        cx.notify();
        true
    }
    pub(super) fn managed_access_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let host = &self.managed_access.host;
        let signed_in = self.cloud_account["user"].is_object();
        let configured = host["configured"] == true;
        let busy = self.managed_access.action.is_some();
        let title = if !configured {
            "Link this computer"
        } else if host["enabled"] != true {
            "Remote access disabled"
        } else if host["accountMatches"] != true {
            "Sign in to the linked account"
        } else if host["tunnelRunning"] == true {
            "Remote access started"
        } else {
            "Starting remote access"
        };
        let description = if configured {
            host["endpoint"]["httpBaseUrl"]
                .as_str()
                .unwrap_or("Create a private invitation below to connect another device.")
        } else {
            "Link your account, then create a private invitation below to connect another device."
        };
        column().gap_5()
            .child(s::frame().child(s::header("Account remote access","Connect your computers across networks using your Zuse account."))
                .child(s::card()
                    .child(s::setting_row(true).child(s::details("Zuse account",self.cloud_account["user"]["email"].as_str().unwrap_or("Sign in to link this computer.").to_owned()))
                        .child(s::button("managed-account-sign-in",if signed_in{"Sign out"}else{"Sign in"})
                            .on_click(cx.listener(move|this,_,_,cx|{if this.connected {this.call(if signed_in{methods::CLOUD_ACCOUNT_SIGN_OUT}else{methods::CLOUD_ACCOUNT_SIGN_IN},json!({}));}cx.notify();}))))
                    .when_some(self.cloud_account["login"]["url"].as_str().filter(|url|url.starts_with("https://")).map(str::to_owned),|card,url|card.child(s::setting_row(false)
                        .child(s::details("Complete sign-in",self.cloud_account["login"]["userCode"].as_str().unwrap_or("Continue in your browser.").to_owned()))
                        .child(s::button("managed-open-sign-in","Open sign-in").on_click(move|_,_,cx|cx.open_url(&url)))))
                    .child(s::setting_row(false).child(s::details(title,description.to_owned()))
                        .when(signed_in || configured,|row|row.child(s::button("managed-host-link",if busy{"Working…"}else if configured{"Unlink computer"}else{"Link computer"})
                            .on_click(cx.listener(move|this,_,_,cx|{
                                let label=this.managed_name.read(cx).value(cx).trim().to_owned();
                                this.managed_action(if configured{methods::UNLINK_MANAGED_HOST}else{methods::LINK_MANAGED_HOST},json!({"label":label}));cx.notify();
                            })))))
                    .when(signed_in && !configured,|card|card.child(s::setting_row(false).child(s::details("Computer name","A name you recognize on your other devices.")).child(s::field().child(self.managed_name.clone()))))
                    .when_some(self.managed_access.error.as_deref().or(host["error"].as_str()).or(self.cloud_account["error"].as_str()),|card,error|card.child(s::setting_row(false).child(s::description_text(error.to_owned()))))
                    .child(s::setting_row(false).child(s::button("refresh-managed-access","Refresh remote access").on_click(cx.listener(|this,_,_,cx|{this.managed_access.error=None;this.refresh_managed_access();cx.notify();}))))))
            .when(signed_in,|view|view
                .child(s::frame().child(s::header("Account computers","Create a private invitation on the computer you want to connect to."))
                    .child(s::card()
                        .when(self.managed_access.computers.is_empty(),|card|card.child(s::setting_row(true).child(s::details("No linked computers","Link this computer to get started."))))
                        .children(self.managed_access.computers.iter().enumerate().map(|(index,computer)|s::setting_row(index==0).child(s::details(computer["label"].as_str().unwrap_or("Linked computer").to_owned().to_owned(),computer["serviceState"].as_str().unwrap_or("Status unavailable").to_owned()))))))
                .child(s::frame().child(s::header("Authorized account devices","Revoke a device that no longer needs account access."))
                    .child(s::card()
                        .when(self.managed_access.devices.is_empty(),|card|card.child(s::setting_row(true).child(s::details("No authorized devices","Devices appear here after they connect through your account."))))
                        .children(self.managed_access.devices.iter().enumerate().map(|(index,device)|{
                            let id=device["clientId"].as_str().unwrap_or_default().to_owned();
                            s::setting_row(index==0).child(s::details(device["label"].as_str().unwrap_or("Account device").to_owned().to_owned(),device["platform"].as_str().unwrap_or("Device").to_owned()))
                                .child(s::button(("revoke-account-device",index),"Revoke").on_click(cx.listener(move|this,_,_,cx|{this.managed_action(methods::REVOKE_ACCOUNT_DEVICE,json!({"clientId":id}));cx.notify();})))
                        }))))
            ).into_any_element()
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
            assert_eq!(view.managed_access.reads.len(),3);
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
