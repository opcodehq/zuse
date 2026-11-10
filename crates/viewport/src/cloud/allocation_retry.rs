//! Explicit rejected-create recovery retains one request key across uncertain replies.
use super::*;
#[derive(Default)]
pub(super) struct State {
    request: Option<u64>,
    binding: Option<(String, Option<String>, String, String, String, i64)>,
    error: Option<String>,
}
fn eligible(target: &Value) -> Option<(&str, &str, i64)> {
    let recovery = &target["allocationRecovery"];
    if recovery["state"] != "rejected"
        || recovery["errorCode"] != "cloud_provider_rejected"
        || recovery["retryAvailable"] != true
    {
        return None;
    }
    Some((
        target["workspaceId"].as_str()?,
        recovery["operationId"].as_str()?,
        recovery["rejectedAt"].as_i64()?,
    ))
}
impl WorkspaceView {
    fn retry_original_allocation(&mut self, cx: &mut Context<Self>) {
        if self.cloud.allocation_retry.request.is_some() {
            return;
        }
        let Some((workspace, operation, rejected_at)) =
            self.cloud.target.as_ref().and_then(eligible)
        else {
            return;
        };
        let Some(account) = self.cloud.account.clone() else {
            return;
        };
        let (workspace, operation) = (workspace.to_owned(), operation.to_owned());
        let state = &mut self.cloud.allocation_retry;
        if state
            .binding
            .as_ref()
            .is_none_or(|(a, o, w, p, _, episode)| {
                a != &account
                    || o != &self.cloud.organization
                    || w != &workspace
                    || p != &operation
                    || *episode != rejected_at
            })
        {
            state.binding = Some((
                account.clone(),
                self.cloud.organization.clone(),
                workspace.clone(),
                operation.clone(),
                uuid::Uuid::new_v4().to_string(),
                rejected_at,
            ));
        }
        let key = state.binding.as_ref().unwrap().4.clone();
        state.error = None;
        let request = self.call(
            methods::CLOUD_ALLOCATION_RETRY,
            json!({"workspaceId":workspace,"operationId":operation,"commandId":key,
            "expectedAccountId":account,"expectedOrganizationId":self.cloud.organization}),
        );
        self.cloud.allocation_retry.request = Some(request);
        cx.notify();
    }
    pub(super) fn receive_allocation_retry(
        &mut self,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(id) = self.cloud.allocation_retry.request else {
            return false;
        };
        if frame["id"].as_u64() != Some(id) {
            return false;
        }
        if let Some(value) = frame.get("ok") {
            let valid =
                self.cloud
                    .allocation_retry
                    .binding
                    .as_ref()
                    .is_some_and(|(a, o, w, p, k, _)| {
                        self.cloud.account.as_ref() == Some(a)
                            && &self.cloud.organization == o
                            && self
                                .cloud
                                .target
                                .as_ref()
                                .and_then(|t| t["workspaceId"].as_str())
                                == Some(w.as_str())
                            && value["accountId"].as_str() == Some(a)
                            && value["organizationId"].as_str() == o.as_deref()
                            && value["allocationRetry"]["operationId"].as_str() == Some(p)
                            && value["allocationRetry"]["commandId"].as_str() == Some(k)
                    });
            if valid {
                self.cloud.notice=Some("The original start request was accepted for retry. Its saved message was not sent again.".into());
            } else {
                self.cloud.allocation_retry.error=Some("The retry response did not match this computer. Refresh its details before continuing.".into());
            }
        }
        if frame.get("err").is_some() {
            self.cloud.allocation_retry.error=Some("The retry could not be confirmed. Refresh details; retrying this action keeps the same request identity.".into());
        }
        if frame.get("ok").is_some() || frame.get("err").is_some() || frame["done"] == true {
            self.pending.remove(&id);
            self.cloud.allocation_retry.request = None;
            self.refresh_computer_metadata();
            cx.notify();
        }
        true
    }
    pub(super) fn allocation_retry_controls(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = self.cloud.target.as_ref()?;
        if target["allocationRecovery"]["state"] != "rejected" {
            return None;
        }
        let mut view=column().gap_1().child(label("Computer start was rejected").px_2().text_size(px(12.)))
            .child(label("Retry the original start after reviewing provider access. Your task and saved message are retained.").px_2().text_size(px(11.)));
        if eligible(target).is_some() {
            if self.cloud.allocation_retry.request.is_some() {
                view = view.child(label("Retrying original start…").px_2());
            } else {
                view = view.child(
                    control("retry-original-allocation", "Retry original start")
                        .on_click(cx.listener(|this, _, _, cx| this.retry_original_allocation(cx))),
                );
            }
        }
        if let Some(error) = &self.cloud.allocation_retry.error {
            view = view.child(label(error.clone()).px_2().text_size(px(11.)));
        }
        Some(view.into_any_element())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn unknown_or_view_only_creates_never_offer_retry() {
        let mut target = json!({"workspaceId":"saved","allocationRecovery":{"operationId":"original","state":"rejected","errorCode":"cloud_provider_rejected","retryAvailable":true,"rejectedAt":123}});
        assert_eq!(eligible(&target), Some(("saved", "original", 123)));
        target["allocationRecovery"]["retryAvailable"] = json!(false);
        assert!(eligible(&target).is_none());
        target["allocationRecovery"]["retryAvailable"] = json!(true);
        target["allocationRecovery"]["state"] = json!("unknown");
        assert!(eligible(&target).is_none());
    }
}
