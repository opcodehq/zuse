//! Machine certification of a managed base on its provider, before the base
//! may be registered as verified. One computer is created from the exact
//! candidate artifact and checked end to end: the installed runtime is the
//! manifest's runtime, both agents start, sleep and resume keep the disk, the
//! memory claim matches what happens, and deletion is confirmed. The report
//! records measured traits (start and wake times) for the app to show.
//!
//! No account, repository, credential or model is involved; the scripted chat
//! is a separate stage on top of this one.
use crate::{
    managed_bases::ManagedBaseRelease,
    providers::{
        artifact::ArtifactBinding,
        contract::{Allocation, OperationState, Provider, ProviderOperation},
    },
};
use serde::Serialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use zuse_protocol::cloud_computers::ProviderId;

const CHECKS: &str = include_str!("certification/checks.sh");
use crate::runtime::machine_layout::Layout;
/// The lifetime production requests on create and resume.
const LIFETIME_SECONDS: u32 = 86400;

/// The candidate under test, read from its staged managed-base record.
#[derive(Clone, Debug)]
pub struct Target {
    pub provider: ProviderId,
    pub release_id: String,
    pub artifact: String,
    pub binding: Option<ArtifactBinding>,
    pub runtime_sha256: String,
    pub preserves_memory: bool,
}
impl Target {
    pub fn from_candidate(candidate: &Value) -> Result<Self, String> {
        let text = |key: &str| {
            candidate[key]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("candidate {key} missing"))
        };
        let provider: ProviderId = serde_json::from_value(candidate["provider"].clone())
            .map_err(|_| "candidate provider invalid".to_owned())?;
        let binding = match candidate.get("providerArtifactBinding") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                serde_json::from_value(value.clone())
                    .map_err(|_| "candidate artifact binding invalid".to_owned())?,
            ),
        };
        Ok(Self {
            provider,
            release_id: text("releaseId")?,
            artifact: text("artifactId")?,
            binding,
            runtime_sha256: text("runtimeSha256")?,
            preserves_memory: candidate["capabilities"]["preservesMemory"] == true,
        })
    }
}

/// The staged (or registered) record for one managed base, as stored.
pub async fn candidate(
    database_url: &str,
    provider: &str,
    release_id: &str,
) -> Result<Value, String> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await
        .map_err(|_| "cannot connect to the cloud database".to_owned())?;
    let value: Option<Option<Value>> = sqlx::query_scalar(
        "SELECT coalesce(candidate_manifest,runtime_manifest) FROM cloud_managed_bases WHERE provider=$1 AND release_id=$2",
    )
    .bind(provider)
    .bind(release_id)
    .fetch_optional(&pool)
    .await
    .map_err(|_| "cannot read managed bases".to_owned())?;
    value
        .flatten()
        .ok_or_else(|| "no staged managed base with that provider and release".to_owned())
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Wait between sleep and resume. Boat keeps a stopped computer warm for
    /// ten minutes; a longer wait certifies its cold restore.
    pub sleep_wait: Duration,
    /// How long the installed runtime may take to answer after create or resume.
    pub runtime_wait: Duration,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            sleep_wait: Duration::ZERO,
            runtime_wait: Duration::from_secs(120),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: &'static str,
    pub passed: bool,
    pub detail: Value,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub version: u32,
    pub kind: &'static str,
    pub provider: ProviderId,
    pub release_id: String,
    pub artifact_id: String,
    pub runtime_sha256: String,
    pub machine_name: String,
    pub started_at: String,
    pub finished_at: String,
    pub passed: bool,
    pub checks: Vec<Check>,
    pub traits: Value,
}
impl Report {
    pub fn sha256(&self) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(serde_json::to_vec(self).unwrap_or_default())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

struct Run<'a> {
    provider: &'a dyn Provider,
    layout: Layout,
    machine: String,
    checks: Vec<Check>,
    traits: serde_json::Map<String, Value>,
}
impl Run<'_> {
    fn check(&mut self, name: &'static str, passed: bool, detail: Value) -> bool {
        self.checks.push(Check {
            name,
            passed,
            detail,
        });
        passed
    }
    fn trait_(&mut self, name: &str, value: Value) {
        self.traits.insert(name.into(), value);
    }
    /// Retry a command until it exits 0, up to `wait`. Returns elapsed ms.
    async fn until_ok(&self, line: &str, wait: Duration) -> Result<u64, String> {
        let started = Instant::now();
        let mut last = String::from("no attempt");
        while started.elapsed() < wait {
            match self.provider.execute(&self.machine, line).await {
                Ok(result) if result.exit_code == 0 => {
                    return Ok(started.elapsed().as_millis() as u64);
                }
                Ok(result) => last = format!("exit {}", result.exit_code),
                Err(error) => last = format!("{error:?}"),
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(last)
    }
    fn runtime(&self) -> String {
        format!("{} --version", self.layout.runtime_binary())
    }
    /// Upload the checks (a cold restore may have dropped /tmp) and run a mode.
    async fn script(&self, mode: &str) -> Result<Value, String> {
        let script = match self.layout {
            Layout::Root => "/tmp/zuse-certification.sh",
            // Prized accepts uploads only under its user's home.
            Layout::Home => "/home/prized/.zuse/boot/zuse-certification.sh",
        };
        let harnesses = match self.layout {
            Layout::Root => "/opt/zuse/harnesses/releases",
            Layout::Home => "/home/prized/.zuse/harnesses/releases",
        };
        self.provider
            .upload(&self.machine, script, CHECKS.as_bytes())
            .await
            .map_err(|e| format!("upload {e:?}"))?;
        let result = self
            .provider
            .execute(
                &self.machine,
                &format!(
                    "ZUSE_RUNTIME_BINARY={} ZUSE_HARNESS_RELEASES={harnesses} bash {script} {mode}",
                    self.layout.runtime_binary()
                ),
            )
            .await
            .map_err(|e| format!("execute {e:?}"))?;
        let stdout = String::from_utf8_lossy(&result.stdout);
        serde_json::from_str(stdout.lines().last().unwrap_or_default())
            .map_err(|_| format!("{mode} exit {}: no report", result.exit_code))
    }
    async fn settle(&self, receipt: &ProviderOperation, wait: Duration) -> Result<u64, String> {
        settle(self.provider, receipt, wait).await
    }
}

async fn settle(
    provider: &dyn Provider,
    receipt: &ProviderOperation,
    wait: Duration,
) -> Result<u64, String> {
    let started = Instant::now();
    let mut last = String::from("no observation");
    while started.elapsed() < wait {
        match provider.operation(receipt).await {
            Ok(OperationState::Completed) => return Ok(started.elapsed().as_millis() as u64),
            Ok(OperationState::Superseded) => return Err("superseded".into()),
            Ok(state) => last = format!("{state:?}"),
            Err(error) => last = format!("{error:?}"),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(last)
}

/// Certify `target` on `provider`. Always attempts to delete the computer it
/// created, and records whether deletion was confirmed.
pub async fn certify(provider: &dyn Provider, target: &Target, options: &Options) -> Report {
    let started_at = chrono::Utc::now().to_rfc3339();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!(
        "zuse-certify-{}-{}",
        target.provider.as_str(),
        &suffix[..12]
    );
    let mut checks = Vec::new();
    let started = Instant::now();
    let created = provider
        .allocate(&Allocation {
            artifact: &target.artifact,
            artifact_binding: target.binding.as_ref(),
            name: &name,
            operation_id: &name,
            size: "default",
            lifetime_seconds: LIFETIME_SECONDS,
        })
        .await;
    let create_ms = started.elapsed().as_millis() as u64;
    let machine = match created {
        Ok(machine) => Some(machine),
        // A lost response may still have created it; reconcile by name.
        Err(error) => match provider.find_allocation(&name).await {
            // An unknown outcome may still be creating; an empty lookup now
            // doesn't prove absence, so leave it for cleanup by name.
            Ok(None) if matches!(error, crate::providers::ProviderError::OutcomeUnknown) => {
                checks.push(Check {
                    name: "create",
                    passed: false,
                    detail: json!({"error":format!("{error:?}"),
                        "cleanup":"uncertain; reconcile by name"}),
                });
                None
            }
            Ok(found) => found,
            Err(lookup) => {
                checks.push(Check {
                    name: "create",
                    passed: false,
                    detail: json!({"error":format!("{error:?}"),"lookup":format!("{lookup:?}"),
                        "cleanup":"uncertain; reconcile by name"}),
                });
                None
            }
        },
    };
    let Some(machine) = machine else {
        if checks.is_empty() {
            checks.push(Check {
                name: "create",
                passed: false,
                detail: json!({"error":"not created","confirmedAbsent":true}),
            });
        }
        return finish(target, name, started_at, checks, serde_json::Map::new());
    };
    let mut run = Run {
        provider,
        layout: Layout::for_provider(target.provider),
        machine: machine.id.clone(),
        checks,
        traits: serde_json::Map::new(),
    };
    run.check(
        "create",
        true,
        json!({"ms":create_ms,"machineId":machine.id}),
    );
    run.trait_("createMs", json!(create_ms));
    lifecycle(&mut run, target, options, started).await;

    // Always clean up, even after a failed step.
    let deleted = match provider.delete(&machine.id).await {
        Ok(receipt) => settle(provider, &receipt, Duration::from_secs(180)).await,
        Err(error) => Err(format!("{error:?}")),
    };
    let absent = matches!(provider.inspect(&machine.id).await, Ok(None))
        || matches!(provider.find_allocation(&name).await, Ok(None));
    run.check(
        "deleteConfirmed",
        deleted.is_ok() && absent,
        json!({"settled":deleted.is_ok(),"error":deleted.err(),"absent":absent}),
    );
    finish(target, name, started_at, run.checks, run.traits)
}

async fn lifecycle(run: &mut Run<'_>, target: &Target, options: &Options, started: Instant) {
    match run.until_ok(&run.runtime(), options.runtime_wait).await {
        Ok(_) => {
            let ms = started.elapsed().as_millis() as u64;
            run.check("runtimeAnswers", true, json!({"msFromCreate":ms}));
            run.trait_("startToRuntimeMs", json!(ms));
        }
        Err(error) => {
            run.check("runtimeAnswers", false, json!({"error":error}));
            return;
        }
    }
    let fresh = match run.script("fresh").await {
        Ok(fresh) => fresh,
        Err(error) => {
            run.check("freshChecks", false, json!({"error":error}));
            return;
        }
    };
    let installed = fresh["runtimeSha256"].as_str().unwrap_or_default();
    run.check(
        "runtimeMatchesManifest",
        installed.eq_ignore_ascii_case(&target.runtime_sha256),
        json!({"installed":installed,"manifest":target.runtime_sha256,"version":fresh["runtimeVersion"]}),
    );
    run.check("claudeAnswers", fresh["claudeAnswers"] == true, json!({}));
    let codex_ms = fresh["codexInitializeMs"].as_i64().unwrap_or(-1);
    run.check(
        "codexStarts",
        codex_ms >= 0,
        json!({"initializeMs":codex_ms}),
    );
    run.trait_("codexColdStartMs", json!(codex_ms));
    run.check("gitAvailable", fresh["git"] == true, json!({}));
    // Root bases run the agent as the zuse task user via sudo; a home base
    // (Prized) runs everything as its only user.
    if run.layout == Layout::Root {
        run.check("taskUserExists", fresh["taskUser"] == true, json!({}));
        run.check("sudoWithoutPassword", fresh["sudo"] == true, json!({}));
        // managed-base-install.sh ships Node.js LTS (node, npm, npx,
        // corepack); machine setup scripts and agents rely on it.
        run.check(
            "nodeAvailable",
            fresh["node"] == true,
            json!({"version":fresh["nodeVersion"]}),
        );
    }
    run.trait_("nodeVersion", fresh["nodeVersion"].clone());
    run.trait_("sshAvailable", fresh["sshd"].clone());
    run.trait_("cpus", fresh["cpus"].clone());
    // A trait, not a check: bases without the in-place runtime update helper
    // remain certifiable (they use "Update computer" instead).
    run.trait_(
        "runtimeUpdateInstalled",
        json!(fresh["runtimeUpdateInstalled"] == true),
    );

    match run.script("before").await {
        Ok(value) if value["agentRunning"] == true => {
            run.check("residentAgentRunning", true, json!({}));
        }
        Ok(value) => {
            run.check("residentAgentRunning", false, value);
            return;
        }
        Err(error) => {
            run.check("residentAgentRunning", false, json!({"error":error}));
            return;
        }
    }
    let sleep = match run.provider.sleep(&run.machine).await {
        Ok(receipt) => run.settle(&receipt, Duration::from_secs(180)).await,
        Err(error) => Err(format!("{error:?}")),
    };
    match sleep {
        Ok(ms) => {
            run.check("sleepSettles", true, json!({"ms":ms}));
            run.trait_("sleepSettleMs", json!(ms));
        }
        Err(error) => {
            run.check("sleepSettles", false, json!({"error":error}));
            return;
        }
    }
    if !options.sleep_wait.is_zero() {
        tokio::time::sleep(options.sleep_wait).await;
        run.trait_("sleptForSecs", json!(options.sleep_wait.as_secs()));
    }
    let resumed = Instant::now();
    let resume = match run.provider.resume(&run.machine, LIFETIME_SECONDS).await {
        Ok(receipt) => run.settle(&receipt, Duration::from_secs(180)).await,
        Err(error) => Err(format!("{error:?}")),
    };
    if let Err(error) = resume {
        run.check("resumeSettles", false, json!({"error":error}));
        return;
    }
    run.check("resumeSettles", true, json!({}));
    match run.until_ok(&run.runtime(), options.runtime_wait).await {
        Ok(_) => {
            let ms = resumed.elapsed().as_millis() as u64;
            run.check(
                "runtimeAnswersAfterResume",
                true,
                json!({"msFromResume":ms}),
            );
            run.trait_("resumeToRuntimeMs", json!(ms));
        }
        Err(error) => {
            run.check("runtimeAnswersAfterResume", false, json!({"error":error}));
            return;
        }
    }
    let after = match run.script("resume").await {
        Ok(after) => after,
        Err(error) => {
            run.check("resumeChecks", false, json!({"error":error}));
            return;
        }
    };
    run.check("diskKeptAcrossSleep", after["diskKept"] == true, json!({}));
    let survived = after["agentSurvived"] == true;
    let restart = after["agentRestartMs"].as_i64().unwrap_or(-1);
    run.check(
        "agentReadyAfterResume",
        survived || restart >= 0,
        json!({"survived":survived,"restartMs":restart}),
    );
    // A base that claims to keep memory must actually keep the agent running.
    run.check(
        "memoryClaimMatches",
        !target.preserves_memory || survived,
        json!({"claimed":target.preserves_memory,"observed":survived,"sameBoot":after["sameBoot"]}),
    );
    run.trait_("keepsAgentAcrossSleep", json!(survived));
    if !survived {
        run.trait_("agentRestartMs", json!(restart));
    }
}

fn finish(
    target: &Target,
    machine_name: String,
    started_at: String,
    checks: Vec<Check>,
    traits: serde_json::Map<String, Value>,
) -> Report {
    Report {
        version: 1,
        kind: "machine",
        provider: target.provider,
        release_id: target.release_id.clone(),
        artifact_id: target.artifact.clone(),
        runtime_sha256: target.runtime_sha256.clone(),
        machine_name,
        started_at,
        finished_at: chrono::Utc::now().to_rfc3339(),
        passed: !checks.is_empty() && checks.iter().all(|check| check.passed),
        checks,
        traits: Value::Object(traits),
    }
}

/// The registrable release for a passed report: the staged candidate bound to
/// this report's digest. Refuses a failed report or a mismatched candidate.
pub fn release(candidate: &Value, report: &Report) -> Result<ManagedBaseRelease, String> {
    if !report.passed {
        return Err("certification did not pass".into());
    }
    let target = Target::from_candidate(candidate)?;
    if target.provider != report.provider
        || target.release_id != report.release_id
        || target.artifact != report.artifact_id
        || target.runtime_sha256 != report.runtime_sha256
    {
        return Err("report does not match this candidate".into());
    }
    let mut value = candidate.clone();
    let object = value
        .as_object_mut()
        .ok_or_else(|| "candidate is not an object".to_owned())?;
    // Candidate-only provenance fields are not part of a registered release.
    for key in ["runtimeSha256", "harnessRelease", "provenanceReportSha256"] {
        object.remove(key);
    }
    object.insert("certificationReportSha256".into(), json!(report.sha256()));
    // A staged candidate is the build plus a "-canary-<suffix>" trial tag; the
    // registered release is the build itself.
    let build = object
        .get("manifest")
        .and_then(|manifest| manifest["buildId"].as_str())
        .ok_or_else(|| "candidate manifest has no build ID".to_owned())?
        .to_owned();
    let staged = target.release_id.as_str();
    if staged != build {
        let suffix = staged
            .strip_prefix(&build)
            .and_then(|rest| rest.strip_prefix("-canary-"))
            .unwrap_or_default();
        if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err("candidate release is not a canary of its build".into());
        }
        object.insert("releaseId".into(), json!(build));
    }
    let release: ManagedBaseRelease = serde_json::from_value(value)
        .map_err(|error| format!("candidate is not a registrable release: {error}"))?;
    release.validate().map_err(|error| error.code.to_string())?;
    Ok(release)
}

#[cfg(test)]
#[path = "certification/tests.rs"]
mod tests;
