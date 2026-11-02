use super::*;
use crate::desktop_import::{Destination, onboarding as migration};
use serde_json::Value;

impl EngineRpc {
    pub(super) async fn desktop_migration(
        &self,
        method: &str,
        params: Value,
    ) -> Result<RpcReply, RpcError> {
        let failed = |e: String| RpcError::Failed(e);
        let profile = self.desktop_import_profile.clone().ok_or_else(|| {
            failed("Import is unavailable on this engine. Update the desktop app.".into())
        })?;
        if method == migration::DISCOVER {
            let custom = params["path"].as_str().map(std::path::PathBuf::from);
            let mut value =
                tokio::task::spawn_blocking(move || migration::discover(custom, &profile))
                    .await
                    .map_err(|e| failed(e.to_string()))?
                    .map_err(|e| failed(e.to_string()))?;
            value["destinationHasChats"] = serde_json::json!(
                !self
                    .workspace
                    .read_chats()
                    .map_err(|e| failed(e.to_string()))?
                    .is_empty()
            );
            return RpcReply::value(&value);
        }
        if self.sessions.any_active() {
            return Err(failed(
                "Finish or stop active GPUI sessions before importing.".into(),
            ));
        }
        let source_id = params["sourceId"]
            .as_str()
            .ok_or_else(|| RpcError::BadParams("Choose an Electron profile.".into()))?;
        let source = migration::resolve(source_id).map_err(|e| failed(e.to_string()))?;
        let request = uuid::Uuid::parse_str(params["requestId"].as_str().unwrap_or_default())
            .map_err(|_| RpcError::BadParams("Invalid import request ID.".into()))?;
        let include_settings = params["includeSettings"].as_bool().unwrap_or(true);
        let destination = Destination {
            workspace: self.workspace.clone(),
            doc_host: self.doc_host.clone(),
            device_id: self.doc_host.device_id().to_owned(),
            device_root: self.repos.data_dir().to_owned(),
            profile_root: profile.clone(),
        };
        let gate = migration::gate(&profile);
        let (events, receiver) = tokio::sync::mpsc::unbounded_channel();
        // Accepted work survives a lost UI connection. The same request and session
        // receipts recover the result without replaying a provider turn.
        tokio::spawn(async move {
            let _guard = gate.lock_owned().await;
            let output = events.clone();
            let result = tokio::task::spawn_blocking(move || {
                migration::run(destination, source, request, include_settings, output)
            })
            .await;
            let error = match result {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            if let Some(message) = error {
                let _ = events.send(
                    serde_json::json!({"kind":"error","message":message,"requestId":request}),
                );
            }
        });
        Ok(RpcReply::Stream(
            futures::stream::unfold(receiver, |mut receiver| async move {
                receiver.recv().await.map(|value| (value, receiver))
            })
            .boxed(),
        ))
    }
}
