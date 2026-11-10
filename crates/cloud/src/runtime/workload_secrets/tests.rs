use super::*;
use crate::{
    access::{Access, Scope},
    auth::Principal,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use rsa::{RsaPrivateKey, RsaPublicKey, traits::PublicKeyParts};
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn workload_vault_is_encrypted_scoped_versioned_and_recipient_fenced() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated database required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("workload_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .after_connect(move |connection, _| {
            let scope = scope.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={scope}"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE api_cloud_projects(project_id text PRIMARY KEY,account_id text,included boolean); CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,account_id text,project_id text,request_config jsonb,state text,desired_state text,runtime_credential_hash text); CREATE TABLE zuse_rust_control_owners(resource_kind text,resource_id text,backend text,generation bigint); INSERT INTO api_cloud_projects VALUES('repo','owner',true),('other','owner',true); INSERT INTO zuse_rust_control_owners VALUES('workspace','computer','rust',1)").execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/0019_workload_secrets.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let master = MasterKey::parse(&B64.encode([61; 32])).unwrap();
    let signing = ed25519_dalek::SigningKey::from_bytes(&[62; 32]);
    let signer=TicketSigner::from_jwk(&json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([62;32]),"x":B64.encode(signing.verifying_key().as_bytes())}).to_string(),"https://fixture.example").unwrap();
    let control = RuntimeControl::from_pool(pool.clone(), master, signer);
    let access = Access {
        actor: Principal {
            account_id: "owner".into(),
        },
        scope: Scope::Personal,
        owner_id: "owner".into(),
        membership: None,
    };
    let save = |revision| SaveWorkloadSecret {
        name: "PROJECT_REGISTRY_TOKEN".into(),
        value: "private-workload-token".into(),
        expected_revision: revision,
    };
    let first = control
        .save_workload_secret(&access, "repo", save(0), 10)
        .await
        .unwrap();
    assert_eq!(first["revision"], 1);
    assert!(!first.to_string().contains("private-workload-token"));
    let id = first["secretId"].as_str().unwrap().to_owned();
    let ciphertext: String =
        sqlx::query_scalar("SELECT ciphertext FROM zuse_rust_workload_secrets WHERE secret_id=$1")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!ciphertext.contains("private-workload-token"));
    assert_eq!(
        &*control
            .master
            .open_api(&context("owner", "repo", &id, 1), &ciphertext)
            .unwrap(),
        b"private-workload-token"
    );
    assert!(
        control
            .master
            .open_api(&context("stranger", "repo", &id, 1), &ciphertext)
            .is_err()
    );
    assert_eq!(
        control
            .save_workload_secret(&access, "repo", save(0), 11)
            .await
            .unwrap_err()
            .code,
        "workload_secret_revision_changed"
    );
    assert!(
        !control
            .workload_secrets(&access, "repo")
            .await
            .unwrap()
            .to_string()
            .contains("private-workload-token")
    );
    let stranger = Access {
        actor: Principal {
            account_id: "stranger".into(),
        },
        scope: Scope::Personal,
        owner_id: "stranger".into(),
        membership: None,
    };
    assert_eq!(
        control
            .workload_secrets(&stranger, "repo")
            .await
            .unwrap_err()
            .code,
        "cloud_project_not_found"
    );
    for name in ["PATH", "ZUSE_RUNTIME_TOKEN", "OPENAI_API_KEY", "LD_PRELOAD"] {
        let request = SaveWorkloadSecret {
            name: name.into(),
            value: "secret".into(),
            expected_revision: 0,
        };
        assert_eq!(
            control
                .save_workload_secret(&access, "repo", request, 12)
                .await
                .unwrap_err()
                .code,
            "invalid_workload_secret"
        );
    }
    let key = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public = RsaPublicKey::from(&key);
    let jwk=json!({"kty":"RSA","e":B64.encode(public.e().to_bytes_be()),"n":B64.encode(public.n().to_bytes_be())}).to_string();
    let thumb = credential_thumbprint(&jwk).unwrap();
    let config = json!({"runtimeGeneration":1,"gatewayEpoch":1,"runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"runtimeBootstrapReceipt":{"workspaceId":"computer","generation":1,"gatewayEpoch":1},"runtimeCredentialExpiresAtMs":10000,"runtimeCredentialKeyThumbprint":thumb,"rustWorkloadSecretsVersion":1,"workloadSecretBindings":[id]});
    sqlx::query(
        "INSERT INTO api_cloud_workspaces VALUES('computer','owner','repo',$1,'ready','ready',$2)",
    )
    .bind(config)
    .bind(hash("runtime-token"))
    .execute(&pool)
    .await
    .unwrap();
    let request = |generation, bindings: Vec<String>| WorkloadGrantRequest {
        protocol_version: 1,
        request_id: uuid::Uuid::new_v4().to_string(),
        runtime_generation: generation,
        credential_public_jwk: jwk.clone(),
        bindings,
    };
    assert_eq!(
        control
            .workload_grant("computer", "wrong-token", request(1, vec![id.clone()]), 100)
            .await
            .unwrap_err()
            .code,
        "workspace_runtime_fenced"
    );
    assert_eq!(
        control
            .workload_grant(
                "computer",
                "runtime-token",
                request(2, vec![id.clone()]),
                100
            )
            .await
            .unwrap_err()
            .code,
        "workspace_runtime_fenced"
    );
    assert_eq!(
        control
            .workload_grant(
                "computer",
                "runtime-token",
                request(1, vec!["unselected".into()]),
                100
            )
            .await
            .unwrap_err()
            .code,
        "workload_secret_not_selected"
    );
    let grant = control
        .workload_grant(
            "computer",
            "runtime-token",
            request(1, vec![id.clone()]),
            100,
        )
        .await
        .unwrap();
    assert!(!grant.to_string().contains("private-workload-token"));
    let parts: Vec<_> = grant["sealedGrant"].as_str().unwrap().split('.').collect();
    let cek = zeroize::Zeroizing::new(
        key.decrypt(rsa::Oaep::new::<Sha256>(), &B64.decode(parts[1]).unwrap())
            .unwrap(),
    );
    use aes_gcm::{
        Aes256Gcm, KeyInit, Nonce,
        aead::{Aead, Payload},
    };
    let mut ciphertext = B64.decode(parts[3]).unwrap();
    ciphertext.extend(B64.decode(parts[4]).unwrap());
    let iv = B64.decode(parts[2]).unwrap();
    let plain = zeroize::Zeroizing::new(
        Aes256Gcm::new_from_slice(&cek)
            .unwrap()
            .decrypt(
                Nonce::from_slice(&iv),
                Payload {
                    msg: &ciphertext,
                    aad: parts[0].as_bytes(),
                },
            )
            .unwrap(),
    );
    let value: Value = serde_json::from_slice(&plain).unwrap();
    assert_eq!(
        value["environment"]["PROJECT_REGISTRY_TOKEN"],
        "private-workload-token"
    );
    assert_eq!(value["ownerId"], "owner");
    assert_eq!(value["runtimeGeneration"], 1);
    control
        .revoke_workload_secret(&access, "repo", &id, 1, 200)
        .await
        .unwrap();
    assert_eq!(
        control
            .workload_grant("computer", "runtime-token", request(1, vec![id]), 201)
            .await
            .unwrap_err()
            .code,
        "workload_secret_revoked"
    );
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
