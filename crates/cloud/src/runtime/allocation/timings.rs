//! Metadata-only measurements of actual provider calls, never credentials/output.
//! A phase describes the latest worker attempt, not provider hardware latency.
use serde_json::{Value, json};
use std::{collections::BTreeMap, future::Future, sync::Mutex, time::Instant};

pub(super) const DEFER_ALLOCATION_SQL: &str = "WITH locked_workspace AS MATERIALIZED (SELECT workspace_id FROM api_cloud_workspaces WHERE workspace_id=$7 FOR UPDATE), deferred AS (UPDATE zuse_rust_jobs SET state='pending',lease_token=NULL,lease_expires_at=NULL,due_at=$3,updated_at=$4 WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$4 AND EXISTS(SELECT 1 FROM locked_workspace) RETURNING resource_id,resource_generation) UPDATE api_cloud_workspaces w SET request_config=jsonb_set(w.request_config,'{startupTimings}',CASE WHEN jsonb_typeof(w.request_config->'startupTimings')='object' THEN w.request_config->'startupTimings' ELSE '{}'::jsonb END||jsonb_build_object('providerPhases',CASE WHEN jsonb_typeof(w.request_config#>'{startupTimings,providerPhases}')='object' THEN w.request_config#>'{startupTimings,providerPhases}' ELSE '{}'::jsonb END||$5::jsonb)) FROM deferred d WHERE w.workspace_id=d.resource_id AND (w.request_config->>'runtimeGeneration')::bigint=d.resource_generation AND w.request_config->>'rustAllocationOperationId'=$6 AND $5::jsonb<>'{}'::jsonb";

#[derive(Default)]
pub(crate) struct Observations(Mutex<BTreeMap<&'static str, Value>>);
impl Observations {
    pub async fn measure<T, E>(
        &self,
        name: &'static str,
        effect: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        let started = Instant::now();
        let result = effect.await;
        tracing::info!(
            phase = name,
            duration_ms = started.elapsed().as_millis() as u64,
            succeeded = result.is_ok(),
            "Cloud startup provider phase"
        );
        self.record(
            name,
            started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            result.is_ok(),
        );
        result
    }
    fn record(&self, name: &'static str, elapsed: u64, succeeded: bool) {
        let Ok(mut phases) = self.0.lock() else {
            return;
        };
        let prior = phases.get(name);
        let count = prior
            .and_then(|v| v["callCount"].as_u64())
            .unwrap_or(0)
            .saturating_add(1);
        let total = prior
            .and_then(|v| v["totalDurationMs"].as_u64())
            .unwrap_or(0)
            .saturating_add(elapsed);
        phases.insert(name, json!({"scope":"worker-attempt","clock":"monotonic-duration","callCount":count,"totalDurationMs":total,"lastDurationMs":elapsed,"lastCompletedAtMs":chrono::Utc::now().timestamp_millis(),"lastResultOk":succeeded}));
    }
    pub fn snapshot(&self) -> Value {
        self.0
            .lock()
            .map(|v| json!(*v))
            .unwrap_or_else(|_| json!({}))
    }
}
/// Only scalar measurements can cross the provider diagnostic boundary.
/// Error strings, output, arbitrary phase names and future payload fields stay out.
pub(super) fn safe_failure_observations(observations: &Value) -> Value {
    let mut safe = serde_json::Map::new();
    const PHASES: &[&str] = &[
        "providerLookup",
        "providerInspect",
        "providerRuntimeReadiness",
        "providerCreate",
        "initialSourceVerification",
        "initialBootstrapHelper",
        "initialBootstrapProofValidation",
        "launchArtifactVerification",
        "launchBinaryVerification",
        "bootCredentialUpload",
        "runtimeLaunchCommand",
    ];
    for name in PHASES {
        let phase = &observations[*name];
        if phase["scope"] != "worker-attempt" || phase["clock"] != "monotonic-duration" {
            continue;
        }
        let (Some(count), Some(total), Some(last), Some(completed), Some(ok)) = (
            phase["callCount"].as_u64(),
            phase["totalDurationMs"].as_u64(),
            phase["lastDurationMs"].as_u64(),
            phase["lastCompletedAtMs"].as_i64(),
            phase["lastResultOk"].as_bool(),
        ) else {
            continue;
        };
        if count == 0 || completed <= 0 || last > total {
            continue;
        }
        safe.insert((*name).into(), json!({"scope":"worker-attempt","clock":"monotonic-duration","callCount":count,"totalDurationMs":total,"lastDurationMs":last,"lastCompletedAtMs":completed,"lastResultOk":ok}));
    }
    // No failed measured stage means there is no failure observation to publish.
    if !safe.values().any(|phase| phase["lastResultOk"] == false) {
        return json!({});
    }
    Value::Object(safe)
}

pub(super) fn merge(config: &mut Value, observations: &Value) {
    let Some(phases) = observations.as_object().filter(|v| !v.is_empty()) else {
        return;
    };
    if !config["startupTimings"].is_object() {
        config["startupTimings"] = json!({});
    }
    if !config["startupTimings"]["providerPhases"].is_object() {
        config["startupTimings"]["providerPhases"] = json!({});
    }
    for (name, value) in phases {
        config["startupTimings"]["providerPhases"][name] = value.clone();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn actual_effect_outcomes_are_measured_without_payload_or_fake_phases() {
        let trace = Observations::default();
        let result: Result<&str, &str> = trace
            .measure("providerCreate", async { Err("secret-error-body") })
            .await;
        assert!(result.is_err());
        let result: Result<&str, &str> = trace
            .measure("providerInspect", async { Ok("private-provider-output") })
            .await;
        assert!(result.is_ok());
        let value = trace.snapshot();
        assert_eq!(value["providerCreate"]["lastResultOk"], false);
        assert_eq!(value["providerInspect"]["lastResultOk"], true);
        assert!(value.get("repositoryFetch").is_none());
        assert!(!value.to_string().contains("private-provider-output"));
        assert!(!value.to_string().contains("secret-error-body"));
    }
    #[test]
    fn failed_observations_allow_only_named_scalar_metrics_and_empty_is_noop() {
        let trace = Observations::default();
        trace.record("providerInspect", 3, false);
        let mut input = trace.snapshot();
        input["providerInspect"]["stdout"] = json!("private-output");
        input["providerInspect"]["error"] = json!("private-key");
        input["private-phase"] = input["providerInspect"].clone();
        let safe = safe_failure_observations(&input);
        assert_eq!(safe["providerInspect"]["lastResultOk"], false);
        assert!(safe.get("private-phase").is_none());
        assert!(!safe.to_string().contains("private"));
        assert_eq!(safe_failure_observations(&json!({})), json!({}));
        input["providerInspect"]["lastResultOk"] = json!(true);
        assert_eq!(safe_failure_observations(&input), json!({}));
        input["providerInspect"]["lastResultOk"] = json!(false);
        input["providerInspect"]["callCount"] = json!("private-count");
        assert_eq!(safe_failure_observations(&input), json!({}));
    }

    #[test]
    fn cumulative_attempt_snapshot_replaces_itself_without_double_counting_or_changing_milestones()
    {
        let trace = Observations::default();
        trace.record("providerInspect", 7, true);
        trace.record("providerInspect", 3, true);
        let mut config = json!({"startupTimings":{"requestedAt":11,"enrolledAt":20}});
        merge(&mut config, &trace.snapshot());
        merge(&mut config, &trace.snapshot());
        assert_eq!(
            config["startupTimings"]["providerPhases"]["providerInspect"]["callCount"],
            2
        );
        assert_eq!(
            config["startupTimings"]["providerPhases"]["providerInspect"]["totalDurationMs"],
            10
        );
        assert_eq!(config["startupTimings"]["enrolledAt"], 20);
        let original = config.clone();
        merge(&mut config, &json!({}));
        assert_eq!(config, original);
    }
}

#[cfg(test)]
#[path = "timings_pg_tests.rs"]
mod pg_tests;
