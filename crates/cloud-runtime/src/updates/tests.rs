use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use std::{os::unix::fs::PermissionsExt, time::Duration};
use swap::{Decision, Phase};
use zuse_protocol::cloud_runtime_updates::{Outcome, SignedRelease};

fn signing() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}
fn release(sha256: &str) -> SignedRelease {
    SignedRelease {
        sha256: sha256.into(),
        version: "0.27.0".into(),
        architecture: std::env::consts::ARCH.into(),
        storage_formats: vec![zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT.into()],
    }
}
fn sign(release: &SignedRelease) -> String {
    STANDARD.encode(signing().sign(&release.signing_bytes()).to_bytes())
}
fn sha(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// `<root>/bin/zuse-cloud-runtime`, data in `<root>/data`.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    config: Config,
}
fn fixture(layout: impl FnOnce(&Path) -> Layout) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("bin")).unwrap();
    let binary = root.join("bin").join(stage::BINARY);
    let config = Config {
        data: root.join("data"),
        layout: layout(&root),
        binary,
        verifying_key: signing().verifying_key(),
        architecture: std::env::consts::ARCH.into(),
    };
    Fixture {
        _dir: dir,
        root,
        config,
    }
}
fn owned() -> Fixture {
    fixture(|root| Layout::Owned {
        journal: root.join("runtime-update"),
    })
}

/// A stand-in runtime: `runtime-capabilities` prints a valid receipt for its
/// own digest (as the real binary does). `marker` makes each version unique.
fn runtime_script(marker: &str) -> Vec<u8> {
    let receipt = serde_json::to_string(&crate::capabilities::CommandAdmissionReceipt {
        version: 1,
        runtime_sha256: "SHA".into(),
        command_admission_version: zuse_protocol::cloud_task_commands::VERSION,
        runtime_rpc_protocol: zuse_protocol::cloud_gateway::RUST_RPC.into(),
        storage_format: zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT.into(),
        session_command_version: 1,
        execution_progress_version: 1,
        managed_agent_version: Some(1),
    })
    .unwrap();
    let (before, after) = receipt.split_once("SHA").unwrap();
    format!(
        "#!/bin/sh\n# {marker}\n[ \"$1\" = runtime-capabilities ] || exit 2\n\
         sha=$(/usr/bin/sha256sum \"$0\" | /usr/bin/cut -d' ' -f1)\n\
         printf '%s' '{before}'\"$sha\"'{after}'\n"
    )
    .into_bytes()
}
fn write_executable(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
/// Stage `bytes` the way the poller does (incoming directory, then rename).
fn stage_bytes(config: &Config, update_id: &str, bytes: &[u8]) -> stage::Staged {
    let release = release(&sha(bytes));
    let incoming = stage::incoming(config).unwrap();
    let binary = incoming.join(stage::BINARY);
    std::fs::write(&binary, bytes).unwrap();
    stage::stage(
        config,
        stage::ReleaseFile {
            signature: sign(&release),
            release,
            update_id: update_id.into(),
        },
        &binary,
    )
    .unwrap()
}

#[test]
fn release_verification_checks_signature_architecture_and_storage() {
    let key = signing().verifying_key();
    let good = release(&"a".repeat(64));
    let arch = std::env::consts::ARCH;
    verify::release(&good, &sign(&good), &key, arch).unwrap();

    let mut tampered = good.clone();
    tampered.version = "9.9.9".into();
    let error = verify::release(&tampered, &sign(&good), &key, arch).unwrap_err();
    assert_eq!(error.to_string(), ARTIFACT_INVALID);

    let other = if arch == "x86_64" {
        "aarch64"
    } else {
        "x86_64"
    };
    let error = verify::release(&good, &sign(&good), &key, other).unwrap_err();
    assert_eq!(error.to_string(), ARTIFACT_INVALID);

    let mut storage = good.clone();
    storage.storage_formats = vec!["future-format".into()];
    let error = verify::release(&storage, &sign(&storage), &key, arch).unwrap_err();
    assert_eq!(error.to_string(), STORAGE_INCOMPATIBLE);

    assert!(verify::release(&good, "not base64!", &key, arch).is_err());
}

#[test]
fn production_release_key_parses() {
    verify::release_key(zuse_protocol::cloud_runtime_updates::RELEASE_SIGNING_PUBLIC_KEY).unwrap();
    assert!(verify::release_key("AAAA").is_err());
}

#[test]
fn decompress_verifies_the_digest_and_stages_by_rename() {
    let f = owned();
    let bytes = runtime_script("v2");
    let release = release(&sha(&bytes));
    let compressed = f.root.join("artifact.zst");
    std::fs::write(&compressed, zstd::encode_all(&bytes[..], 3).unwrap()).unwrap();

    let incoming = stage::incoming(&f.config).unwrap();
    let binary = stage::decompress(&compressed, &incoming, &release).unwrap();
    let staged = stage::stage(
        &f.config,
        stage::ReleaseFile {
            signature: sign(&release),
            release: release.clone(),
            update_id: "update-1".into(),
        },
        &binary,
    )
    .unwrap();
    assert!(!incoming.exists());
    assert_eq!(std::fs::read(&staged.binary).unwrap(), bytes);
    let mode = std::fs::metadata(&staged.binary)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
    let latest = stage::latest(&f.config).unwrap();
    assert_eq!(latest.file.update_id, "update-1");

    // A different digest is refused before anything is staged.
    let wrong = self::release(&"b".repeat(64));
    let incoming = stage::incoming(&f.config).unwrap();
    let error = stage::decompress(&compressed, &incoming, &wrong).unwrap_err();
    assert_eq!(error.to_string(), ARTIFACT_INVALID);
}

#[test]
fn staging_keeps_one_release_and_ignores_tampered_metadata() {
    let f = owned();
    let first = stage_bytes(&f.config, "update-1", b"first");
    let second = stage_bytes(&f.config, "update-2", b"second");
    assert!(!first.dir.exists());
    assert_eq!(stage::latest(&f.config).unwrap().dir, second.dir);

    let mut file = second.file.clone();
    file.release.version = "tampered".into();
    std::fs::write(
        second.dir.join("release.json"),
        serde_json::to_vec(&file).unwrap(),
    )
    .unwrap();
    assert!(stage::latest(&f.config).is_none());
}

#[test]
fn layout_prefers_an_owned_binary_then_the_root_helper() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("bin").join(stage::BINARY);
    write_executable(&binary, b"runtime");
    let helper = dir.path().join("helper.py");
    let result = dir.path().join("result.json");
    assert_eq!(
        detect_layout(&binary, &helper, &result),
        Layout::Owned {
            journal: dir.path().join("runtime-update")
        }
    );
    let missing = dir.path().join("missing").join(stage::BINARY);
    assert_eq!(
        detect_layout(&missing, &helper, &result),
        Layout::Unsupported
    );
    std::fs::write(&helper, b"").unwrap();
    assert_eq!(
        detect_layout(&missing, &helper, &result),
        Layout::Root { result }
    );
}

#[test]
fn state_lists_are_deduplicated_and_bounded() {
    let mut list = Vec::new();
    for i in 0..40 {
        State::push(&mut list, &format!("id-{i}"));
        State::push(&mut list, &format!("id-{i}"));
    }
    assert_eq!(list.len(), 32);
    assert_eq!(list.first().unwrap(), "id-8");
}

#[cfg(target_os = "linux")]
#[test]
fn owned_swap_probes_keeps_previous_and_commits_after_probation() {
    let f = owned();
    let old = runtime_script("v1");
    let new = runtime_script("v2");
    write_executable(&f.config.binary, &old);
    let staged = stage_bytes(&f.config, "update-1", &new);
    let Layout::Owned { journal } = f.config.layout.clone() else {
        unreachable!()
    };

    let next = swap::install(&f.config, &staged, &sha(&old), Duration::ZERO, &|| false).unwrap();
    assert_eq!(next.as_deref(), Some(f.config.binary.as_path()));
    assert_eq!(std::fs::read(&f.config.binary).unwrap(), new);
    assert_eq!(std::fs::read(journal.join("previous")).unwrap(), old);
    assert_eq!(swap::read_journal(&journal).unwrap().phase, Phase::Swapped);
    let pending = stage::read_pending(&f.config).unwrap();
    assert_eq!(swap::decision(&f.config, &pending), Decision::Installed);
    // The old image never attests the swap it handed over.
    assert_eq!(
        swap::attestation_outcome(&f.config, &pending, &sha(&old)),
        None
    );

    // New runtime starts: probation, then commits once healthy.
    assert_eq!(
        swap::cold_boot(&f.config, &sha(&new), Duration::ZERO).unwrap(),
        None
    );
    assert_eq!(
        swap::read_journal(&journal).unwrap().phase,
        Phase::Probation
    );
    assert_eq!(
        swap::attestation_outcome(&f.config, &pending, &sha(&new)),
        None
    );
    swap::commit_with(&f.config, &sha(&new)).unwrap();
    assert_eq!(
        swap::read_journal(&journal).unwrap().phase,
        Phase::Committed
    );
    assert_eq!(
        swap::attestation_outcome(&f.config, &pending, &sha(&new)),
        Some((Outcome::Committed, None))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn owned_swap_rolls_back_when_the_new_runtime_never_becomes_healthy() {
    let f = owned();
    let old = runtime_script("v1");
    let new = runtime_script("v2");
    write_executable(&f.config.binary, &old);
    let staged = stage_bytes(&f.config, "update-1", &new);
    let Layout::Owned { journal } = f.config.layout.clone() else {
        unreachable!()
    };
    swap::install(&f.config, &staged, &sha(&old), Duration::ZERO, &|| false)
        .unwrap()
        .unwrap();
    // First start enters probation; a second start still in probation rolls back.
    swap::cold_boot(&f.config, &sha(&new), Duration::ZERO).unwrap();
    let next = swap::cold_boot(&f.config, &sha(&new), Duration::ZERO).unwrap();
    assert_eq!(next.as_deref(), Some(f.config.binary.as_path()));
    assert_eq!(std::fs::read(&f.config.binary).unwrap(), old);
    let entry = swap::read_journal(&journal).unwrap();
    assert_eq!(entry.phase, Phase::RolledBack);

    // The restored runtime attests the rollback and does not retry the release.
    let pending = stage::read_pending(&f.config).unwrap();
    assert_eq!(
        swap::attestation_outcome(&f.config, &pending, &sha(&old)),
        Some((Outcome::RolledBack, Some(ROLLED_BACK.into())))
    );
    assert_eq!(
        swap::cold_boot(&f.config, &sha(&old), Duration::ZERO).unwrap(),
        None
    );
    assert_eq!(std::fs::read(&f.config.binary).unwrap(), old);
}

#[cfg(target_os = "linux")]
#[test]
fn failed_start_in_probation_restores_previous() {
    let f = owned();
    let old = runtime_script("v1");
    let new = runtime_script("v2");
    write_executable(&f.config.binary, &old);
    let staged = stage_bytes(&f.config, "update-1", &new);
    swap::install(&f.config, &staged, &sha(&old), Duration::ZERO, &|| false)
        .unwrap()
        .unwrap();
    swap::cold_boot(&f.config, &sha(&new), Duration::ZERO).unwrap();
    let next = swap::rollback_with(&f.config, &sha(&new)).unwrap();
    assert_eq!(next.as_deref(), Some(f.config.binary.as_path()));
    assert_eq!(std::fs::read(&f.config.binary).unwrap(), old);
}

#[cfg(target_os = "linux")]
#[test]
fn owned_swap_refuses_a_release_whose_probe_fails() {
    let f = owned();
    let old = runtime_script("v1");
    write_executable(&f.config.binary, &old);
    let broken = b"#!/bin/sh\nexit 1\n".to_vec();
    let staged = stage_bytes(&f.config, "update-1", &broken);
    let Layout::Owned { journal } = f.config.layout.clone() else {
        unreachable!()
    };
    let next = swap::install(&f.config, &staged, &sha(&old), Duration::ZERO, &|| false).unwrap();
    assert_eq!(next, None);
    assert_eq!(std::fs::read(&f.config.binary).unwrap(), old);
    let entry = swap::read_journal(&journal).unwrap();
    assert_eq!(
        (entry.phase, entry.error_code.as_deref()),
        (Phase::Failed, Some(PROBE_FAILED))
    );
    assert!(!journal.join("previous").exists());
    assert!(stage::latest(&f.config).is_none());
    let pending = stage::read_pending(&f.config).unwrap();
    assert_eq!(
        swap::attestation_outcome(&f.config, &pending, &sha(&old)),
        Some((Outcome::Failed, Some(PROBE_FAILED.into())))
    );
}

fn root() -> Fixture {
    fixture(|root| Layout::Root {
        result: root.join("result.json"),
    })
}
/// Plays the root helper: waits for `request.json`, then writes `result`.
fn helper(f: &Fixture, outcome: &'static str) -> std::thread::JoinHandle<()> {
    let request = stage::request_path(&f.config);
    let result = f.root.join("result.json");
    std::thread::spawn(move || {
        for _ in 0..500 {
            if let Some(request) = read_json::<stage::Request>(&request) {
                let body = serde_json::json!({
                    "updateId": request.update_id,
                    "outcome": outcome,
                    "sha256": request.sha256,
                    "previousSha256": "c".repeat(64),
                    "errorCode": (outcome != "committed").then_some("runtime_update_probe_failed"),
                });
                std::fs::write(&result, serde_json::to_vec(&body).unwrap()).unwrap();
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })
}

#[test]
fn root_swap_restarts_once_the_helper_commits() {
    let f = root();
    let staged = stage_bytes(&f.config, "update-1", b"new");
    let helper = helper(&f, "committed");
    let next = swap::install(
        &f.config,
        &staged,
        &"c".repeat(64),
        Duration::from_secs(10),
        &|| false,
    )
    .unwrap();
    helper.join().unwrap();
    assert_eq!(next.as_deref(), Some(f.config.binary.as_path()));
    assert!(!stage::request_path(&f.config).exists());
    let pending = stage::read_pending(&f.config).unwrap();
    assert_eq!(
        swap::attestation_outcome(&f.config, &pending, &sha(b"new")),
        Some((Outcome::Committed, None))
    );
}

#[test]
fn root_swap_keeps_serving_when_the_helper_refuses() {
    let f = root();
    let staged = stage_bytes(&f.config, "update-1", b"new");
    let helper = helper(&f, "failed");
    let next = swap::install(
        &f.config,
        &staged,
        &"c".repeat(64),
        Duration::from_secs(10),
        &|| false,
    )
    .unwrap();
    helper.join().unwrap();
    assert_eq!(next, None);
    assert!(stage::latest(&f.config).is_none());
    let pending = stage::read_pending(&f.config).unwrap();
    assert_eq!(
        swap::attestation_outcome(&f.config, &pending, &"c".repeat(64)),
        Some((Outcome::Failed, Some(PROBE_FAILED.into())))
    );
}

#[test]
fn root_swap_withdraws_an_unanswered_request() {
    let f = root();
    let staged = stage_bytes(&f.config, "update-1", b"new");
    let next = swap::install(
        &f.config,
        &staged,
        &"c".repeat(64),
        Duration::from_millis(200),
        &|| false,
    )
    .unwrap();
    assert_eq!(next, None);
    assert!(!stage::request_path(&f.config).exists());
    let mut pending = stage::read_pending(&f.config).unwrap();
    assert_eq!(
        swap::attestation_outcome(&f.config, &pending, &"c".repeat(64)),
        None
    );
    pending.requested_at -= swap::UNCONFIRMED_AFTER_MS + 1;
    assert_eq!(
        swap::attestation_outcome(&f.config, &pending, &"c".repeat(64)),
        Some((Outcome::Failed, Some(SWAP_UNCONFIRMED.into())))
    );
    // Cold boot does not ask again for a release that awaits attestation.
    assert_eq!(
        swap::cold_boot(&f.config, &"c".repeat(64), Duration::ZERO).unwrap(),
        None
    );
    assert!(!stage::request_path(&f.config).exists());
}

#[test]
fn root_result_for_another_update_is_ignored() {
    let f = root();
    let staged = stage_bytes(&f.config, "update-1", b"new");
    std::fs::write(
        f.root.join("result.json"),
        serde_json::to_vec(&serde_json::json!({
            "updateId": "update-0",
            "outcome": "committed",
            "sha256": staged.file.release.sha256,
        }))
        .unwrap(),
    )
    .unwrap();
    swap::request_root(&f.config, &staged, &"c".repeat(64)).unwrap();
    let pending = stage::read_pending(&f.config).unwrap();
    assert_eq!(swap::decision(&f.config, &pending), Decision::Undecided);
}

#[test]
fn cold_boot_discards_a_rejected_release() {
    let f = root();
    stage_bytes(&f.config, "update-1", b"new");
    let mut state = State::default();
    State::push(&mut state.rejected, "update-1");
    state.save(&f.config).unwrap();
    assert_eq!(
        swap::cold_boot(&f.config, &"c".repeat(64), Duration::ZERO).unwrap(),
        None
    );
    assert!(stage::latest(&f.config).is_none());
    assert!(!stage::request_path(&f.config).exists());
}

#[test]
fn cold_boot_does_nothing_without_a_way_to_swap() {
    let f = fixture(|_| Layout::Unsupported);
    stage_bytes(&f.config, "update-1", b"new");
    assert_eq!(
        swap::cold_boot(&f.config, &"c".repeat(64), Duration::ZERO).unwrap(),
        None
    );
    assert!(stage::read_pending(&f.config).is_none());
}

#[test]
fn mailbox_blocks_a_swap_until_every_lease_is_acknowledged_and_delivered() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("runtime.sqlite3");
    assert!(!agent::mailbox_quiescent(&database));
    let db = rusqlite::Connection::open(&database).unwrap();
    db.execute_batch("CREATE TABLE unrelated (id INTEGER);")
        .unwrap();
    assert!(agent::mailbox_quiescent(&database));
    db.execute_batch(
        "CREATE TABLE zuse_cloud_command_receipts (command_id TEXT PRIMARY KEY, acknowledgment TEXT, delivered INTEGER NOT NULL DEFAULT 0);",
    )
    .unwrap();
    assert!(agent::mailbox_quiescent(&database));
    db.execute(
        "INSERT INTO zuse_cloud_command_receipts(command_id) VALUES ('a')",
        [],
    )
    .unwrap();
    assert!(!agent::mailbox_quiescent(&database));
    db.execute(
        "UPDATE zuse_cloud_command_receipts SET acknowledgment='{}' WHERE command_id='a'",
        [],
    )
    .unwrap();
    assert!(!agent::mailbox_quiescent(&database));
    db.execute(
        "UPDATE zuse_cloud_command_receipts SET delivered=1 WHERE command_id='a'",
        [],
    )
    .unwrap();
    assert!(agent::mailbox_quiescent(&database));
}

#[test]
fn bootstrap_capabilities_advertise_runtime_updates() {
    assert!(
        crate::identity::bootstrap_capabilities()
            .iter()
            .any(|c| c == zuse_protocol::cloud_runtime_updates::CAPABILITY)
    );
}
