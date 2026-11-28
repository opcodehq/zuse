//! What a failed runtime launch printed, kept long enough to record it.
//!
//! Provider launch calls return a bare `ProviderError`, so a launcher that
//! exits non-zero (for example the harness gate's `ValueError` on
//! 2026-11-24) used to leave nothing but a generic status code. Every launch
//! path now records the exit code and a bounded, scrubbed tail of the output
//! here, keyed by machine; the worker that owns the failed attempt takes it
//! and stores it on the workspace as `rustLastLaunchFailure`, next to the
//! status code it already writes.
use crate::providers::ExecResult;
use serde::Serialize;
use std::sync::Mutex;

/// Reports kept at once; older ones are dropped (a report is only useful to
/// the attempt that produced it).
const KEPT: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LaunchReport {
    pub exit_code: i32,
    /// Stable classification for dashboards and support.
    pub code: String,
    /// Printable, credential-scrubbed tail of stderr (else stdout).
    pub detail: String,
    pub at: i64,
    /// Distinguishes identical failures recorded in the same millisecond.
    #[serde(skip)]
    pub id: u64,
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

static REPORTS: Mutex<Vec<(String, LaunchReport)>> = Mutex::new(Vec::new());

/// A stable code for what the launcher said.
pub(crate) fn classify(exit_code: i32, detail: &str) -> String {
    if exit_code == 78 {
        return "launcher_runtime_hash_mismatch".into();
    }
    if detail.contains("harness release failed") {
        return "launcher_harness_release_refused".into();
    }
    // The launcher's own refusals are single identifiers on stderr.
    if let Some(word) = detail.split(' ').find(|word| {
        (word.starts_with("runtime_") || word.starts_with("fresh_boot_"))
            && word.len() <= 64
            && word.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
    }) {
        return word.into();
    }
    format!("launcher_exit_{}", exit_code.clamp(-255, 255))
}

pub(crate) fn report(result: &ExecResult, now: i64) -> LaunchReport {
    let detail = super::launch_repair::tail(result);
    LaunchReport {
        exit_code: result.exit_code,
        code: classify(result.exit_code, &detail),
        detail,
        at: now,
        id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    }
}

/// Record a failed launch on `machine`. Successful launches clear it.
pub(crate) fn record(machine: &str, result: &ExecResult) {
    let mut reports = REPORTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    reports.retain(|(id, _)| id != machine);
    if result.exit_code != 0 {
        let report = report(result, chrono::Utc::now().timestamp_millis());
        tracing::warn!(
            machine,
            exit_code = report.exit_code,
            code = %report.code,
            detail = %report.detail,
            "Cloud runtime launch command failed"
        );
        reports.push((machine.to_owned(), report));
        let excess = reports.len().saturating_sub(KEPT);
        reports.drain(..excess);
    }
}

fn peek(machine: &str) -> Option<LaunchReport> {
    REPORTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(id, _)| id == machine)
        .map(|(_, report)| report.clone())
}
/// Forget `report` once stored; a newer report recorded meanwhile stays.
fn stored(machine: &str, report: &LaunchReport) {
    REPORTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|(id, kept)| !(id == machine && kept == report));
}

/// The last failed launch on `machine`, once.
#[cfg(test)]
pub(crate) fn take(machine: &str) -> Option<LaunchReport> {
    let mut reports = REPORTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let index = reports.iter().position(|(id, _)| id == machine)?;
    Some(reports.remove(index).1)
}

/// Store the last failed launch of the workspace's machine, if any, next to
/// the status code the caller just wrote.
pub(crate) async fn persist(pool: &sqlx::PgPool, workspace: &str) -> Result<(), sqlx::Error> {
    let machine: Option<String> = sqlx::query_scalar(
        "SELECT provider_sandbox_id FROM api_cloud_workspaces WHERE workspace_id=$1",
    )
    .bind(workspace)
    .fetch_optional(pool)
    .await?
    .flatten();
    let Some(machine) = machine else {
        return Ok(());
    };
    // Kept until the write succeeds, so a failed write can be retried.
    let Some(report) = peek(&machine) else {
        return Ok(());
    };
    let value = serde_json::to_value(&report).unwrap_or_default();
    // Never replace a newer stored report (an overlapping, older attempt).
    let written = sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustLastLaunchFailure}',$2) WHERE workspace_id=$1 AND provider_sandbox_id=$3 AND COALESCE((request_config#>>'{rustLastLaunchFailure,at}')::bigint,0)<=$4")
        .bind(workspace)
        .bind(value)
        .bind(&machine)
        .bind(report.at)
        .execute(pool)
        .await?;
    // Stored, or superseded by a newer stored report: either way done. No
    // row because the machine changed meanwhile keeps it.
    let superseded = written.rows_affected() == 0
        && sqlx::query_scalar::<_, bool>("SELECT COALESCE((request_config#>>'{rustLastLaunchFailure,at}')::bigint,0)>$3 FROM api_cloud_workspaces WHERE workspace_id=$1 AND provider_sandbox_id=$2")
            .bind(workspace)
            .bind(&machine)
            .bind(report.at)
            .fetch_optional(pool)
            .await?
            .unwrap_or(false);
    if written.rows_affected() == 1 || superseded {
        stored(&machine, &report);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn failed(exit_code: i32, stderr: &str) -> ExecResult {
        ExecResult {
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
            exit_code,
        }
    }
    #[test]
    fn a_failed_launch_keeps_its_exit_code_and_a_scrubbed_classified_tail() {
        let secret = "sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdef";
        record(
            "machine-a",
            &failed(
                1,
                &format!("token={secret}\nharness release failed: ValueError\n"),
            ),
        );
        let report = take("machine-a").expect("recorded");
        assert_eq!(report.exit_code, 1);
        assert_eq!(report.code, "launcher_harness_release_refused");
        assert!(report.detail.contains("ValueError"));
        assert!(!report.detail.contains(secret), "{}", report.detail);
        assert!(report.detail.len() <= 4 * 1024);
        assert!(take("machine-a").is_none(), "taken once");
        // Identical failures in the same millisecond stay distinct.
        let a = super::report(&failed(1, "same\n"), 7);
        let b = super::report(&failed(1, "same\n"), 7);
        assert_ne!(a, b);
        // A later success clears an older failure.
        record("machine-b", &failed(1, "runtime_supervisor_missing\n"));
        record("machine-b", &failed(0, ""));
        assert!(take("machine-b").is_none());
    }
    #[tokio::test]
    #[ignore = "requires explicit isolated localhost PostgreSQL"]
    async fn the_owning_worker_stores_the_report_on_the_workspace() {
        let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
        assert!(matches!(
            reqwest::Url::parse(&url).unwrap().host_str(),
            Some("localhost" | "127.0.0.1" | "::1")
        ));
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("launch_report_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let scope = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |c, _| {
                let scope = scope.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={scope}"))
                        .execute(c)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY, provider_sandbox_id text, request_config jsonb NOT NULL DEFAULT '{}'); INSERT INTO api_cloud_workspaces(workspace_id,provider_sandbox_id) VALUES('ws-report','machine-report')")
            .execute(&pool)
            .await
            .unwrap();
        record(
            "machine-report",
            &failed(1, "harness release failed: ValueError\n"),
        );
        // A failed UPDATE (the lookup works) keeps the report for next time.
        sqlx::raw_sql("CREATE FUNCTION refuse() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RAISE EXCEPTION 'refused'; END$$; CREATE TRIGGER refuse BEFORE UPDATE ON api_cloud_workspaces FOR EACH ROW EXECUTE FUNCTION refuse()")
            .execute(&pool)
            .await
            .unwrap();
        assert!(persist(&pool, "ws-report").await.is_err());
        assert!(peek("machine-report").is_some());
        sqlx::raw_sql("DROP TRIGGER refuse ON api_cloud_workspaces")
            .execute(&pool)
            .await
            .unwrap();
        // An UPDATE that matches no row (the machine changed) keeps it too.
        sqlx::query("UPDATE api_cloud_workspaces SET provider_sandbox_id='machine-other'")
            .execute(&pool)
            .await
            .unwrap();
        record("machine-other", &failed(1, "other\n"));
        sqlx::query("UPDATE api_cloud_workspaces SET provider_sandbox_id='machine-report'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(take("machine-other").is_some());
        persist(&pool, "ws-report").await.unwrap();
        assert!(peek("machine-report").is_none(), "forgotten once stored");
        // An older report never replaces a newer stored one.
        {
            let mut older = super::report(&failed(2, "older\n"), 1);
            older.code = "older".into();
            REPORTS
                .lock()
                .unwrap()
                .push(("machine-report".into(), older));
        }
        persist(&pool, "ws-report").await.unwrap();
        let kept: String = sqlx::query_scalar(
            "SELECT request_config#>>'{rustLastLaunchFailure,code}' FROM api_cloud_workspaces",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(kept, "launcher_harness_release_refused");
        assert!(
            peek("machine-report").is_none(),
            "a superseded report is dropped"
        );
        let stored: serde_json::Value = sqlx::query_scalar(
            "SELECT request_config->'rustLastLaunchFailure' FROM api_cloud_workspaces",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stored["exitCode"], 1);
        assert_eq!(stored["code"], "launcher_harness_release_refused");
        assert!(stored["detail"].as_str().unwrap().contains("ValueError"));
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
    #[test]
    fn launcher_refusals_classify_to_stable_codes() {
        assert_eq!(classify(78, ""), "launcher_runtime_hash_mismatch");
        assert_eq!(
            classify(1, "runtime_supervisor_missing"),
            "runtime_supervisor_missing"
        );
        assert_eq!(classify(127, "sh: not found"), "launcher_exit_127");
        assert_eq!(classify(9999, ""), "launcher_exit_255");
    }
}
