//! Host-scoped configuration reads and conditional writes for native clients.
use crate::{RpcClient, RpcError, methods};
use serde_json::{Value, json};
use zuse_protocol::ChatConfig;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChatConfigTarget {
    pub chat_id: String,
    pub device_id: String,
}

impl ChatConfigTarget {
    pub fn params(&self) -> Value {
        json!({"chatId":self.chat_id,"targetDeviceId":self.device_id})
    }

    pub fn decode(&self, value: Value) -> Result<Option<ChatConfig>, RpcError> {
        if value["chatId"] != self.chat_id || value["deviceId"] != self.device_id {
            return Err(RpcError::Failed(
                "The chat or execution host changed. Refresh its configuration.".into(),
            ));
        }
        let config = value
            .get("config")
            .ok_or_else(|| RpcError::Failed("The host omitted the chat configuration.".into()))?;
        if config.is_null() {
            return Ok(None);
        }
        serde_json::from_value(config.clone())
            .map(Some)
            .map_err(|_| {
                RpcError::Failed("The host returned an unsupported chat configuration.".into())
            })
    }
}

impl RpcClient {
    pub async fn read_chat_config(
        &self,
        target: &ChatConfigTarget,
    ) -> Result<Option<ChatConfig>, RpcError> {
        let mut watch = self
            .subscribe_checked(methods::WATCH_CHAT_CONFIG, target.params())
            .await?;
        let value = watch.recv().await.ok_or(RpcError::Closed)?;
        target.decode(value)
    }

    /// No retry: an interrupted receipt does not prove that a write failed.
    pub async fn compare_set_chat_config(
        &self,
        target: &ChatConfigTarget,
        expected: &Option<ChatConfig>,
        config: &ChatConfig,
    ) -> Result<(), RpcError> {
        let receipt = self.call(methods::MUTATE, json!({
            "op":"setChatConfig", "chatId":target.chat_id, "targetDeviceId":target.device_id,
            "expectedConfig":{"deviceId":target.device_id,"config":expected}, "config":config
        })).await?;
        if receipt["ok"] != true {
            return Err(RpcError::Failed(
                "The configuration write was not acknowledged.".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshots_require_the_exact_chat_host_and_configuration_field() {
        let target = ChatConfigTarget {
            chat_id: "chat".into(),
            device_id: "host".into(),
        };
        assert_eq!(
            target
                .decode(json!({"chatId":"chat","deviceId":"host","config":null}))
                .unwrap(),
            None
        );
        for value in [
            json!({"chatId":"chat","deviceId":"foreign","config":null}),
            json!({"chatId":"other","deviceId":"host","config":null}),
            json!({"chatId":"chat","deviceId":"host"}),
            json!({"chatId":"chat","deviceId":"host","config":{"harness":"future-agent"}}),
        ] {
            assert!(target.decode(value).is_err());
        }
    }
    struct ContractHost {
        requests: std::sync::Mutex<Vec<(String, Value)>>,
        malformed: std::sync::atomic::AtomicBool,
    }
    #[async_trait::async_trait]
    impl crate::RpcService for ContractHost {
        async fn handle(&self, method: &str, params: Value) -> Result<crate::RpcReply, RpcError> {
            use futures::StreamExt;
            self.requests
                .lock()
                .unwrap()
                .push((method.into(), params.clone()));
            match method {
                methods::WATCH_CHAT_CONFIG => Ok(crate::RpcReply::Stream(futures::stream::once(async move {
                    json!({"chatId":params["chatId"],"deviceId":params["targetDeviceId"],"config":null})
                }).boxed())),
                methods::MUTATE => Ok(crate::RpcReply::Value(if self.malformed.load(std::sync::atomic::Ordering::SeqCst) { json!({}) } else { json!({"ok":true}) })),
                _ => Err(RpcError::UnknownMethod(method.into())),
            }
        }
    }
    #[tokio::test]
    async fn native_config_transport_scopes_reads_and_writes_and_never_retries_bad_receipts() {
        let host = std::sync::Arc::new(ContractHost {
            requests: Default::default(),
            malformed: false.into(),
        });
        let client = crate::memory_client(host.clone());
        let target = ChatConfigTarget {
            chat_id: "chat".into(),
            device_id: "execution-host".into(),
        };
        assert!(client.read_chat_config(&target).await.unwrap().is_none());
        let config: ChatConfig = serde_json::from_value(
            json!({"harness":"codex","sandbox":"auto","plan":true,"model":"chosen"}),
        )
        .unwrap();
        client
            .compare_set_chat_config(&target, &None, &config)
            .await
            .unwrap();
        host.malformed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            client
                .compare_set_chat_config(&target, &Some(config.clone()), &config)
                .await
                .is_err()
        );
        let requests = host.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        for (_, request) in requests.iter() {
            assert_eq!(request["chatId"], "chat");
            assert_eq!(request["targetDeviceId"], "execution-host");
        }
        assert_eq!(
            requests[1].1["expectedConfig"],
            json!({"deviceId":"execution-host","config":null})
        );
        assert_eq!(requests[2].1["config"]["sandbox"], "read-only");
    }
}
