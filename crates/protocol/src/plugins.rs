//! Managed plugin contracts shared by browser, desktop and agent gateway.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Identifier prefix for servers that run as local processes on the engine host.
pub const LOCAL_SERVER_PREFIX: &str = "local-";
pub fn is_local_server(id: &str) -> bool {
    id.starts_with(LOCAL_SERVER_PREFIX)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PluginReturnTo {
    Web,
    Desktop { port: u16 },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PluginApiKey {
    pub header: String,
    pub value: String,
}

// Requests containing credentials intentionally do not implement Debug.
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PluginRequest {
    List {
        #[serde(skip_serializing_if = "Option::is_none")]
        tenant_id: Option<String>,
    },
    Connect {
        tenant_id: String,
        plugin_id: String,
        label: String,
        request_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        return_to: Option<PluginReturnTo>,
        #[serde(skip_serializing_if = "Option::is_none")]
        api_key: Option<String>,
    },
    AddServer {
        tenant_id: String,
        request_id: String,
        name: String,
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        api_key: Option<PluginApiKey>,
        #[serde(skip_serializing_if = "Option::is_none")]
        return_to: Option<PluginReturnTo>,
    },
    /// A stdio MCP server launched on the engine host. It never reaches the hosted API.
    AddLocalServer {
        tenant_id: String,
        request_id: String,
        name: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    RemoveServer {
        tenant_id: String,
        plugin_id: String,
    },
    Poll {
        tenant_id: String,
        attempt_id: String,
    },
    Cancel {
        tenant_id: String,
        attempt_id: String,
    },
    Complete {
        tenant_id: String,
        ticket: String,
    },
    Disconnect {
        tenant_id: String,
        connection_id: String,
    },
    SetEnabled {
        tenant_id: String,
        connection_id: String,
        enabled: bool,
    },
}

impl PluginRequest {
    pub fn tenant_id(&self) -> Option<&str> {
        match self {
            Self::List { tenant_id } => tenant_id.as_deref(),
            Self::Connect { tenant_id, .. }
            | Self::AddServer { tenant_id, .. }
            | Self::AddLocalServer { tenant_id, .. }
            | Self::RemoveServer { tenant_id, .. }
            | Self::Poll { tenant_id, .. }
            | Self::Cancel { tenant_id, .. }
            | Self::Complete { tenant_id, .. }
            | Self::Disconnect { tenant_id, .. }
            | Self::SetEnabled { tenant_id, .. } => Some(tenant_id),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum PluginToolRequest {
    List,
    Search {
        query: String,
    },
    Schema {
        address: String,
    },
    Call {
        address: String,
        arguments: Map<String, Value>,
    },
}

impl PluginToolRequest {
    pub fn invokes_external_tool(&self) -> bool {
        matches!(self, Self::Call { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginTenant {
    pub id: String,
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub featured: bool,
    #[serde(default)]
    pub custom: bool,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub api_key_header: Option<String>,
    /// Present only on local stdio servers. Environment values are never included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginConnectionState {
    Connecting,
    Connected,
    NeedsAuth,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginConnection {
    pub id: String,
    pub plugin_id: String,
    pub label: String,
    pub owner: String,
    pub state: PluginConnectionState,
    pub created_at: f64,
    #[serde(default = "enabled")]
    pub enabled: bool,
}
fn enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PluginResponse {
    Snapshot {
        #[serde(default = "enabled")]
        can_manage: bool,
        tenants: Vec<PluginTenant>,
        tenant_id: String,
        endpoint: String,
        catalog: Vec<PluginDefinition>,
        connections: Vec<PluginConnection>,
    },
    Attempt {
        id: String,
        connection_id: String,
        state: String,
        authorization_url: Option<String>,
        expires_at: f64,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        plugin_id: String,
    },
    Ok,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn remote_server_requests_keep_their_wire_shape_and_local_requests_stay_distinct() {
        let remote = PluginRequest::AddServer {
            tenant_id: "personal:user".into(),
            request_id: "request".into(),
            name: "Tools".into(),
            url: "https://tools.example/mcp".into(),
            api_key: None,
            return_to: Some(PluginReturnTo::Web),
        };
        assert_eq!(
            serde_json::to_string(&remote).unwrap(),
            r#"{"action":"addServer","tenantId":"personal:user","requestId":"request","name":"Tools","url":"https://tools.example/mcp","returnTo":{"kind":"web"}}"#
        );
        let local: PluginRequest = serde_json::from_value(json!({
            "action":"addLocalServer","tenantId":"personal:user","requestId":"request",
            "name":"Files","command":"npx","args":["-y","server"],"env":{"TOKEN":"value"}
        }))
        .unwrap();
        let PluginRequest::AddLocalServer { args, env, .. } = &local else {
            panic!("expected a local server request");
        };
        assert_eq!(args, &["-y", "server"]);
        assert_eq!(env["TOKEN"], "value");
        assert_eq!(local.tenant_id(), Some("personal:user"));
        let definition: PluginDefinition =
            serde_json::from_value(json!({"id":"hosted","name":"Hosted","description":""}))
                .unwrap();
        assert!(definition.command.is_none());
        assert!(
            serde_json::to_value(&definition)
                .unwrap()
                .get("command")
                .is_none()
        );
        assert!(is_local_server("local-1") && !is_local_server("hosted"));
    }
}
