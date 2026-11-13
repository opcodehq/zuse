//! A message can wait on its computer's in-place runtime update. The app polls
//! the update's progress through the host, which holds the account token.
use super::*;
use zuse_protocol::cloud_runtime_updates::{
    RUNTIME_UPDATE_UNAVAILABLE, RUNTIME_UPDATING, UpdateStatus,
};

/// Admission refusals the app handles itself: it keeps the message and either
/// waits for the update or offers Update computer.
pub(super) fn rejection(code: &str) -> Option<&'static str> {
    match code {
        RUNTIME_UPDATING => Some(RUNTIME_UPDATING),
        RUNTIME_UPDATE_UNAVAILABLE => Some(RUNTIME_UPDATE_UNAVAILABLE),
        _ => None,
    }
}

/// Only a well-formed status reaches the app; anything else is an error it
/// retries on its next poll.
fn status(value: Value) -> Result<UpdateStatus> {
    let status: UpdateStatus = serde_json::from_value(value)
        .map_err(|_| anyhow::anyhow!("The computer's update status couldn't be read."))?;
    ensure!(
        status
            .error_code
            .as_deref()
            .is_none_or(|code| code.len() <= 128),
        "The computer's update status couldn't be read."
    );
    Ok(status)
}

impl CloudApi {
    pub(super) async fn runtime_update_status(
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
        let value = self
            .request_scoped(
                token,
                &["v1", "cloud", "workspaces", workspace, "runtime-update"],
                None,
                organization,
                &[],
            )
            .await?;
        let status = status(value)?;
        Ok(
            json!({"accountId":account,"organizationId":organization,"workspaceId":workspace,"status":status}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_refusals_reach_the_app_as_their_codes() {
        for code in [RUNTIME_UPDATING, RUNTIME_UPDATE_UNAVAILABLE] {
            let rpc = TaskRejection(rejection(code).unwrap()).rpc();
            assert!(matches!(
                rpc,
                zuse_rpc::RpcError::Capability(text)
                    if text == format!("{}{code}", zuse_protocol::cloud_task_commands::TASK_REJECTED_PREFIX)
            ));
        }
        assert!(rejection("cloud_task_model_not_installed").is_none());
    }

    #[test]
    fn status_accepts_the_contract_shape_only() {
        let parsed = status(
            json!({"updateId":"u1","state":"rolled-back","errorCode":"runtime_update_rolled_back"}),
        )
        .unwrap();
        assert_eq!(
            parsed.state,
            zuse_protocol::cloud_runtime_updates::UpdateState::RolledBack
        );
        assert!(status(json!({"state":"exploded"})).is_err());
        assert!(status(json!({"state":"failed","errorCode":"x".repeat(129)})).is_err());
    }
}
