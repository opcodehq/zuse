use super::*;

/// The account's latest machine instructions, skills and files ride in the
/// bootstrap of a runtime that advertises it; older runtimes get nothing new.
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn bootstrap_delivers_the_latest_machine_recipe_only_to_capable_runtimes() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::SigningKey;
    use sha2::{Digest, Sha256};
    use zuse_cloud::{crypto::MasterKey, runtime::RuntimeControl, tickets::TicketSigner};
    use zuse_protocol::cloud_computers::{MACHINE_DELIVERY_CAPABILITY, MachineDelivery};
    use zuse_protocol::cloud_gateway::RUST_RPC;
    let (admin, pool, schema) = fixture().await;
    let crypto: Value =
        serde_json::from_str(include_str!("../fixtures/cloud-crypto.json")).unwrap();
    let master = MasterKey::parse(crypto["masterKey"].as_str().unwrap()).unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let key = SigningKey::from_bytes(&[93; 32]);
    let jwk=json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([93;32]),"x":URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes())}).to_string();
    let signer = TicketSigner::from_jwk(&jwk, "https://api.example.test").unwrap();
    let guest = SigningKey::from_bytes(&[94; 32]);
    let intent = json!({"commandId":"first-command","turnId":"first-turn","title":"Original","agent":"claude-code","model":"fixture","permissions":[]});
    let sealed = master
        .seal(
            b"zuse-cloud-workspace-launch-intent-v1\nuser-bob\nws-bob",
            intent.to_string().as_bytes(),
        )
        .unwrap();
    let control = RuntimeControl::from_pool(pool.clone(), master, signer);
    sqlx::query("INSERT INTO api_cloud_workspace_launch_intents VALUES('ws-bob','user-bob','chat-bob','session-bob','first-turn','first-command',$1,$2,$3)")
        .bind(sealed).bind(now+120000).bind(now).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-bob','rust',2,$1)")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    // Each enrollment is a fresh boot token (a new computer start).
    let enroll = |attempt: i64, capabilities: Vec<&'static str>| {
        let pool = pool.clone();
        let control = &control;
        let crypto = crypto.clone();
        let guest = guest.clone();
        async move {
            let boot = format!("machine-delivery-{attempt}");
            let digest = Sha256::digest(boot.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            let config = json!({"runtimeGeneration":2,"gatewayEpoch":5,"runtimeRpcProtocol":RUST_RPC,
                "rustRuntimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"disk"}});
            sqlx::query("UPDATE api_cloud_workspaces SET runtime_boot_token_hash=$1,runtime_boot_token_expires_at=$2,request_config=$3,runtime_credential_hash=NULL WHERE workspace_id='ws-bob'")
                .bind(digest).bind(now+120000).bind(config).execute(&pool).await.unwrap();
            let body = json!({"credentialPublicJwk":crypto["runtimePublicJwk"].to_string(),"signingPublicJwk":json!({"kty":"OKP","crv":"Ed25519","x":URL_SAFE_NO_PAD.encode(guest.verifying_key().as_bytes())}).to_string(),"rpcProtocol":RUST_RPC,
                "capabilities":capabilities});
            control
                .bootstrap("ws-bob", &boot, serde_json::from_value(body).unwrap(), now)
                .await
                .unwrap()
        }
    };

    // Before the machine tables exist (an API ahead of its migration), a
    // capable runtime still enrolls, with nothing to apply.
    let first = enroll(2, vec!["rust-rpc-v1", MACHINE_DELIVERY_CAPABILITY]).await;
    assert!(first.get("machineDelivery").is_none());

    sqlx::raw_sql(include_str!("../../migrations/0031_machine_images.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let save = |revision: i64, recipe: Value| {
        let pool = pool.clone();
        async move {
            sqlx::query("INSERT INTO cloud_machine_versions(owner_id,revision,actor_id,recipe,digest,created_at) VALUES('user-bob',$1,'user-bob',$2,$3,$4)")
                .bind(revision).bind(recipe).bind("a".repeat(64)).bind(now).execute(&pool).await.unwrap();
        }
    };
    save(
        1,
        json!({"version":1,"setupScript":"sudo apt-get install -y jq","instructions":"Old rules."}),
    )
    .await;
    save(2, json!({"version":1,"setupScript":"sudo apt-get install -y jq","instructions":"Keep commits small.",
        "skills":[{"name":"release-notes","content":"# Release notes"}],
        "files":[{"path":".gitconfig","content":"[user]\n"}]}))
    .await;
    // Another account's recipe never leaks into this task.
    sqlx::query("INSERT INTO cloud_machine_versions(owner_id,revision,actor_id,recipe,digest,created_at) VALUES('user-alice',9,'user-alice',$1,$2,$3)")
        .bind(json!({"version":1,"instructions":"Alice only."})).bind("b".repeat(64)).bind(now).execute(&pool).await.unwrap();

    let older = enroll(3, vec!["rust-rpc-v1"]).await;
    assert!(
        older.get("machineDelivery").is_none(),
        "a runtime that doesn't advertise the capability receives nothing new"
    );

    let capable = enroll(4, vec!["rust-rpc-v1", MACHINE_DELIVERY_CAPABILITY]).await;
    let delivery: MachineDelivery =
        serde_json::from_value(capable["machineDelivery"].clone()).unwrap();
    assert_eq!(delivery.revision, 2, "the latest version");
    assert_eq!(delivery.instructions, "Keep commits small.");
    assert_eq!(delivery.skills[0].name, "release-notes");
    assert_eq!(delivery.files[0].path, ".gitconfig");
    assert!(
        capable["machineDelivery"].get("setupScript").is_none(),
        "the setup script waits for the image build"
    );
    // The runtime's own bootstrap type keeps it as an extension field.
    let typed: zuse_protocol::cloud::CloudRuntimeBootstrap =
        serde_json::from_value(capable.clone()).unwrap();
    assert!(typed.extra.contains_key("machineDelivery"));

    // A recipe with only a setup script has nothing to apply without an image.
    save(
        3,
        json!({"version":1,"setupScript":"sudo apt-get install -y jq"}),
    )
    .await;
    let script_only = enroll(5, vec!["rust-rpc-v1", MACHINE_DELIVERY_CAPABILITY]).await;
    assert!(script_only.get("machineDelivery").is_none());

    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
