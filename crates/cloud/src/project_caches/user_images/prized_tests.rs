//! The Prized phase logic against an in-memory sandbox host: the paused
//! builder becomes the image, the verifier is forked from it, and a paused
//! sandbox is never sent a command.
use super::prized::{PrizedUserImages, SandboxHost};
use super::*;
use crate::providers::{ExecResult, Machine, MachineState, ProviderError};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

const BASE: &str = "sbx_base";
const BASE_VERSION: &str = "20261114120000000";
const RUNTIME: &str = "a";
const TREE: &str = "7";

fn plan(script: &str) -> BuildPlan {
    let mut plan = super::tests::plan(script);
    plan.identity.provider = ProviderId::Prized;
    plan.identity.base_artifact_id = BASE.into();
    plan.identity.base_artifact_version = BASE_VERSION.into();
    plan.initial_runtime_bootstrap = None;
    plan
}

struct Guest {
    setup_exit: i64,
    sanitize_error: Option<&'static str>,
    check_clean: bool,
}

struct State {
    /// id → (machine, parent)
    machines: HashMap<String, (Machine, String)>,
    forks: usize,
    pauses: usize,
    deletes: usize,
    setup_launches: usize,
    setup_running: bool,
    writes: Vec<String>,
    lose_fork_reply: bool,
    guest: Guest,
}

#[derive(Clone)]
struct Host(Arc<Mutex<State>>);

fn machine(id: &str, name: &str, state: MachineState) -> Machine {
    Machine {
        id: id.into(),
        name: name.into(),
        state,
        vcpu: Some(2),
        memory_bytes: None,
    }
}

impl Host {
    fn new() -> Self {
        let mut machines = HashMap::new();
        machines.insert(
            BASE.into(),
            (machine(BASE, BASE, MachineState::Paused), String::new()),
        );
        Self(Arc::new(Mutex::new(State {
            machines,
            forks: 0,
            pauses: 0,
            deletes: 0,
            setup_launches: 0,
            setup_running: false,
            writes: vec![],
            lose_fork_reply: false,
            guest: Guest {
                setup_exit: 0,
                sanitize_error: None,
                check_clean: true,
            },
        })))
    }
}

fn helper_args(command: &str) -> Option<Vec<String>> {
    let quoted = super::boxd::quote(PRIZED_GUEST_HELPER);
    let (_, tail) = command.split_once(&quoted)?;
    Some(
        tail.split(" '")
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().trim_end_matches('\'').to_owned())
            .collect(),
    )
}
fn reply(value: Value) -> Result<ExecResult, ProviderError> {
    Ok(ExecResult {
        stdout: serde_json::to_vec(&value).unwrap(),
        stderr: vec![],
        exit_code: 0,
    })
}

#[async_trait::async_trait]
impl SandboxHost for Host {
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .machines
            .get(id)
            .map(|(m, _)| m.clone()))
    }
    async fn find(&self, name: &str) -> Result<Option<Machine>, ProviderError> {
        let state = self.0.lock().unwrap();
        Ok(state
            .machines
            .values()
            .find(|(m, _)| m.name == name)
            .map(|(m, _)| m.clone()))
    }
    async fn fork(
        &self,
        parent: &str,
        name: &str,
        _max_runtime_min: u32,
    ) -> Result<Machine, ProviderError> {
        let mut state = self.0.lock().unwrap();
        assert!(
            state.machines.contains_key(parent),
            "fork of a missing parent"
        );
        assert!(
            state.machines.values().all(|(m, _)| m.name != name),
            "a second sandbox with one name"
        );
        state.forks += 1;
        let id = format!("sbx_{}", state.forks);
        let child = machine(&id, name, MachineState::Running);
        state
            .machines
            .insert(id.clone(), (child.clone(), parent.into()));
        if std::mem::take(&mut state.lose_fork_reply) {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(child)
    }
    async fn created_at(&self, id: &str) -> Result<Option<String>, ProviderError> {
        let state = self.0.lock().unwrap();
        Ok(state.machines.get(id).map(|_| {
            if id == BASE {
                BASE_VERSION.into()
            } else {
                "20261114130000000".into()
            }
        }))
    }
    async fn exec(&self, id: &str, command: &str) -> Result<ExecResult, ProviderError> {
        let mut state = self.0.lock().unwrap();
        let (machine, parent) = state
            .machines
            .get(id)
            .cloned()
            .ok_or(ProviderError::NotFound)?;
        assert_eq!(
            machine.state,
            MachineState::Running,
            "a command would wake a paused sandbox"
        );
        assert!(!command.contains("sudo"), "Prized has no sudo");
        let Some(args) = helper_args(command) else {
            if command.contains("base64") {
                state.writes.push(command.to_owned());
            }
            return Ok(ExecResult {
                stdout: vec![],
                stderr: vec![],
                exit_code: 0,
            });
        };
        match args[0].as_str() {
            "baseline" => {
                assert_eq!(args[1], RUNTIME.repeat(64));
                assert_eq!(parent, BASE, "baseline runs on a fork of the base");
                reply(json!({"version":1,"pinsVerified":true,"treeSha256":TREE.repeat(64)}))
            }
            "setup-status" => {
                if state.setup_running {
                    state.setup_running = false;
                    return reply(json!({"version":1,"state":"running"}));
                }
                if state.setup_launches == 0 {
                    return reply(json!({"version":1,"state":"not-started"}));
                }
                reply(
                    json!({"version":1,"state":"exited","exitCode":state.guest.setup_exit,"log":"setup log"}),
                )
            }
            "setup-start" => {
                state.setup_launches += 1;
                state.setup_running = true;
                reply(json!({"version":1,"state":"running"}))
            }
            "sanitize" => {
                assert_eq!(args[2], TREE.repeat(64));
                match state.guest.sanitize_error {
                    Some(error) => reply(json!({"version":1,"error":error})),
                    None => reply(
                        json!({"version":1,"sanitized":true,"pinsVerified":true,"runtimeStopped":true}),
                    ),
                }
            }
            "check" => {
                assert_eq!(args[2], TREE.repeat(64));
                if state.guest.check_clean {
                    reply(
                        json!({"version":1,"clean":true,"pinsVerified":true,"supervisorIdle":true}),
                    )
                } else {
                    reply(json!({"version":1,"error":"user_image_not_clean"}))
                }
            }
            other => panic!("unexpected helper command {other}"),
        }
    }
    async fn pause(&self, id: &str) -> Result<(), ProviderError> {
        let mut state = self.0.lock().unwrap();
        state.pauses += 1;
        let (machine, _) = state.machines.get_mut(id).ok_or(ProviderError::NotFound)?;
        machine.state = MachineState::Paused;
        Ok(())
    }
    async fn delete(&self, id: &str) -> Result<(), ProviderError> {
        let mut state = self.0.lock().unwrap();
        assert_ne!(id, BASE, "the managed base is never deleted");
        state.deletes += 1;
        state.machines.remove(id);
        Ok(())
    }
}

/// Drive the adapter the way the worker does: one phase at a time, receipts
/// carried forward, Reconcile after a lost reply.
async fn drive(
    backend: &PrizedUserImages,
    plan: &BuildPlan,
    mut phase: String,
    artifact: Option<String>,
) -> (String, Value, Option<&'static str>) {
    let mut resources = json!({"effects":{}});
    for _ in 0..64 {
        let requested = resources["effects"][&phase]["requested"] == true;
        resources["effects"][&phase] = json!({"requested":true});
        let build = Build {
            image_id: plan.image_id.clone(),
            owner_id: plan.identity.owner_id.clone(),
            plan: plan.clone(),
            phase: phase.clone(),
            resources: resources.clone(),
            artifact_id: artifact.clone().or_else(|| {
                resources["provider"]["artifactId"]
                    .as_str()
                    .map(str::to_owned)
            }),
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

#[test]
fn prized_plans_carry_no_fresh_boot_pins_and_root_plans_must() {
    plan("npm i -g bun").validate().unwrap();
    let mut pinned = plan("npm i -g bun");
    pinned.initial_runtime_bootstrap = Some(super::tests::bootstrap());
    assert!(pinned.validate().is_err());
    let mut root = super::tests::plan("npm i -g bun");
    root.initial_runtime_bootstrap = None;
    assert!(root.validate().is_err());
    // A root plan serializes exactly as before the field became optional.
    let value = serde_json::to_value(super::tests::plan("x")).unwrap();
    assert!(value["initialRuntimeBootstrap"].is_object());
    assert!(
        serde_json::to_value(plan("x")).unwrap()["initialRuntimeBootstrap"].is_null()
            && !serde_json::to_value(plan("x"))
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("initialRuntimeBootstrap")
    );
}

#[tokio::test]
async fn prized_base_version_is_the_paused_parents_creation_stamp() {
    let host = Host::new();
    let backend = PrizedUserImages::new(Arc::new(host.clone()));
    assert_eq!(
        backend
            .base_version(ProviderId::Prized, BASE)
            .await
            .unwrap()
            .as_deref(),
        Some(BASE_VERSION)
    );
    assert_eq!(
        backend
            .base_version(ProviderId::Prized, "sbx_missing")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        backend
            .base_version(ProviderId::Prized, "prized-base-v1")
            .await
            .unwrap(),
        None
    );
    host.0
        .lock()
        .unwrap()
        .machines
        .get_mut(BASE)
        .unwrap()
        .0
        .state = MachineState::Running;
    assert_eq!(
        backend
            .base_version(ProviderId::Prized, BASE)
            .await
            .unwrap(),
        None,
        "a running parent is not a quiet base"
    );
}

#[tokio::test]
async fn prized_build_pauses_the_builder_as_the_image_and_verifies_a_fork_of_it() {
    let host = Host::new();
    host.0.lock().unwrap().lose_fork_reply = true;
    let backend = PrizedUserImages::new(Arc::new(host.clone()));
    let plan = plan("npm i -g bun");
    let (state, resources, failure) = drive(&backend, &plan, "allocate".into(), None).await;
    assert_eq!((state.as_str(), failure), ("ready", None));
    let state = host.0.lock().unwrap();
    // Builder and verifier: one fork each despite the lost fork reply.
    assert_eq!(state.forks, 2);
    assert_eq!(state.setup_launches, 1);
    assert_eq!(state.pauses, 1);
    let (builder, verifier, _) = plan.names();
    let artifact = resources["provider"]["artifactId"].as_str().unwrap();
    let (image, parent) = &state.machines[artifact];
    assert_eq!(
        (image.name.as_str(), parent.as_str()),
        (builder.as_str(), BASE)
    );
    assert_eq!(image.state, MachineState::Paused);
    assert!(
        state.machines.values().all(|(m, _)| m.name != verifier),
        "the verifier is gone"
    );
    assert_eq!(state.machines.len(), 2, "base and image only");
    assert_eq!(resources["provider"]["verifierAbsent"], true);
    assert!(resources["provider"]["builderAbsent"].is_null());
    assert!(
        state
            .writes
            .iter()
            .all(|c| c.contains("/home/prized/.zuse-user-image/"))
    );
}

#[tokio::test]
async fn prized_failed_setup_reports_log_and_cleanup_deletes_the_builder() {
    let host = Host::new();
    host.0.lock().unwrap().guest.setup_exit = 1;
    let backend = PrizedUserImages::new(Arc::new(host.clone()));
    let plan = plan("exit 1");
    let (phase, _, failure) = drive(&backend, &plan, "allocate".into(), None).await;
    assert_eq!(
        (phase.as_str(), failure),
        ("setup", Some("user_image_setup_failed"))
    );
    let (state, resources, failure) = drive(&backend, &plan, "cleanup".into(), None).await;
    assert_eq!((state.as_str(), failure), ("complete", None));
    assert_eq!(resources["provider"]["unpublishedArtifactAbsent"], true);
    let host = host.0.lock().unwrap();
    assert_eq!(host.machines.len(), 1, "only the base is left");
    assert!(host.machines.contains_key(BASE));
}

#[tokio::test]
async fn prized_rejects_images_that_changed_managed_files_or_leaked_state() {
    let host = Host::new();
    host.0.lock().unwrap().guest.sanitize_error = Some("user_image_pinned_file_changed");
    let backend = PrizedUserImages::new(Arc::new(host.clone()));
    let (phase, _, failure) = drive(&backend, &plan("x"), "allocate".into(), None).await;
    assert_eq!(
        (phase.as_str(), failure),
        ("sanitize", Some("user_image_runtime_files_changed"))
    );

    let host = Host::new();
    host.0.lock().unwrap().guest.check_clean = false;
    let backend = PrizedUserImages::new(Arc::new(host.clone()));
    let (phase, _, failure) = drive(&backend, &plan("x"), "allocate".into(), None).await;
    assert_eq!(
        (phase.as_str(), failure),
        ("verify", Some("user_image_verification_failed"))
    );
}

#[tokio::test]
async fn prized_base_replaced_after_planning_fails_before_any_fork() {
    let host = Host::new();
    let backend = PrizedUserImages::new(Arc::new(host.clone()));
    let mut plan = plan("x");
    plan.identity.base_artifact_version = "20261101000000000".into();
    let (phase, _, failure) = drive(&backend, &plan, "allocate".into(), None).await;
    assert_eq!(
        (phase.as_str(), failure),
        ("allocate", Some("user_image_managed_base_pins_mismatch"))
    );
    assert_eq!(host.0.lock().unwrap().forks, 0);
}

#[tokio::test]
async fn prized_retirement_deletes_only_the_published_image() {
    let host = Host::new();
    let backend = PrizedUserImages::new(Arc::new(host.clone()));
    let plan = plan("x");
    let (_, resources, _) = drive(&backend, &plan, "allocate".into(), None).await;
    let artifact = resources["provider"]["artifactId"]
        .as_str()
        .unwrap()
        .to_owned();
    // Another sandbox under a foreign name is never touched.
    host.0.lock().unwrap().machines.insert(
        "sbx_other".into(),
        (
            machine("sbx_other", "zr-computer", MachineState::Paused),
            artifact.clone(),
        ),
    );
    let (state, _, failure) = drive(&backend, &plan, "retire".into(), Some(artifact.clone())).await;
    assert_eq!((state.as_str(), failure), ("retired", None));
    let host = host.0.lock().unwrap();
    assert!(!host.machines.contains_key(&artifact));
    assert!(host.machines.contains_key("sbx_other") && host.machines.contains_key(BASE));
}
