//! Repair controls are explicit and retain the original operation on retry.
use super::*;
#[derive(Default)]
pub(super) struct State {
    request: Option<u64>,
    action: Option<String>,
    dirty: bool,
    workspace: Option<String>,
    key: Option<String>,
    operation: Value,
    confirm: bool,
    expanded: bool,
    error: Option<String>,
    pub(super) poll: Option<Task<()>>,
}
impl WorkspaceView {
    pub(super) fn close_cloud_repair(&mut self) {
        if let Some(id) = self.cloud.repair.request.take() {
            self.cancel(id);
            self.pending.remove(&id);
        }
        self.cloud.repair = State::default();
    }
    fn cloud_repair_operation(&self) -> Value {
        if self
            .cloud
            .target
            .as_ref()
            .and_then(|target| target["workspaceId"].as_str())
            == self.cloud.repair.workspace.as_deref()
            && self.cloud.repair.operation.is_object()
        {
            self.cloud.repair.operation.clone()
        } else {
            self.cloud
                .target
                .as_ref()
                .map(|target| target["runtimeRepair"].clone())
                .unwrap_or(Value::Null)
        }
    }
    pub(super) fn sync_cloud_repair_metadata(&mut self) {
        let Some(target) = &self.cloud.target else {
            return;
        };
        let workspace = target["workspaceId"].as_str().map(str::to_owned);
        let summary = target["runtimeRepair"].clone();
        if summary["operationId"].is_string() {
            let changed = self.cloud.repair.workspace != workspace
                || self.cloud.repair.operation["operationId"] != summary["operationId"];
            let newer = summary["updatedAt"].as_i64().unwrap_or(0)
                > self.cloud.repair.operation["updatedAt"]
                    .as_i64()
                    .unwrap_or(0);
            if changed || newer {
                self.cloud.repair.workspace = workspace;
                self.cloud.repair.operation = summary;
            }
            if changed || newer || self.cloud.repair.operation["recovery"].is_null() {
                if self.cloud.repair.request.is_some() {
                    self.cloud.repair.dirty = true;
                } else {
                    self.refresh_cloud_runtime_repair();
                }
            }
        }
    }
    pub(super) fn refresh_cloud_runtime_repair(&mut self) {
        if self.cloud.repair.request.is_none()
            && self.cloud_repair_operation()["operationId"].is_string()
        {
            self.request_cloud_runtime_repair("status", None);
        }
    }
    fn request_cloud_runtime_repair(&mut self, action: &str, decision: Option<&str>) {
        if self.cloud.repair.request.is_some() {
            return;
        }
        let Some(workspace) = self
            .cloud
            .target
            .as_ref()
            .and_then(|target| target["workspaceId"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        if action != "status" {
            let target = self.cloud.target.as_ref().unwrap();
            if target["runtimeRepairCanEdit"] != true
                || (action == "start" && target["runtimeRepairAvailable"] != true)
            {
                self.cloud.repair.error=Some("Runtime restart is not available with this computer's current permissions and managed base.".into());
                return;
            }
        }
        let prior = self.cloud_repair_operation();
        if self.cloud.repair.workspace.as_deref() != Some(workspace.as_str()) {
            self.close_cloud_repair();
            self.cloud.repair.workspace = Some(workspace.clone());
            self.cloud.repair.operation = prior.clone();
        }
        let mut body = json!({"workspaceId":workspace,"expectedAccountId":self.cloud.account,"expectedOrganizationId":self.cloud.organization,"action":action});
        if action == "start" {
            self.cloud.repair.poll = None;
            if matches!(prior["state"].as_str(), Some("ready" | "failed")) {
                self.cloud.repair.key = None;
            }
            let key = self
                .cloud
                .repair
                .key
                .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
                .clone();
            body["idempotencyKey"] = json!(key);
        } else {
            let Some(operation) = prior["operationId"].as_str() else {
                return;
            };
            body["operationId"] = json!(operation);
            if let Some(decision) = decision {
                body["decision"] = json!(decision);
            }
        }
        self.cloud.repair.error = None;
        self.cloud.repair.action = Some(action.to_owned());
        self.cloud.repair.confirm = false;
        self.cloud.repair.request = Some(self.call(methods::CLOUD_RUNTIME_REPAIR, body));
    }
    pub(super) fn receive_cloud_repair(&mut self, frame: &Value, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.cloud.repair.request else {
            return false;
        };
        if frame["id"].as_u64() != Some(id) {
            return false;
        }
        if let Some(value) = frame.get("ok") {
            if value["accountId"].as_str() == self.cloud.account.as_deref()
                && value["organizationId"].as_str() == self.cloud.organization.as_deref()
                && value["workspaceId"].as_str()
                    == self
                        .cloud
                        .target
                        .as_ref()
                        .and_then(|target| target["workspaceId"].as_str())
            {
                let operation = value["runtimeRepair"].clone();
                if operation["operationId"].is_string() {
                    let current = self.cloud_repair_operation();
                    if accepts_operation(
                        &current,
                        &operation,
                        self.cloud.repair.action.as_deref() == Some("status"),
                    ) {
                        self.cloud.repair.operation = operation;
                    }
                    if self.cloud.repair.operation["state"] == "ready" {
                        self.cloud.repair.poll = None;
                        self.cloud.notice = Some(
                            "Runtime restarted. Reconnect to continue your conversation.".into(),
                        );
                    }
                } else {
                    self.cloud.repair.error = Some(
                        "Restart operation was not returned. Check this computer's status.".into(),
                    );
                }
            } else {
                self.cloud.repair.error =
                    Some("Restart response did not match this account and computer.".into());
            }
        }
        if let Some(error) = frame.get("err") {
            self.cloud.repair.error=Some(error.as_str().or_else(||error["message"].as_str()).unwrap_or("Restart was not confirmed. Retry the same operation to reconcile its state.").into());
        }
        if frame.get("ok").is_some() || frame.get("err").is_some() || frame["done"] == true {
            self.pending.remove(&id);
            self.cloud.repair.request = None;
            self.cloud.repair.action = None;
            if std::mem::take(&mut self.cloud.repair.dirty) {
                self.refresh_cloud_runtime_repair();
            }
            let state = self.cloud_repair_operation()["state"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            if !self.cloud.account_events.live
                && !state.is_empty()
                && !matches!(state.as_str(), "ready" | "failed" | "attention-required")
                && self.cloud.repair.poll.is_none()
            {
                self.cloud.repair.poll = Some(cx.spawn(async move |this, cx| {
                    for _ in 0..60 {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(2))
                            .await;
                        if !this
                            .update(cx, |this, _| {
                                let state = this.cloud_repair_operation()["state"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_owned();
                                if this.cloud.account_events.live
                                    || matches!(
                                        state.as_str(),
                                        "ready" | "failed" | "attention-required"
                                    )
                                {
                                    return false;
                                }
                                this.refresh_cloud_runtime_repair();
                                true
                            })
                            .unwrap_or(false)
                        {
                            return;
                        }
                    }
                }));
            }
        }
        cx.notify();
        true
    }
    pub(super) fn cloud_repair_controls(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = self.cloud.target.as_ref()?;
        let operation = self.cloud_repair_operation();
        if target["runtimeRepairAvailable"] != true && !operation["operationId"].is_string() {
            return None;
        }
        let editable = target["runtimeRepairCanEdit"] == true;
        let available = target["runtimeRepairAvailable"] == true;
        let busy = self.cloud.repair.request.is_some();
        let state = operation["state"].as_str().unwrap_or_default();
        let active = !state.is_empty() && state != "ready";
        let mut panel = column().gap_1();
        if editable && available && self.cloud.repair.confirm {
            panel=panel.child(label("Restart the Zuse runtime? Active work is interrupted. Your repository and installed software stay on this computer.").text_size(px(11.)))
                .child(row().gap_2().child(control("runtime-repair-cancel","Keep running").on_click(cx.listener(|this,_,_,cx|{this.cloud.repair.confirm=false;cx.notify();})))
                .child(control("runtime-repair-confirm","Restart runtime").on_click(cx.listener(|this,_,_,cx|{this.request_cloud_runtime_repair("start",None);cx.notify();}))));
        } else if editable && available && !active && !busy {
            panel = panel.child(
                control("computer-restart-runtime", "Restart runtime…")
                    .w_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cloud.repair.confirm = true;
                        cx.notify();
                    })),
            );
        }
        if !state.is_empty() || busy {
            let description = if busy {
                "Checking runtime restart…"
            } else {
                match state {
                    "quiescing" => "Settling active commands…",
                    "stopping" => "Stopping the old runtime…",
                    "stopped" => "Old runtime stopped",
                    "starting" => "Starting the runtime…",
                    "waiting-for-runtime" => "Waiting for the runtime connection…",
                    "attention-required" => "Interrupted work needs your decision",
                    "ready" => "Runtime restarted",
                    "failed" => "Runtime restart failed",
                    _ => "Checking runtime restart…",
                }
            };
            panel = panel.child(label(description).text_size(px(11.)));
        }
        if state == "attention-required" {
            let count = operation["recovery"]["quarantinedCommandCount"]
                .as_u64()
                .unwrap_or(0);
            panel=panel.child(label(format!("{count} commands have uncertain outcomes. They will never be replayed automatically.")).text_size(px(11.)));
            if editable {
                panel=panel.child(label("Continue permits new commands. Discard also cancels messages that never started.").text_size(px(11.)))
                    .child(control("repair-continue","Continue with new commands").w_full().on_click(cx.listener(|this,_,_,cx|{this.request_cloud_runtime_repair("resolve",Some("continue"));cx.notify();})))
                    .child(control("repair-discard","Discard unstarted messages").w_full().on_click(cx.listener(|this,_,_,cx|{this.request_cloud_runtime_repair("resolve",Some("discard"));cx.notify();})));
            } else {
                panel = panel.child(
                    label("An editor must review interrupted work before new commands can run.")
                        .text_size(px(11.)),
                );
            }
            panel = panel.child(
                control(
                    "repair-receipts",
                    if self.cloud.repair.expanded {
                        "Hide command receipts"
                    } else {
                        "Show command receipts"
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.cloud.repair.expanded = !this.cloud.repair.expanded;
                    cx.notify();
                })),
            );
            if self.cloud.repair.expanded {
                for receipt in operation["recovery"]["receipts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(64)
                {
                    panel = panel.child(
                        label(format!(
                            "{} · {}",
                            receipt["commandId"].as_str().unwrap_or("Command"),
                            receipt["state"].as_str().unwrap_or("uncertain")
                        ))
                        .text_size(px(10.))
                        .truncate(),
                    );
                }
            }
        }
        if editable
            && !busy
            && operation["errorCode"].is_string()
            && !matches!(state, "ready" | "attention-required")
        {
            panel=panel.child(label("The restart stopped at this stage. Retry checks the same operation without repeating interrupted commands.").text_size(px(11.)))
                .child(control("repair-retry","Retry this restart").w_full().on_click(cx.listener(|this,_,_,cx|{this.request_cloud_runtime_repair("retry",None);cx.notify();})));
        }
        if state == "ready" {
            panel = panel.child(
                control("repair-reconnect", "Reconnect to computer")
                    .w_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.connect_cloud();
                        cx.notify();
                    })),
            );
        }
        if operation["operationId"].is_string() && !busy {
            panel = panel.child(
                control("repair-status", "Check restart status")
                    .w_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.refresh_cloud_runtime_repair();
                        cx.notify();
                    })),
            );
        }
        if let Some(error) = &self.cloud.repair.error {
            panel = panel.child(
                label(error.clone())
                    .text_size(px(11.))
                    .text_color(rgb(0xe58477)),
            );
        }
        Some(panel.into_any_element())
    }
}

fn accepts_operation(current: &Value, incoming: &Value, read: bool) -> bool {
    if current["operationId"].is_string() {
        if read && current["operationId"] != incoming["operationId"] {
            return false;
        }
        if current["operationId"] == incoming["operationId"]
            && incoming["updatedAt"].as_i64().unwrap_or(0)
                < current["updatedAt"].as_i64().unwrap_or(0)
        {
            return false;
        }
    }
    true
}
#[cfg(test)]
mod repair_order_tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn stale_status_cannot_replace_newer_metadata_or_another_operation() {
        let newer = json!({"operationId":"new","state":"ready","updatedAt":20});
        assert!(!accepts_operation(
            &newer,
            &json!({"operationId":"new","state":"starting","updatedAt":10}),
            true
        ));
        assert!(!accepts_operation(
            &newer,
            &json!({"operationId":"old","state":"ready","updatedAt":30}),
            true
        ));
        assert!(accepts_operation(
            &newer,
            &json!({"operationId":"next","state":"quiescing","updatedAt":30}),
            false
        ));
    }
}
