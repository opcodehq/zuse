//! Device-login intent stays bound to the owner, selected connection and request key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Binding {
    pub owner: (String, Option<String>),
    pub agent: String,
    pub connection: Option<String>,
    pub idempotency_key: String,
    pub operation: Option<String>,
}
impl Binding {
    pub fn can_submit(&self, supports_reconnect: bool) -> bool {
        self.connection.is_none() || supports_reconnect
    }

    pub fn matches(
        &self,
        owner: &(String, Option<String>),
        agent: &str,
        connection: Option<&str>,
    ) -> bool {
        &self.owner == owner && self.agent == agent && self.connection.as_deref() == connection
    }
    pub fn request_connection<'a>(
        &'a self,
        owner: &(String, Option<String>),
        agent: &str,
        connections: impl Iterator<Item = (&'a str, &'a str, &'a str)>,
    ) -> Result<Option<&'a str>, &'static str> {
        self.request_configuration(owner, agent, "device-code", connections)
    }
    pub fn request_configuration<'a>(
        &'a self,
        owner: &(String, Option<String>),
        agent: &str,
        expected_method: &str,
        mut connections: impl Iterator<Item = (&'a str, &'a str, &'a str)>,
    ) -> Result<Option<&'a str>, &'static str> {
        if &self.owner != owner || self.agent != agent {
            return Err("The account or workspace changed. Reopen coding agent setup.");
        }
        if let Some(id) = self.connection.as_deref() {
            if !connections.any(|(candidate, provider, method)| {
                candidate == id && provider == agent && method == expected_method
            }) {
                return Err(
                    "The selected connection is no longer available. Refresh its account before reconnecting.",
                );
            }
        }
        Ok(self.connection.as_deref())
    }
    pub fn accept_update(
        &mut self,
        owner: &(String, Option<String>),
        agent: &str,
        connection: &str,
        operation: &str,
    ) -> bool {
        self.operation.as_deref() == Some(operation)
            && self.accept(owner, agent, connection, operation)
    }
    pub fn accept(
        &mut self,
        owner: &(String, Option<String>),
        agent: &str,
        connection: &str,
        operation: &str,
    ) -> bool {
        if &self.owner != owner
            || self.agent != agent
            || connection.is_empty()
            || operation.is_empty()
            || self
                .connection
                .as_deref()
                .is_some_and(|id| id != connection)
            || self.operation.as_deref().is_some_and(|id| id != operation)
        {
            return false;
        }
        self.connection = Some(connection.into());
        self.operation = Some(operation.into());
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn selected() -> Binding {
        Binding {
            owner: ("actor".into(), Some("org".into())),
            agent: "codex".into(),
            connection: Some("task-pinned-connection".into()),
            idempotency_key: "stable-login-request".into(),
            operation: None,
        }
    }
    #[test]
    fn reconnect_preserves_immutable_task_connection_and_rejects_other_owner_agent_or_method() {
        let binding = selected();
        assert!(!binding.can_submit(false));
        assert!(binding.can_submit(true));
        let owner = binding.owner.clone();
        assert_eq!(
            binding
                .request_connection(
                    &owner,
                    "codex",
                    [("task-pinned-connection", "codex", "device-code")].into_iter()
                )
                .unwrap(),
            Some("task-pinned-connection")
        );
        assert!(
            binding
                .request_connection(
                    &("other".into(), Some("org".into())),
                    "codex",
                    std::iter::empty()
                )
                .is_err()
        );
        assert!(
            binding
                .request_connection(&owner, "grok", std::iter::empty())
                .is_err()
        );
        assert!(
            binding
                .request_connection(
                    &owner,
                    "codex",
                    [("task-pinned-connection", "grok", "device-code")].into_iter()
                )
                .is_err()
        );
        assert!(
            binding
                .request_connection(
                    &owner,
                    "codex",
                    [("task-pinned-connection", "codex", "api-key")].into_iter()
                )
                .is_err()
        );
        assert!(
            binding
                .request_connection(&("actor".into(), None), "codex", std::iter::empty())
                .is_err()
        );
    }
    #[test]
    fn replacement_keeps_exact_setup_token_and_api_key_binding() {
        let binding = selected();
        let owner = binding.owner.clone();
        for method in ["api-key", "setup-token"] {
            assert_eq!(
                binding
                    .request_configuration(
                        &owner,
                        "codex",
                        method,
                        [("task-pinned-connection", "codex", method)].into_iter()
                    )
                    .unwrap(),
                Some("task-pinned-connection")
            );
            assert!(
                binding
                    .request_configuration(
                        &owner,
                        "codex",
                        method,
                        [("other", "codex", method)].into_iter()
                    )
                    .is_err()
            );
            assert!(
                binding
                    .request_configuration(
                        &owner,
                        "codex",
                        method,
                        [("task-pinned-connection", "codex", "device-code")].into_iter()
                    )
                    .is_err()
            );
        }
    }
    #[test]
    fn pending_reopen_keeps_operation_request_key_and_connection_without_retargeting() {
        let mut binding = selected();
        let owner = binding.owner.clone();
        assert!(
            !binding.accept_update(
                &owner,
                "codex",
                "task-pinned-connection",
                "previous-operation"
            ),
            "late status cannot bind an unconfirmed new login"
        );
        assert!(binding.operation.is_none());
        assert!(binding.accept(
            &owner,
            "codex",
            "task-pinned-connection",
            "pending-operation"
        ));
        assert!(binding.accept_update(
            &owner,
            "codex",
            "task-pinned-connection",
            "pending-operation"
        ));
        let mut reopened = binding.clone();
        assert!(reopened.accept(
            &owner,
            "codex",
            "task-pinned-connection",
            "pending-operation"
        ));
        assert!(!reopened.accept(&owner, "codex", "another-account", "pending-operation"));
        assert!(!reopened.accept(&owner, "codex", "task-pinned-connection", "stale-operation"));
        assert_eq!(reopened.idempotency_key, "stable-login-request");
        let mut add = selected();
        add.connection = None;
        assert!(add.can_submit(false));
        assert_eq!(
            add.request_connection(&owner, "codex", std::iter::empty())
                .unwrap(),
            None
        );
        assert!(add.accept(
            &owner,
            "codex",
            "explicitly-new-connection",
            "new-operation"
        ));
        assert!(!add.matches(&owner, "codex", Some("task-pinned-connection")));
    }
}
