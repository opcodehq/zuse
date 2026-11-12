//! A message whose delivery outcome is unknown blocks new runs on its
//! conversation until it is reviewed. The app needs that fact itself, not the
//! generic "busy" text a 409 would otherwise become, so it can offer Retry or
//! Discard on that message.
use zuse_protocol::cloud_task_commands::TASK_UNKNOWN_OUTCOME_PREFIX;

#[derive(Debug)]
pub(crate) struct UnknownOutcome(pub &'static str);
impl std::fmt::Display for UnknownOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            "review-required" => {
                "An earlier message didn't go through. Retry or discard it, then send again."
            }
            _ => "That message no longer needs review.",
        })
    }
}
impl std::error::Error for UnknownOutcome {}
impl UnknownOutcome {
    /// The server's error code, when it is about an unknown outcome.
    pub(crate) fn from_code(code: &str) -> Option<Self> {
        Some(Self(match code {
            "cloud_task_unknown_outcome_review_required" => "review-required",
            "cloud_task_unknown_outcome_not_found" => "not-found",
            _ => return None,
        }))
    }
    pub(crate) fn rpc(&self) -> zuse_rpc::RpcError {
        zuse_rpc::RpcError::Capability(format!("{TASK_UNKNOWN_OUTCOME_PREFIX}{}", self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_outcome_codes_reach_the_app_instead_of_busy() {
        let required = UnknownOutcome::from_code("cloud_task_unknown_outcome_review_required")
            .expect("review required is recognized");
        assert!(matches!(
            super::super::rpc_error(required.into()),
            zuse_rpc::RpcError::Capability(code)
                if code == format!("{TASK_UNKNOWN_OUTCOME_PREFIX}review-required")
        ));
        let missing = UnknownOutcome::from_code("cloud_task_unknown_outcome_not_found").unwrap();
        assert!(matches!(
            missing.rpc(),
            zuse_rpc::RpcError::Capability(code)
                if code == format!("{TASK_UNKNOWN_OUTCOME_PREFIX}not-found")
        ));
        assert!(UnknownOutcome::from_code("cloud_task_model_not_installed").is_none());
    }
}
