//! Existing-task recovery never falls back to another account of the same agent.
use zuse_protocol::cloud_connections::{AgentConnection, ConnectionState};
pub(super) fn selected<'a>(
    current: Option<&(String, Option<String>)>,
    expected: &(String, Option<String>),
    agent: &str,
    id: &str,
    connections: &'a [AgentConnection],
) -> Result<&'a AgentConnection, &'static str> {
    if current != Some(expected) {
        return Err(
            "The account changed. Reopen this computer before reconnecting its coding agent.",
        );
    }
    let connection = connections.iter().find(|row| row.connection_id == id && row.agent.as_str() == agent)
        .ok_or("This task's selected connection is unavailable in this account. Ask its owner to review access; no other account was selected.")?;
    if connection.state == ConnectionState::Revoked {
        return Err(
            "This task's selected connection is disconnected. Review that original connection with its owner; existing tasks were not switched to another account.",
        );
    }
    Ok(connection)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn rows() -> Vec<AgentConnection> {
        ["pinned", "last-other"].iter().map(|id|serde_json::from_value(json!({"connectionId":id,"agent":"codex","method":"device-code","state":"connected","version":1})).unwrap()).collect()
    }
    #[test]
    fn exact_pinned_connection_wins_over_last_account() {
        let owner = ("actor".into(), None);
        let rows = rows();
        assert_eq!(
            selected(Some(&owner), &owner, "codex", "pinned", &rows)
                .unwrap()
                .connection_id,
            "pinned"
        );
    }
    #[test]
    fn exact_non_device_connection_can_be_reviewed_without_retargeting() {
        let owner = ("actor".into(), None);
        let mut rows = rows();
        for method in [
            zuse_protocol::cloud_connections::ConnectionMethod::ApiKey,
            zuse_protocol::cloud_connections::ConnectionMethod::SetupToken,
        ] {
            rows[0].method = method;
            assert_eq!(
                selected(Some(&owner), &owner, "codex", "pinned", &rows)
                    .unwrap()
                    .method,
                method
            );
        }
    }
    #[test]
    fn missing_revoked_wrong_agent_and_changed_scope_never_retarget() {
        let owner = ("actor".into(), None);
        let mut rows = rows();
        assert!(selected(Some(&owner), &owner, "codex", "missing", &rows).is_err());
        assert!(selected(Some(&owner), &owner, "grok", "pinned", &rows).is_err());
        assert!(
            selected(
                Some(&("other".into(), None)),
                &owner,
                "codex",
                "pinned",
                &rows
            )
            .is_err()
        );
        rows[0].state = ConnectionState::Revoked;
        assert!(selected(Some(&owner), &owner, "codex", "pinned", &rows).is_err());
    }
}
