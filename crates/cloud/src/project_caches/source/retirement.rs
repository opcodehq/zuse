//! Tombstones retain exact object ownership even after a late/uncertain PUT.
use super::*;
impl PublicBundles {
    pub(super) async fn reap(&self, now: i64) -> Result<bool, ApiError> {
        if !self.objects.supports_purge() {
            return Err(ApiError::unavailable(
                "project_cache_object_retirement_unsupported",
            ));
        }
        let lease = uuid::Uuid::new_v4().to_string();
        // A source still used by a live artifact, operation or resolver remains.
        // A stale writing row can have an unknown remote PUT: keep its cleanup
        // tombstone forever and resweep; never claim a finite deletion proves
        // that a formerly cancelled upload cannot arrive later.
        // Disconnected projects need no day-long retention grace. An original
        // resolver with a live lease still holds its source until fenced/released.
        let row:Option<Value>=sqlx::query_scalar("WITH chosen AS (SELECT s.owner_id,s.project_id,s.bundle_sha256 FROM zuse_rust_project_source_bundles s WHERE (s.cleanup_lease_expires_at IS NULL OR s.cleanup_lease_expires_at<=$1) AND ((s.state='cleaning' AND coalesce(s.cleanup_due_at,0)<=$1) OR (s.state IN('ready','writing') AND (s.updated_at<$1-86400000 OR NOT EXISTS(SELECT 1 FROM api_cloud_projects p WHERE p.account_id=s.owner_id AND p.project_id=s.project_id AND p.included=true)) AND NOT EXISTS(SELECT 1 FROM zuse_rust_project_cache_operations o JOIN zuse_rust_project_cache_entries e USING(cache_key) WHERE o.owner_id=s.owner_id AND o.project_id=s.project_id AND o.plan->>'sourceBundleSha256'=s.bundle_sha256 AND (o.state IN('queued','running','uncertain','cleaning') OR e.state='ready')) AND NOT EXISTS(SELECT 1 FROM zuse_rust_project_preparation_requests r WHERE r.owner_id=s.owner_id AND r.project_id=s.project_id AND r.state IN('queued','blocked','resolving','preparing') AND r.last_error_code IS DISTINCT FROM 'project_preparation_disabled' AND (coalesce(r.lease_expires_at,0)>$1 OR EXISTS(SELECT 1 FROM api_cloud_projects p WHERE p.account_id=s.owner_id AND p.project_id=s.project_id AND p.included=true))))) ORDER BY coalesce(s.cleanup_due_at,s.updated_at),s.owner_id,s.project_id FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE zuse_rust_project_source_bundles s SET state='cleaning',cleanup_lease_id=$2,cleanup_lease_expires_at=$1+60000,cleanup_attempts=cleanup_attempts+1 FROM chosen c WHERE s.owner_id=c.owner_id AND s.project_id=c.project_id AND s.bundle_sha256=c.bundle_sha256 RETURNING to_jsonb(s)").bind(now).bind(&lease).fetch_optional(&self.pool).await?;
        let Some(row) = row else { return Ok(false) };
        let owner = row["owner_id"]
            .as_str()
            .ok_or_else(|| ApiError::conflict("project_cache_source_identity_invalid"))?;
        let project = row["project_id"]
            .as_str()
            .ok_or_else(|| ApiError::conflict("project_cache_source_identity_invalid"))?;
        let hash = row["bundle_sha256"]
            .as_str()
            .filter(|s| {
                s.len() == 64
                    && s.bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
            .ok_or_else(|| ApiError::conflict("project_cache_source_identity_invalid"))?;
        let prefix = format!(
            "project-environments/{:x}/{:x}/{hash}/",
            Sha256::digest(owner.as_bytes()),
            Sha256::digest(project.as_bytes())
        );
        let absent = self.objects.purge_prefix(&prefix).await.unwrap_or(false);
        let delay = if absent { 3600000 } else { 5000 };
        // Do not delete the original row: a cancelled PUT can complete later.
        sqlx::query("UPDATE zuse_rust_project_source_bundles SET cleanup_due_at=$5,cleanup_lease_id=NULL,cleanup_lease_expires_at=NULL WHERE owner_id=$1 AND project_id=$2 AND bundle_sha256=$3 AND cleanup_lease_id=$4").bind(owner).bind(project).bind(hash).bind(lease).bind(now+delay).execute(&self.pool).await?;
        Ok(true)
    }
}
