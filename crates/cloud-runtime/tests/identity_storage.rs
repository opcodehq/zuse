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

#[cfg(unix)]
#[test]
fn fresh_store_accepts_real_launcher_metadata_and_rejects_legacy_or_mismatched_artifacts() {
    use std::{
        io::{Read, Write},
        os::unix::{
            fs::{PermissionsExt, symlink},
            net::UnixStream,
        },
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    struct Processes(std::process::Child, Option<i32>);
    impl Drop for Processes {
        fn drop(&mut self) {
            unsafe {
                if let Some(pid) = self.1 {
                    libc::kill(-pid, libc::SIGTERM);
                }
                libc::kill(self.0.id() as i32, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if self.0.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let _lock = DataLock::acquire(&data, None).unwrap();
    let _identity = Identity::open(&data, &binding(), Some("synthetic-first-boot")).unwrap();
    let infra = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../infra/cloud");
    let installed = root.path().join("installed");
    std::fs::create_dir(&installed).unwrap();
    let checkout = root.path().join("repo");
    std::fs::create_dir(&checkout).unwrap();
    let status = root.path().join("status");
    std::fs::create_dir(&status).unwrap();
    let fake = installed.join("runtime");
    std::fs::write(&fake,"#!/bin/sh\nif test \"${1:-}\" = --version; then echo 'zuse-cloud-runtime 0.25.1'; exit 0; fi\nexec sleep 60\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    // These are the actual supervised bundle scripts, relocated only so the
    // test never writes /usr/local or starts a real model/authority connection.
    for name in [
        "harness-release.py",
        "runtime-supervisor.py",
        "runtime-repair.py",
        "runtime-recreation.py",
        "launch-recovery.sh",
        "managed-base-install.sh",
    ] {
        std::fs::copy(infra.join(name), installed.join(name)).unwrap();
    }
    let launch = std::fs::read_to_string(infra.join("launch-runtime.sh"))
        .unwrap()
        .replace(
            "/usr/local/lib/zuse/harness-release.py",
            installed.join("harness-release.py").to_str().unwrap(),
        );
    std::fs::write(installed.join("launch-runtime.sh"), launch).unwrap();
    std::fs::set_permissions(
        installed.join("launch-runtime.sh"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let harness = root.path().join("harnesses");
    let generated=Command::new("python3").args(["-c",r#"
import hashlib,json,platform,shutil,sys
from pathlib import Path
r=Path(sys.argv[1]);(r/'channels').mkdir(parents=True);(r/'.release-lock').touch()
m={'schema':2,'platform':platform.system().lower()+'-'+platform.machine(),'runtimeVersion':'0.25.1','harnesses':{a:{'version':'1.0.0','sha256':hashlib.sha256(Path('/usr/bin/true').read_bytes()).hexdigest()} for a in ('claude','codex','grok')}}
b=json.dumps(m,sort_keys=True,separators=(',',':')).encode();h=hashlib.sha256(b).hexdigest();d=r/'releases'/h;d.mkdir(parents=True)
for a in ('claude','codex','grok'):shutil.copyfile('/usr/bin/true',d/a);(d/a).chmod(0o555)
(d/'manifest.json').write_bytes(b);(r/'channels/staging.json').write_text(json.dumps({'release':h,'previous':None}))
"#]).arg(&harness).status().unwrap();
    assert!(generated.success());
    let wrapper = installed.join("launch");
    std::fs::write(&wrapper,format!("#!/bin/sh\nexport ZUSE_HARNESS_ROOT='{}' ZUSE_HARNESS_CHANNEL=staging\nexec '{}' \"$@\"\n",harness.display(),installed.join("launch-runtime.sh").display())).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let server = Command::new("python3")
        .arg(installed.join("runtime-supervisor.py"))
        .arg("--data-dir")
        .arg(&data)
        .arg("--workspace-root")
        .arg(&checkout)
        .arg("--status-dir")
        .arg(&status)
        .arg("--launcher")
        .arg(&wrapper)
        .arg("--binary")
        .arg(&fake)
        .arg("serve")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut processes = Processes(server, None);
    let deadline = Instant::now() + Duration::from_secs(3);
    let socket = data.join(".runtime-supervision/control.sock");
    while !socket.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut stream = UnixStream::connect(&socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let request = json!({"version":1,"action":"launch","operationId":"launch-original","workspaceId":"workspace-original","generation":2,"storageIncarnationId":"original-storage","apiUrl":"https://api-rust.zuse.sh","retained":true});
    stream
        .write_all(format!("{}\n", request).as_bytes())
        .unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    let response: Value = serde_json::from_slice(&bytes).unwrap();
    processes.1 = response["childPid"].as_i64().map(|p| p as i32);
    assert!(processes.1.is_some(), "private launch must succeed");
    let pin = data.join("cloud-harness-release.json");
    while !pin.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(pin.exists(), "actual launcher/manager creates the pin");
    let mut boot = bootstrap();
    boot.extra
        .insert("rustAllocationOperationId".into(), json!("launch-original"));
    let database = data.join("orgs/workspace-original/user-original/docs.sqlite3");
    let prepare = || {
        Storage::prepare(
            &data,
            &database,
            &boot,
            "original-storage",
            "enrollment-original",
        )
    };
    let control = data.join("cloud-runtime-supervision.json");
    let original = std::fs::read(&control).unwrap();
    for (field, value) in [
        ("workspaceId", json!("other-workspace")),
        ("generation", json!(3)),
        ("storageIncarnationId", json!("other-disk")),
        ("apiUrl", json!("https://other.example")),
    ] {
        let mut wrong: Value = serde_json::from_slice(&original).unwrap();
        wrong["identity"][field] = value;
        std::fs::write(&control, serde_json::to_vec(&wrong).unwrap()).unwrap();
        assert!(prepare().is_err(), "{field}");
        assert!(!data.join("rust-runtime-storage.json").exists());
    }
    std::fs::write(&control, &original).unwrap();
    let legacy = data.join(".runtime-supervision/legacy.sqlite");
    std::fs::write(&legacy, b"retain unknown bytes").unwrap();
    assert!(prepare().is_err());
    assert_eq!(std::fs::read(&legacy).unwrap(), b"retain unknown bytes");
    std::fs::remove_file(&legacy).unwrap();
    let retained = root.path().join("original-pin");
    std::fs::rename(&pin, &retained).unwrap();
    symlink(&retained, &pin).unwrap();
    assert!(prepare().is_err());
    assert!(retained.is_file());
    std::fs::remove_file(&pin).unwrap();
    std::fs::rename(&retained, &pin).unwrap();
    let unknown = data.join("legacy.sqlite");
    std::fs::write(&unknown, b"legacy db").unwrap();
    assert!(prepare().is_err());
    assert_eq!(std::fs::read(&unknown).unwrap(), b"legacy db");
    std::fs::remove_file(&unknown).unwrap();
    let storage = prepare().unwrap();
    assert!(!storage.initialized());
    assert!(data.join("rust-runtime-storage.json").is_file());
    assert!(control.is_file());
    assert!(pin.is_file());
    assert!(
        !database.exists(),
        "guard does not create an empty replacement database"
    );
}

#[tokio::test]
async fn negotiated_storage_uses_gateway_bind_and_preparation_waits_for_commit() {
    use tokio::time::{Duration, timeout};
    use tokio_util::sync::CancellationToken;
    use zuse_cloud_runtime::{Authority, preparation::wait_startup_binding};
    let mut boot = bootstrap();
    boot.extra.insert("startupBindingVersion".into(), json!(2));
    boot.extra
        .insert("storageEnrollmentId".into(), json!("original-enrollment"));
    // No authority is listening: negotiated preparation/commit must not send HTTP.
    let authority = Authority::new("http://127.0.0.1:9").unwrap();
    assert_eq!(
        authority.prepare_storage(&boot).await.unwrap(),
        "original-enrollment"
    );
    assert!(
        authority
            .commit_storage(&boot, "replacement-enrollment")
            .await
            .is_err()
    );
    authority
        .commit_storage(&boot, "original-enrollment")
        .await
        .unwrap();
    let grant = authority.gateway(&boot).unwrap();
    let binding = grant.storage_binding.unwrap();
    assert_eq!(binding["phase"], "commit");
    assert_eq!(binding["enrollmentId"], "original-enrollment");
    assert_eq!(binding["storageIncarnationId"], "original-storage");
    assert_eq!(binding["runtimeGeneration"], 2);
    let (updates, receiver) = tokio::sync::watch::channel(boot.clone());
    let cancel = CancellationToken::new();
    assert!(
        timeout(
            Duration::from_millis(20),
            wait_startup_binding(&boot, receiver.clone(), &cancel)
        )
        .await
        .is_err(),
        "local storage commit alone cannot authorize publication"
    );
    let mut connected = boot.clone();
    connected
        .extra
        .insert("startupBindingCommitted".into(), json!(true));
    updates.send_replace(connected.clone());
    wait_startup_binding(&boot, receiver.clone(), &cancel)
        .await
        .unwrap();
    connected.runtime_generation += 1;
    updates.send_replace(connected);
    assert!(
        wait_startup_binding(&boot, receiver.clone(), &cancel)
            .await
            .is_err(),
        "a replacement generation cannot release the wait"
    );
    updates.send_replace(boot.clone());
    cancel.cancel();
    assert!(
        wait_startup_binding(&boot, receiver.clone(), &cancel)
            .await
            .is_err()
    );
    let legacy = bootstrap();
    wait_startup_binding(&legacy, receiver, &CancellationToken::new())
        .await
        .unwrap();
    assert!(
        authority
            .gateway(&legacy)
            .unwrap()
            .storage_binding
            .is_none()
    );
}
