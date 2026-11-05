//! Recreation controls are explicit and retain the original operation on retry.
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
    error: Option<String>,
}
impl WorkspaceView {
    pub(super) fn close_cloud_recreation(&mut self) {
        if let Some(id) = self.cloud.recreation.request.take() {
            self.cancel(id);
            self.pending.remove(&id);
        }
        self.cloud.recreation = State::default();
    }
    fn cloud_recreation_operation(&self) -> Value {
        if self
            .cloud
            .target
            .as_ref()
            .and_then(|target| target["workspaceId"].as_str())
            == self.cloud.recreation.workspace.as_deref()
            && self.cloud.recreation.operation.is_object()
        {
            self.cloud.recreation.operation.clone()
        } else {
            self.cloud
                .target
                .as_ref()
                .map(|target| target["computerRecreation"].clone())
                .unwrap_or(Value::Null)
        }
    }
    pub(super) fn sync_cloud_recreation_metadata(&mut self) {
        let Some(target) = &self.cloud.target else {
            return;
        };
        let workspace = target["workspaceId"].as_str().map(str::to_owned);
        let summary = target["computerRecreation"].clone();
        if summary["operationId"].is_string() {
            let changed = self.cloud.recreation.workspace != workspace
                || self.cloud.recreation.operation["operationId"] != summary["operationId"];
            let newer = summary["updatedAt"].as_i64().unwrap_or(0)
                > self.cloud.recreation.operation["updatedAt"]
                    .as_i64()
                    .unwrap_or(0);
            if changed || newer {
                self.cloud.recreation.workspace = workspace;
                self.cloud.recreation.operation = summary;
            }
            if changed || newer {
                if self.cloud.recreation.request.is_some() {
                    self.cloud.recreation.dirty = true;
                } else {
                    self.refresh_cloud_recreation();
                }
            }
        }
    }
    pub(super) fn refresh_cloud_recreation(&mut self) {
        if self.cloud.recreation.request.is_none()
            && self.cloud_recreation_operation()["operationId"].is_string()
        {
            self.request_cloud_recreation("status", None);
        }
    }
    fn request_cloud_recreation(&mut self, action: &str, decision: Option<&str>) {
        if self.cloud.recreation.request.is_some() {
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
            if target["computerRecreationCanEdit"] != true
                || (action == "start" && target["computerRecreationAvailable"] != true)
            {
                self.cloud.recreation.error=Some("Computer recreation is not available with this computer's current permissions and managed base.".into());
                return;
            }
        }
        let prior = self.cloud_recreation_operation();
        if self.cloud.recreation.workspace.as_deref() != Some(workspace.as_str()) {
            self.close_cloud_recreation();
            self.cloud.recreation.workspace = Some(workspace.clone());
            self.cloud.recreation.operation = prior.clone();
        }
        let mut body = json!({"workspaceId":workspace,"expectedAccountId":self.cloud.account,"expectedOrganizationId":self.cloud.organization,"action":action});
        if action == "start" {
            if matches!(prior["state"].as_str(), Some("ready" | "cancelled")) {
                self.cloud.recreation.key = None;
            }
            let key = self
                .cloud
                .recreation
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
        self.cloud.recreation.error = None;
        self.cloud.recreation.action = Some(action.to_owned());
        self.cloud.recreation.confirm = false;
        self.cloud.recreation.request = Some(self.call(methods::CLOUD_COMPUTER_RECREATION, body));
    }
    pub(super) fn receive_cloud_recreation(
        &mut self,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(id) = self.cloud.recreation.request else {
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
                let operation = value["computerRecreation"].clone();
                if operation["operationId"].is_string() {
                    let current = self.cloud_recreation_operation();
                    if accepts_operation(
                        &current,
                        &operation,
                        self.cloud.recreation.action.as_deref() != Some("start"),
                    ) {
                        self.cloud.recreation.operation = operation;
                    }
                    if self.cloud.recreation.operation["state"] == "ready" {
                        self.cloud.notice = Some(
                            "Computer recreated. Reconnect to continue your conversation.".into(),
                        );
                    }
                } else {
                    self.cloud.recreation.error = Some(
                        "Recreation operation was not returned. Check this computer's status."
                            .into(),
                    );
                }
            } else {
                self.cloud.recreation.error =
                    Some("Recreation response did not match this account and computer.".into());
            }
        }
        if let Some(error) = frame.get("err") {
            self.cloud.recreation.error=Some(error.as_str().or_else(||error["message"].as_str()).unwrap_or("Recreation was not confirmed. Retry the same operation to reconcile its state.").into());
        }
        if frame.get("ok").is_some() || frame.get("err").is_some() || frame["done"] == true {
            self.pending.remove(&id);
            self.cloud.recreation.request = None;
            self.cloud.recreation.action = None;
            if std::mem::take(&mut self.cloud.recreation.dirty) {
                self.refresh_cloud_recreation();
            }
        }
        cx.notify();
        true
    }
    pub(super) fn cloud_recreation_controls(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = self.cloud.target.as_ref()?;
        let operation = self.cloud_recreation_operation();
        if target["computerRecreationAvailable"] != true && !operation["operationId"].is_string() {
            return None;
        }
        let editable = target["computerRecreationCanEdit"] == true;
        let available = target["computerRecreationAvailable"] == true;
        let busy = self.cloud.recreation.request.is_some();
        let state = operation["state"].as_str().unwrap_or_default();
        let active = !state.is_empty() && state != "ready";
        let mut panel = column().gap_1();
        if editable && available && self.cloud.recreation.confirm {
            panel=panel.child(label("Recreate this computer? Zuse verifies a backup before retiring the original, then restores exported repositories, Zuse data and supported agent-session data. Declared environment setup runs again. Other home files and unrecorded system software are not restored. Sleep/Resume retains the current disk and its installed software.").text_size(px(11.)))
                .child(row().gap_2().child(control("runtime-recreation-cancel","Keep this computer").on_click(cx.listener(|this,_,_,cx|{this.cloud.recreation.confirm=false;cx.notify();})))
                .child(control("runtime-recreation-confirm","Recreate computer").on_click(cx.listener(|this,_,_,cx|{this.request_cloud_recreation("start",None);cx.notify();}))));
        } else if editable && available && !active && !busy {
            panel = panel.child(
                control("computer-recreate", "Recreate computer…")
                    .w_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cloud.recreation.confirm = true;
                        cx.notify();
                    })),
            );
        }
        if !state.is_empty() || busy {
            let description = if busy {
                "Checking computer recreation…"
            } else {
                match state {
                    "waking" => "Resuming the original computer for backup…",
                    "quiescing" => "Settling active commands…",
                    "capturing" => "Encrypting and verifying the recovery backup…",
                    "retiring" => "Retiring the backed-up computer…",
                    "restoring" => "Restoring your files and environment…",
                    "stopping" => "Stopping the old runtime…",
                    "stopped" => "Old runtime stopped",
                    "starting" => "Starting the runtime…",
                    "waiting-for-runtime" => "Waiting for the runtime connection…",
                    "attention-required" => "Interrupted work needs your decision",
                    "ready" => "Computer recreated",
                    "failed" => "Computer recreation failed",
                    _ => "Checking computer recreation…",
                }
            };
            panel = panel.child(label(description).text_size(px(11.)));
        }
        if state == "attention-required" {
            panel=panel.child(label("Recreation stopped safely. Interrupted commands are never replayed automatically.").text_size(px(11.)));
            let stage = match operation["failedStage"].as_str() {
                Some("waking") => "Resuming the original computer",
                Some("quiescing") => "Settling active commands",
                Some("capturing") => "Verifying the recovery backup",
                Some("retiring") => "Retiring the backed-up computer",
                Some("restoring") => "Restoring files and environment",
                Some("waiting-for-runtime") => "Connecting the restored runtime",
                _ => "Checking recreation",
            };
            panel = panel.child(label(stage).text_size(px(11.)));
        }
        if let Some(code) = operation["errorCode"].as_str().filter(|code| {
            code.len() <= 128 && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        }) {
            panel = panel.child(
                label(code.replace('_', " "))
                    .text_size(px(11.))
                    .text_color(rgb(0xe58477)),
            );
        }
        if editable && !busy && operation["errorCode"].is_string() && state != "ready" {
            panel=panel.child(label("Recreation stopped at this stage. Retry checks the same operation without repeating interrupted commands.").text_size(px(11.)))
                .child(control("recreation-retry","Retry this recreation").w_full().on_click(cx.listener(|this,_,_,cx|{this.request_cloud_recreation("retry",None);cx.notify();})));
        }
        if state == "ready" {
            panel = panel.child(
                control("recreation-reconnect", "Reconnect to computer")
                    .w_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.connect_cloud();
                        cx.notify();
                    })),
            );
        }
        if operation["operationId"].is_string() && !busy {
            panel = panel.child(
                control("recreation-status", "Check recreation status")
                    .w_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.refresh_cloud_recreation();
                        cx.notify();
                    })),
            );
        }
        if let Some(error) = &self.cloud.recreation.error {
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
