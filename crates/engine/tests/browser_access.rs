//! Real engine RPC owns a listener without extending the engine owner's lifetime.
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zuse_engine::{
    EngineCore, EngineProfile, HarnessRegistry,
    browser_access::{Owner, Start},
};
use zuse_protocol::HarnessId;
use zuse_rpc::methods;
fn assets(root: &std::path::Path) -> std::path::PathBuf {
    let assets = root.join("web");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("index.html"), "<title>Zuse test client</title>").unwrap();
    assets
}
#[tokio::test]
async fn settings_listener_serves_paired_rpc_and_stops_without_revoking_saved_devices() {
    let root = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble_with_profile(
        EngineProfile::local(root.path()).unwrap(),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let client = zuse_rpc::memory_client(core.rpc_service());
    let start = json!({"listen":"127.0.0.1:0","assets":assets(root.path()),"targetDeviceId":core.device_id});
    let status = client
        .call(methods::START_BROWSER_ACCESS, start.clone())
        .await
        .unwrap();
    assert_eq!(status["running"], true);
    assert_eq!(status["managed"], true);
    assert!(!status.to_string().contains("token"));
    let origin = status["origin"].as_str().unwrap();
    assert!(
        reqwest::get(format!("{origin}/health"))
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let (_, invitation) = core.pairings.create("Paired viewport").unwrap();
    let target = zuse_remote::PairedTarget::new(origin, invitation.clone()).unwrap();
    let paired = zuse_rpc::connect_paired(&target).await.unwrap();
    assert_eq!(
        paired.call(methods::LOCAL_DEVICE, json!({})).await.unwrap()["deviceId"],
        core.device_id
    );
    let mut watch = paired
        .subscribe_scoped(methods::WATCH_CHATS, json!({}))
        .await
        .unwrap();
    assert!(watch.recv().await.is_some());
    let stopped = tokio::time::timeout(
        Duration::from_secs(3),
        client.call(methods::STOP_BROWSER_ACCESS, json!({})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(stopped["running"], false);
    assert!(
        tokio::time::timeout(
            Duration::from_secs(3),
            paired.call(methods::LOCAL_DEVICE, json!({}))
        )
        .await
        .unwrap()
        .is_err()
    );
    assert!(core.pairings.credentials(&invitation.id).unwrap().is_live());
    drop(watch);
    drop(paired);
    let restarted = client
        .call(methods::START_BROWSER_ACCESS, start)
        .await
        .unwrap();
    let target =
        zuse_remote::PairedTarget::new(restarted["origin"].as_str().unwrap(), invitation).unwrap();
    let paired = zuse_rpc::connect_paired(&target).await.unwrap();
    assert!(paired.call(methods::LOCAL_DEVICE, json!({})).await.is_ok());
    core.shutdown().await;
    assert!(
        client
            .call(
                methods::START_BROWSER_ACCESS,
                json!({"listen":"127.0.0.1:0"})
            )
            .await
            .is_err()
    );
    assert_eq!(
        client
            .call(methods::BROWSER_ACCESS_STATUS, json!({}))
            .await
            .unwrap()["running"],
        false
    );
}
#[tokio::test]
async fn invalid_access_settings_and_foreign_profile_targets_do_not_open_a_listener() {
    let root = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble_with_profile(
        EngineProfile::local(root.path()).unwrap(),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let client = zuse_rpc::memory_client(core.rpc_service());
    let assets = assets(root.path());
    for params in [
        json!({"listen":"0.0.0.0:0","assets":assets}),
        json!({"listen":"127.0.0.1:0","publicOrigin":"http://engine.example","assets":assets}),
        json!({"listen":"127.0.0.1:0","tlsCert":"missing.pem","assets":assets}),
        json!({"listen":"127.0.0.1:0","assets":root.path().join("missing")}),
        json!({"listen":"127.0.0.1:0","assets":assets,"targetDeviceId":"another-host"}),
    ] {
        assert!(
            client
                .call(methods::START_BROWSER_ACCESS, params)
                .await
                .is_err()
        );
        assert_eq!(
            client
                .call(methods::BROWSER_ACCESS_STATUS, json!({}))
                .await
                .unwrap()["running"],
            false
        );
    }
    core.shutdown().await;
}
struct Echo {
    // Model the listener -> service -> controller -> listener ownership cycle.
    _control: zuse_engine::browser_access::BrowserAccess,
}
#[async_trait::async_trait]
impl zuse_rpc::RpcService for Echo {
    async fn handle(
        &self,
        _: &str,
        params: serde_json::Value,
    ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
        zuse_rpc::RpcReply::value(&params)
    }
}
#[tokio::test]
async fn dropping_the_engine_owner_closes_listener_held_rpc_cycles_and_fences_new_starts() {
    let root = tempfile::tempdir().unwrap();
    let pairings = zuse_remote::authority::Authority::open(root.path()).unwrap();
    let owner = Owner::new(root.path(), "host".into(), pairings);
    let control = owner.control();
    let service = Arc::new(Echo {
        _control: control.clone(),
    });
    let config = Start {
        listen: "127.0.0.1:0".parse().unwrap(),
        public_origin: None,
        tls_cert: None,
        tls_key: None,
        assets: Some(assets(root.path())),
    };
    let (first, second) = tokio::join!(
        control.start(config.clone(), service.clone()),
        control.start(config.clone(), service.clone())
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let status = first.or(second).unwrap();
    let address = status.listen.unwrap();
    drop(owner);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if tokio::net::TcpStream::connect(address).await.is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!control.status().await.running);
    assert!(control.start(config, service).await.is_err());
}
