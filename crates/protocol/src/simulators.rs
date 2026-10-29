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
