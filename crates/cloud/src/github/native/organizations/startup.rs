//! Restart and missed-webhook recovery seeds one coalesced inspection per
//! enabled organization installation. No machine is allocated or woken here.
use super::*;
impl Joining {
    pub async fn seed(&self, now: i64) -> Result<(), ApiError> {
        let organizations: Vec<_> = self.organizations.iter().cloned().collect();
        let ids:Vec<i64>=sqlx::query_scalar("SELECT DISTINCT installation_id FROM api_github_join_policies WHERE organization_id=ANY($1)").bind(organizations).fetch_all(&self.pool).await?;
        for id in ids {
            self.schedule(id, now).await?;
        }
        Ok(())
    }
}
