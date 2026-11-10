//! Durable coalesced work for membership webhooks and admin enablement. Lease
//! fencing means a late attempt cannot acknowledge a newer requested revision.
use super::*;
impl Joining {
    pub async fn schedule(&self, installation: i64, now: i64) -> Result<(), ApiError> {
        if installation <= 0 {
            return Err(ApiError::bad_request("invalid_github_installation"));
        }
        sqlx::query("INSERT INTO zuse_rust_github_join_jobs(installation_id,updated_at) VALUES($1,$2) ON CONFLICT(installation_id) DO UPDATE SET requested_version=zuse_rust_github_join_jobs.requested_version+1,next_attempt_at=LEAST(zuse_rust_github_join_jobs.next_attempt_at,$2),updated_at=$2").bind(installation).bind(now).execute(&self.pool).await?;
        self.notify_dispatch();
        Ok(())
    }
    pub async fn run_once(&self, now: i64) -> Result<bool, ApiError> {
        let lease = uuid::Uuid::new_v4().to_string();
        let job:Option<(i64,i64)>=sqlx::query_as("UPDATE zuse_rust_github_join_jobs j SET lease_id=$1,lease_until=$2,attempts=attempts+1,updated_at=$3 WHERE installation_id=(SELECT installation_id FROM zuse_rust_github_join_jobs WHERE requested_version>completed_version AND next_attempt_at<=$3 AND (lease_until IS NULL OR lease_until<=$3) ORDER BY next_attempt_at,installation_id FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING installation_id,requested_version").bind(&lease).bind(now+180000).bind(now).fetch_optional(&self.pool).await?;
        let Some((installation, version)) = job else {
            return Ok(false);
        };
        // Reconciliation re-inspects WorkOS before retrying uncertain member changes.
        // Idempotent roster replacement and durable seat reservations survive crashes.
        match tokio::time::timeout(
            Duration::from_secs(120),
            self.reconcile_installation(installation, now),
        )
        .await
        {
            Ok(Ok(())) => {
                sqlx::query("UPDATE zuse_rust_github_join_jobs SET completed_version=GREATEST(completed_version,$3),lease_id=NULL,lease_until=NULL,attempts=0,last_error=NULL,updated_at=$4 WHERE installation_id=$1 AND lease_id=$2 AND lease_until>$4").bind(installation).bind(&lease).bind(version).bind(chrono::Utc::now().timestamp_millis()).execute(&self.pool).await?;
            }
            failure => {
                let code = match failure {
                    Ok(Err(error)) => error.code,
                    _ => "github_join_reconciliation_timeout",
                };
                sqlx::query("UPDATE zuse_rust_github_join_jobs SET lease_id=NULL,lease_until=NULL,next_attempt_at=$3+LEAST(300000,(1000::bigint<<LEAST(attempts,8))),last_error=$4,updated_at=$3 WHERE installation_id=$1 AND lease_id=$2").bind(installation).bind(&lease).bind(chrono::Utc::now().timestamp_millis()).bind(code).execute(&self.pool).await?;
            }
        }
        Ok(true)
    }
}
impl NativeGithub {
    /// Atomically enqueue before the verified delivery receipt advances. A crash
    /// between webhook acceptance and dispatch cannot lose enrollment work.
    pub(in crate::github::native) async fn commit_membership_jobs(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &str,
        value: &Value,
    ) -> Result<(), ApiError> {
        if !matches!(
            event,
            "organization" | "membership" | "member" | "installation" | "installation_repositories"
        ) {
            return Ok(());
        }
        let configured: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_github_join_jobs') IS NOT NULL")
                .fetch_one(&mut **tx)
                .await?;
        if !configured {
            return Ok(());
        }
        let ids: Vec<(i64, String)> = if let Some(id) =
            value["installation"]["id"].as_i64().filter(|id| *id > 0)
        {
            sqlx::query_as("SELECT installation_id,organization_id FROM api_github_join_policies WHERE installation_id=$1").bind(id).fetch_all(&mut **tx).await?
        } else if let Some(org) = value["organization"]["id"].as_i64().filter(|id| *id > 0) {
            sqlx::query_as("SELECT installation_id,organization_id FROM api_github_join_policies WHERE github_org_id=$1").bind(org).fetch_all(&mut **tx).await?
        } else {
            vec![]
        };
        for (id, org) in ids {
            if self.enabled(&format!("organization:{org}")) {
                // Fence cached eligibility and in-flight authorization in every
                // process before acknowledging this delivery. Scheduling alone
                // leaves the prior policy snapshot valid until reconciliation.
                sqlx::query("UPDATE api_github_join_policies SET data=jsonb_set(data,'{revision}',to_jsonb($3::text)) WHERE organization_id=$1 AND installation_id=$2")
                    .bind(&org).bind(id).bind(uuid::Uuid::new_v4().to_string())
                    .execute(&mut **tx).await?;
                sqlx::query("INSERT INTO zuse_rust_github_join_jobs(installation_id,updated_at) VALUES($1,$2) ON CONFLICT(installation_id) DO UPDATE SET requested_version=zuse_rust_github_join_jobs.requested_version+1,next_attempt_at=LEAST(zuse_rust_github_join_jobs.next_attempt_at,$2),updated_at=$2").bind(id).bind(now()).execute(&mut **tx).await?;
            }
        }
        Ok(())
    }
}
