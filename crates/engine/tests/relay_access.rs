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
        methods::RESUME_RELAY_ACCESS,
        methods::SET_RELAY_AUTO_START,
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

fn local_core(root: &std::path::Path) -> EngineCore {
    EngineCore::assemble_with_profile(
        zuse_engine::EngineProfile::local(root).unwrap(),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap()
}

#[tokio::test]
async fn saved_startup_survives_cold_core_reopen_and_broker_outage_without_new_grants() {
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("engine");
    let credential = root.path().join("private-host-token");
    let secret = BrowserServer::random_token();
    std::fs::write(&credential, &secret).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = format!("http://{address}");
    let stop = tokio_util::sync::CancellationToken::new();
    let broker = tokio::spawn(
        RelayBroker::new(HostToken::parse(secret.clone()).unwrap()).serve_with_shutdown(
            listener,
            root.path().into(),
            None,
            stop.clone().cancelled_owned(),
        ),
    );
    let core = local_core(&profile);
    let client = zuse_rpc::memory_client(core.rpc_service());
    assert!(
        client
            .call(methods::SET_RELAY_AUTO_START, json!({"enabled":true}))
            .await
            .is_err()
    );
    client
        .call(
            methods::START_RELAY_ACCESS,
            json!({"origin":origin,"hostTokenFile":credential}),
        )
        .await
        .unwrap();
    assert_eq!(status(&client).await["autoStart"], false);
    client
        .call(methods::SET_RELAY_AUTO_START, json!({"enabled":true}))
        .await
        .unwrap();
    let invitation = client
        .call(methods::CREATE_PAIRING, json!({"label":"Persisted client"}))
        .await
        .unwrap();
    let target = zuse_remote::PairedTarget::new(
        &origin,
        zuse_remote::Invitation::parse_fragment(invitation["fragment"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    let remote = paired(&target).await;
    let device = remote.call(methods::LOCAL_DEVICE, json!({})).await.unwrap()["deviceId"].clone();
    let file = profile.join("profiles/local/relay-access.json");
    let bytes = std::fs::read(&file).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains(&secret));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let stopped = client
        .call(methods::STOP_RELAY_ACCESS, json!({}))
        .await
        .unwrap();
    assert_eq!(
        stopped["autoStart"], true,
        "stop preserves the next-launch choice"
    );
    assert_eq!(stopped["configured"], true);
    drop(remote);
    core.shutdown().await;
    drop(client);
    drop(core);

    let core = local_core(&profile);
    let access = core.relay_access.control();
    access.restore_on_startup(core.rpc_service()).await;
    let client = zuse_rpc::memory_client(core.rpc_service());
    let remote = paired(&target).await;
    assert_eq!(
        remote.call(methods::LOCAL_DEVICE, json!({})).await.unwrap()["deviceId"],
        device
    );
    assert_eq!(status(&client).await["autoStart"], true);
    stop.cancel();
    broker.await.unwrap().unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(3),
            remote.call(methods::LOCAL_DEVICE, json!({}))
        )
        .await
        .unwrap()
        .is_err()
    );
    drop(remote);
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let stop = tokio_util::sync::CancellationToken::new();
    let broker = tokio::spawn(
        RelayBroker::new(HostToken::parse(secret).unwrap()).serve_with_shutdown(
            listener,
            root.path().into(),
            None,
            stop.clone().cancelled_owned(),
        ),
    );
    let remote = paired(&target).await;
    assert_eq!(
        remote.call(methods::LOCAL_DEVICE, json!({})).await.unwrap()["deviceId"],
        device
    );
    client
        .call(methods::STOP_RELAY_ACCESS, json!({}))
        .await
        .unwrap();
    std::fs::remove_file(&credential).unwrap();
    access.restore_on_startup(core.rpc_service()).await;
    let failed = status(&client).await;
    assert_eq!(failed["running"], false);
    assert_eq!(failed["autoStart"], true);
    assert!(
        failed["error"]
            .as_str()
            .unwrap()
            .contains("Automatic relay startup failed")
    );
    assert!(!failed.to_string().contains("private-host-token"));
    client
        .call(methods::SET_RELAY_AUTO_START, json!({"enabled":false}))
        .await
        .unwrap();
    assert_eq!(status(&client).await["autoStart"], false);
    drop(remote);
    core.shutdown().await;
    drop(client);
    drop(core);
    let core = local_core(&profile);
    core.relay_access
        .control()
        .restore_on_startup(core.rpc_service())
        .await;
    let client = zuse_rpc::memory_client(core.rpc_service());
    assert_eq!(status(&client).await["running"], false);
    assert_eq!(status(&client).await["autoStart"], false);
    core.shutdown().await;
    stop.cancel();
    broker.await.unwrap().unwrap();
}

#[tokio::test]
async fn malformed_or_foreign_saved_configuration_keeps_local_engine_available() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("profiles/local");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(
        store.join("relay-access.json"),
        serde_json::to_vec(&json!({
            "version":1,"deviceId":"foreign","origin":"https://relay.example",
            "hostTokenFile":"/private/credential","autoStart":true
        }))
        .unwrap(),
    )
    .unwrap();
    let core = local_core(root.path());
    core.relay_access
        .control()
        .restore_on_startup(core.rpc_service())
        .await;
    let client = zuse_rpc::memory_client(core.rpc_service());
    let state = status(&client).await;
    assert_eq!(state["configured"], false);
    assert_eq!(state["running"], false);
    assert!(
        state["error"]
            .as_str()
            .unwrap()
            .contains("could not be loaded")
    );
    assert!(!state.to_string().contains("/private/credential"));
    assert!(client.call(methods::LOCAL_DEVICE, json!({})).await.is_ok());
    assert!(
        client
            .call(methods::RESUME_RELAY_ACCESS, json!({}))
            .await
            .is_err()
    );
    core.shutdown().await;
}

#[tokio::test]
async fn concurrent_attachment_connections_preserve_chat_and_shared_session_limit() {
    use base64::Engine as _;
    let root = tempfile::tempdir().unwrap();
    let credential = root.path().join("host-token");
    let secret = BrowserServer::random_token();
    std::fs::write(&credential, &secret).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let stop = tokio_util::sync::CancellationToken::new();
    let broker = tokio::spawn(
        RelayBroker::new(HostToken::parse(secret).unwrap()).serve_with_shutdown(
            listener,
            root.path().into(),
            None,
            stop.clone().cancelled_owned(),
        ),
    );
    let core = local_core(&root.path().join("engine"));
    let client = zuse_rpc::memory_client(core.rpc_service());
    client
        .call(
            methods::START_RELAY_ACCESS,
            json!({"origin":origin,"hostTokenFile":credential}),
        )
        .await
        .unwrap();
    let invitation = client
        .call(
            methods::CREATE_PAIRING,
            json!({"label":"Concurrent attachments"}),
        )
        .await
        .unwrap();
    let target = zuse_remote::PairedTarget::new(
        &origin,
        zuse_remote::Invitation::parse_fragment(invitation["fragment"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    let chat = paired(&target).await;
    let mut watch = chat
        .subscribe_scoped(methods::WATCH_CHATS, json!({}))
        .await
        .unwrap();
    assert!(watch.recv().await.is_some());
    let barrier = Arc::new(tokio::sync::Barrier::new(16));
    let uploads=futures::future::join_all((0..16).map(|index|{
        let target=target.clone();let barrier=barrier.clone();
        async move {
            let connection=zuse_rpc::connect_paired(&target).await.unwrap();
            barrier.wait().await;
            let bytes=vec![index as u8;45_000];
            let id=format!("{index:08}attachment");
            connection.call(methods::UPLOAD_CHUNK,json!({"uploadId":id,"seq":0,"data":base64::engine::general_purpose::STANDARD.encode(&bytes)})).await.unwrap();
            let receipt=connection.call(methods::UPLOAD_COMMIT,json!({"uploadId":id,"fileName":format!("file-{index}.bin")})).await.unwrap();
            let path=receipt["path"].as_str().unwrap().to_owned();
            assert_eq!(std::fs::read(&path).unwrap(),bytes);
            (connection,path)
        }
    })).await;
    assert_eq!(uploads.len(), 16);
    assert_eq!(status(&client).await["sessions"], 17);
    let slots = core.browser_access.control().pairing_slots();
    assert_eq!(slots.available_permits(), 47);
    let remaining = slots.clone().acquire_many_owned(47).await.unwrap();
    assert!(
        zuse_rpc::connect_paired(&target).await.is_err(),
        "relay must share direct admission instead of growing the host limit"
    );
    assert_eq!(
        chat.call(methods::LOCAL_DEVICE, json!({})).await.unwrap()["deviceId"],
        core.device_id
    );
    drop(remaining);
    drop(uploads);
    tokio::time::timeout(Duration::from_secs(3), async {
        while slots.available_permits() != 63 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(status(&client).await["sessions"], 1);
    assert!(
        status(&client).await["sockets"].as_u64().unwrap() <= 5,
        "completed uploads should not retain extra standby workers"
    );
    client
        .call(
            methods::REVOKE_PAIRING,
            json!({"pairId":invitation["peer"]["id"]}),
        )
        .await
        .unwrap();
    drop(watch);
    drop(chat);
    core.shutdown().await;
    stop.cancel();
    broker.await.unwrap().unwrap();
}
