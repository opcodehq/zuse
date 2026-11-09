//! Installation tokens are used only for organization roster authorization.
//! Task Git operations still require the selected user's scoped token.
use super::*;
pub struct GithubRoster(pub Arc<NativeGithub>);
#[async_trait]
impl Roster for GithubRoster {
    async fn members(&self, policy: &Policy) -> Result<BTreeSet<i64>, ApiError> {
        let authority = &self.0;
        let owner = format!("organization:{}", policy.organization_id);
        if !authority.enabled(&owner) {
            return Err(ApiError::forbidden("github_native_disabled"));
        }
        let metadata = authority
            .observe(&owner, policy.installation_id)
            .await?
            .ok_or_else(|| ApiError::forbidden("github_installation_unavailable"))?;
        if metadata["account"]["id"] != policy.github_org_id
            || metadata["account"]["type"] != "Organization"
            || !metadata["suspended_at"].is_null()
        {
            return Err(ApiError::forbidden("github_installation_unavailable"));
        }
        let nonce = opaque();
        let revision:i64=sqlx::query_scalar("UPDATE zuse_rust_github_installations SET observation_id=$3 WHERE owner_id=$1 AND installation_id=$2 AND state='active' RETURNING revision").bind(&owner).bind(policy.installation_id).bind(&nonce).fetch_optional(&authority.pool).await?.ok_or_else(||ApiError::forbidden("github_installation_unavailable"))?;
        let login = metadata["account"]["login"]
            .as_str()
            .filter(|s| valid_component(s))
            .ok_or_else(unavailable)?;
        let (status, mut value) = authority
            .response(
                authority
                    .http
                    .post(authority.api(&format!(
                        "/app/installations/{}/access_tokens",
                        policy.installation_id
                    )))
                    .bearer_auth(authority.app_token()?)
                    .json(&json!({"permissions":{"members":"read"}})),
            )
            .await?;
        let token = match value.get_mut("token").map(Value::take) {
            Some(Value::String(value)) => Zeroizing::new(value),
            _ => return Err(unavailable()),
        };
        if status != 201
            || !matches!(
                value["permissions"]["members"].as_str(),
                Some("read" | "write")
            )
        {
            return Err(ApiError::unavailable("github_members_permission_required"));
        }
        let mut members = BTreeSet::new();
        let mut complete = false;
        for page in 1..=100 {
            let response = authority
                .get(
                    &format!("/orgs/{login}/members?per_page=100&page={page}"),
                    token.as_str(),
                )
                .await?;
            let entries = response
                .as_array()
                .filter(|e| e.len() <= 100)
                .ok_or_else(|| ApiError::unavailable("github_members_invalid"))?;
            for entry in entries {
                let id = entry["id"]
                    .as_i64()
                    .filter(|id| *id > 0)
                    .ok_or_else(|| ApiError::unavailable("github_members_invalid"))?;
                members.insert(id);
            }
            if entries.len() < 100 {
                complete = true;
                break;
            }
        }
        if !complete {
            return Err(ApiError::unavailable("github_roster_too_large"));
        }
        // Recheck connection after the last network page. Webhook disconnect cannot
        // resurrect roster authorization when an old fetch completes afterward.
        let mut tx = authority.pool.begin().await?;
        let connected:Option<i64>=sqlx::query_scalar("SELECT revision FROM zuse_rust_github_installations WHERE owner_id=$1 AND installation_id=$2 AND state='active' AND metadata->'account'->>'id'=$3 AND observation_id=$4 FOR SHARE").bind(&owner).bind(policy.installation_id).bind(policy.github_org_id.to_string()).bind(&nonce).fetch_optional(&mut *tx).await?;
        if connected != Some(revision) {
            return Err(ApiError::conflict("github_roster_superseded"));
        }
        sqlx::query("DELETE FROM api_github_org_members WHERE installation_id=$1")
            .bind(policy.installation_id)
            .execute(&mut *tx)
            .await?;
        for id in &members {
            sqlx::query(
                "INSERT INTO api_github_org_members(installation_id,github_user_id) VALUES($1,$2)",
            )
            .bind(policy.installation_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(members)
    }
}
