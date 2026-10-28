//! The production broker and SDK bridge, with all HTTP replaced by local replay.
use async_trait::async_trait;
use std::{collections::BTreeMap, path::Path, sync::Arc};
use zuse_harness::{
    bridge::{Credential, ModelBridge},
    broker::{AccountBroker, BrokerHost, Connection},
    *,
};

struct LocalModels(Arc<ModelBridge>);

#[async_trait]
impl BrokerHost for LocalModels {
    async fn connections(&self, _: &ModelInput) -> Result<Vec<Connection>, Error> {
        Ok(vec![Connection {
            id: "local".into(),
            authorized: true,
            preferred: true,
        }])
    }
    async fn request(&self, _: &str, input: ModelInput) -> Result<ModelOutput, Error> {
        self.0
            .request(
                input,
                Credential {
                    connection_id: "local".into(),
                    provider: "chatgpt".into(),
                    access_token: "local-replay-only".into(),
                },
            )
            .await
    }
    fn switched(&self, _: &str, _: Option<&str>, _: &str) {}
}

pub fn create(directory: &Path) -> std::io::Result<(Arc<ModelBridge>, Arc<AccountBroker>)> {
    let preload = directory.join("local-fetch.mjs");
    std::fs::write(&preload, include_str!("sdk-replay.mjs"))?;
    // Node parses NODE_OPTIONS itself; no shell evaluates this path.
    let options = format!("--import={}", serde_json::to_string(&preload).unwrap());
    let bridge = Arc::new(
        ModelBridge::new(directory.join("sdk")).with_environment_source(Arc::new(move || {
            BTreeMap::from([("NODE_OPTIONS".into(), options.clone().into())])
        })),
    );
    let broker = Arc::new(AccountBroker::new(Arc::new(LocalModels(bridge.clone()))));
    Ok((bridge, broker))
}
