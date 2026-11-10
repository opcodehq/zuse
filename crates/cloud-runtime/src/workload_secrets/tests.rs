use super::*;
use rsa::{Oaep, RsaPublicKey, pkcs8::DecodePrivateKey};
fn recipient() -> Recipient {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../cloud/tests/fixtures/cloud-crypto.json"
    ))
    .unwrap();
    Recipient {
        key: RsaPrivateKey::from_pkcs8_der(
            &B64.decode(fixture["runtimePrivatePkcs8"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap(),
        public: fixture["runtimePublicJwk"].to_string(),
        thumb: fixture["runtimeThumbprint"].as_str().unwrap().into(),
        workspace: "task".into(),
        owner: "owner".into(),
        generation: 2,
    }
}
fn seal(recipient: &Recipient, value: &Value) -> String {
    use aes_gcm::{
        Aes256Gcm, KeyInit, Nonce,
        aead::{Aead, Payload},
    };
    let header =
        B64.encode(serde_json::to_vec(&json!({"alg":"RSA-OAEP-256","enc":"A256GCM"})).unwrap());
    let cek = [72; 32];
    let nonce = [73; 12];
    let wrapped = RsaPublicKey::from(&recipient.key)
        .encrypt(&mut rand::thread_rng(), Oaep::new::<Sha256>(), &cek)
        .unwrap();
    let sealed = Aes256Gcm::new_from_slice(&cek)
        .unwrap()
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &serde_json::to_vec(value).unwrap(),
                aad: header.as_bytes(),
            },
        )
        .unwrap();
    let (ciphertext, tag) = sealed.split_at(sealed.len() - 16);
    format!(
        "{header}.{}.{}.{}.{}",
        B64.encode(wrapped),
        B64.encode(nonce),
        B64.encode(ciphertext),
        B64.encode(tag)
    )
}
#[test]
fn workload_recipient_rejects_cross_task_key_expiry_and_unselected_bindings() {
    let recipient = recipient();
    let expiry = now_ms() + 60000;
    let mut plain = json!({"protocolVersion":1,"requestId":"request","workspaceId":"task","ownerId":"owner","runtimeGeneration":2,"credentialKeyThumbprint":recipient.thumb,"expiresAtMs":expiry,"bindings":{"secret_fixture":1},"environment":{"PROJECT_REGISTRY_TOKEN":"private-project-secret"}});
    let response = |plain: &Value| json!({"protocolVersion":1,"requestId":"request","runtimeGeneration":2,"expiresAtMs":plain["expiresAtMs"],"sealedGrant":seal(&recipient,plain)});
    let (values, _) = recipient
        .open(&response(&plain), "request", &["secret_fixture".into()])
        .unwrap();
    assert_eq!(
        values["PROJECT_REGISTRY_TOKEN"].as_str(),
        "private-project-secret"
    );
    for (key, value) in [
        ("workspaceId", json!("other-task")),
        ("ownerId", json!("other-owner")),
        ("runtimeGeneration", json!(3)),
        ("credentialKeyThumbprint", json!("other-key")),
        ("expiresAtMs", json!(now_ms() - 1)),
    ] {
        let saved = plain[key].clone();
        plain[key] = value;
        assert!(
            recipient
                .open(&response(&plain), "request", &["secret_fixture".into()])
                .is_err()
        );
        plain[key] = saved;
    }
    assert!(
        recipient
            .open(
                &response(&plain),
                "different-request",
                &["secret_fixture".into()]
            )
            .is_err()
    );
    assert!(
        recipient
            .open(&response(&plain), "request", &["another-secret".into()])
            .is_err()
    );
    plain["environment"] = json!({"ZUSE_RUNTIME_TOKEN":"must-not-override"});
    assert!(
        recipient
            .open(&response(&plain), "request", &["secret_fixture".into()])
            .is_err()
    );
}
