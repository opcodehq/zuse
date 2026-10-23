//! Upload bytes use the existing viewport/host cloud connection, never another socket.
use futures::{
    FutureExt,
    channel::{mpsc, oneshot},
};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::HashMap};
type Reply = Result<Value, String>;
thread_local! {static PENDING:RefCell<HashMap<String,oneshot::Sender<Reply>>>=RefCell::default();}
#[derive(Clone)]
pub(super) struct Target {
    pub workspace: String,
    pub viewport: String,
    pub epoch: u64,
}
impl Target {
    pub fn from_frame(frame: &Value) -> Option<Self> {
        Some(Self {
            workspace: frame["cloudWorkspaceId"].as_str()?.to_owned(),
            viewport: frame["cloudViewport"].as_str()?.to_owned(),
            epoch: frame["cloudEpoch"].as_u64()?,
        })
    }
    pub fn event(&self, mut frame: Value) -> Value {
        frame["cloudViewport"] = json!(self.viewport);
        frame["epoch"] = json!(self.epoch);
        frame
    }
    pub async fn call(
        &self,
        events: &mpsc::UnboundedSender<Value>,
        method: &str,
        params: Value,
    ) -> Reply {
        let id = uuid::Uuid::new_v4().to_string();
        let (send, receive) = oneshot::channel();
        let admitted = PENDING.with(|pending| {
            let mut pending = pending.borrow_mut();
            if pending.len() >= 64 {
                return false;
            }
            pending.insert(id.clone(), send);
            true
        });
        if !admitted {
            return Err("Too many simultaneous uploads. Attach fewer files at once.".into());
        }
        let _pending = Pending(id.clone());
        events.unbounded_send(json!({"cloudUpload":{"id":id,"workspaceId":self.workspace,"viewport":self.viewport,"epoch":self.epoch,"method":method,"params":params}})).map_err(|_|"Cloud upload connection closed")?;
        futures::select_biased! {
            reply=receive.fuse()=>reply.map_err(|_|"Cloud upload connection closed".to_owned())?,
            _=super::connection::delay(30_000).fuse()=>Err("Cloud upload timed out. Attach the file again after reconnecting.".into()),
        }
    }
}
struct Pending(String);
impl Drop for Pending {
    fn drop(&mut self) {
        PENDING.with(|pending| pending.borrow_mut().remove(&self.0));
    }
}
pub(super) fn handle(frame: &Value) -> bool {
    if frame["ui"] != "cloudUploadReply" {
        return false;
    }
    let Some(id) = frame["callback"].as_str() else {
        return true;
    };
    if let Some(reply) = PENDING.with(|pending| pending.borrow_mut().remove(id)) {
        let result = match frame["err"].as_str() {
            Some(error) => Err(error.to_owned()),
            None => frame
                .get("ok")
                .cloned()
                .ok_or_else(|| "Invalid cloud upload response".into()),
        };
        let _ = reply.send(result);
    }
    true
}
