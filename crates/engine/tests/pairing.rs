//! Pairing management crosses the same canonical RPC boundary as the app.
use serde_json::json;
use std::sync::Arc;
use zuse_engine::{EngineCore, EngineProfile, HarnessRegistry};
use zuse_protocol::HarnessId;
use zuse_rpc::methods;

fn core(profile: EngineProfile) -> EngineCore {
    EngineCore::assemble_with_profile(
        profile,
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap()
}

#[tokio::test]
async fn pairing_rpc_keeps_capabilities_private_and_profile_scoped_across_restarts() {
    let root = tempfile::tempdir().unwrap();
    let local = EngineProfile::local(root.path()).unwrap();
    let account = EngineProfile::development(root.path(), "account-org", "account-user");
    let host = core(local.clone());
    host.browser_access
        .control()
        .register_external(
            "127.0.0.1:27656".parse().unwrap(),
            "https://relay.example".into(),
            true,
        )
        .await;
    let client = zuse_rpc::memory_client(host.rpc_service());
    let created = client
        .call(methods::CREATE_PAIRING, json!({"label":"My laptop"}))
        .await
        .unwrap();
    let invitation =
        zuse_remote::Invitation::parse_fragment(created["fragment"].as_str().unwrap()).unwrap();
    assert_eq!(created["peer"]["id"], invitation.id.as_str());
    assert_eq!(created["peer"]["label"], "My laptop");
    assert_eq!(created["origin"], "https://relay.example");
    let listed = client
        .call(methods::LIST_PAIRINGS, json!({}))
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(!listed.to_string().contains(&invitation.secret.expose()));
    assert!(
        client
            .call(
                methods::CREATE_PAIRING,
                json!({"label":"Foreign", "targetDeviceId":"other-host"})
            )
            .await
            .is_err()
    );
    assert!(
        client
            .call(
                methods::CREATE_PAIRING,
                json!({"label":"Invalid", "targetDeviceId":12})
            )
            .await
            .is_err()
    );
    let credentials = host.pairings.credentials(&invitation.id).unwrap();
    host.shutdown().await;
    assert!(!credentials.is_live());
    assert!(
        client
            .call(methods::LIST_PAIRINGS, json!({}))
            .await
            .is_err()
    );
    drop(client);
    drop(host);

    let different = core(account);
    let different_client = zuse_rpc::memory_client(different.rpc_service());
    assert_eq!(
        different_client
            .call(methods::LIST_PAIRINGS, json!({}))
            .await
            .unwrap(),
        json!([])
    );
    assert!(different.pairings.credentials(&invitation.id).is_err());
    different.shutdown().await;
    drop(different_client);
    drop(different);

    let host = core(local);
    let client = zuse_rpc::memory_client(host.rpc_service());
    assert_eq!(
        client
            .call(methods::LIST_PAIRINGS, json!({}))
            .await
            .unwrap()[0]["id"],
        invitation.id.as_str()
    );
    client
        .call(
            methods::REVOKE_PAIRING,
            json!({"pairId":invitation.id.as_str()}),
        )
        .await
        .unwrap();
    assert!(host.pairings.credentials(&invitation.id).is_err());
    assert_eq!(
        client
            .call(methods::LIST_PAIRINGS, json!({}))
            .await
            .unwrap(),
        json!([])
    );
    host.shutdown().await;
}
