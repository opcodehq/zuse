use super::super::remote_connection::Source as _;
use super::*;
use futures::{StreamExt, stream};
use zuse_rpc::{RpcReply, RpcService};

struct Controller {
    account: watch::Sender<Value>,
    calls: std::sync::Mutex<Vec<Value>>,
}
#[async_trait]
impl RpcService for Controller {
    async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::AUTH_STATUS => {
                let rx = self.account.subscribe();
                let initial = rx.borrow().clone();
                Ok(RpcReply::Stream(Box::pin(
                    stream::once(async move { initial }).chain(stream::unfold(
                        rx,
                        |mut rx| async move {
                            rx.changed().await.ok()?;
                            let value = rx.borrow().clone();
                            Some((value, rx))
                        },
                    )),
                )))
            }
            methods::CLOUD_ENVIRONMENT => {
                assert_eq!(params["workspaceId"], "workspace-one");
                Ok(RpcReply::Stream(Box::pin(
                    stream::once(async { json!({"state":"ready","epoch":1}) })
                        .chain(stream::pending()),
                )))
            }
            methods::CLOUD_RPC => {
                assert_eq!(params["workspaceId"], "workspace-one");
                self.calls.lock().unwrap().push(params.clone());
                Ok(RpcReply::Stream(Box::pin(stream::iter([
                    json!({"id":params["requestId"],"item":null}),
                    json!({"id":params["requestId"],"done":true}),
                ]))))
            }
            methods::CLOUD_ACCOUNT_SIGN_OUT => RpcReply::value(&json!({"root":true})),
            _ => Err(RpcError::UnknownMethod(method.into())),
        }
    }
}
#[tokio::test]
async fn native_cloud_tools_share_workspace_authority_and_end_on_account_change() {
    let (account, _) = watch::channel(
        json!({"state":"signedIn","user":{"id":"owner","email":"owner@example.test","name":null},"orgId":null}),
    );
    let root = Arc::new(Controller {
        account,
        calls: Default::default(),
    });
    let target = CloudTarget {
        controller: EngineHandle::from_test_client(zuse_rpc::memory_client(root.clone())),
        subject: "owner".into(),
        workspace: "workspace-one".into(),
        initial_session: "session-one".into(),
    };
    let source = Source::open(target.clone()).await.unwrap();
    let (mut channel, origin) = source.clone().connect().await.unwrap();
    assert_eq!(origin, "cloud:workspace-one");
    channel
        .outgoing
        .send(json!({"id":42,"method":"ReadFile","params":{"path":"a.txt"}}).to_string())
        .await
        .unwrap();
    let first: Value = serde_json::from_str(&channel.incoming.recv().await.unwrap()).unwrap();
    assert_eq!(first, json!({"id":42,"item":null}));
    let done: Value = serde_json::from_str(&channel.incoming.recv().await.unwrap()).unwrap();
    assert_eq!(done, json!({"id":42,"done":true}));
    channel
        .outgoing
        .send(json!({"id":43,"method":methods::CLOUD_ACCOUNT_SIGN_OUT,"params":{}}).to_string())
        .await
        .unwrap();
    let reply: Value = serde_json::from_str(&channel.incoming.recv().await.unwrap()).unwrap();
    assert_eq!(reply["id"], 43);
    assert_eq!(reply["ok"]["root"], true);
    assert_eq!(root.calls.lock().unwrap().len(), 1);
    root.account.send_replace(json!({"state":"signedOut"}));
    tokio::time::timeout(std::time::Duration::from_secs(2), source.closed.cancelled())
        .await
        .unwrap();
    assert!(Source::open(target).await.is_err());
}
