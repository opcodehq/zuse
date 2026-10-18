//! Shared plugin management state. Writes are reconciled by reads, never replayed.
use crate::plugins::PluginResponse;
use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    Connection,
    Server,
}

pub struct Confirmation {
    pub kind: Removal,
    pub label: String,
    tenant: String,
    id: String,
}
impl Confirmation {
    pub fn title(&self) -> &'static str {
        match self.kind {
            Removal::Connection => "Disconnect plugin?",
            Removal::Server => "Remove MCP server?",
        }
    }
    pub fn button(&self) -> &'static str {
        match self.kind {
            Removal::Connection => "Disconnect",
            Removal::Server => "Remove server",
        }
    }
}
struct Pending {
    action: String,
    tenant: Option<String>,
}
#[derive(Default)]
pub struct PluginState {
    pub snapshot: Value,
    pub attempt: Value,
    pub confirmation: Option<Confirmation>,
    pub notice: Option<String>,
    pending: Option<Pending>,
    attempt_tenant: Option<String>,
    selection_uncertain: bool,
}
impl PluginState {
    pub fn tenant(&self) -> Option<&str> {
        self.snapshot["tenantId"]
            .as_str()
            .filter(|id| !id.is_empty())
    }
    pub fn active_attempt(&self) -> bool {
        self.attempt["state"] == "pending"
            && self.attempt["id"].is_string()
            && self.attempt_tenant.as_deref() == self.tenant()
    }
    pub fn refresh(&self) -> Value {
        Self::list(if self.selection_uncertain {
            None
        } else {
            self.tenant()
        })
    }
    fn list(tenant: Option<&str>) -> Value {
        match tenant {
            Some(tenant) => json!({"action":"list","tenantId":tenant}),
            None => json!({"action":"list"}),
        }
    }
    pub fn begin_workspace_selection(&mut self, request: Value) -> Result<Value, &'static str> {
        if self.active_attempt() {
            return Err("Finish or cancel the current plugin sign-in first.");
        }
        if request["action"] != "list"
            || request["tenantId"].as_str().is_none_or(|id| id.is_empty())
        {
            return Err("Choose a plugin workspace.");
        }
        let request = self.begin(request)?;
        self.pending.as_mut().unwrap().action = "selectWorkspace".into();
        Ok(request)
    }
    pub fn begin(&mut self, mut request: Value) -> Result<Value, &'static str> {
        if self.pending.is_some() {
            return Err("Wait for the current plugin action to finish.");
        }
        let action = request["action"]
            .as_str()
            .ok_or("Choose a plugin action.")?
            .to_owned();
        if !matches!(
            action.as_str(),
            "list"
                | "connect"
                | "addServer"
                | "removeServer"
                | "poll"
                | "cancel"
                | "complete"
                | "disconnect"
                | "setEnabled"
        ) {
            return Err("Choose a supported plugin action.");
        }
        if action == "list" && request["tenantId"].is_null() && !self.selection_uncertain {
            if let Some(tenant) = self.tenant() {
                request["tenantId"] = json!(tenant);
            }
        }
        let tenant = request["tenantId"].as_str().map(str::to_owned);
        if action != "list" {
            if self.snapshot["canManage"] == false {
                return Err("Only workspace administrators can manage these connections.");
            }
            if tenant.as_deref().is_none() || tenant.as_deref() != self.tenant() {
                return Err("Refresh the selected workspace before changing plugins.");
            }
        }
        if self.active_attempt()
            && ((action == "list" && tenant.as_deref() != self.tenant())
                || !matches!(action.as_str(), "list" | "poll" | "cancel" | "complete"))
        {
            return Err("Finish or cancel the current plugin sign-in first.");
        }
        if matches!(action.as_str(), "poll" | "cancel")
            && (!self.active_attempt() || request["attemptId"] != self.attempt["id"])
        {
            return Err("This plugin sign-in is no longer active. Refresh connections.");
        }
        self.confirmation = None;
        if action != "list" && action != "poll" {
            self.notice = None;
        }
        self.pending = Some(Pending { action, tenant });
        Ok(request)
    }
    pub fn apply(&mut self, response: &Value) -> Result<Option<Value>, &'static str> {
        let pending = self
            .pending
            .as_ref()
            .ok_or("No plugin request is awaiting a response.")?;
        let decoded = serde_json::from_value::<PluginResponse>(response.clone())
            .map_err(|_| "The plugin service returned an invalid response. Refresh connections.")?;
        match decoded {
            PluginResponse::Snapshot { ref tenant_id, .. } => {
                if !matches!(pending.action.as_str(), "list" | "selectWorkspace")
                    || pending
                        .tenant
                        .as_deref()
                        .is_some_and(|tenant| tenant != tenant_id)
                {
                    return Err(
                        "The plugin service returned a different workspace. Refresh connections.",
                    );
                }
                let changed = self.tenant() != Some(tenant_id.as_str());
                if changed || self.attempt["state"] != "pending" {
                    self.attempt = Value::Null;
                    self.attempt_tenant = None;
                }
                if changed {
                    self.notice = None;
                }
                self.selection_uncertain = false;
                self.snapshot = response.clone();
                self.pending = None;
                Ok(None)
            }
            PluginResponse::Attempt { ref state, .. } => {
                if !matches!(
                    pending.action.as_str(),
                    "connect" | "addServer" | "poll" | "complete"
                ) || !matches!(
                    state.as_str(),
                    "pending" | "connected" | "failed" | "cancelled"
                ) {
                    return Err(
                        "The plugin service returned an invalid connection state. Refresh connections.",
                    );
                }
                let tenant = pending.tenant.clone();
                if pending.action == "poll" && response["id"] != self.attempt["id"] {
                    return Err(
                        "The plugin service returned a different sign-in attempt. Refresh connections.",
                    );
                }
                self.attempt = response.clone();
                self.attempt_tenant = tenant.clone();
                self.pending = None;
                self.notice = match state.as_str() {
                    "failed" => Some("Plugin connection failed. Try connecting again.".into()),
                    "cancelled" => Some("Plugin sign-in canceled.".into()),
                    _ => None,
                };
                Ok((state != "pending").then(|| Self::list(tenant.as_deref())))
            }
            PluginResponse::Ok => {
                if !matches!(
                    pending.action.as_str(),
                    "removeServer" | "disconnect" | "setEnabled" | "cancel" | "complete"
                ) {
                    return Err(
                        "The plugin service did not confirm the requested action. Refresh connections.",
                    );
                }
                let next = Self::list(pending.tenant.as_deref());
                if matches!(pending.action.as_str(), "cancel" | "complete") {
                    self.attempt = Value::Null;
                    self.attempt_tenant = None;
                    self.notice =
                        (pending.action == "cancel").then(|| "Plugin sign-in canceled.".into());
                }
                self.pending = None;
                Ok(Some(next))
            }
        }
    }
    pub fn failed(&mut self) -> Option<Value> {
        let pending = self.pending.take()?;
        if pending.action == "selectWorkspace" {
            self.selection_uncertain = true;
            self.notice = Some(
                "Workspace selection could not be confirmed. Refreshing the saved selection."
                    .into(),
            );
            Some(self.refresh())
        } else if pending.action == "list" {
            None
        } else {
            self.notice = Some(
                "The plugin action could not be confirmed. Check connections before trying again."
                    .into(),
            );
            Some(Self::list(pending.tenant.as_deref()))
        }
    }
    pub fn interrupted(&mut self) {
        if let Some(pending) = self.pending.take() {
            self.selection_uncertain |= pending.action == "selectWorkspace";
            self.notice = Some(
                "Connection interrupted. Check refreshed connections before trying an action again.".into(),
            );
        }
        self.confirmation = None;
    }
    fn removable(&self, kind: Removal, id: &str) -> Option<&Value> {
        let (key, id_field) = match kind {
            Removal::Connection => ("connections", "id"),
            Removal::Server => ("catalog", "id"),
        };
        self.snapshot[key]
            .as_array()?
            .iter()
            .find(|row| row[id_field] == id && (kind != Removal::Server || row["custom"] == true))
    }
    pub fn ask_removal(&mut self, kind: Removal, id: &str) -> Result<(), &'static str> {
        if self.pending.is_some() || self.active_attempt() {
            return Err("Finish the current plugin action first.");
        }
        if self.snapshot["canManage"] == false {
            return Err("Only workspace administrators can manage these connections.");
        }
        let tenant = self.tenant().ok_or("Choose a workspace first.")?.to_owned();
        let row = self
            .removable(kind, id)
            .ok_or("This connection or custom server is no longer available.")?;
        let label = row[if kind == Removal::Connection {
            "label"
        } else {
            "name"
        }]
        .as_str()
        .unwrap_or("Plugin")
        .to_owned();
        self.confirmation = Some(Confirmation {
            kind,
            label,
            tenant,
            id: id.into(),
        });
        Ok(())
    }
    pub fn confirm_removal(&mut self) -> Result<Value, &'static str> {
        if self.pending.is_some() {
            return Err("Wait for the current plugin action to finish.");
        }
        let confirmed = self
            .confirmation
            .as_ref()
            .ok_or("Choose a connection or custom server to remove.")?;
        if self.snapshot["canManage"] == false
            || self.tenant() != Some(confirmed.tenant.as_str())
            || self.removable(confirmed.kind, &confirmed.id).is_none()
        {
            self.confirmation = None;
            return Err("Connections changed. Review them before removing anything.");
        }
        let request = match confirmed.kind {
            Removal::Connection => {
                json!({"action":"disconnect","tenantId":confirmed.tenant,"connectionId":confirmed.id})
            }
            Removal::Server => {
                json!({"action":"removeServer","tenantId":confirmed.tenant,"pluginId":confirmed.id})
            }
        };
        self.confirmation = None;
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(tenant: &str) -> Value {
        json!({"kind":"snapshot","canManage":true,"tenants":[{"id":tenant,"kind":"organization","name":"Team"}],"tenantId":tenant,"endpoint":"https://fixture.invalid","catalog":[{"id":"custom","name":"Custom tools","description":"Fixture","custom":true},{"id":"catalog","name":"Catalog tools","description":"Fixture"}],"connections":[{"id":"connection","pluginId":"custom","label":"Shared tools","owner":"owner","state":"connected","createdAt":0,"enabled":true}]})
    }
    fn loaded() -> PluginState {
        let mut state = PluginState::default();
        state
            .begin(json!({"action":"list","tenantId":"organization:team"}))
            .unwrap();
        state.apply(&snapshot("organization:team")).unwrap();
        state
    }
    fn attempt(id: &str, state: &str) -> Value {
        json!({"kind":"attempt","id":id,"connectionId":"connection","state":state,"authorizationUrl":"https://auth.example/authorize","expiresAt":100,"pluginId":"custom"})
    }
    #[test]
    fn plugin_mutations_refresh_their_workspace_without_optimistic_removal() {
        for (kind, id) in [
            (Removal::Connection, "connection"),
            (Removal::Server, "custom"),
        ] {
            let mut state = loaded();
            let before = state.snapshot.clone();
            state.ask_removal(kind, id).unwrap();
            assert_eq!(state.snapshot, before);
            let request = state.confirm_removal().unwrap();
            assert_eq!(request["tenantId"], "organization:team");
            state.begin(request).unwrap();
            let next = state.apply(&json!({"kind":"ok"})).unwrap().unwrap();
            assert_eq!(
                next,
                json!({"action":"list","tenantId":"organization:team"})
            );
            assert_eq!(state.snapshot, before);
            state.begin(next).unwrap();
            let mut canonical = before;
            canonical["connections"] = json!([]);
            state.apply(&canonical).unwrap();
            assert!(state.snapshot["connections"].as_array().unwrap().is_empty());
        }
    }
    #[test]
    fn plugin_signin_stays_bound_until_it_finishes_or_is_canceled() {
        let mut state = loaded();
        state
            .begin(json!({"action":"connect","tenantId":"organization:team"}))
            .unwrap();
        assert!(
            state
                .apply(&attempt("attempt", "pending"))
                .unwrap()
                .is_none()
        );
        assert!(state.active_attempt());
        assert!(
            state
                .begin(json!({"action":"list","tenantId":"personal:owner"}))
                .is_err()
        );
        assert!(
            state
                .begin(json!({"action":"connect","tenantId":"organization:team"}))
                .is_err()
        );
        assert!(
            state
                .begin(json!({"action":"poll","tenantId":"organization:team","attemptId":"wrong"}))
                .is_err()
        );
        state
            .begin(json!({"action":"cancel","tenantId":"organization:team","attemptId":"attempt"}))
            .unwrap();
        let next = state.apply(&json!({"kind":"ok"})).unwrap().unwrap();
        assert!(!state.active_attempt());
        assert_eq!(next["tenantId"], "organization:team");
        state.begin(next).unwrap();
        state.apply(&snapshot("organization:team")).unwrap();
        state
            .begin(json!({"action":"list","tenantId":"personal:owner"}))
            .unwrap();
        state.apply(&snapshot("personal:owner")).unwrap();
        assert_eq!(state.tenant(), Some("personal:owner"));
    }
    #[test]
    fn plugin_completion_receipt_releases_the_pending_signin() {
        let mut state = loaded();
        state
            .begin(json!({"action":"connect","tenantId":"organization:team"}))
            .unwrap();
        state.apply(&attempt("attempt", "pending")).unwrap();
        state.begin(json!({"action":"complete","tenantId":"organization:team","ticket":"fixture-ticket"})).unwrap();
        let next = state.apply(&json!({"kind":"ok"})).unwrap().unwrap();
        assert!(!state.active_attempt());
        assert_eq!(next["tenantId"], "organization:team");
        assert!(state.notice.is_none());
    }
    #[test]
    fn plugin_failed_or_malformed_writes_only_schedule_one_scoped_read() {
        let mut state = loaded();
        let before = state.snapshot.clone();
        state.begin(json!({"action":"disconnect","tenantId":"organization:team","connectionId":"connection"})).unwrap();
        assert!(state.apply(&json!({"ok":true})).is_err());
        let next = state.failed().unwrap();
        assert_eq!(next["action"], "list");
        assert_eq!(next["tenantId"], "organization:team");
        assert_eq!(state.snapshot, before);
        assert!(state.failed().is_none());
        state.begin(next).unwrap();
        assert!(state.failed().is_none());
        assert_eq!(state.snapshot, before);
    }
    #[test]
    fn plugin_removal_rechecks_tenant_permissions_and_record_identity() {
        let mut state = loaded();
        assert!(state.ask_removal(Removal::Server, "catalog").is_err());
        for change in 0..3 {
            let mut state = loaded();
            state
                .ask_removal(Removal::Connection, "connection")
                .unwrap();
            match change {
                0 => state.snapshot["tenantId"] = json!("another"),
                1 => state.snapshot["canManage"] = json!(false),
                _ => state.snapshot["connections"] = json!([]),
            }
            assert!(state.confirm_removal().is_err());
            assert!(state.confirmation.is_none());
        }
    }
    #[test]
    fn plugin_responses_cannot_cross_workspaces_or_signin_attempts() {
        let mut state = loaded();
        let before = state.snapshot.clone();
        state.begin(json!({"action":"list"})).unwrap();
        assert!(state.apply(&snapshot("personal:other")).is_err());
        assert_eq!(state.snapshot, before);
        state.failed();
        state
            .begin(json!({"action":"connect","tenantId":"organization:team"}))
            .unwrap();
        state.apply(&attempt("first", "pending")).unwrap();
        state
            .begin(json!({"action":"poll","tenantId":"organization:team","attemptId":"first"}))
            .unwrap();
        assert!(state.apply(&attempt("second", "connected")).is_err());
        assert_eq!(state.attempt["id"], "first");
        state.failed();
    }
    #[test]
    fn uncertain_workspace_selection_reads_saved_default_without_replaying_selection() {
        let mut state = loaded();
        state
            .begin_workspace_selection(json!({"action":"list","tenantId":"personal:owner"}))
            .unwrap();
        let read = state.failed().unwrap();
        assert_eq!(read, json!({"action":"list"}));
        assert_eq!(state.tenant(), Some("organization:team"));
        assert_eq!(state.begin(read.clone()).unwrap(), read);
        state.apply(&snapshot("personal:owner")).unwrap();
        assert_eq!(state.tenant(), Some("personal:owner"));
        state
            .begin_workspace_selection(json!({"action":"list","tenantId":"organization:team"}))
            .unwrap();
        state.interrupted();
        assert_eq!(state.refresh(), json!({"action":"list"}));
        assert_eq!(
            state.begin(state.refresh()).unwrap(),
            json!({"action":"list"})
        );
        state.apply(&snapshot("organization:team")).unwrap();
        assert_eq!(
            state.refresh(),
            json!({"action":"list","tenantId":"organization:team"})
        );
    }
}
