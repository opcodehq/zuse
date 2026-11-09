//! Scoped, remembered agent selection for new cloud drafts.
use serde_json::Value;
use std::collections::HashMap;
use zuse_protocol::HarnessId;

/// Preferences for a new draft only. Existing tasks keep server-owned bindings.
#[derive(Default)]
pub(super) struct ConnectionChoices {
    scope: Option<(String, Option<String>)>,
    agents: HashMap<HarnessId, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use zuse_protocol::{
        cloud_computers::AgentId,
        cloud_connections::{AgentConnection, ConnectionMethod, ConnectionState},
    };

    fn row(id: &str, agent: AgentId, scope: &str) -> Value {
        // Use the actual public authority contract, then add the route's scope tag.
        let mut row = serde_json::to_value(AgentConnection {
            connection_id: id.into(),
            agent,
            method: ConnectionMethod::ApiKey,
            state: ConnectionState::Connected,
            account_label: None,
            expires_at: None,
            version: 1,
            error_code: None,
            credential_update: None,
            credential_update_key: None,
        })
        .unwrap();
        row["connectionScope"] = json!(scope);
        row
    }
    fn context(rows: Vec<Value>, organization: Option<&str>) -> Value {
        json!({"accountId":"actor","organizationId":organization,
            "ownerId":organization.map(|id|format!("organization:{id}")).unwrap_or_else(||"actor".into()),
            "agentConnections":rows})
    }
    #[test]
    fn multiple_connected_accounts_are_stable_across_response_order_and_refresh() {
        let mut choices = ConnectionChoices::default();
        let mut ctx = context(
            vec![
                row("z", AgentId::Codex, "personal"),
                row("b", AgentId::Codex, "personal"),
                row("claude", AgentId::Claude, "personal"),
            ],
            None,
        );
        assert_eq!(
            choices
                .resolve(&ctx, "actor", None, HarnessId::Codex)
                .as_deref(),
            Some("b")
        );
        ctx["agentConnections"].as_array_mut().unwrap().reverse();
        ctx["agentConnections"]
            .as_array_mut()
            .unwrap()
            .push(row("a", AgentId::Codex, "personal"));
        assert_eq!(
            choices
                .resolve(&ctx, "actor", None, HarnessId::ClaudeCode)
                .as_deref(),
            Some("claude")
        );
        assert_eq!(
            choices
                .resolve(&ctx, "actor", None, HarnessId::Codex)
                .as_deref(),
            Some("b")
        );
    }
    #[test]
    fn organization_prefers_its_own_connection_and_personal_context_excludes_it() {
        let ctx = context(
            vec![
                row("a-personal", AgentId::Codex, "personal"),
                row("z-team", AgentId::Codex, "organization"),
            ],
            Some("team"),
        );
        let mut choices = ConnectionChoices::default();
        assert_eq!(
            choices
                .resolve(&ctx, "actor", Some("team"), HarnessId::Codex)
                .as_deref(),
            Some("z-team")
        );
        let personal = context(ctx["agentConnections"].as_array().unwrap().clone(), None);
        assert_eq!(
            choices
                .resolve(&personal, "actor", None, HarnessId::Codex)
                .as_deref(),
            Some("a-personal")
        );
    }
    #[test]
    fn revoked_or_missing_remembered_identity_never_switches_to_another_account() {
        let mut choices = ConnectionChoices::default();
        let mut ctx = context(
            vec![
                row("a", AgentId::Codex, "personal"),
                row("b", AgentId::Codex, "personal"),
            ],
            None,
        );
        assert_eq!(
            choices
                .resolve(&ctx, "actor", None, HarnessId::Codex)
                .as_deref(),
            Some("a")
        );
        ctx["agentConnections"][0]["state"] = json!("revoked");
        assert!(
            choices
                .resolve(&ctx, "actor", None, HarnessId::Codex)
                .is_none()
        );
        ctx["agentConnections"].as_array_mut().unwrap().remove(0);
        assert!(
            choices
                .resolve(&ctx, "actor", None, HarnessId::Codex)
                .is_none()
        );
    }
    #[test]
    fn stale_context_and_explicit_foreign_owner_cannot_supply_credentials() {
        let mut choices = ConnectionChoices::default();
        let mut ctx = context(vec![row("a", AgentId::Grok, "personal")], None);
        assert!(
            choices
                .resolve(&ctx, "other", None, HarnessId::Grok)
                .is_none()
        );
        assert!(
            choices
                .resolve(&ctx, "actor", Some("other"), HarnessId::Grok)
                .is_none()
        );
        ctx["agentConnections"][0]["ownerId"] = json!("other");
        assert!(
            choices
                .resolve(&ctx, "actor", None, HarnessId::Grok)
                .is_none()
        );
        ctx["agentConnections"][0]["ownerId"] = json!("actor");
        ctx["agentConnections"][0]["organizationId"] = json!("foreign");
        assert!(
            choices
                .resolve(&ctx, "actor", None, HarnessId::Grok)
                .is_none()
        );
    }
    #[test]
    fn sole_connection_and_legacy_personal_scope_remain_usable() {
        let mut choices = ConnectionChoices::default();
        let mut ctx = context(vec![row("sole", AgentId::Claude, "personal")], None);
        ctx["agentConnections"][0]
            .as_object_mut()
            .unwrap()
            .remove("connectionScope");
        assert_eq!(
            choices
                .resolve(&ctx, "actor", None, HarnessId::ClaudeCode)
                .as_deref(),
            Some("sole")
        );
        assert!(
            choices
                .resolve(&ctx, "actor", None, HarnessId::Codex)
                .is_none()
        );
    }
}
impl ConnectionChoices {
    pub(super) fn resolve(
        &mut self,
        context: &Value,
        account: &str,
        organization: Option<&str>,
        harness: HarnessId,
    ) -> Option<String> {
        let owner = organization
            .map(|id| format!("organization:{id}"))
            .unwrap_or_else(|| account.to_owned());
        if context["accountId"].as_str() != Some(account)
            || context["organizationId"].as_str() != organization
            || context["ownerId"].as_str() != Some(owner.as_str())
        {
            return None;
        }
        let scope = (account.to_owned(), organization.map(str::to_owned));
        if self.scope.as_ref() != Some(&scope) {
            self.scope = Some(scope);
            self.agents.clear();
        }
        let agent: String = if harness == HarnessId::ClaudeCode {
            "claude".into()
        } else {
            harness.into()
        };
        let rows = context["agentConnections"].as_array()?;
        let mut eligible = rows.iter().filter_map(|row| {
            if row["agent"] != agent || row["state"] != "connected" {
                return None;
            }
            let (rank, connection_owner, connection_organization) = match row["connectionScope"]
                .as_str()
            {
                Some("organization") if organization.is_some() => (0, owner.as_str(), organization),
                Some("personal") => (u8::from(organization.is_some()), account, None),
                // Older personal contexts did not label their already-scoped rows.
                None if organization.is_none() => (0, account, None),
                _ => return None,
            };
            if row
                .get("ownerId")
                .is_some_and(|value| !value.is_null() && value.as_str() != Some(connection_owner))
                || row.get("organizationId").is_some_and(|value| {
                    !value.is_null() && value.as_str() != connection_organization
                })
            {
                return None;
            }
            let id = row["connectionId"]
                .as_str()
                .filter(|id| !id.is_empty() && id.trim() == *id)?;
            Some((rank, id))
        });
        if let Some(remembered) = self.agents.get(&harness) {
            // Revocation/removal must not silently select another identity.
            return eligible
                .find(|(_, id)| *id == remembered.as_str())
                .map(|(_, id)| id.to_owned());
        }
        // The wire format has no configured-default field. Use scoped, stable
        // ordering rather than API response order, and retain it across refreshes.
        let (_, selected) = eligible.min()?;
        self.agents.insert(harness, selected.to_owned());
        Some(selected.to_owned())
    }
}
