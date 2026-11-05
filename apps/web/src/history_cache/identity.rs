//! Browser history request keys; parameter order must not change cache identity.
use serde_json::Value;

pub(crate) fn request_identity(params: &Value) -> String {
    let mut canonical = params.clone();
    canonical.sort_all_objects();
    canonical.to_string()
}

pub(crate) fn opening_identity(params: &Value) -> String {
    let mut lookup = params.clone();
    if let Some(lookup) = lookup.as_object_mut() {
        lookup.remove("cacheOnly");
        lookup.remove("sessionId");
    }
    request_identity(&lookup)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ErrorPolicy {
    pub(crate) purge: bool,
    pub(crate) revoked: bool,
}

pub(crate) fn error_policy(error: &Value) -> ErrorPolicy {
    let text = error
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| error.to_string())
        .to_lowercase();
    let revoked = [
        "denied",
        "account changed",
        "sign in",
        "unauthorized",
        "you do not have access to this workspace.",
        "cloud could not verify your session.",
    ]
    .iter()
    .any(|needle| text.contains(needle))
        || matches!(
            text.as_str(),
            "401"
                | "403"
                | "http 401"
                | "http 403"
                | "workspace_not_found"
                | "capability error: workspace_not_found"
        );
    ErrorPolicy {
        purge: revoked,
        revoked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_head_matches_the_actual_persisted_request_after_reload() {
        let stored: Value = serde_json::from_str(r#"{"workspaceId":"visual-workspace","incremental":true,"expectedAccountId":"visual-account","expectedOrganizationId":null}"#).unwrap();
        let opening: Value = serde_json::from_str(r#"{"workspaceId":"visual-workspace","sessionId":"visual-session","cacheOnly":true,"incremental":true,"expectedAccountId":"visual-account","expectedOrganizationId":null}"#).unwrap();
        assert_eq!(opening_identity(&opening), request_identity(&stored));
    }

    #[test]
    fn equivalent_page_and_tool_parameters_ignore_nested_object_order() {
        let first: Value = serde_json::from_str(r#"{"workspaceId":"w","beforeSequence":156,"cursor":{"epoch":"e","version":1},"expectedAccountId":"a","expectedOrganizationId":"o"}"#).unwrap();
        let reordered: Value = serde_json::from_str(r#"{"expectedOrganizationId":"o","expectedAccountId":"a","cursor":{"version":1,"epoch":"e"},"beforeSequence":156,"workspaceId":"w"}"#).unwrap();
        assert_eq!(request_identity(&first), request_identity(&reordered));
    }

    #[test]
    fn identity_preserves_account_org_workspace_cursor_and_tool_offset() {
        let original = serde_json::json!({"workspaceId":"w","expectedAccountId":"a","expectedOrganizationId":"o","cursor":{"epoch":"e","version":1},"beforeSequence":156,"toolBlobRef":"s/t","offset":0});
        for (field, value) in [
            ("workspaceId", serde_json::json!("other")),
            ("expectedAccountId", serde_json::json!("other")),
            ("expectedOrganizationId", serde_json::Value::Null),
            ("cursor", serde_json::json!({"epoch":"other","version":1})),
            ("beforeSequence", serde_json::json!(132)),
            ("toolBlobRef", serde_json::json!("s/other")),
            ("offset", serde_json::json!(64)),
        ] {
            let mut changed = original.clone();
            changed[field] = value;
            assert_ne!(
                request_identity(&original),
                request_identity(&changed),
                "{field}"
            );
        }
    }
    #[test]
    fn actual_http_authorization_messages_revoke_encrypted_history() {
        for message in [
            "You do not have access to this workspace.",
            "Cloud could not verify your session. Reconnect in Settings → General.",
            "capability error: workspace_access_denied",
            "workspace_not_found",
            "capability error: workspace_not_found",
            "account changed",
        ] {
            assert_eq!(
                error_policy(&serde_json::json!(message)),
                ErrorPolicy {
                    purge: true,
                    revoked: true
                },
                "{message}"
            );
        }
    }

    #[test]
    fn temporary_failures_drop_fresh_authorization_without_purging_offline_pages() {
        for message in [
            "transport: connection closed",
            "Cloud service is unavailable. Try again.",
            "connection closed",
            "HTTP 503",
            "HTTP 404",
            "projection_not_found",
            "tool_output_not_published",
            "task_history_not_found",
            "request timed out",
            "transport request timed out after 401 ms",
        ] {
            assert_eq!(
                error_policy(&serde_json::json!(message)),
                ErrorPolicy {
                    purge: false,
                    revoked: false
                },
                "{message}"
            );
        }
    }
}
