//! Factual turn boundaries; admission and provider text are separate concerns.
use crate::EngineError;
use std::sync::Arc;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    WaitingInput,
    Completed,
    Cancelled,
    Failed,
    OutcomeUnknown,
}
#[derive(Clone, Debug)]
pub struct Fact {
    pub chat_id: String,
    pub message_id: String,
    pub command_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub state: State,
    pub at: i64,
}
pub type Observer = Arc<dyn Fn(&Fact) -> Result<(), EngineError> + Send + Sync>;
