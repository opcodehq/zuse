//! The Boat phase logic against an in-memory Boat: saved snapshots by name
//! with a `snapshotId` version, lazily hydrating sandboxes found by name.
use super::boat::{BoatImageHost, BoatUserImages};
use super::tests::{Guest, HostState, drive, guest_exec, plan};
use super::*;
use crate::providers::{
    ExecResult, Machine, MachineState, ProviderError,
    boat::{Sandbox, SavedSnapshot, SavedSnapshotState},
};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct BoatState {
    /// id → (sandbox, inspections left before hydrated)
    sandboxes: HashMap<String, (Sandbox, usize)>,
    /// name → (snapshot, polls left before ready)
    snapshots: HashMap<String, (SavedSnapshot, usize)>,
    creates: usize,
    saves: usize,
    destroys: usize,
    snapshot_deletes: usize,
    lose_reply: Option<&'static str>,
    /// Writes to a builder after its save was requested.
    writes_after_save: usize,
    guest: HostState,
}

#[derive(Clone, Default)]
struct FakeBoat(Arc<Mutex<BoatState>>);

impl FakeBoat {
    fn new() -> Self {
        let host = Self::default();
        host.0.lock().unwrap().snapshots.insert(
            "zuse-base-build-1".into(),
            (
                SavedSnapshot {
                    name: "zuse-base-build-1".into(),
                    state: SavedSnapshotState::Ready,
                    version: Some("base-disk-1".into()),
                    source_machine_id: Some("bx_base".into()),
                },
                0,
            ),
        );
        host
    }
    fn lost(state: &mut BoatState, name: &'static str) -> bool {
        if state.lose_reply == Some(name) {
            state.lose_reply = None;
            true
        } else {
            false
        }
    }
    fn view(entry: &mut (Sandbox, usize)) -> Sandbox {
        if entry.1 == 0 {
            entry.0.hydrated = true;
        } else {
            entry.1 -= 1;
        }
        entry.0.clone()
    }
}

#[async_trait::async_trait]
impl BoatImageHost for FakeBoat {
    async fn sandbox(&self, id: &str) -> Result<Option<Sandbox>, ProviderError> {
        Ok(self.0.lock().unwrap().sandboxes.get_mut(id).map(Self::view))
    }
    async fn find_sandbox(&self, name: &str) -> Result<Option<Sandbox>, ProviderError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .sandboxes
            .values_mut()
            .find(|(s, _)| s.machine.name == name)
            .map(Self::view))
    }
    async fn create_from_snapshot(
        &self,
        snapshot: &str,
        name: &str,
        lifetime_seconds: u32,
    ) -> Result<Sandbox, ProviderError> {
        let mut state = self.0.lock().unwrap();
        assert!((60..=86400).contains(&lifetime_seconds));
        assert_eq!(
            state.snapshots.get(snapshot).map(|(s, _)| s.state),
            Some(SavedSnapshotState::Ready),
            "starts only from a ready saved snapshot"
        );
        assert!(
            !state
                .sandboxes
                .values()
                .any(|(s, _)| s.machine.name == name),
            "never a second create"
        );
        state.creates += 1;
        let sandbox = Sandbox {
            machine: Machine {
                id: format!("bx_{}", state.creates),
                name: name.into(),
                state: MachineState::Running,
                vcpu: None,
                memory_bytes: None,
            },
            hydrated: false,
            source_artifact: Some(snapshot.into()),
        };
        state
            .sandboxes
            .insert(sandbox.machine.id.clone(), (sandbox.clone(), 1));
        if Self::lost(&mut state, "create") {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(sandbox)
    }
    async fn exec(&self, id: &str, command: &str) -> Result<ExecResult, ProviderError> {
        let mut state = self.0.lock().unwrap();
        let (sandbox, _) = state.sandboxes.get(id).expect("exec on a live sandbox");
        assert!(sandbox.hydrated, "nothing runs before the disk is hydrated");
        if state
            .snapshots
            .values()
            .any(|(s, _)| s.source_machine_id.as_deref() == Some(id))
        {
            state.writes_after_save += 1;
        }
        guest_exec(&mut state.guest, command)
    }
    async fn destroy(&self, id: &str) -> Result<(), ProviderError> {
        let mut state = self.0.lock().unwrap();
        if let Some((s, _)) = state.snapshots.values().find(|(s, _)| {
            s.source_machine_id.as_deref() == Some(id) && s.state == SavedSnapshotState::Saving
        }) {
            panic!("builder {} deleted while saving", s.name);
        }
        state.destroys += 1;
        state.sandboxes.remove(id);
        Ok(())
    }
    async fn save_snapshot(&self, id: &str, name: &str) -> Result<(), ProviderError> {
        let mut state = self.0.lock().unwrap();
        assert!(state.sandboxes.contains_key(id));
        assert!(!state.snapshots.contains_key(name), "never a second save");
        state.saves += 1;
        let version = format!("disk-{}", state.saves);
        state.snapshots.insert(
            name.into(),
            (
                SavedSnapshot {
                    name: name.into(),
                    state: SavedSnapshotState::Saving,
                    version: Some(version),
                    source_machine_id: Some(id.into()),
                },
                1,
            ),
        );
        if Self::lost(&mut state, "save") {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(())
    }
    async fn snapshot(&self, name: &str) -> Result<Option<SavedSnapshot>, ProviderError> {
        let mut state = self.0.lock().unwrap();
        Ok(state.snapshots.get_mut(name).map(|(s, polls)| {
            if s.state == SavedSnapshotState::Saving {
                if *polls == 0 {
                    s.state = SavedSnapshotState::Ready;
                } else {
                    *polls -= 1;
                }
            }
            s.clone()
        }))
    }
    async fn delete_snapshot(&self, name: &str) -> Result<(), ProviderError> {
        let mut state = self.0.lock().unwrap();
        assert_ne!(
            name, "zuse-base-build-1",
            "the managed base is never deleted"
        );
        state.snapshot_deletes += 1;
        state.snapshots.remove(name);
        Ok(())
    }
}

fn boat_plan(script: &str) -> BuildPlan {
    let mut plan = plan(script);
    plan.identity.provider = ProviderId::Boat;
    plan.identity.base_artifact_id = "zuse-base-build-1".into();
    plan.identity.base_artifact_version = "base-disk-1".into();
    plan.initial_runtime_bootstrap.as_mut().unwrap().mode = "post-hydration-first-start-v1".into();
    plan.validate().unwrap();
    plan
}

fn fake(guest: Guest) -> (FakeBoat, BoatUserImages) {
    let host = FakeBoat::new();
    host.0.lock().unwrap().guest = HostState {
        guest,
        ..Default::default()
    };
    let backend = BoatUserImages::new(Arc::new(host.clone()));
    (host, backend)
}

#[tokio::test]
async fn boat_build_saves_once_verifies_a_hydrated_second_boot_and_leaves_no_sandboxes() {
    let (host, backend) = fake(Guest::default());
    host.0.lock().unwrap().lose_reply = Some("create");
    let plan = boat_plan("npm i -g bun");
    assert_eq!(
        backend
            .base_version(ProviderId::Boat, "zuse-base-build-1")
            .await
            .unwrap()
            .as_deref(),
        Some("base-disk-1")
    );
    assert_eq!(
        backend
            .base_version(ProviderId::Boat, "missing")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        backend
            .base_version(ProviderId::Boxd, "zuse-base-build-1")
            .await
            .unwrap(),
        None
    );
    let (state, resources, failure) = drive(&backend, &plan, "allocate".into()).await;
    assert_eq!((state.as_str(), failure), ("ready", None));
    let state = host.0.lock().unwrap();
    // Builder and verifier: one create each despite the lost create reply.
    assert_eq!(state.creates, 2);
    assert_eq!(state.saves, 1);
    assert_eq!(
        state.writes_after_save, 0,
        "the builder is untouched after save"
    );
    assert_eq!(state.guest.setup_launches, 1);
    assert!(state.sandboxes.is_empty(), "builder and verifier are gone");
    let (_, _, snapshot_name) = plan.names();
    assert_eq!(resources["provider"]["artifactId"], snapshot_name.as_str());
    assert_eq!(resources["provider"]["artifactVersion"], "disk-1");
    assert_eq!(resources["provider"]["builderAbsent"], true);
    assert_eq!(resources["provider"]["verifierAbsent"], true);
    assert!(
        state
            .guest
            .uploads
            .iter()
            .all(|p| p.starts_with("/opt/zuse/user-image/"))
    );
}

#[tokio::test]
async fn boat_lost_save_reply_is_found_by_name_not_saved_again() {
    let (host, backend) = fake(Guest::default());
    host.0.lock().unwrap().lose_reply = Some("save");
    let (state, _, failure) = drive(&backend, &boat_plan("x"), "allocate".into()).await;
    assert_eq!((state.as_str(), failure), ("ready", None));
    assert_eq!(host.0.lock().unwrap().saves, 1);
}

#[tokio::test]
async fn boat_base_saved_again_since_the_plan_is_not_built_on() {
    let (host, backend) = fake(Guest::default());
    host.0
        .lock()
        .unwrap()
        .snapshots
        .get_mut("zuse-base-build-1")
        .unwrap()
        .0
        .version = Some("base-disk-2".into());
    let (phase, _, failure) = drive(&backend, &boat_plan("x"), "allocate".into()).await;
    assert_eq!(
        (phase.as_str(), failure),
        ("allocate", Some("user_image_managed_base_changed"))
    );
    assert_eq!(host.0.lock().unwrap().creates, 0);
}

#[tokio::test]
async fn boat_failures_are_reported_and_cleanup_deletes_everything_it_made() {
    for (guest, expected) in [
        (
            Guest {
                setup_exit: 100,
                ..Default::default()
            },
            ("setup", "user_image_setup_failed"),
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
                fresh_boot_ok: false,
                ..Default::default()
            },
            ("verify", "user_image_fresh_boot_rejected"),
        ),
    ] {
        let (host, backend) = fake(guest);
        let plan = boat_plan("x");
        let (phase, resources, failure) = drive(&backend, &plan, "allocate".into()).await;
        assert_eq!((phase.as_str(), failure), (expected.0, Some(expected.1)));
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
        assert!(state.sandboxes.is_empty());
        assert_eq!(
            state.snapshots.keys().collect::<Vec<_>>(),
            vec!["zuse-base-build-1"],
            "only the managed base remains"
        );
    }
}

#[tokio::test]
async fn boat_retirement_deletes_only_the_published_snapshot() {
    let (host, backend) = fake(Guest::default());
    let plan = boat_plan("x");
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
    assert!(state.snapshots.contains_key("zuse-base-build-1"));
    // A Boxd plan never reaches the Boat builder.
    drop(state);
    assert!(
        backend
            .step(
                &Build {
                    plan: super::tests::plan("x"),
                    ..build
                },
                StepMode::Begin
            )
            .await
            .is_err()
    );
}
