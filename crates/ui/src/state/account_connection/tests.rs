use super::*;
use futures::{StreamExt, stream};
use serde_json::{Value, json};
use std::sync::{
    RwLock,
    atomic::{AtomicUsize, Ordering},
};
use zuse_rpc::{RpcReply, RpcService};

struct Host {
    generation: RwLock<CancellationToken>,
    writes: AtomicUsize,
    watches: AtomicUsize,
    changed: std::sync::atomic::AtomicBool,
}
fn info() -> EngineInfo {
    EngineInfo {
        device_id: "account-test-host".into(),
        workspace_scope: WorkspaceScope::Local,
        cursor_sdk_version: None,
        capabilities: vec![],
    }
}
impl zuse_serve::managed::Authority for Host {
    fn authorize(&self, token: &str) -> Option<zuse_serve::managed::Authorization> {
        if token != "account-test-grant" {
            return None;
        }
        Some(zuse_serve::managed::Authorization {
            device_id: info().device_id,
            subject: "account-test-user".into(),
            expires_at: chrono::Utc::now().timestamp() + 60,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(60),
            account_closed: CancellationToken::new(),
            host_closed: self.generation.read().unwrap().clone(),
        })
    }
}
#[async_trait]
impl RpcService for Host {
    async fn handle(&self, method: &str, _: Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::ENGINE_INFO => {
                let mut info = info();
                if self.changed.load(Ordering::SeqCst) {
                    info.cursor_sdk_version = Some("updated".into());
                }
                RpcReply::value(&info)
            }
            "Watch" => {
                let n = self.watches.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(RpcReply::Stream(Box::pin(
                    stream::once(async move { json!(n) }).chain(stream::pending()),
                )))
            }
            "Write" => {
                self.writes.fetch_add(1, Ordering::SeqCst);
                futures::future::pending().await
            }
            "Read" => RpcReply::value(&"fresh"),
            _ => Err(RpcError::UnknownMethod(method.into())),
        }
    }
}
struct Controller {
    origin: RwLock<String>,
    remembered: std::sync::Mutex<Vec<String>>,
    account: tokio::sync::watch::Sender<Value>,
    grants: AtomicUsize,
}
#[async_trait]
impl RpcService for Controller {
    async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::CLOUD_ACCOUNT_STATUS => {
                let rx = self.account.subscribe();
                let first = rx.borrow().clone();
                let changes = stream::unfold(rx, |mut rx| async move {
                    rx.changed().await.ok()?;
                    let next = rx.borrow().clone();
                    Some((next, rx))
                });
                Ok(RpcReply::Stream(Box::pin(
                    stream::once(async move { first }).chain(changes),
                )))
            }
            methods::BOOTSTRAP_ACCOUNT_CONNECTION => {
                assert_eq!(params["expectedSubject"], "account-test-user");
                assert_eq!(params["environmentId"], info().device_id);
                self.grants.fetch_add(1, Ordering::SeqCst);
                RpcReply::value(
                    &json!({"environmentId":info().device_id,"origin":self.origin.read().unwrap().clone(),"connectToken":"account-test-grant","expiresAt":chrono::Utc::now().timestamp_millis()+60000}),
                )
            }
            methods::REMEMBER_ACCOUNT_ROUTE => {
                self.remembered
                    .lock()
                    .unwrap()
                    .push(params["origin"].as_str().unwrap().to_owned());
                RpcReply::value(&json!({"computers":[]}))
            }
            _ => Err(RpcError::UnknownMethod(method.into())),
        }
    }
}
#[tokio::test]
async fn native_account_reconnect_preserves_handle_drops_lost_write_and_stops_on_account_change() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let host = Arc::new(Host {
        generation: RwLock::new(CancellationToken::new()),
        writes: AtomicUsize::new(0),
        watches: AtomicUsize::new(0),
        changed: std::sync::atomic::AtomicBool::new(false),
    });
    let server = zuse_serve::BrowserServer::new(host.clone(), "x".repeat(64), None)
        .unwrap()
        .with_managed_authority(host.clone());
    let stop = CancellationToken::new();
    let task = tokio::spawn(server.serve_with_shutdown(
        listener,
        root.path().into(),
        None,
        stop.clone().cancelled_owned(),
    ));
    let (account, _) = tokio::sync::watch::channel(json!({"user":{"id":"account-test-user"}}));
    let controller = Arc::new(Controller {
        origin: RwLock::new(origin),
        remembered: std::sync::Mutex::new(vec![]),
        account,
        grants: AtomicUsize::new(0),
    });
    let target = AccountTarget {
        controller: EngineHandle::from_test_client(zuse_rpc::memory_client(controller.clone())),
        subject: "account-test-user".into(),
        environment: info().device_id,
        label: "Account host".into(),
    };
    let engine = bootstrap(target.clone()).await.unwrap();
    let original = engine.clone();
    let mut status = engine.inner.remote_status().unwrap();
    let mut watch = engine
        .client()
        .subscribe_scoped("Watch", json!({}))
        .await
        .unwrap();
    assert_eq!(watch.recv().await.unwrap(), 1);
    let handle = engine.clone();
    let write = tokio::spawn(async move { handle.client().call("Write", json!({})).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while host.writes.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let old = std::mem::replace(
        &mut *host.generation.write().unwrap(),
        CancellationToken::new(),
    );
    old.cancel();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), write)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("may have completed")
    );
    assert!(watch.recv().await.is_none());
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if controller.grants.load(Ordering::SeqCst) >= 2
                && *status.borrow_and_update() == Status::Ready
            {
                break;
            }
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(engine.same_connection(&original));
    let refreshed = engine.current_connection();
    assert!(engine.same_engine(&refreshed));
    assert!(!engine.same_connection(&refreshed));
    assert!(std::ptr::eq(engine.client(), refreshed.client()));
    // Simulate a blocked UI thread that coalesces Reconnecting into Ready.
    // Cached binding keys and configuration/alert readiness must still retire.
    let mut view = AppState::new();
    view.engine = Some(original.clone());
    view.connection = ConnectionStatus::Ready;
    view.connectivity_observed = true;
    view.selected_chat = Some("retained-chat".into());
    assert!(view.apply_remote_status(&original, &Status::Ready));
    assert!(!view.connectivity_observed);
    assert_eq!(view.selected_chat.as_deref(), Some("retained-chat"));
    assert!(view.engine().unwrap().same_connection(&refreshed));
    assert_eq!(
        engine.client().call("Read", json!({})).await.unwrap(),
        "fresh"
    );
    let mut watch = original
        .client()
        .subscribe_scoped("Watch", json!({}))
        .await
        .unwrap();
    assert_eq!(watch.recv().await.unwrap(), 2);
    assert_eq!(host.writes.load(Ordering::SeqCst), 1);
    assert_eq!(controller.grants.load(Ordering::SeqCst), 2);
    // A changed runtime must retire auxiliary access as well as the main
    // channel; reopening explicitly captures the new EngineInfo contract.
    host.changed.store(true, Ordering::SeqCst);
    let old = std::mem::replace(
        &mut *host.generation.write().unwrap(),
        CancellationToken::new(),
    );
    old.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if matches!(&*status.borrow_and_update(), Status::Blocked(_)) {
                break;
            }
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(!engine.retry_connection());
    assert!(engine.media_client().await.is_err());
    assert_eq!(controller.grants.load(Ordering::SeqCst), 3);
    let engine = bootstrap(target).await.unwrap();
    status = engine.inner.remote_status().unwrap();
    let mut watch = engine
        .client()
        .subscribe_scoped("Watch", json!({}))
        .await
        .unwrap();
    assert_eq!(watch.recv().await.unwrap(), 3);
    controller
        .account
        .send_replace(json!({"user":{"id":"another-account"}}));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if matches!(&*status.borrow_and_update(), Status::Blocked(_)) {
                break;
            }
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(watch.recv().await.is_none());
    assert!(!engine.retry_connection());
    assert!(
        engine
            .client()
            .call("Write", json!({}))
            .await
            .unwrap_err()
            .to_string()
            .contains("not sent")
    );
    assert_eq!(host.writes.load(Ordering::SeqCst), 1);
    assert_eq!(controller.grants.load(Ordering::SeqCst), 4);
    engine.shutdown().await;
    stop.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn account_verified_main_endpoint_is_not_changed_by_auxiliary_grants() {
    async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(std::time::Duration::from_secs(5), future)
            .await
            .unwrap()
    }
    let root = tempfile::tempdir().unwrap();
    let host = Arc::new(Host {
        generation: RwLock::new(CancellationToken::new()),
        writes: AtomicUsize::new(0),
        watches: AtomicUsize::new(0),
        changed: std::sync::atomic::AtomicBool::new(false),
    });
    let mut servers = Vec::new();
    let mut origins = Vec::new();
    for _ in 0..2 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        origins.push(format!("http://{}", listener.local_addr().unwrap()));
        let server = zuse_serve::BrowserServer::new(host.clone(), "x".repeat(64), None)
            .unwrap()
            .with_managed_authority(host.clone());
        let stop = CancellationToken::new();
        let task = tokio::spawn(server.serve_with_shutdown(
            listener,
            root.path().into(),
            None,
            stop.clone().cancelled_owned(),
        ));
        servers.push((stop, task));
    }
    let (account, _) = tokio::sync::watch::channel(json!({"user":{"id":"account-test-user"}}));
    let controller = Arc::new(Controller {
        origin: RwLock::new(origins[0].clone()),
        remembered: std::sync::Mutex::new(vec![]),
        account,
        grants: AtomicUsize::new(0),
    });
    let target = AccountTarget {
        controller: EngineHandle::from_test_client(zuse_rpc::memory_client(controller.clone())),
        subject: "account-test-user".into(),
        environment: info().device_id,
        label: "Route host".into(),
    };
    let engine = bounded(bootstrap(target)).await.unwrap();
    let mut status = engine.inner.remote_status().unwrap();
    assert_eq!(
        engine.mode(),
        EngineMode::Remote {
            url: origins[0].clone()
        }
    );
    *controller.origin.write().unwrap() = origins[1].clone();
    let media = bounded(engine.media_client()).await.unwrap();
    assert_eq!(
        bounded(media.call("Read", json!({}))).await.unwrap(),
        "fresh"
    );
    assert_eq!(
        engine.mode(),
        EngineMode::Remote {
            url: origins[0].clone()
        }
    );
    assert_eq!(
        *controller.remembered.lock().unwrap(),
        vec![origins[0].clone()]
    );
    drop(media);
    let old = std::mem::replace(
        &mut *host.generation.write().unwrap(),
        CancellationToken::new(),
    );
    old.cancel();
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
        engine.mode(),
        EngineMode::Remote {
            url: origins[1].clone()
        }
    );
    *controller.origin.write().unwrap() = origins[0].clone();
    let media = bounded(engine.media_client()).await.unwrap();
    assert_eq!(
        bounded(media.call("Read", json!({}))).await.unwrap(),
        "fresh"
    );
    assert_eq!(
        engine.mode(),
        EngineMode::Remote {
            url: origins[1].clone()
        }
    );
    assert_eq!(*controller.remembered.lock().unwrap(), origins);
    assert_eq!(host.writes.load(Ordering::SeqCst), 0);
    engine.shutdown().await;
    drop(media);
    for (stop, task) in servers {
        stop.cancel();
        bounded(task).await.unwrap().unwrap();
    }
}
