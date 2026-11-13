//! Fake bundles end to end: build, sign, serve, download, verify, install,
//! adopt at a boundary, keep what is in use, roll back, attest.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zuse_protocol::cloud_harness_updates::{
    HarnessArtifact, HarnessAttestation, HarnessDirective, HarnessOffer, HarnessOutcome,
    HarnessPoll,
};

fn signing() -> SigningKey {
    SigningKey::from_bytes(&[9u8; 32])
}
fn sha(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// A deterministic bundle tar: `bin/<agent>` prints `version`, plus a notice.
fn bundle_tar(agent: &str, version: &str, extra: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut builder = ::tar::Builder::new(Vec::new());
    let script =
        format!("#!/bin/sh\n[ \"$1\" = --version ] || exit 2\necho '{version} (fixture)'\n");
    let mut files: Vec<(String, Vec<u8>, u32)> = vec![
        ("LICENSE".into(), b"fixture".to_vec(), 0o444),
        (format!("bin/{agent}"), script.into_bytes(), 0o555),
    ];
    files.extend(
        extra
            .iter()
            .map(|(p, b, m)| ((*p).to_owned(), b.to_vec(), *m)),
    );
    for (path, bytes, mode) in files {
        let mut header = ::tar::Header::new_ustar();
        header.set_size(bytes.len() as u64);
        header.set_mode(mode);
        header.set_mtime(0);
        header.set_entry_type(::tar::EntryType::Regular);
        builder
            .append_data(&mut header, &path, bytes.as_slice())
            .unwrap();
    }
    builder.into_inner().unwrap()
}

fn signed(agent: &str, version: &str, tar: &[u8], reads: &[&str]) -> (SignedHarnessBundle, String) {
    let bundle = SignedHarnessBundle {
        sha256: sha(tar),
        agent: agent.into(),
        version: version.into(),
        architecture: std::env::consts::ARCH.into(),
        executable: format!("bin/{agent}"),
        adapter: wire::adapter_for(agent).unwrap().into(),
        state_format: format!("{agent}-sessions-v1"),
        reads_state_formats: std::iter::once(format!("{agent}-sessions-v1"))
            .chain(reads.iter().map(|r| (*r).to_owned()))
            .collect(),
        capabilities: vec![],
    };
    let signature = STANDARD.encode(signing().sign(&bundle.signing_bytes()).to_bytes());
    (bundle, signature)
}

struct Fixture {
    dir: tempfile::TempDir,
    config: Config,
}
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let image = |version: &str| {
        Some(HarnessBinding {
            sha256: None,
            version: version.into(),
            state_format: "image-tested-v1".into(),
        })
    };
    let config = Config {
        root: dir.path().join("data/harnesses"),
        verifying_key: signing().verifying_key(),
        architecture: std::env::consts::ARCH.into(),
        agents: vec![
            AgentConfig {
                agent: "claude".into(),
                image: image("2.1.300"),
            },
            AgentConfig {
                agent: "codex".into(),
                image: None,
            },
        ],
    };
    Fixture { dir, config }
}

/// Install a bundle the way the downloader does, without HTTP.
fn install(h: &Harnesses, agent: &str, version: &str, update: &str, reads: &[&str]) -> String {
    install_with(h, agent, version, update, reads, &[])
}
fn install_with(
    h: &Harnesses,
    agent: &str,
    version: &str,
    update: &str,
    reads: &[&str],
    extra: &[(&str, &[u8], u32)],
) -> String {
    let tar = bundle_tar(agent, version, extra);
    let (bundle, signature) = signed(agent, version, &tar, reads);
    let dir = h.config().root.join("incoming").join(update);
    private_directory(&dir).unwrap();
    let compressed = dir.join("bundle.tar.zst");
    std::fs::write(&compressed, zstd::encode_all(tar.as_slice(), 3).unwrap()).unwrap();
    store::decompress(&compressed, &dir.join("bundle.tar"), &bundle.sha256).unwrap();
    let tree = dir.join("tree");
    private_directory(&tree).unwrap();
    let executable = store::extract(&dir.join("bundle.tar"), &tree, &bundle).unwrap();
    store::probe(&executable, version).unwrap();
    let sha = bundle.sha256.clone();
    h.stage(
        &tree,
        BundleFile {
            bundle,
            signature,
            update_id: update.into(),
        },
    )
    .unwrap();
    sha
}

#[test]
fn bundles_adopt_side_by_side_and_keep_what_is_in_use() {
    let f = fixture();
    let h = Harnesses::open(f.config.clone());
    // Nothing installed: new processes use the image's CLI.
    assert_eq!(h.executable("claude"), None);
    assert_eq!(h.current("claude").unwrap().version, "2.1.300");
    // The image's CLI is known for claude only; codex is not polled for.
    let poll = h.poll();
    assert_eq!(poll.agents.len(), 1);
    assert_eq!(poll.agents[0].adapters, vec!["claude-stream-json-v1"]);

    // A native helper inside the bundle stands in for a long-running CLI.
    let (sleep, sleep_args) = native_sleep();
    let first = install_with(
        &h,
        "claude",
        "2.1.312",
        "u1",
        &["image-tested-v1"],
        &[("bin/helper", &sleep, 0o555)],
    );
    // Staged is not adopted: running and new processes still use the image.
    assert_eq!(h.executable("claude"), None);
    assert_eq!(h.adopt_staged().unwrap(), vec!["claude"]);
    let v1 = h.executable("claude").unwrap();
    assert_eq!(v1, h.bundle_dir(&first).join("bin/claude"));
    // Installed read-only.
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(h.bundle_dir(&first))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o555
    );
    let attestation = &h.pending_attestations()[0];
    assert_eq!(attestation.outcome, HarnessOutcome::Adopted);
    assert_eq!(attestation.previous_sha256, None);

    // A process started from v1 keeps running across the next adoptions.
    let mut running = std::process::Command::new(h.bundle_dir(&first).join("bin/helper"))
        .args(sleep_args)
        .spawn()
        .unwrap();
    let second = install(&h, "claude", "2.1.313", "u2", &[]);
    h.adopt_staged().unwrap();
    assert_eq!(
        h.state().active["claude"].previous_sha256,
        Some(first.clone())
    );
    let third = install(&h, "claude", "2.1.314", "u3", &[]);
    h.adopt_staged().unwrap();
    assert_eq!(
        h.executable("claude").unwrap(),
        h.bundle_dir(&third).join("bin/claude")
    );
    assert!(running.try_wait().unwrap().is_none(), "never killed");
    // v1 is no longer active or previous, but a live process uses it.
    assert!(h.bundle_dir(&first).is_dir());
    assert!(h.bundle_dir(&second).is_dir());
    running.kill().unwrap();
    running.wait().unwrap();
    h.collect_garbage();
    assert!(!h.bundle_dir(&first).exists());
    assert!(h.bundle_dir(&second).is_dir(), "previous kept for rollback");

    // The active bundle's files vanish: fall back to the previous bundle.
    store_remove(&h.bundle_dir(&third));
    assert_eq!(
        h.executable("claude").unwrap(),
        h.bundle_dir(&second).join("bin/claude")
    );
    store_remove(&h.bundle_dir(&second));
    assert_eq!(h.executable("claude"), None);
    drop(f.dir);
}
/// A native (ELF) sleep, so `/proc/<pid>/exe` names the bundle file.
fn native_sleep() -> (Vec<u8>, &'static [&'static str]) {
    for (path, args) in [
        ("/bin/sleep", &["30"][..]),
        ("/usr/bin/sleep", &["30"][..]),
        ("/usr/bin/coreutils", &["--coreutils-prog=sleep", "30"][..]),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            if bytes.starts_with(b"\x7fELF") {
                return (bytes, args);
            }
        }
    }
    panic!("no native sleep executable");
}
fn store_remove(path: &Path) {
    let _ = std::process::Command::new("chmod")
        .args(["-R", "u+w"])
        .arg(path)
        .status();
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn incompatible_bundles_stay_pinned_and_cold_start_adopts_staged() {
    let f = fixture();
    let h = Harnesses::open(f.config.clone());
    let tar = bundle_tar("claude", "2.1.400", &[]);
    // Cannot continue the image CLI's session state.
    let (foreign, _) = signed("claude", "2.1.400", &tar, &[]);
    assert_eq!(
        h.compatible(&foreign).unwrap_err().to_string(),
        STATE_INCOMPATIBLE
    );
    let mut wrong_adapter = signed("claude", "2.1.400", &tar, &["image-tested-v1"]).0;
    wrong_adapter.adapter = "claude-stream-json-v9".into();
    assert_eq!(
        h.compatible(&wrong_adapter).unwrap_err().to_string(),
        ADAPTER_INCOMPATIBLE
    );
    // An agent whose current CLI is unknown is never updated.
    let (codex, _) = signed("codex", "0.166.0", &tar, &[]);
    assert_eq!(
        h.compatible(&codex).unwrap_err().to_string(),
        STATE_INCOMPATIBLE
    );

    let staged = install(&h, "claude", "2.1.312", "u1", &["image-tested-v1"]);
    assert!(h.has_staged());
    drop(h);
    // A restart is a boundary: no harness process exists yet.
    let reopened = Harnesses::open(f.config.clone());
    assert!(!reopened.has_staged());
    assert_eq!(reopened.state().active["claude"].sha256, staged);
    assert_eq!(reopened.pending_attestations().len(), 1);
}

#[test]
fn artifacts_are_verified_before_anything_is_installed() {
    let f = fixture();
    let dir = f.dir.path().join("work");
    private_directory(&dir).unwrap();
    let tar = bundle_tar("claude", "2.1.312", &[]);
    let (bundle, signature) = signed("claude", "2.1.312", &tar, &[]);
    verify_bundle(
        &bundle,
        &signature,
        &signing().verifying_key(),
        std::env::consts::ARCH,
    )
    .unwrap();
    let mut tampered = bundle.clone();
    tampered.version = "2.1.313".into();
    assert!(
        verify_bundle(
            &tampered,
            &signature,
            &signing().verifying_key(),
            std::env::consts::ARCH
        )
        .is_err()
    );
    assert!(
        verify_bundle(
            &bundle,
            &signature,
            &SigningKey::from_bytes(&[1; 32]).verifying_key(),
            std::env::consts::ARCH
        )
        .is_err()
    );
    assert!(verify_bundle(&bundle, &signature, &signing().verifying_key(), "riscv64").is_err());

    // Digest mismatch.
    let compressed = dir.join("evil.zst");
    std::fs::write(&compressed, zstd::encode_all(&b"evil"[..], 3).unwrap()).unwrap();
    assert_eq!(
        store::decompress(&compressed, &dir.join("evil.tar"), &bundle.sha256)
            .unwrap_err()
            .to_string(),
        ARTIFACT_INVALID
    );
    // Links and traversal are refused.
    for evil in ["link", "traversal"] {
        let mut builder = ::tar::Builder::new(Vec::new());
        let mut header = ::tar::Header::new_gnu();
        header.set_size(0);
        if evil == "link" {
            header.set_entry_type(::tar::EntryType::Symlink);
            builder
                .append_link(&mut header, "bin/claude", "/etc/passwd")
                .unwrap();
        } else {
            header.set_entry_type(::tar::EntryType::Regular);
            header.as_old_mut().name[..9].copy_from_slice(b"../escape");
            header.set_cksum();
            builder.append(&header, &[][..]).unwrap();
        }
        let path = dir.join(format!("{evil}.tar"));
        std::fs::write(&path, builder.into_inner().unwrap()).unwrap();
        let tree = dir.join(format!("{evil}-tree"));
        private_directory(&tree).unwrap();
        assert!(store::extract(&path, &tree, &bundle).is_err(), "{evil}");
    }
    // The probe demands the exact version.
    let tree = dir.join("tree");
    private_directory(&tree).unwrap();
    std::fs::write(dir.join("ok.tar"), &tar).unwrap();
    let executable = store::extract(&dir.join("ok.tar"), &tree, &bundle).unwrap();
    store::probe(&executable, "2.1.312").unwrap();
    assert_eq!(
        store::probe(&executable, "2.1.31").unwrap_err().to_string(),
        PROBE_FAILED
    );
    assert!(store::reports_version("claude 2.1.312 (x)", "2.1.312"));
    assert!(!store::reports_version("2.1.3120", "2.1.312"));
    assert!(!store::reports_version("12.1.312", "2.1.312"));
}

#[test]
fn image_bindings_come_from_the_managed_harness_release() {
    let dir = tempfile::tempdir().unwrap();
    let release = "a".repeat(64);
    let path = dir.path().join("releases").join(&release);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(
        path.join("manifest.json"),
        r#"{"stateFormat":"codex-claude-tested-v1","harnesses":{"claude":{"version":"2.1.312"},"codex":{"version":"0.165.0"}}}"#,
    )
    .unwrap();
    let manifest = image_manifest(Some(dir.path().into()), Some(release)).unwrap();
    let claude = image_binding(&manifest, "claude").unwrap();
    assert_eq!(claude.version, "2.1.312");
    assert_eq!(claude.state_format, "codex-claude-tested-v1");
    assert!(image_binding(&manifest, "grok").is_none());
    assert!(image_manifest(Some(dir.path().into()), Some("../x".into())).is_none());
}

struct Idle;
impl crate::updates::Activity for Idle {
    fn turn_active(&self) -> bool {
        false
    }
    fn mailbox_quiescent(&self) -> bool {
        true
    }
}

/// Poll → offer → download over HTTP → verify → install → adopt → attest,
/// against a fake server, with a broken offer refused and attested.
#[tokio::test]
async fn poll_download_adopt_and_attest_against_a_fake_server() {
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    let f = fixture();
    let h = Harnesses::open(f.config.clone());
    let tar = bundle_tar("claude", "2.1.312", &[]);
    let compressed = zstd::encode_all(tar.as_slice(), 3).unwrap();
    let (bundle, signature) = signed("claude", "2.1.312", &tar, &["image-tested-v1"]);
    let (bad, _) = signed("claude", "2.1.399", &tar, &["image-tested-v1"]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let polls: Arc<Mutex<Vec<HarnessPoll>>> = Arc::default();
    let attested: Arc<Mutex<Vec<HarnessAttestation>>> = Arc::default();
    let offer = HarnessOffer {
        directives: vec![
            HarnessDirective {
                update_id: "good".into(),
                artifact: HarnessArtifact {
                    bundle: bundle.clone(),
                    signature: signature.clone(),
                    url: format!("{origin}/artifact"),
                    compressed_size_bytes: compressed.len() as u64,
                    url_expires_at: crate::now_ms() + 600_000,
                },
            },
            HarnessDirective {
                update_id: "forged".into(),
                artifact: HarnessArtifact {
                    // A signature for another bundle.
                    bundle: bad,
                    signature: signature.clone(),
                    url: format!("{origin}/artifact"),
                    compressed_size_bytes: compressed.len() as u64,
                    url_expires_at: crate::now_ms() + 600_000,
                },
            },
        ],
    };
    let app = Router::new()
        .route(
            "/v1/cloud/workspaces/workspace-original/runtime/harness-updates",
            post({
                let polls = polls.clone();
                move |Json(poll): Json<HarnessPoll>| {
                    let offer = offer.clone();
                    polls.lock().unwrap().push(poll);
                    async move { Json(offer) }
                }
            }),
        )
        .route(
            "/v1/cloud/workspaces/workspace-original/runtime/harness-updates/attestation",
            post({
                let attested = attested.clone();
                move |Json(a): Json<HarnessAttestation>| {
                    attested.lock().unwrap().push(a);
                    async { Json(serde_json::json!({"recorded": true})) }
                }
            }),
        )
        .route("/artifact", get(move || async move { compressed.clone() }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let authority = crate::Authority::new(&origin).unwrap();
    let boot: zuse_protocol::cloud::CloudRuntimeBootstrap = serde_json::from_value(serde_json::json!({"workspaceId":"workspace-original","zuseAccountId":"user-original","chatId":"chat","initialSessionId":"session","runtimeCredential":"synthetic-runtime","runtimeGeneration":2,"gatewayEpoch":1,"runtimeCredentialExpiresAt":9999999999999i64,"gatewayUrl":"wss://test.invalid","gatewayProtocol":"test","sealedTranscriptKey":"synthetic"})).unwrap();
    let (_boot_tx, boot_rx) = tokio::sync::watch::channel(boot);
    let updater = Updater::new(h.clone()).unwrap().with_grace(Duration::ZERO);
    let cancel = tokio_util::sync::CancellationToken::new();
    updater.cycle(&authority, &boot_rx, &Idle, &cancel).await;
    // Staged, not yet adopted; the forged offer was refused before download.
    assert!(h.has_staged());
    assert_eq!(h.executable("claude"), None);
    updater.adopt(&authority, &boot_rx).await;
    assert_eq!(
        h.executable("claude").unwrap(),
        h.bundle_dir(&bundle.sha256).join("bin/claude")
    );
    let poll = polls.lock().unwrap()[0].clone();
    assert_eq!(poll.agents[0].current.as_ref().unwrap().version, "2.1.300");
    let reports = attested.lock().unwrap().clone();
    assert!(
        reports
            .iter()
            .any(|a| a.update_id.as_deref() == Some("forged")
                && a.outcome == HarnessOutcome::Failed
                && a.error_code.as_deref() == Some(ARTIFACT_INVALID))
    );
    assert!(
        reports
            .iter()
            .any(|a| a.update_id.as_deref() == Some("good")
                && a.outcome == HarnessOutcome::Adopted
                && a.sha256 == bundle.sha256)
    );
    assert!(h.pending_attestations().is_empty());
    // The next poll reports the adopted bundle; handled offers are not retried.
    updater.cycle(&authority, &boot_rx, &Idle, &cancel).await;
    let poll = polls.lock().unwrap()[1].clone();
    assert_eq!(
        poll.agents[0].current.as_ref().unwrap().sha256.as_deref(),
        Some(bundle.sha256.as_str())
    );
    assert!(!h.has_staged());
    assert_eq!(attested.lock().unwrap().len(), reports.len());
    server.abort();
}
