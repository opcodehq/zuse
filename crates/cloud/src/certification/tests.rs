use super::*;
use crate::providers::{ExecResult, Machine, MachineState, ProviderError, contract::DesiredState};
use std::sync::{Arc, Mutex};

const RUNTIME_SHA: &str = "7ae4b2179f3e8360d4f52f6f2dd5ce7de443a429b08f7cfaf27fa1006a904dda";

/// A computer whose behavior each test chooses.
#[derive(Default)]
struct Fake {
    installed_sha: String,
    agent_survives: bool,
    runtime_never_answers: bool,
    deleted: Mutex<bool>,
    log: Arc<Mutex<Vec<String>>>,
}
fn machine() -> Machine {
    Machine {
        id: "machine-1".into(),
        name: "certify".into(),
        state: MachineState::Running,
        vcpu: Some(2),
        memory_bytes: Some(4 << 30),
    }
}
fn operation(desired: DesiredState) -> ProviderOperation {
    ProviderOperation {
        resource_id: "machine-1".into(),
        operation_id: None,
        desired,
    }
}
fn output(line: &str) -> Result<ExecResult, ProviderError> {
    Ok(ExecResult {
        stdout: format!("{line}\n").into_bytes(),
        stderr: vec![],
        exit_code: 0,
    })
}
#[async_trait::async_trait]
impl Provider for Fake {
    fn id(&self) -> ProviderId {
        ProviderId::E2b
    }
    async fn inspect(&self, _: &str) -> Result<Option<Machine>, ProviderError> {
        Ok((!*self.deleted.lock().unwrap()).then(machine))
    }
    async fn find_allocation(&self, _: &str) -> Result<Option<Machine>, ProviderError> {
        Ok(None)
    }
    async fn verify_artifact(
        &self,
        _: &str,
        _: Option<&ArtifactBinding>,
    ) -> Result<(), ProviderError> {
        Ok(())
    }
    async fn allocate(&self, request: &Allocation<'_>) -> Result<Machine, ProviderError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("allocate {}", request.artifact));
        Ok(machine())
    }
    async fn sleep(&self, _: &str) -> Result<ProviderOperation, ProviderError> {
        self.log.lock().unwrap().push("sleep".into());
        Ok(operation(DesiredState::Sleeping))
    }
    async fn resume(&self, _: &str, _: u32) -> Result<ProviderOperation, ProviderError> {
        self.log.lock().unwrap().push("resume".into());
        Ok(operation(DesiredState::Running))
    }
    async fn delete(&self, _: &str) -> Result<ProviderOperation, ProviderError> {
        self.log.lock().unwrap().push("delete".into());
        *self.deleted.lock().unwrap() = true;
        Ok(operation(DesiredState::Deleted))
    }
    async fn operation(&self, _: &ProviderOperation) -> Result<OperationState, ProviderError> {
        Ok(OperationState::Completed)
    }
    async fn execute(&self, _: &str, command: &str) -> Result<ExecResult, ProviderError> {
        if command == RUNTIME {
            if self.runtime_never_answers {
                return Err(ProviderError::Unavailable);
            }
            return output("zuse-cloud-runtime 0.26.0");
        }
        match command.rsplit(' ').next().unwrap_or_default() {
            "fresh" => output(&format!(
                r#"{{"runtimeSha256":"{}","runtimeVersion":"zuse-cloud-runtime 0.26.0","claudeAnswers":true,"codexInitializeMs":180,"git":true,"sudo":true,"sshd":false,"taskUser":true,"cpus":2}}"#,
                self.installed_sha
            )),
            "before" => output(r#"{"agentRunning":true}"#),
            "resume" => output(&format!(
                r#"{{"sameBoot":{0},"agentSurvived":{0},"agentRestartMs":{1},"diskKept":true}}"#,
                self.agent_survives,
                if self.agent_survives { 0 } else { 140 }
            )),
            other => panic!("unexpected command {other}"),
        }
    }
    async fn upload(&self, _: &str, path: &str, contents: &[u8]) -> Result<(), ProviderError> {
        assert_eq!(path, SCRIPT);
        assert_eq!(contents, CHECKS.as_bytes());
        Ok(())
    }
}

fn target(preserves_memory: bool) -> Target {
    Target {
        provider: ProviderId::E2b,
        release_id: "zuse-base-release".into(),
        artifact: "template-1".into(),
        binding: None,
        runtime_sha256: RUNTIME_SHA.into(),
        preserves_memory,
    }
}
fn failed(report: &Report) -> Vec<&'static str> {
    report
        .checks
        .iter()
        .filter(|c| !c.passed)
        .map(|c| c.name)
        .collect()
}

#[tokio::test]
async fn a_working_base_passes_and_records_its_traits() {
    let fake = Fake {
        installed_sha: RUNTIME_SHA.into(),
        agent_survives: true,
        ..Fake::default()
    };
    let report = certify(&fake, &target(true), &Options::default()).await;
    assert!(report.passed, "{:?}", failed(&report));
    assert_eq!(report.traits["keepsAgentAcrossSleep"], true);
    assert_eq!(report.traits["sshAvailable"], false);
    assert_eq!(report.traits["codexColdStartMs"], 180);
    assert_eq!(
        *fake.log.lock().unwrap(),
        ["allocate template-1", "sleep", "resume", "delete"]
    );
    assert_eq!(report.sha256().len(), 64);
}

#[tokio::test]
async fn a_memory_claim_the_provider_does_not_keep_fails() {
    let fake = Fake {
        installed_sha: RUNTIME_SHA.into(),
        agent_survives: false,
        ..Fake::default()
    };
    let honest = certify(&fake, &target(false), &Options::default()).await;
    assert!(honest.passed, "{:?}", failed(&honest));
    assert_eq!(honest.traits["agentRestartMs"], 140);
    let report = certify(&fake, &target(true), &Options::default()).await;
    assert_eq!(failed(&report), ["memoryClaimMatches"]);
}

#[tokio::test]
async fn a_different_installed_runtime_fails() {
    let fake = Fake {
        installed_sha: "0".repeat(64),
        agent_survives: true,
        ..Fake::default()
    };
    let report = certify(&fake, &target(true), &Options::default()).await;
    assert_eq!(failed(&report), ["runtimeMatchesManifest"]);
}

#[tokio::test]
async fn a_computer_whose_runtime_never_answers_is_still_deleted() {
    let fake = Fake {
        runtime_never_answers: true,
        ..Fake::default()
    };
    let options = Options {
        runtime_wait: Duration::from_millis(600),
        ..Options::default()
    };
    let report = certify(&fake, &target(true), &options).await;
    assert!(!report.passed);
    assert_eq!(failed(&report), ["runtimeAnswers"]);
    assert!(
        report
            .checks
            .iter()
            .any(|c| c.name == "deleteConfirmed" && c.passed)
    );
    assert_eq!(*fake.log.lock().unwrap(), ["allocate template-1", "delete"]);
}

#[test]
fn a_release_comes_only_from_a_passed_report_for_the_same_candidate() {
    let candidate = json!({"provider":"e2b","releaseId":"zuse-base-release","artifactId":"template-1",
        "runtimeSha256":RUNTIME_SHA,"capabilities":{"preservesMemory":true}});
    let parsed = Target::from_candidate(&candidate).unwrap();
    assert!(parsed.preserves_memory && parsed.binding.is_none());
    let mut report = finish(
        &parsed,
        "machine".into(),
        "start".into(),
        vec![],
        Default::default(),
    );
    assert!(!report.passed, "no checks is never a pass");
    assert_eq!(
        release(&candidate, &report).err().as_deref(),
        Some("certification did not pass")
    );
    report.passed = true;
    report.release_id = "other".into();
    assert_eq!(
        release(&candidate, &report).err().as_deref(),
        Some("report does not match this candidate")
    );
}

#[test]
fn a_passed_canary_registers_as_its_build_with_the_report_digest() {
    // The actual staged E2B candidate (identifiers and hashes only).
    let candidate: Value =
        serde_json::from_str(include_str!("fixtures/e2b-candidate.json")).unwrap();
    let target = Target::from_candidate(&candidate).unwrap();
    assert!(target.binding.is_some() && target.preserves_memory);
    let mut report = finish(
        &target,
        "machine".into(),
        "start".into(),
        vec![],
        Default::default(),
    );
    report.passed = true;
    let release = release(&candidate, &report).unwrap();
    assert_eq!(
        release.release_id,
        candidate["manifest"]["buildId"].as_str().unwrap()
    );
    assert_ne!(release.release_id, target.release_id);
    assert_eq!(release.certification_report_sha256, report.sha256());

    let mut forged = candidate.clone();
    forged["releaseId"] = json!(format!("{}-other", target.release_id));
    report.release_id = forged["releaseId"].as_str().unwrap().into();
    assert_eq!(
        release_of(&forged, &report),
        Some("candidate release is not a canary of its build".into())
    );
}

fn release_of(candidate: &Value, report: &Report) -> Option<String> {
    release(candidate, report).err()
}
