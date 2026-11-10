//! Host-owned credentials never cross the shared Computer panel RPC boundary.
use super::*;
impl CloudApi {
    pub(super) async fn computer_recreation(
        &self,
        token: &str,
        account: &str,
        organization: Option<&str>,
        params: &Value,
    ) -> Result<Value> {
        ensure!(
            params["expectedAccountId"].as_str() == Some(account)
                && params["expectedOrganizationId"].as_str() == organization,
            "The account changed. Select the computer again."
        );
        let workspace = params["workspaceId"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 512)
            .ok_or_else(|| anyhow::anyhow!("Select a computer."))?;
        let mut path = vec![
            "v1",
            "cloud",
            "workspaces",
            workspace,
            "computer-recreations",
        ];
        let action = params["action"].as_str().unwrap_or("start");
        let body = match action {
            "start" => {
                let key = params["idempotencyKey"]
                    .as_str()
                    .filter(|id| uuid::Uuid::parse_str(id).is_ok())
                    .ok_or_else(|| anyhow::anyhow!("Computer recreation request is missing."))?;
                Some(json!({"idempotencyKey":key}))
            }
            "status" | "retry" => {
                let operation = params["operationId"]
                    .as_str()
                    .filter(|id| uuid::Uuid::parse_str(id).is_ok())
                    .ok_or_else(|| anyhow::anyhow!("Computer recreation operation is missing."))?;
                path.push(operation);
                if action == "retry" {
                    path.push("retry");
                    Some(json!({}))
                } else {
                    None
                }
            }
            _ => anyhow::bail!("Unknown computer recreation action."),
        };
        let result = self
            .request_scoped(token, &path, body, organization, &[])
            .await?;
        if action == "start" || action == "retry" {
            self.environments
                .retire_scoped(account, organization, workspace);
        }
        Ok(
            json!({"accountId":account,"organizationId":organization,"workspaceId":workspace,"computerRecreation":result}),
        )
    }
}
