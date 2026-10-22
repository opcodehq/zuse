use serde_json::{Value, json};
use zuse_cloud_runtime::{DataLock, Identity, IdentityBinding, Storage};
use zuse_protocol::cloud::CloudRuntimeBootstrap;

fn bootstrap() -> CloudRuntimeBootstrap {
    serde_json::from_value(json!({"workspaceId":"workspace-original","zuseAccountId":"user-original","chatId":"thread-original","initialSessionId":"conversation-original","runtimeCredential":"runtime-synthetic","runtimeGeneration":2,"gatewayEpoch":3,"runtimeCredentialExpiresAt":9999999999999i64,"gatewayUrl":"wss://api-rust.zuse.sh/v1/cloud/workspaces/workspace-original/gateway","gatewayProtocol":"zuse-workspace-rust-v1","rpcProtocol":"zuse-rpc-v1","sealedTranscriptKey":"ciphertext","runtimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"original-storage"}})).unwrap()
}
fn binding() -> IdentityBinding {
    IdentityBinding {
        workspace: "workspace-original".into(),
        api_url: "https://api-rust.zuse.sh".into(),
        incarnation: "original-storage".into(),
        generation: 2,
    }
}
#[test]
fn lifetime_lock_and_storage_binding_preserve_existing_bytes_and_reject_empty_replacements() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("runtime");
    let guard = DataLock::acquire(&directory, None).unwrap();
    assert!(DataLock::acquire(&directory, None).is_err());
    let database = directory.join("orgs/workspace-original/user-original/docs.sqlite3");
    let boot = bootstrap();
    let mut storage = Storage::prepare(
        &directory,
        &database,
        &boot,
        "original-storage",
        "enrollment-original",
    )
    .unwrap();
    std::fs::create_dir_all(database.parent().unwrap()).unwrap();
    let db = rusqlite::Connection::open(&database).unwrap();
    db.execute_batch("CREATE TABLE retained_content (body TEXT); INSERT INTO retained_content VALUES ('original transcript bytes');").unwrap();
    storage.commit().unwrap();
    let reopened = Storage::prepare(
        &directory,
        &database,
        &boot,
        "original-storage",
        "enrollment-original",
    )
    .unwrap();
    drop(reopened);
    let retained: String = db
        .query_row("SELECT body FROM retained_content", [], |r| r.get(0))
        .unwrap();
    assert_eq!(retained, "original transcript bytes");
    let mut wrong = boot.clone();
    wrong.zuse_account_id = "other-account".into();
    assert!(
        Storage::prepare(
            &directory,
            &database,
            &wrong,
            "original-storage",
            "enrollment-original"
        )
        .is_err()
    );
    drop(db);
    let retained = database.with_extension("retained");
    std::fs::rename(&database, &retained).unwrap();
    assert!(
        Storage::prepare(
            &directory,
            &database,
            &boot,
            "original-storage",
            "enrollment-original"
        )
        .unwrap_err_message()
        .contains("runtime_storage_missing")
    );
    assert!(!database.exists());
    assert!(retained.is_file());
    std::fs::rename(retained, &database).unwrap();
    let db = rusqlite::Connection::open(&database).unwrap();
    db.execute(
        "UPDATE zuse_cloud_storage_identity SET workspace_id='unrelated-workspace'",
        [],
    )
    .unwrap();
    assert!(
        Storage::prepare(
            &directory,
            &database,
            &boot,
            "original-storage",
            "enrollment-original"
        )
        .is_err()
    );
    drop(db);
    drop(guard);
    DataLock::acquire(&directory, None).unwrap();
    let legacy = root.path().join("legacy");
    let _guard = DataLock::acquire(&legacy, None).unwrap();
    std::fs::write(legacy.join("db.sqlite"), "retained legacy state").unwrap();
    assert!(
        Storage::prepare(
            &legacy,
            &legacy.join("orgs/new/docs.sqlite3"),
            &boot,
            "original-storage",
            "enrollment-original"
        )
        .unwrap_err_message()
        .contains("runtime_storage_migration_required")
    );
    assert_eq!(
        std::fs::read_to_string(legacy.join("db.sqlite")).unwrap(),
        "retained legacy state"
    );
    assert!(!legacy.join("rust-runtime-storage.json").exists());
}
trait ErrorMessage {
    fn unwrap_err_message(self) -> String;
}
impl<T> ErrorMessage for anyhow::Result<T> {
    fn unwrap_err_message(self) -> String {
        match self {
            Ok(_) => panic!("Expected admission rejection"),
            Err(e) => e.to_string(),
        }
    }
}
#[test]
fn durable_identity_keeps_keys_and_rejects_other_workspace_and_storage_owners() {
    let root = tempfile::tempdir().unwrap();
    let _lock = DataLock::acquire(root.path(), None).unwrap();
    let original = binding();
    let identity = Identity::open(root.path(), &original, Some("synthetic-boot")).unwrap();
    let request = identity.request();
    drop(identity);
    let bytes = std::fs::read(root.path().join("cloud-runtime-identity.json")).unwrap();
    let reopened = Identity::open(root.path(), &original, None).unwrap();
    assert_eq!(
        request.credential_public_jwk,
        reopened.request().credential_public_jwk
    );
    assert_eq!(
        request.signing_public_jwk,
        reopened.request().signing_public_jwk
    );
    drop(reopened);
    for wrong in [
        IdentityBinding {
            workspace: "other".into(),
            ..original.clone()
        },
        IdentityBinding {
            incarnation: "new-database".into(),
            ..original.clone()
        },
    ] {
        assert!(Identity::open(root.path(), &wrong, Some("new-boot")).is_err());
    }
    assert_eq!(
        bytes,
        std::fs::read(root.path().join("cloud-runtime-identity.json")).unwrap()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(root.path().join("cloud-runtime-identity.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
    }
}
#[cfg(target_os = "linux")]
#[test]
fn inherited_original_launcher_lock_keeps_its_kernel_ownership() {
    use std::os::fd::AsRawFd;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(".workspace-runtime.lock");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.try_lock().unwrap();
    let guest = DataLock::acquire(root.path(), Some(file.as_raw_fd())).unwrap();
    drop(file);
    assert!(DataLock::acquire(root.path(), None).is_err());
    drop(guest);
    DataLock::acquire(root.path(), None).unwrap();
}
#[test]
#[ignore = "requires Node WebCrypto for independent legacy JWK compatibility"]
fn persisted_guest_keys_import_in_original_webcrypto_consumers() {
    let root = tempfile::tempdir().unwrap();
    let _lock = DataLock::acquire(root.path(), None).unwrap();
    let _identity = Identity::open(root.path(), &binding(), Some("synthetic-boot")).unwrap();
    let script = r#"import {readFileSync} from 'node:fs'; import {webcrypto} from 'node:crypto';
const state=JSON.parse(readFileSync(process.env.TEST_IDENTITY_PATH,'utf8'));
const rsa=await webcrypto.subtle.importKey('jwk',JSON.parse(state.credentialPrivateJwk),{name:'RSA-OAEP',hash:'SHA-256'},true,['decrypt']);
const ed=await webcrypto.subtle.importKey('jwk',JSON.parse(state.signingPrivateJwk),{name:'Ed25519'},true,['sign']);
const publicEd=await webcrypto.subtle.importKey('jwk',JSON.parse(state.signingPublicJwk),{name:'Ed25519'},true,['verify']);
const signature=await webcrypto.subtle.sign('Ed25519',ed,new TextEncoder().encode('legacy runtime'));
if(!await webcrypto.subtle.verify('Ed25519',publicEd,signature,new TextEncoder().encode('legacy runtime')))process.exit(1);
if((await webcrypto.subtle.exportKey('jwk',rsa)).n!==JSON.parse(state.credentialPublicJwk).n)process.exit(1);
process.stdout.write(JSON.stringify({rsa:true,signing:true}));"#;
    let result = std::process::Command::new("node")
        .args(["--input-type=module", "-e", script])
        .env(
            "TEST_IDENTITY_PATH",
            root.path().join("cloud-runtime-identity.json"),
        )
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "WebCrypto rejected the persisted guest keys"
    );
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report, json!({"rsa":true,"signing":true}));
}
