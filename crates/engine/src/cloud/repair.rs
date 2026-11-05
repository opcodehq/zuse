//! Explicit repair operations preserve command identities and uncertain outcomes.
//! Retrying start reuses its idempotency key; resolution never replays a command.
use super::*;
impl CloudApi {
    pub(super) async fn runtime_repair(
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
        let action = params["action"].as_str().unwrap_or("start");
        let mut path = vec!["v1", "cloud", "workspaces", workspace, "runtime-repairs"];
        let body = match action {
            "start" => {
                let key = params["idempotencyKey"]
                    .as_str()
                    .filter(|key| uuid::Uuid::parse_str(key).is_ok())
                    .ok_or_else(|| anyhow::anyhow!("Runtime restart request is missing."))?;
                self.environments
                    .retire_scoped(account, organization, workspace);
                Some(json!({"idempotencyKey":key}))
            }
            "status" | "resolve" | "retry" => {
                let operation = params["operationId"]
                    .as_str()
                    .filter(|id| !id.is_empty() && id.len() <= 128)
                    .ok_or_else(|| anyhow::anyhow!("Select a runtime restart operation."))?;
                path.push(operation);
                if action == "retry" {
                    path.push("retry");
                    Some(json!({}))
                } else if action == "resolve" {
                    let decision = params["decision"]
                        .as_str()
                        .filter(|value| matches!(*value, "continue" | "discard"))
                        .ok_or_else(|| {
                            anyhow::anyhow!("Choose how to resolve the interrupted work.")
                        })?;
                    path.push("resolve");
                    Some(json!({"decision":decision}))
                } else {
                    None
                }
            }
            _ => anyhow::bail!("Unknown runtime restart action."),
        };
        let result = self
            .request_scoped(token, &path, body, organization, &[])
            .await?;
        Ok(
            json!({"accountId":account,"organizationId":organization,"workspaceId":workspace,"runtimeRepair":result}),
        )
    }
}
