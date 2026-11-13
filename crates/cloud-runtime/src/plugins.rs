//! Plugin execution uses the current renewable runtime credential, never a WorkOS session.
use crate::Authority;
use tokio::sync::watch;
use zuse_protocol::{cloud::CloudRuntimeBootstrap, plugins::PluginToolRequest};

pub struct RuntimePluginClient {
    authority: Authority,
    credentials: watch::Receiver<CloudRuntimeBootstrap>,
    workspace: String,
    owner: String,
    generation: i64,
}
impl RuntimePluginClient {
    pub fn new(authority: Authority, credentials: watch::Receiver<CloudRuntimeBootstrap>) -> Self {
        let current = credentials.borrow().clone();
        Self {
            authority,
            credentials,
            workspace: current.workspace_id,
            owner: current.zuse_account_id,
            generation: current.runtime_generation,
        }
    }
}
#[async_trait::async_trait]
impl zuse_engine::plugins::RuntimePlugins for RuntimePluginClient {
    async fn tools(&self, request: &PluginToolRequest) -> anyhow::Result<serde_json::Value> {
        anyhow::ensure!(
            self.credentials.has_changed().is_ok(),
            "runtime_plugin_supervisor_closed"
        );
        let bootstrap = self.credentials.borrow().clone();
        anyhow::ensure!(
            bootstrap.workspace_id == self.workspace
                && bootstrap.zuse_account_id == self.owner
                && bootstrap.runtime_generation == self.generation,
            "runtime_plugin_identity_changed"
        );
        self.authority.plugin_tools(&bootstrap, request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
    use zuse_engine::plugins::RuntimePlugins;
    #[tokio::test]
    async fn plugin_credentials_rotate_and_cannot_change_enrollment() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        async fn receive(
            State(seen): State<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
            headers: HeaderMap,
        ) -> Json<serde_json::Value> {
            seen.lock()
                .unwrap()
                .push(headers["authorization"].to_str().unwrap().into());
            Json(serde_json::json!([]))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = Router::new()
            .route("/v1/plugins/runtime/workspace/tools", post(receive))
            .with_state(seen.clone());
        let task = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
        let bootstrap:CloudRuntimeBootstrap=serde_json::from_value(serde_json::json!({"workspaceId":"workspace","zuseAccountId":"owner","chatId":"chat","initialSessionId":"session","runtimeCredential":"first","runtimeGeneration":1,"gatewayEpoch":1,"runtimeCredentialExpiresAt":crate::now_ms()+60_000,"gatewayUrl":"wss://api.test.example","gatewayProtocol":"fixture","rpcProtocol":"zuse-rpc-v1","sealedTranscriptKey":"fixture"})).unwrap();
        let (updates, receiver) = watch::channel(bootstrap.clone());
        let client = RuntimePluginClient::new(Authority::new(&origin).unwrap(), receiver);
        client.tools(&PluginToolRequest::List).await.unwrap();
        let mut renewed = bootstrap.clone();
        renewed.runtime_credential = "second".into();
        updates.send_replace(renewed.clone());
        client.tools(&PluginToolRequest::List).await.unwrap();
        assert_eq!(*seen.lock().unwrap(), ["Bearer first", "Bearer second"]);
        renewed.runtime_credential_expires_at = crate::now_ms() - 1;
        updates.send_replace(renewed);
        assert!(client.tools(&PluginToolRequest::List).await.is_err());
        let mut changed = bootstrap.clone();
        changed.zuse_account_id = "different-owner".into();
        updates.send_replace(changed);
        assert!(client.tools(&PluginToolRequest::List).await.is_err());
        let mut changed = bootstrap.clone();
        changed.runtime_generation = 2;
        updates.send_replace(changed);
        assert!(client.tools(&PluginToolRequest::List).await.is_err());
        updates.send_replace(bootstrap);
        drop(updates);
        assert!(client.tools(&PluginToolRequest::List).await.is_err());
        assert_eq!(seen.lock().unwrap().len(), 2);
        task.abort();
    }
}
