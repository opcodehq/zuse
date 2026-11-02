use super::*;
use zuse_protocol::plugin_state::Removal;
impl WorkspaceView {
    pub(super) fn receive_plugins(&mut self, id: u64, frame: &Value, cx: &mut Context<Self>) {
        if self.plugin_request != Some(id) {
            self.pending.remove(&id);
            return;
        }
        let next = if let Some(value) = frame.get("ok").or_else(|| frame.get("item")) {
            match self.plugin_state.apply(value) {
                Ok(next) => next,
                Err(error) => {
                    self.plugin_state.notice = Some(error.into());
                    self.plugin_state.failed()
                }
            }
        } else if frame.get("err").is_some() || frame["done"] == true {
            self.plugin_state.notice = Some(
                "Could not confirm the plugin request. Check connections before trying again."
                    .into(),
            );
            self.plugin_state.failed()
        } else {
            return;
        };
        self.plugin_request = None;
        self.pending.remove(&id);
        if let Some(next) = next {
            self.plugin_action(next);
        }
        self.schedule_plugin_poll(cx);
        cx.notify();
    }
    fn schedule_plugin_poll(&mut self, cx: &mut Context<Self>) {
        self.plugin_poll = None;
        if !self.plugin_state.active_attempt() || self.plugin_request.is_some() {
            return;
        }
        let id = self.plugin_state.attempt["id"].clone();
        let tenant = self.plugin_state.snapshot["tenantId"].clone();
        let owner = self.cloud_account["user"]["id"].clone();
        let epoch = self.connection_epoch;
        self.plugin_poll = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1500))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.connected
                    && this.connection_epoch == epoch
                    && this.cloud_account["user"]["id"] == owner
                    && this.plugin_state.active_attempt()
                    && this.plugin_state.attempt["id"] == id
                    && this.plugin_state.snapshot["tenantId"] == tenant
                    && this.plugin_request.is_none()
                {
                    this.plugin_action(json!({"action":"poll","tenantId":tenant,"attemptId":id}));
                    cx.notify();
                }
            });
        }));
    }
    pub(super) fn ask_plugin_removal(&mut self, kind: Removal, id: &str, cx: &mut Context<Self>) {
        if !self.connected {
            return;
        }
        if let Err(error) = self.plugin_state.ask_removal(kind, id) {
            self.plugin_state.notice = Some(error.into());
        }
        cx.notify();
    }
    pub(super) fn confirm_plugin_removal(&mut self, cx: &mut Context<Self>) {
        if !self.connected {
            return;
        }
        match self.plugin_state.confirm_removal() {
            Ok(request) => self.plugin_action(request),
            Err(error) => self.plugin_state.notice = Some(error.into()),
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn plugin_receipt_loss_reads_once_and_old_account_frames_cannot_publish(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut received) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window.update(cx,|view,_,cx|{
            view.connected=true;view.connection_epoch=1;view.cloud_account=json!({"user":{"id":"owner"}});
            view.plugin_state.snapshot=json!({"tenantId":"organization:team","canManage":true});
            view.error=Some("Unrelated upload failure".into());
            view.plugin_action(json!({"action":"disconnect","tenantId":"organization:team","connectionId":"connection"}));let write=view.plugin_request.unwrap();
            view.receive(json!({"id":write,"err":"Lost confirmation","done":true}),cx);
            assert_eq!(view.error.as_deref(),Some("Unrelated upload failure"));let read=view.plugin_request.unwrap();assert_ne!(read,write);
            assert_eq!(view.plugin_state.tenant(),Some("organization:team"));
            let account=view.call(methods::CLOUD_ACCOUNT_STATUS,json!({}));
            view.receive(json!({"id":account,"item":{"user":{"id":"another"}}}),cx);
            let current=view.plugin_request.unwrap();assert_ne!(current,read);assert!(view.plugin_state.snapshot.is_null());
            view.receive(json!({"id":read,"err":"Old account error","done":true}),cx);
            assert_eq!(view.plugin_request,Some(current));assert_eq!(view.error.as_deref(),Some("Unrelated upload failure"));
        }).unwrap();
        let mut plugin_calls = vec![];
        while let Ok(request) = received.try_recv() {
            if request["method"] == methods::PLUGINS {
                plugin_calls.push(request["params"].clone());
            }
        }
        assert_eq!(plugin_calls.len(), 3);
        assert_eq!(plugin_calls[0]["action"], "disconnect");
        assert_eq!(
            plugin_calls[1],
            json!({"action":"list","tenantId":"organization:team"})
        );
        assert_eq!(plugin_calls[2], json!({"action":"list"}));
    }
    #[gpui::test]
    fn workspace_selection_uses_a_mutation_and_reconciles_its_uncertain_receipt(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut received) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window.update(cx, |view, _, cx| {
            view.connected = true;
            view.plugin_state.snapshot = json!({"tenantId":"personal:owner","canManage":true});
            view.plugin_select_workspace(json!("organization:team"));
            let select = view.plugin_request.unwrap();
            view.receive(json!({"id":select,"err":"Receipt lost","done":true}),cx);
            let read = view.plugin_request.unwrap();
            assert_ne!(select,read);
            view.receive(json!({"id":read,"ok":{"kind":"snapshot","tenantId":"organization:team","tenants":[],"endpoint":"unused","catalog":[],"connections":[]}}),cx);
            assert_eq!(view.plugin_state.tenant(),Some("organization:team"));
            assert!(view.plugin_request.is_none());
        }).unwrap();
        let mut calls = vec![];
        while let Ok(request) = received.try_recv() {
            if matches!(
                request["method"].as_str(),
                Some(methods::PLUGINS | methods::SELECT_PLUGIN_WORKSPACE)
            ) {
                calls.push(request);
            }
        }
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["method"], methods::SELECT_PLUGIN_WORKSPACE);
        assert_eq!(calls[0]["params"]["tenantId"], "organization:team");
        assert_eq!(calls[1]["method"], methods::PLUGINS);
        assert_eq!(calls[1]["params"], json!({"action":"list"}));
    }
}
