//! Selected api.proto wire fields from @boxd-sh/sdk 0.2.14.
//! MIT, Copyright (c) 2026 Azin. See LICENSE.boxd.
//! Protobuf's unknown fields permit additive upstream messages.

#[derive(Clone, PartialEq, prost::Message)]
pub struct MachineId {
    #[prost(string, tag = "1")]
    pub vm_id: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Empty {}

#[derive(Clone, PartialEq, prost::Message)]
pub struct CreateFromSnapshot {
    #[prost(string, tag = "1")]
    pub snapshot: String,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, optional, tag = "3")]
    pub config: Option<VmConfig>,
    #[prost(string, tag = "4")]
    pub org: String,
    #[prost(bool, tag = "5")]
    pub isolated: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct VmConfig {
    #[prost(message, optional, tag = "4")]
    pub srf: Option<SrfConfig>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SrfConfig {
    #[prost(uint32, optional, tag = "1")]
    pub auto_suspend_timeout_secs: Option<u32>,
    #[prost(uint32, tag = "2")]
    pub auto_destroy_timeout_secs: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct UploadFile {
    #[prost(string, tag = "1")]
    pub vm_id: String,
    #[prost(string, tag = "2")]
    pub path: String,
    #[prost(bytes = "vec", tag = "3")]
    pub data: Vec<u8>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Uploaded {
    #[prost(uint64, tag = "1")]
    pub bytes_written: u64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct CreatedMachine {
    #[prost(string, tag = "1")]
    pub vm_id: String,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(string, tag = "6")]
    pub status: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct MachineDetails {
    #[prost(string, tag = "1")]
    pub vm_id: String,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(string, tag = "5")]
    pub status: String,
    #[prost(uint32, tag = "15")]
    pub vcpu: u32,
    #[prost(uint64, tag = "16")]
    pub memory_bytes: u64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ResumeResult {
    #[prost(uint64, tag = "1")]
    pub resume_us: u64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct HibernateTimeout {
    #[prost(string, tag = "1")]
    pub vm_id: String,
    #[prost(uint32, tag = "2")]
    pub timeout_secs: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ExecChunk {
    #[prost(bytes = "vec", tag = "1")]
    pub data: Vec<u8>,
    #[prost(bool, tag = "2")]
    pub stdin: bool,
    #[prost(bool, tag = "3")]
    pub tty: bool,
    #[prost(string, tag = "4")]
    pub command: String,
    #[prost(string, tag = "5")]
    pub vm_id: String,
    #[prost(int32, tag = "6")]
    pub exit_code: i32,
    #[prost(uint32, tag = "7")]
    pub cols: u32,
    #[prost(uint32, tag = "8")]
    pub rows: u32,
    #[prost(bool, tag = "9")]
    pub window_change: bool,
    #[prost(bool, tag = "10")]
    pub is_stderr: bool,
    #[prost(string, tag = "11")]
    pub device_id: String,
}
