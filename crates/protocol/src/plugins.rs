//! Managed plugin contracts shared by browser, desktop and agent gateway.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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
