use super::*;
use futures::{StreamExt, stream};
use serde_json::{Value, json};
use std::{
    sync::{
        RwLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use zuse_rpc::{RpcReply, RpcService};
struct Service {
    info: RwLock<EngineInfo>,
    writes: AtomicUsize,
    watches: AtomicUsize,
}
#[async_trait]
impl RpcService for Service {
    async fn handle(&self, method: &str, _: Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::ENGINE_INFO => RpcReply::value(&*self.info.read().unwrap()),
            "Write" => {
                self.writes.fetch_add(1, Ordering::SeqCst);
                futures::future::pending().await
            }
            "Read" => RpcReply::value(&"fresh"),
            "Watch" => {
                let n = self.watches.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(RpcReply::Stream(Box::pin(
                    stream::once(async move { json!(n) }).chain(stream::pending()),
                )))
            }
            _ => Err(RpcError::UnknownMethod(method.into())),
        }
    }
}
async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("Paired recovery deadline")
}
fn serve(
    listener: tokio::net::TcpListener,
    service: Arc<Service>,
    authority: zuse_remote::authority::Authority,
    root: &std::path::Path,
) -> (
    CancellationToken,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let server = zuse_serve::BrowserServer::new(service, "x".repeat(64), None)
        .unwrap()
        .with_pairings(authority, "paired-host");
    let stop = CancellationToken::new();
    let task = tokio::spawn(server.serve_with_shutdown(
        listener,
        root.into(),
        None,
        stop.clone().cancelled_owned(),
    ));
    (stop, task)
}
#[tokio::test]
async fn native_paired_reconnect_uses_fresh_encryption_without_replaying_a_write() {
    let root = tempfile::tempdir().unwrap();
    let authority = zuse_remote::authority::Authority::open(root.path()).unwrap();
    let (_, invitation) = authority.create("Recovery client").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let target = zuse_remote::PairedTarget::new(&format!("http://{address}"), invitation).unwrap();
    let service = Arc::new(Service {
        info: RwLock::new(EngineInfo {
            device_id: "paired-host".into(),
            workspace_scope: WorkspaceScope::Local,
            cursor_sdk_version: None,
            capabilities: vec![],
        }),
        writes: AtomicUsize::new(0),
        watches: AtomicUsize::new(0),
    });
    let (stop, task) = serve(listener, service.clone(), authority.clone(), root.path());
    let engine = bounded(paired(target.clone())).await.unwrap();
    let mut status = engine.inner.remote_status().unwrap();
    let mut watch = engine
        .client()
        .subscribe_scoped("Watch", json!({}))
        .await
        .unwrap();
    assert_eq!(bounded(watch.recv()).await.unwrap(), 1);
    let sender = engine.clone();
    let write = tokio::spawn(async move { sender.client().call("Write", json!({})).await });
    bounded(async {
        while service.writes.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    stop.cancel();
    bounded(task).await.unwrap().unwrap();
    assert!(
        bounded(write)
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("may have completed")
    );
    assert!(bounded(watch.recv()).await.is_none());
    assert!(
        engine
            .client()
            .call("Write", json!({}))
            .await
            .unwrap_err()
            .to_string()
            .contains("not sent")
    );
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let (stop, task) = serve(listener, service.clone(), authority.clone(), root.path());
    bounded(async {
        loop {
            if !engine.same_connection(&engine.current_connection())
                && *status.borrow_and_update() == Status::Ready
            {
                break;
            }
            status.changed().await.unwrap();
        }
    })
    .await;
    assert_eq!(
        bounded(engine.client().call("Read", json!({})))
            .await
            .unwrap(),
        "fresh"
    );
    let mut watch = engine
        .client()
        .subscribe_scoped("Watch", json!({}))
        .await
        .unwrap();
    assert_eq!(bounded(watch.recv()).await.unwrap(), 2);
    let media = bounded(engine.media_client()).await.unwrap();
    assert_eq!(
        bounded(media.call("Read", json!({}))).await.unwrap(),
        "fresh"
    );
    // Detaching this viewport also retires its auxiliary socket, without stopping
    // the host or invalidating a different client's saved authorization.
    engine.shutdown().await;
    assert!(bounded(watch.recv()).await.is_none());
    assert!(bounded(media.call("Read", json!({}))).await.is_err());
    let independent = bounded(zuse_rpc::connect_paired(&target)).await.unwrap();
    assert_eq!(
        bounded(independent.call("Read", json!({}))).await.unwrap(),
        "fresh"
    );
    drop(independent);
    let engine = bounded(paired(target)).await.unwrap();
    let mut status = engine.inner.remote_status().unwrap();
    service.info.write().unwrap().cursor_sdk_version = Some("updated".into());
    stop.cancel();
    bounded(task).await.unwrap().unwrap();
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let (stop, task) = serve(listener, service.clone(), authority, root.path());
    bounded(async {
        loop {
            if matches!(&*status.borrow_and_update(), Status::Blocked(_)) {
                break;
            }
            status.changed().await.unwrap();
        }
    })
    .await;
    assert!(!engine.retry_connection());
    assert!(bounded(engine.media_client()).await.is_err());
    assert_eq!(service.writes.load(Ordering::SeqCst), 1);
    stop.cancel();
    bounded(task).await.unwrap().unwrap();
}
