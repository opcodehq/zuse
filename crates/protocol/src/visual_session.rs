//! Transient host-owned visual sessions. Controls never retry after uncertain delivery.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CAPABILITY: &str = "visual-sessions-v1";
pub const VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Open {
    pub chat_id: String,
    pub visual_id: String,
    pub target_device_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Control {
    pub lease: String,
    pub revision: u64,
    pub handler: u32,
    pub argument: Argument,
    pub target_device_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum Argument {
    None,
    Text(String),
    Bool(bool),
    Number(f64),
    Point([f32; 2]),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Draw {
    pub lease: String,
    pub revision: u64,
    pub handler: u32,
    pub width: f32,
    pub height: f32,
    pub time: f64,
    pub pointer: Option<Pointer>,
    pub target_device_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pointer {
    pub x: f32,
    pub y: f32,
    pub down: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub version: u32,
    pub lease: String,
    pub revision: u64,
    pub tree: Value,
}
