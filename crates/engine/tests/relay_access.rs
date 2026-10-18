//! Real broker, engine-owned connector and concurrent encrypted clients.
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use zuse_engine::{EngineCore, HarnessRegistry};
use zuse_protocol::HarnessId;
use zuse_rpc::{RpcClient, methods};
use zuse_serve::{
    BrowserServer,
    relay::{HostToken, RelayBroker},
};

async fn paired(target: &zuse_remote::PairedTarget) -> RpcClient {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Ok(client) = zuse_rpc::connect_paired(target).await {
                return client;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("connector did not register")
}
async fn status(client: &RpcClient) -> Value {
    client
        .call(methods::RELAY_ACCESS_STATUS, json!({}))
        .await
        .unwrap()
}
#[tokio::test]
async fn settings_relay_owns_concurrent_sessions_stop_restart_revocation_and_owner_drop() {
    let root = tempfile::tempdir().unwrap();
    let secret = BrowserServer::random_token();
    let path = root.path().join("host-token");
    std::fs::write(&path, &secret).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let stop = tokio_util::sync::CancellationToken::new();
    let broker = tokio::spawn(
        RelayBroker::new(HostToken::parse(secret.clone()).unwrap()).serve_with_shutdown(
            listener,
            root.path().into(),
            None,
            stop.clone().cancelled_owned(),
        ),
    );
    let core = EngineCore::assemble(
        &root.path().join("profile"),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let client = zuse_rpc::memory_client(core.rpc_service());
    let start = json!({"origin":origin,"hostTokenFile":path});
    assert_eq!(
        client
            .call(methods::START_RELAY_ACCESS, start.clone())
            .await
            .unwrap()["running"],
        true
    );
    assert!(
        client
            .call(methods::START_RELAY_ACCESS, start.clone())
            .await
            .is_err()
    );
    let invitation = client
        .call(methods::CREATE_PAIRING, json!({"label":"Remote viewport"}))
        .await
        .unwrap();
    assert_eq!(invitation["origin"], origin);
    let target = zuse_remote::PairedTarget::new(
        &origin,
        zuse_remote::Invitation::parse_fragment(invitation["fragment"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    let first = paired(&target).await;
    let second = paired(&target).await;
    let mut stream = first
        .subscribe_scoped(methods::WATCH_CHATS, json!({}))
        .await
        .unwrap();
    assert!(stream.recv().await.is_some());
    assert_eq!(
        second.call(methods::LOCAL_DEVICE, json!({})).await.unwrap()["deviceId"],
        core.device_id
    );
    assert!(
        core.browser_access
            .control()
            .pairing_slots()
            .available_permits()
            <= 62
    );
    let current = status(&client).await;
    assert!(current["sessions"].as_u64().unwrap() >= 2);
    assert!(current["sockets"].as_u64().unwrap() >= 2);
    assert!(!current.to_string().contains(&secret));
    assert!(!current.to_string().contains("host-token"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        client
            .call(methods::STOP_RELAY_ACCESS, json!({}))
            .await
            .unwrap()["running"],
        false
    );
    assert!(
        tokio::time::timeout(
            Duration::from_secs(3),
            first.call(methods::LOCAL_DEVICE, json!({}))
        )
        .await
        .unwrap()
        .is_err()
    );
    assert!(
        core.pairings
            .credentials(&target.invitation().id)
            .unwrap()
            .is_live()
    );
    drop(stream);
    drop(first);
    drop(second);
    client
        .call(methods::START_RELAY_ACCESS, start.clone())
        .await
        .unwrap();
    let third = paired(&target).await;
    third.call(methods::LOCAL_DEVICE, json!({})).await.unwrap();
    client
        .call(
            methods::REVOKE_PAIRING,
            json!({"pairId":invitation["peer"]["id"]}),
        )
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(3),
            third.call(methods::LOCAL_DEVICE, json!({}))
        )
        .await
        .unwrap()
        .is_err()
    );
    let next = client
        .call(methods::CREATE_PAIRING, json!({"label":"Owner lifetime"}))
        .await
        .unwrap();
    let next = zuse_remote::PairedTarget::new(
        &origin,
        zuse_remote::Invitation::parse_fragment(next["fragment"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    let active = paired(&next).await;
    active.call(methods::LOCAL_DEVICE, json!({})).await.unwrap();
    // RPC clones outlive the engine but cannot keep its connector admitted.
    drop(core);
    assert_eq!(status(&client).await["running"], false);
    assert!(
        tokio::time::timeout(
            Duration::from_secs(3),
            active.call(methods::LOCAL_DEVICE, json!({}))
        )
        .await
        .unwrap()
        .is_err()
    );
    assert!(
        client
            .call(methods::START_RELAY_ACCESS, start)
            .await
            .is_err()
    );
    stop.cancel();
    broker.await.unwrap().unwrap();
}
#[tokio::test]
async fn relay_controls_reject_foreign_targets_and_invalid_configuration_without_admission() {
    let root = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        root.path(),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let client = zuse_rpc::memory_client(core.rpc_service());
    for method in [
        methods::RELAY_ACCESS_STATUS,
        methods::START_RELAY_ACCESS,
        methods::STOP_RELAY_ACCESS,
    ] {
        assert!(
            client
                .call(method, json!({"targetDeviceId":"foreign"}))
                .await
                .is_err()
        );
    }
    for params in [
        json!({"origin":"http://untrusted.example","hostTokenFile":"/missing"}),
        json!({"origin":"https://relay.example","hostTokenFile":"/missing"}),
        json!({"origin":"https://relay.example","hostTokenFile":"/missing","token":"must-not-be-accepted"}),
    ] {
        assert!(
            client
                .call(methods::START_RELAY_ACCESS, params)
                .await
                .is_err()
        );
    }
    assert_eq!(status(&client).await["running"], false);
    core.shutdown().await;
}
