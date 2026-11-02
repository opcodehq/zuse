//! Host-owned simulator sessions; viewers and agent consent belong to a chat.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Ios,
    Android,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Requirement {
    pub platform: Platform,
    pub ready: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// Stable UDID or AVD name, never a physical Android device's serial.
    pub id: String,
    pub name: String,
    pub platform: Platform,
    pub runtime: String,
    pub booted: bool,
    pub serial: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    #[serde(default)]
    pub setup: Option<Setup>,
    pub enabled: bool,
    pub agent_access: bool,
    pub requirements: Vec<Requirement>,
    pub devices: Vec<Device>,
    pub sessions: Vec<Device>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub chat_id: String,
    pub device_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Input {
    /// Coordinates normalized to the displayed image, in [0, 1].
    Tap {
        x: f64,
        y: f64,
    },
    Text {
        text: String,
    },
    Home,
    Back,
    Recents,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub data: String,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
}

/// Stream errors are explicit so an interrupted device cannot look live.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FrameEvent {
    Frame { frame: Frame },
    Error { message: String },
}

/// Progress comes from the host installer; the renderer never advances it on a timer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SetupStatus {
    #[default]
    Disabled,
    Checking,
    Installing,
    Starting,
    Discovering,
    Ready,
    Unavailable,
    Error,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentStatus {
    #[default]
    Off,
    Installing,
    Ready,
    Error,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRequirement {
    pub label: String,
    pub ready: bool,
    pub detail: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setup {
    #[serde(default)]
    pub step: usize,
    pub status: SetupStatus,
    pub detail: String,
    pub requirements: Vec<SetupRequirement>,
    pub agent_status: AgentStatus,
    pub agent_detail: String,
    pub hub_version: String,
    pub agent_version: String,
}
impl Default for Setup {
    fn default() -> Self {
        Self {
            step: 0,
            status: SetupStatus::Disabled,
            detail: String::new(),
            requirements: Vec::new(),
            agent_status: AgentStatus::Off,
            agent_detail: String::new(),
            hub_version: "0.15.3".into(),
            agent_version: "0.21.23".into(),
        }
    }
}
