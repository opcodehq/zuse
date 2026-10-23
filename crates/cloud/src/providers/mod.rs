//! Sandbox infrastructure. Guest RPC owns interactive tools and agent execution.
pub mod boxd;
#[cfg(test)]
mod boxd_tests;
pub(crate) mod boxd_wire;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderError {
    NotFound,
    Rejected,
    Conflict,
    RateLimited,
    Unavailable,
    /// A mutation may have been accepted. Reconcile its identity before retrying.
    OutcomeUnknown,
    InvalidResponse,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MachineState {
    Running,
    Paused,
    Stopped,
    Starting,
    Unknown(String),
}

#[derive(Clone, Debug)]
pub struct Machine {
    pub id: String,
    pub name: String,
    pub state: MachineState,
    pub vcpu: Option<u32>,
    pub memory_bytes: Option<u64>,
}

pub struct ExecResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}
