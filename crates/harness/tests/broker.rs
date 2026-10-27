use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use zuse_harness::{broker::*, *};
struct Budget(AtomicUsize);
#[async_trait]
impl RequestBudget for Budget {
    async fn reserve(&self) -> Result<(), Error> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}
struct Connections {
    calls: Mutex<Vec<String>>,
    unauthorized: bool,
}
#[async_trait]
impl BrokerHost for Connections {
    async fn connections(&self, _: &ModelInput) -> Result<Vec<Connection>, Error> {
        Ok(vec![
            Connection {
                id: "a".into(),
                authorized: true,
                preferred: true,
            },
            Connection {
                id: "b".into(),
                authorized: true,
                preferred: false,
            },
        ])
    }
    async fn request(&self, id: &str, _: ModelInput) -> Result<ModelOutput, Error> {
        self.calls.lock().unwrap().push(id.into());
        if id == "a" {
            return Err(Error::Model {
                code: if self.unauthorized {
                    "http_401"
                } else {
                    "subscription_sharing_usage_limit_exceeded"
                }
                .into(),
                status: Some(if self.unauthorized { 401 } else { 429 }),
            });
        }
        Ok(ModelOutput {
            messages: vec![],
            calls: vec![],
            text: "ok".into(),
            usage: Usage {
                connection_id: id.into(),
                ..Default::default()
            },
        })
    }
    fn switched(&self, _: &str, _: Option<&str>, _: &str) {}
}
fn input(budget: Arc<Budget>) -> ModelInput {
    ModelInput {
        root_id: "root".into(),
        agent_id: "root".into(),
        model: "model".into(),
        reasoning: None,
        messages: vec![],
        tools: Tools::new(),
        cancel: tokio_util::sync::CancellationToken::new(),
        text: Arc::new(|_| {}),
        budget,
    }
}
#[tokio::test]
async fn quota_switch_preserves_root_affinity_for_children_and_charges_retry() {
    let host = Arc::new(Connections {
        calls: Mutex::new(vec![]),
        unauthorized: false,
    });
    let broker = AccountBroker::new(host.clone());
    let budget = Arc::new(Budget(AtomicUsize::new(0)));
    assert_eq!(
        broker
            .request(input(budget.clone()))
            .await
            .unwrap()
            .usage
            .connection_id,
        "b"
    );
    let mut child = input(budget.clone());
    child.agent_id = "child".into();
    broker.request(child).await.unwrap();
    assert_eq!(*host.calls.lock().unwrap(), vec!["a", "b", "b"]);
    assert_eq!(budget.0.load(Ordering::Relaxed), 1);
}
#[tokio::test]
async fn authentication_failure_requires_reauthorization_instead_of_account_fallback() {
    let host = Arc::new(Connections {
        calls: Mutex::new(vec![]),
        unauthorized: true,
    });
    let broker = AccountBroker::new(host.clone());
    assert!(matches!(
        broker
            .request(input(Arc::new(Budget(AtomicUsize::new(0)))))
            .await,
        Err(Error::Model {
            status: Some(401),
            ..
        })
    ));
    assert_eq!(*host.calls.lock().unwrap(), vec!["a"]);
}
#[test]
fn portable_history_removes_account_bound_metadata_and_preserves_tool_outcomes() {
    let message:Message=serde_json::from_value(json!({"role":"assistant","providerOptions":{"openai":{"id":"private-a"}},"content":[{"type":"reasoning","text":"private"},{"type":"text","text":"answer","providerOptions":{"openai":{"id":"private-b"}}}]})).unwrap();
    let message = Arc::new(message);
    let portable = portable_history(std::slice::from_ref(&message));
    assert!(message.metadata.contains_key("providerOptions"));
    assert_eq!(message.content.as_array().unwrap().len(), 2);
    assert!(!Arc::ptr_eq(&message, &portable[0]));
    let again = portable_history(&portable);
    assert!(Arc::ptr_eq(&portable[0], &again[0]));
    let portable = serde_json::to_value(portable).unwrap();
    assert_eq!(portable[0]["content"].as_array().unwrap().len(), 1);
    assert_eq!(portable[0]["content"][0]["text"], "answer");
    assert_eq!(portable[0].get("providerOptions"), None);
    assert_eq!(portable[0]["content"][0].get("providerOptions"), None);
    let _: Value = portable;
}
