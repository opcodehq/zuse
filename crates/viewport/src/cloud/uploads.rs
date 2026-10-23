//! Browser file objects stay client-local; only bounded chunks enter the selected guest RPC.
use super::*;
impl WorkspaceView {
    pub(super) fn close_cloud_uploads(&mut self) {
        for (id, callback) in std::mem::take(&mut self.cloud.uploads) {
            self.cancel(id);
            self.cloud_upload_reply(
                callback,
                None,
                Some("The cloud environment changed. Attach the file again.".into()),
            );
        }
        self.cloud.viewport = uuid::Uuid::new_v4().to_string();
    }
    fn cloud_upload_reply(&self, callback: Value, ok: Option<Value>, error: Option<String>) {
        let mut frame = json!({"ui":"cloudUploadReply","callback":callback});
        if let Some(ok) = ok {
            frame["ok"] = ok;
        }
        if let Some(error) = error {
            frame["err"] = json!(error);
        }
        let _ = self.requests.unbounded_send(frame);
    }
    pub(super) fn cloud_file_picker(&self, mut frame: Value) {
        let Some(target) = &self.cloud.target else {
            return;
        };
        frame["cloudViewport"] = json!(self.cloud.viewport);
        frame["cloudWorkspaceId"] = target["workspaceId"].clone();
        frame["cloudEpoch"] = frame["epoch"].clone();
        frame["epoch"] = json!(self.connection_epoch);
        let _ = self.requests.unbounded_send(frame);
    }
    pub(super) fn receive_cloud_upload(&mut self, frame: &Value, _cx: &mut Context<Self>) -> bool {
        if frame.get("cloudUpload").is_some() {
            let request = &frame["cloudUpload"];
            let callback = request["id"].clone();
            let valid = self.connected
                && !self.cloud.viewport.is_empty()
                && request["viewport"] == self.cloud.viewport
                && self
                    .cloud
                    .target
                    .as_ref()
                    .is_some_and(|t| t["workspaceId"] == request["workspaceId"])
                && self
                    .cloud
                    .connected_epoch
                    .is_some_and(|epoch| request["epoch"].as_u64() == Some(epoch))
                && self.cloud.status["state"] == "ready"
                && matches!(
                    request["method"].as_str(),
                    Some(methods::UPLOAD_CHUNK | methods::UPLOAD_COMMIT)
                );
            if !valid {
                self.cloud_upload_reply(
                    callback,
                    None,
                    Some(
                        "The cloud connection changed. Attach the file again after reconnecting."
                            .into(),
                    ),
                );
            } else {
                let id=self.call(methods::CLOUD_RPC,json!({"workspaceId":request["workspaceId"],"method":request["method"],"params":request["params"],"requestId":0}));
                self.cloud.uploads.insert(id, callback);
            }
            return true;
        }
        if frame.get("cloudViewport").is_some() {
            if frame["cloudViewport"] == self.cloud.viewport {
                if let Some(events) = &self.cloud.events {
                    let mut delivery = frame.clone();
                    delivery.as_object_mut().unwrap().remove("cloudViewport");
                    let _ = events.unbounded_send(delivery);
                }
            }
            return true;
        }
        let Some(id) = frame["id"].as_u64() else {
            return false;
        };
        if !self.cloud.uploads.contains_key(&id) {
            return false;
        }
        let inner = frame.get("item");
        let error = frame["err"]
            .as_str()
            .or_else(|| inner.and_then(|v| v["err"].as_str()));
        let ok = inner.and_then(|v| v.get("ok"));
        if ok.is_some() || error.is_some() || frame["done"] == true {
            let callback = self.cloud.uploads.remove(&id).unwrap();
            self.pending.remove(&id);
            let error = if ok.is_none() && error.is_none() {
                Some("Cloud upload ended without a receipt.".to_owned())
            } else {
                error.map(str::to_owned)
            };
            self.cloud_upload_reply(callback, ok.cloned(), error);
        }
        true
    }
    pub(crate) fn paste_cloud_attachments(
        &mut self,
        ticket: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        self.cloud.target.as_ref()?;
        Some(self.cloud.child.clone().is_some_and(|child| {
            child.update(cx, |view, cx| view.paste_attachments(ticket, window, cx))
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn upload_admission_fences_environment_connection_and_selection_changes(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut received) = futures::channel::mpsc::unbounded();
        let (_send, events) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(Transport { requests, events }, window, cx)
        });
        window.update(cx,|view,_,cx|{
            view.connected=true;
            view.cloud.viewport="chosen".into();
            view.cloud.target=Some(json!({"workspaceId":"workspace-a"}));
            view.cloud.status=json!({"state":"ready"});
            view.cloud.connected_epoch=Some(3);
            let (events,mut delivered)=futures::channel::mpsc::unbounded();
            view.cloud.events=Some(events);
            view.receive(json!({"cloudViewport":"chosen","uploading":1,"epoch":3,"generation":7}),cx);
            let delivery=delivered.try_recv().unwrap();
            assert_eq!(delivery["uploading"],1);
            assert!(delivery.get("cloudViewport").is_none(),"Routing metadata must be removed before guest delivery");
            let request=json!({"id":"callback","viewport":"chosen","workspaceId":"workspace-a","epoch":3,"method":methods::UPLOAD_CHUNK,"params":{"uploadId":"file","seq":0,"data":"Ynl0ZXM="}});
            for (field,value) in [("workspaceId",json!("workspace-b")),("viewport",json!("old-selection")),("epoch",json!(2)),("method",json!(methods::WRITE_WORKSPACE_FILE))] {
                let mut wrong=request.clone();wrong[field]=value;
                assert!(view.receive_cloud_upload(&json!({"cloudUpload":wrong}),cx));
            }
            assert!(view.cloud.uploads.is_empty());
            assert!(view.receive_cloud_upload(&json!({"cloudUpload":request}),cx));
            assert_eq!(view.cloud.uploads.len(),1);
            view.close_cloud();
            assert!(view.cloud.uploads.is_empty());
            view.receive(json!({"cloudViewport":"chosen","attachment":{"path":"/foreign/file"},"epoch":3,"generation":0}),cx);
            assert!(view.attachments.is_empty());
        }).unwrap();
        let mut forwarded = vec![];
        let mut rejected = 0;
        while let Ok(frame) = received.try_recv() {
            if frame["method"] == methods::CLOUD_RPC {
                forwarded.push(frame.clone());
            }
            if frame["ui"] == "cloudUploadReply" && frame["err"].is_string() {
                rejected += 1;
            }
        }
        assert_eq!(forwarded.len(), 1);
        assert_eq!(forwarded[0]["params"]["workspaceId"], "workspace-a");
        assert_eq!(forwarded[0]["params"]["method"], methods::UPLOAD_CHUNK);
        assert_eq!(
            rejected, 5,
            "Four invalid admissions and the cancelled upload must fail explicitly"
        );
    }
}
