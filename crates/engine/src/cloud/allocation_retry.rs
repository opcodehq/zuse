//! A retry addresses the original rejected allocation; it never creates a task.
use super::*;
impl CloudApi {
    pub(super) async fn allocation_retry(
        &self,
        token: &str,
        account: &str,
        organization: Option<&str>,
        params: &Value,
    ) -> Result<Value> {
        ensure!(
            params["expectedAccountId"].as_str() == Some(account)
                && params["expectedOrganizationId"].as_str() == organization,
            "The account changed. Reopen this computer before retrying."
        );
        let (workspace, operation, command) = request(params)?;
        let result = self
            .request_scoped(
                token,
                &[
                    "v1",
                    "cloud",
                    "workspaces",
                    workspace,
                    "allocations",
                    operation,
                    "retry",
                ],
                Some(json!({"commandId":command})),
                organization,
                &[],
            )
            .await?;
        ensure!(
            result["workspaceId"].as_str() == Some(workspace)
                && result["operationId"].as_str() == Some(operation)
                && result["commandId"].as_str() == Some(command),
            "Allocation retry response did not match the original request."
        );
        Ok(
            json!({"accountId":account,"organizationId":organization,"workspaceId":workspace,"allocationRetry":result}),
        )
    }
}
fn request(params: &Value) -> Result<(&str, &str, &str)> {
    let id = |key: &str| {
        params[key]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control))
            .ok_or_else(|| anyhow::anyhow!("The original allocation retry is missing."))
    };
    let workspace = id("workspaceId")?;
    let operation = id("operationId")?;
    let command = id("commandId")?;
    ensure!(
        uuid::Uuid::parse_str(command).is_ok(),
        "The retry request identity is invalid."
    );
    Ok((workspace, operation, command))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_allocation_and_exact_retry_identity_are_required() {
        let mut body = json!({"workspaceId":"original-task","operationId":"original-allocation","commandId":"f7552dc6-25aa-4974-826c-7c68c5134c12"});
        assert_eq!(
            request(&body).unwrap(),
            (
                "original-task",
                "original-allocation",
                "f7552dc6-25aa-4974-826c-7c68c5134c12"
            )
        );
        body["commandId"] = json!("new-task");
        assert!(request(&body).is_err());
        body["operationId"] = Value::Null;
        assert!(request(&body).is_err());
    }
}
