//! Pairing capabilities stay in this transient view, outside UI preferences.
use super::*;
use crate::settings_style as s;
use crate::shell::{column, row};

impl WorkspaceView {
    pub(super) fn refresh_pairings(&mut self) {
        if !self.connected || self.pairing_fetch.is_some() {
            return;
        }
        self.pairing_fetch = Some(self.call(methods::LIST_PAIRINGS, json!({})));
    }
    fn pairing_action(&mut self, method: &'static str, params: Value) {
        if !self.connected || self.pairing_action.is_some() {
            return;
        }
        self.pairing_action = Some(self.call(method, params));
    }
    pub(super) fn receive_pairing(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let fetch = self.pairing_fetch == Some(id);
        let action = self.pairing_action == Some(id);
        if !fetch && !action {
            return false;
        }
        if let Some(error) = frame["err"].as_str() {
            self.error = Some(error.into());
        }
        if let Some(value) = frame.get("ok") {
            if fetch {
                self.pairings = value.clone();
            }
            if action {
                self.pairing_fragment = value["fragment"].as_str().map(str::to_owned);
                if let Some(origin) = value["origin"]
                    .as_str()
                    .and_then(|origin| zuse_remote::normalized_origin(origin).ok())
                {
                    self.pairing_origin = origin;
                }
                if let Some(fetch) = self.pairing_fetch.take() {
                    self.cancel(fetch);
                }
                self.refresh_pairings();
            }
        }
        self.pending.remove(&id);
        if fetch {
            self.pairing_fetch = None;
        }
        if action {
            self.pairing_action = None;
        }
        cx.notify();
        true
    }
    pub(super) fn pairing_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let link = self
            .pairing_fragment
            .as_ref()
            .map(|fragment| format!("{}/{}", self.pairing_origin, fragment));
        column().gap_5()
            .child(s::frame()
                .child(s::header("Connect a device","Invite a trusted device to use this engine’s agents, files, terminals and settings.")
                    .child(s::button("refresh-pairings","Refresh").on_click(cx.listener(|this,_,_,cx|{this.refresh_pairings();cx.notify();}))))
                .child(s::card()
                    .child(s::setting_row(true).child(s::field().child(self.pairing_name.clone()))
                        .child(s::button("create-pairing",if self.pairing_action.is_some(){"Working…"}else{"Create invitation"})
                            .on_click(cx.listener(|this,_,_,cx|{
                                let name=this.pairing_name.read(cx).value(cx);
                                if name.trim().is_empty(){this.error=Some("Enter a name for the device you want to pair.".into());}
                                else{this.pairing_action(methods::CREATE_PAIRING,json!({"label":name}));}
                                cx.notify();
                            }))))
                    .when_some(link,|card,link|{
                        let copy=link.clone();
                        card.child(s::setting_row(false).flex_col().items_start()
                            .child(s::details("Private invitation","Share only with someone you trust. Unused invitations expire in 10 minutes."))
                            .child(row().gap_2()
                                .child(s::button("copy-pairing-link","Copy invitation").on_click(move|_,_,cx|cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))))
                                .child(s::button("open-pairing-link","Open invitation").on_click(move|_,_,cx|cx.open_url(&link)))
                                .child(s::button("hide-pairing-link","Hide").on_click(cx.listener(|this,_,_,cx|{this.pairing_fragment=None;cx.notify();})))))
                    })))
            .child(s::frame().child(s::header("Connected devices","Revoke access when a device is lost or no longer needs this workspace."))
                .child(s::card()
                    .when(self.pairings.as_array().is_none_or(|peers|peers.is_empty()),|card|card.child(s::setting_row(true).child(s::details("No paired devices","Create an invitation to connect your first device."))))
                    .children(self.pairings.as_array().into_iter().flatten().enumerate().map(|(index,peer)|{
                        let id=peer["id"].as_str().unwrap_or_default().to_owned();
                        s::setting_row(index==0).child(s::details(peer["label"].as_str().unwrap_or("Paired device").to_owned(),if peer["pairedAt"].is_number(){"Paired"}else if peer["expiresAt"].as_u64().is_some_and(|expiry|expiry<=chrono::Utc::now().timestamp().max(0) as u64){"Invitation expired"}else{"Invitation pending"}))
                            .child(s::button(("revoke-pairing",index),"Revoke").on_click(cx.listener(move|this,_,_,cx|{this.pairing_action(methods::REVOKE_PAIRING,json!({"pairId":id}));cx.notify();})))
                    }))))
            .when(self.paired,|view|view.child(s::frame().child(s::header("This browser","Manage the access saved in this tab."))
                .child(s::card().child(s::setting_row(true).child(s::details("Forget paired access","This reloads the tab. Save local drafts first."))
                    .child(s::button("forget-browser-pairing","Forget pairing").on_click(cx.listener(|this,_,_,_|{let _=this.requests.unbounded_send(json!({"ui":"forgetPairing"}));})))))))
            .into_any_element()
    }
}
