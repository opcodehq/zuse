use super::*;

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn bootstrap_storage_bind_is_idempotent_and_fenced_on_gateway_commit() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::SigningKey;
    use sha2::{Digest, Sha256};
    use zuse_cloud::{crypto::MasterKey, runtime::RuntimeControl, tickets::TicketSigner};
    use zuse_protocol::cloud::{CloudStorageEnrollment, STARTUP_BIND_CAPABILITY};
    use zuse_protocol::cloud_gateway::{RUST_PROTOCOL, RUST_RPC};
    let (admin, pool, schema) = fixture().await;
    let crypto: Value =
        serde_json::from_str(include_str!("../fixtures/cloud-crypto.json")).unwrap();
    let master = MasterKey::parse(crypto["masterKey"].as_str().unwrap()).unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let key = SigningKey::from_bytes(&[91; 32]);
    let jwk=json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([91;32]),"x":URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes())}).to_string();
    let signer = TicketSigner::from_jwk(&jwk, "https://api.example.test").unwrap();
    let browser=signer.client(json!({"sub":"user-bob","actorId":"user-bob","permission":"edit","generation":2,"gatewayEpoch":5,"protocol":RUST_PROTOCOL,"rpcProtocol":RUST_RPC}),"ws-bob",now).unwrap();
    let boot = "startup-bind-fixture";
    let digest = Sha256::digest(boot.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let config = json!({"runtimeGeneration":2,"gatewayEpoch":5,"runtimeRpcProtocol":RUST_RPC,"rustFastStartVersion":1,
        "rustRuntimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"original-disk"},"rustAllocationOperationId":"original-operation"});
    sqlx::query("UPDATE api_cloud_workspaces SET runtime_boot_token_hash=$1,runtime_boot_token_expires_at=$2,request_config=$3 WHERE workspace_id='ws-bob'")
        .bind(digest).bind(now+120000).bind(config).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-bob','rust',2,$1)")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    let intent = json!({"commandId":"first-command","turnId":"first-turn","title":"Original","agent":"codex","model":"fixture","permissions":[],"firstMessage":"Do this exactly once"});
    let sealed = master
        .seal(
            b"zuse-cloud-workspace-launch-intent-v1\nuser-bob\nws-bob",
            intent.to_string().as_bytes(),
        )
        .unwrap();
    sqlx::query("INSERT INTO api_cloud_workspace_launch_intents VALUES('ws-bob','user-bob','chat-bob','session-bob','first-turn','first-command',$1,$2,$3)")
        .bind(sealed).bind(now+120000).bind(now).execute(&pool).await.unwrap();
    let control = Arc::new(
        RuntimeControl::from_pool(pool.clone(), master, signer)
            .with_fast_start_accounts(vec!["user-bob".into()]),
    );
    let guest = SigningKey::from_bytes(&[92; 32]);
    let body = json!({"credentialPublicJwk":crypto["runtimePublicJwk"].to_string(),"signingPublicJwk":json!({"kty":"OKP","crv":"Ed25519","x":URL_SAFE_NO_PAD.encode(guest.verifying_key().as_bytes())}).to_string(),"rpcProtocol":RUST_RPC,
        "capabilities":["rust-fast-start-v1",STARTUP_BIND_CAPABILITY]});
    let first = control
        .bootstrap(
            "ws-bob",
            boot,
            serde_json::from_value(body.clone()).unwrap(),
            now,
        )
        .await
        .unwrap();
    let retry = control
        .bootstrap(
            "ws-bob",
            boot,
            serde_json::from_value(body.clone()).unwrap(),
            now,
        )
        .await
        .unwrap();
    assert_eq!(
        first, retry,
        "a lost bootstrap response preserves the original storage identity"
    );
    assert_eq!(first["startupBindingVersion"], 2);
    assert_eq!(first["launchIntent"], intent);
    let credential = first["runtimeCredential"].as_str().unwrap();
    let enrollment = first["storageEnrollmentId"].as_str().unwrap();
    assert!(!enrollment.is_empty());
    let request = || json!({"phase":"commit","runtimeGeneration":2,"gatewayEpoch":5,"storageIncarnationId":"original-disk","enrollmentId":enrollment,"capabilities":["rust-rpc-v1"]});
    assert_eq!(
        control
            .gateway_authorize("ws-bob", RUST_PROTOCOL, credential, now)
            .await
            .err()
            .unwrap()
            .code,
        "runtime_storage_enrollment_required"
    );
    for (field, value) in [
        ("runtimeGeneration", json!(3)),
        ("gatewayEpoch", json!(6)),
        ("storageIncarnationId", json!("different-disk")),
        ("enrollmentId", json!("different-enrollment")),
        ("phase", json!("prepare")),
        // Unknown names are accepted (a newer runtime may know more than this
        // server), but malformed names are not.
        ("capabilities", json!(["Not A Capability!"])),
    ] {
        let mut invalid = request();
        invalid[field] = value;
        assert!(
            control
                .gateway_authorize_with_storage(
                    "ws-bob",
                    RUST_PROTOCOL,
                    credential,
                    now,
                    Some(serde_json::from_value::<CloudStorageEnrollment>(invalid).unwrap())
                )
                .await
                .is_err()
        );
        let phase:String=sqlx::query_scalar("SELECT request_config#>>'{rustStorageEnrollment,phase}' FROM api_cloud_workspaces WHERE workspace_id='ws-bob'").fetch_one(&pool).await.unwrap();
        assert_eq!(
            phase, "prepared",
            "rejected binds cannot partially commit storage"
        );
    }
    assert!(
        control
            .gateway_authorize_with_storage(
                "ws-bob",
                RUST_PROTOCOL,
                &browser,
                now,
                Some(serde_json::from_value(request()).unwrap())
            )
            .await
            .is_err(),
        "a browser ticket must never commit runtime storage"
    );
    // Exercise the actual WebSocket route/header, not only the control method.
    let app = router(
        AppState {
            store: Store::from_pool(pool.clone()),
            identity: Arc::new(TestIdentity),
            organizations_enabled: false,
            organization_ids: Arc::new(vec![]),
            runtime: Some(control.clone()),
            gateway: Arc::new(zuse_cloud::gateway::Hub::default()),
        },
        vec![],
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "ws://{}/v1/cloud/workspaces/ws-bob/gateway",
        listener.local_addr().unwrap()
    );
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap()
    }));
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut upgrade = endpoint.into_client_request().unwrap();
    upgrade.headers_mut().insert(
        "sec-websocket-protocol",
        format!("{RUST_PROTOCOL}, {credential}").parse().unwrap(),
    );
    upgrade.headers_mut().insert(
        zuse_protocol::cloud::STARTUP_BIND_HEADER,
        request().to_string().parse().unwrap(),
    );
    let (mut socket, response) = tokio_tungstenite::connect_async(upgrade).await.unwrap();
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    socket.close(None).await.unwrap();
    drop(server);
    for _ in 0..2 {
        let grant = control
            .gateway_authorize_with_storage(
                "ws-bob",
                RUST_PROTOCOL,
                credential,
                now,
                Some(serde_json::from_value(request()).unwrap()),
            )
            .await
            .unwrap();
        assert!(grant.runtime);
    }
    let config: Value = sqlx::query_scalar(
        "SELECT request_config FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(config["rustStorageEnrollment"]["phase"], "committed");
    assert_eq!(config["rustStorageEnrollment"]["enrollmentId"], enrollment);
    assert!(config["runtimeBootstrapReceipt"]["acknowledgedAtMs"].is_number());
    control
        .gateway_authorize("ws-bob", RUST_PROTOCOL, credential, now)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE api_cloud_workspaces SET desired_state='deleted' WHERE workspace_id='ws-bob'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        control
            .gateway_authorize_with_storage(
                "ws-bob",
                RUST_PROTOCOL,
                credential,
                now,
                Some(serde_json::from_value(request()).unwrap())
            )
            .await
            .is_err()
    );
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
