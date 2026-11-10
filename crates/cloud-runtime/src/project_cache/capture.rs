//! Preparation-only snapshot barrier. Never enrolls a task or uses legacy retirement.
use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
const JOURNAL: &str = "/var/lib/zuse/project-cache-capture";
const HYGIENE: &str = include_str!("../../../../infra/cloud/base-supervisor-hygiene.py");
fn protected(path: &Path, owner: u32, directory: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.uid() == owner
            && !metadata.file_type().is_symlink()
            && if directory {
                metadata.is_dir() && metadata.mode() & 0o777 == 0o700
            } else {
                metadata.is_file() && metadata.nlink() == 1 && metadata.mode() & 0o777 == 0o600
            },
        "project_cache_capture_journal_unsafe"
    );
    Ok(())
}
fn store(root: &Path, key: &str, owner: u32) -> Result<PathBuf> {
    directory(root)?;
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    protected(root, owner, true)?;
    let path = root.join(key);
    directory(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    protected(&path, owner, true)?;
    Ok(path)
}
fn saved(path: &Path, owner: u32) -> Result<Value> {
    protected(path, owner, false)?;
    bounded_json(path)
}
fn ready(plan: &Plan, path: &Path, owner: u32) -> Result<Value> {
    // A durable stop receipt does not authorize a later bound runtime. Restored
    // verifiers may have a fresh idle resident, but must remain authority-free.
    crate::image::clean_home(
        &plan.home,
        plan.home.join(".zuse-data/.runtime-supervision").exists(),
    )?;
    let value = saved(&path.join("ready.json"), owner)?;
    ensure!(
        value["version"] == 1
            && value["preparationCaptureVersion"] == 1
            && value["cacheKey"] == plan.cache_key
            && value["operationId"] == plan.operation_id
            && value["runtimeSha256"] == plan.identity["baseRuntimeSha256"]
            && value["preparedReceiptSha256"]
                == digest_file(
                    &plan.cache_root.join(&plan.cache_key).join("receipt.json"),
                    MAX_PLAN
                )?,
        "project_cache_capture_receipt_mismatch"
    );
    validate_hygiene(&value["hygiene"], plan)?;
    Ok(value)
}
fn validate_hygiene(value: &Value, plan: &Plan) -> Result<()> {
    ensure!(
        value["version"] == 1
            && value["operationId"] == plan.operation_id
            && value["runtimeSha256"] == plan.identity["baseRuntimeSha256"]
            && value["atomicStopAcknowledged"] == true
            && value["legacyBuilder"] == false
            && value["originalResidentExited"] == true
            && value["supervisorLockReleased"] == true
            && value["runtimeDataAbsent"] == true
            && value["enabledBootServiceRetained"] == true
            && value["residentPid"].as_u64().is_some_and(|pid| pid > 1)
            && value["residentStartTicks"]
                .as_u64()
                .is_some_and(|ticks| ticks > 0)
            && value["bootServiceSha256"].as_str().is_some_and(hash)
            && value["bootNonceSha256"].as_str().is_some_and(hash),
        "project_cache_capture_hygiene_required"
    );
    let removed = value["removed"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("project_cache_capture_hygiene_required"))?;
    ensure!(
        removed.iter().any(|v| v == "supervisor.lock")
            && removed.iter().all(|v| matches!(
                v.as_str(),
                Some("supervisor.lock" | "supervisor.json" | "control.sock")
            )),
        "project_cache_capture_hygiene_required"
    );
    Ok(())
}
fn prepared(plan: &Plan) -> Result<String> {
    let stamp = plan.cache_root.join(&plan.cache_key).join("receipt.json");
    let value = bounded_json(&stamp)?;
    ensure!(
        value["identity"] == plan.identity && value["receipt"] == receipt(plan),
        "project_cache_capture_preparation_mismatch"
    );
    ensure!(
        git(
            Some(&plan.cache_root.join(&plan.cache_key).join("checkout")),
            &[os("rev-parse"), os("HEAD")]
        )? == plan.identity["sourceCommit"],
        "project_cache_capture_commit_mismatch"
    );
    digest_file(&stamp, MAX_PLAN)
}
fn once(
    plan: &Plan,
    root: &Path,
    owner: u32,
    status: bool,
    hygiene: impl FnOnce() -> Result<Value>,
) -> Result<Value> {
    validate(plan)?;
    let target = root.join(&plan.cache_key);
    if status && !target.exists() {
        return Ok(json!({"state":"not-started"}));
    }
    if status {
        protected(root, owner, true)?;
        protected(&target, owner, true)?;
        if target.join("ready.json").exists() {
            return Ok(json!({"state":"ready","receipt":ready(plan,&target,owner)?}));
        }
        if target.join("failed.json").exists() {
            saved(&target.join("failed.json"), owner)?;
            return Ok(json!({"state":"failed"}));
        }
        if target.join("intent.json").exists() {
            let intent = saved(&target.join("intent.json"), owner)?;
            ensure!(
                intent["operationId"] == plan.operation_id && intent["cacheKey"] == plan.cache_key,
                "project_cache_capture_operation_changed"
            );
            return Ok(json!({"state":"uncertain"}));
        }
        return Ok(json!({"state":"not-started"}));
    }
    let target = store(root, &plan.cache_key, owner)?;
    let _lock = crate::DataLock::acquire(&target, None)?;
    if target.join("ready.json").exists() {
        return Ok(json!({"state":"ready","receipt":ready(plan,&target,owner)?}));
    }
    if target.join("failed.json").exists() {
        saved(&target.join("failed.json"), owner)?;
        return Ok(json!({"state":"failed"}));
    }
    if target.join("intent.json").exists() {
        let intent = saved(&target.join("intent.json"), owner)?;
        ensure!(
            intent["operationId"] == plan.operation_id && intent["cacheKey"] == plan.cache_key,
            "project_cache_capture_operation_changed"
        );
        return Ok(json!({"state":"uncertain"}));
    }
    if status {
        return Ok(json!({"state":"not-started"}));
    }
    crate::image::clean_home(&plan.home, plan.supervisor)?;
    let before = prepared(plan)?;
    crate::identity::atomic_write(
        &target.join("intent.json"),
        &serde_json::to_vec(
            &json!({"version":1,"cacheKey":plan.cache_key,"operationId":plan.operation_id,"preparedReceiptSha256":before}),
        )?,
    )?;
    let result = (|| {
        let hygiene = hygiene()?;
        validate_hygiene(&hygiene, plan)?;
        // Prepared checkout/dependencies live under /opt/zuse/project-caches;
        // generic home/runtime authority checks remain completely empty.
        crate::image::clean_home(&plan.home, false)?;
        ensure!(
            prepared(plan)? == before,
            "project_cache_capture_preparation_changed"
        );
        let value = json!({"version":1,"preparationCaptureVersion":1,"cacheKey":plan.cache_key,"operationId":plan.operation_id,"runtimeSha256":plan.identity["baseRuntimeSha256"],"preparedReceiptSha256":before,"hygiene":hygiene});
        crate::identity::atomic_write(&target.join("ready.json"), &serde_json::to_vec(&value)?)?;
        Ok(json!({"state":"ready","receipt":value}))
    })();
    if result.is_err() {
        crate::identity::atomic_write(
            &target.join("failed.json"),
            b"{\"state\":\"failed\",\"code\":\"project_cache_capture_rejected\"}",
        )?;
    }
    result
}
/// Root-owned, immutable plan digest; unsupported/PID1 ownership never snapshots.
pub fn cli(file: &Path, plan_sha: &str, status: bool) -> Result<Value> {
    ensure!(
        unsafe { libc::geteuid() } == 0 && hash(plan_sha),
        "project_cache_capture_operator_required"
    );
    ensure!(
        digest_file(file, MAX_PLAN)? == plan_sha,
        "project_cache_capture_plan_changed"
    );
    let plan: Plan = serde_json::from_value(bounded_json(file)?)?;
    ensure!(
        plan.home == Path::new("/home/zuse")
            && plan.cache_root == Path::new(ROOT)
            && plan.bundle == Path::new("/opt/zuse/preparation/source.bundle")
            && plan.supervisor,
        "project_cache_path_invalid"
    );
    for path in [
        Path::new("/var"),
        Path::new("/var/lib"),
        Path::new("/var/lib/zuse"),
    ] {
        let m = fs::symlink_metadata(path)?;
        ensure!(
            m.is_dir() && !m.file_type().is_symlink() && m.uid() == 0 && m.mode() & 0o022 == 0,
            "project_cache_capture_parent_unsafe"
        );
    }
    once(&plan, Path::new(JOURNAL), 0, status, || {
        let runtime = plan.identity["baseRuntimeSha256"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("project_cache_runtime_mismatch"))?;
        let output = Command::new("/usr/bin/python3")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .args([
                "-I",
                "-B",
                "-c",
                HYGIENE,
                "--runtime-sha256",
                runtime,
                "--operation-id",
                &plan.operation_id,
                "--report-only",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()?;
        ensure!(
            output.status.success() && output.stdout.len() <= MAX_PLAN as usize,
            "project_cache_capture_hygiene_failed"
        );
        let proof: Value = serde_json::from_slice(&output.stdout)?;
        // No preparation descendants or orphan shell survives into a reusable
        // snapshot. Unknown user processes fail closed; they are never killed.
        for entry in fs::read_dir("/proc")? {
            let entry = entry?;
            if !entry
                .file_name()
                .as_encoded_bytes()
                .iter()
                .all(u8::is_ascii_digit)
            {
                continue;
            }
            match fs::metadata(entry.path()) {
                Ok(m) => ensure!(
                    m.uid() != 10001,
                    "project_cache_capture_user_process_present"
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(proof)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hygiene(plan: &Plan) -> Value {
        json!({"version":1,"operationId":plan.operation_id,"runtimeSha256":plan.identity["baseRuntimeSha256"],"atomicStopAcknowledged":true,"legacyBuilder":false,"originalResidentExited":true,"supervisorLockReleased":true,"runtimeDataAbsent":true,"enabledBootServiceRetained":true,"residentPid":42,"residentStartTicks":17,"bootServiceSha256":"a".repeat(64),"bootNonceSha256":"b".repeat(64),"removed":["supervisor.lock","supervisor.json","control.sock"]})
    }
    #[tokio::test]
    async fn capture_preserves_project_and_dependency_files_and_never_repeats_stop() {
        let t = tempfile::tempdir().unwrap();
        let (source, commit) = super::super::tests::source(t.path());
        let plan = super::super::tests::plan(
            t.path(),
            &source,
            &commit,
            &"a".repeat(64),
            "mkdir -p deps; printf retained > deps/value",
        );
        prepare(&plan).await.unwrap();
        let root = t.path().join("protected-capture");
        let owner = unsafe { libc::geteuid() };
        assert_eq!(
            once(&plan, &root, owner, true, || panic!("status cannot stop")).unwrap()["state"],
            "not-started"
        );
        assert!(!root.exists(), "status must not create a journal or lock");
        let first = once(&plan, &root, owner, false, || Ok(hygiene(&plan))).unwrap();
        assert_eq!(first["state"], "ready");
        assert_eq!(
            fs::read_to_string(
                plan.cache_root
                    .join(&plan.cache_key)
                    .join("checkout/deps/value")
            )
            .unwrap(),
            "retained"
        );
        assert_eq!(
            once(&plan, &root, owner, false, || panic!(
                "stop must never replay"
            ))
            .unwrap(),
            first
        );
        assert_eq!(
            once(&plan, &root, owner, true, || panic!("status is read-only")).unwrap(),
            first
        );
        fs::create_dir_all(plan.home.join(".zuse-data")).unwrap();
        let identity = plan.home.join(".zuse-data/cloud-runtime-identity.json");
        fs::write(&identity, "later binding").unwrap();
        assert!(once(&plan, &root, owner, true, || unreachable!()).is_err());
        fs::remove_file(identity).unwrap();
        fs::write(
            plan.cache_root.join(&plan.cache_key).join("receipt.json"),
            b"{}",
        )
        .unwrap();
        assert!(once(&plan, &root, owner, true, || unreachable!()).is_err());
    }
    #[tokio::test]
    async fn capture_rejects_bound_data_legacy_or_missing_atomic_proof_and_keeps_uncertainty() {
        let t = tempfile::tempdir().unwrap();
        let (source, commit) = super::super::tests::source(t.path());
        let plan = super::super::tests::plan(t.path(), &source, &commit, &"b".repeat(64), "true");
        prepare(&plan).await.unwrap();
        let owner = unsafe { libc::geteuid() };
        let root = t.path().join("capture");
        fs::create_dir_all(plan.home.join(".zuse-data")).unwrap();
        fs::write(
            plan.home.join(".zuse-data/cloud-runtime-identity.json"),
            "bound",
        )
        .unwrap();
        assert!(
            once(&plan, &root, owner, false, || panic!(
                "cannot stop bound runtime"
            ))
            .is_err()
        );
        fs::remove_file(plan.home.join(".zuse-data/cloud-runtime-identity.json")).unwrap();
        let target = store(&root, &plan.cache_key, owner).unwrap();
        crate::identity::atomic_write(
            &target.join("intent.json"),
            &serde_json::to_vec(
                &json!({"cacheKey":plan.cache_key,"operationId":plan.operation_id}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            once(&plan, &root, owner, false, || panic!(
                "uncertain stop cannot replay"
            ))
            .unwrap()["state"],
            "uncertain"
        );
        for field in [
            "atomicStopAcknowledged",
            "runtimeDataAbsent",
            "supervisorLockReleased",
        ] {
            let mut invalid = hygiene(&plan);
            invalid[field] = json!(false);
            assert!(validate_hygiene(&invalid, &plan).is_err());
        }
        let mut invalid = hygiene(&plan);
        invalid["residentPid"] = json!(1);
        assert!(validate_hygiene(&invalid, &plan).is_err());
        invalid = hygiene(&plan);
        invalid["legacyBuilder"] = json!(true);
        assert!(validate_hygiene(&invalid, &plan).is_err());
    }
}
