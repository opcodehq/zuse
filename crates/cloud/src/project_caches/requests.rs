//! Saving one project's recipe enqueues only that project, without checking payment.
use super::*;
impl crate::computers::ComputerControl {
    pub(crate) async fn enqueue_preparation_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        owner: &str,
        actor: &str,
        actor_metadata: Option<Value>,
        project: &str,
        revision: i64,
        digest: &str,
        repository_identity: &str,
        requested_ref: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        let ready = if let Some(value) = self.cache_schema_ready.get() {
            *value
        } else {
            let value = schema_ready_tx(tx).await?;
            let _ = self.cache_schema_ready.set(value);
            value
        };
        if !ready {
            return Ok(());
        }
        sqlx::query("INSERT INTO zuse_rust_project_preparation_requests(request_id,owner_id,actor_id,actor,project_id,environment_revision,recipe_digest,repository_identity,requested_ref,state,next_attempt_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'queued',$10,$10,$10) ON CONFLICT(owner_id,project_id,environment_revision,requested_ref) DO NOTHING")
            .bind(format!("prep_{}",uuid::Uuid::new_v4().simple())).bind(owner).bind(actor).bind(actor_metadata)
            .bind(project).bind(revision).bind(digest).bind(repository_identity).bind(requested_ref).bind(now).execute(&mut **tx).await?;
        Ok(())
    }
}

impl crate::store::Store {
    pub async fn project_preparation(&self, owner: &str, project: &str) -> Result<Value, ApiError> {
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_projects WHERE account_id=$1 AND project_id=$2 AND included=true)").bind(owner).bind(project).fetch_one(&self.pool).await?;
        if !exists {
            return Err(ApiError::not_found("cloud_project_not_found"));
        }
        let Some(control) = &self.computers else {
            return Ok(
                json!({"projectId":project,"state":"unavailable","managedBaseFallback":true}),
            );
        };
        let ready = if let Some(value) = control.cache_schema_ready.get() {
            *value
        } else {
            let value = schema_ready(&control.pool).await?;
            let _ = control.cache_schema_ready.set(value);
            value
        };
        if !ready {
            return Ok(
                json!({"projectId":project,"state":"unavailable","managedBaseFallback":true}),
            );
        }
        let request:Option<Value>=sqlx::query_scalar("SELECT jsonb_build_object('requestId',request_id,'revision',environment_revision,'state',state,'errorCode',last_error_code,'updatedAt',updated_at) FROM zuse_rust_project_preparation_requests WHERE owner_id=$1 AND project_id=$2 ORDER BY environment_revision DESC LIMIT 1").bind(owner).bind(project).fetch_optional(&self.pool).await?;
        Ok(json!({"projectId":project,"preparation":request,"managedBaseFallback":true}))
    }
}
