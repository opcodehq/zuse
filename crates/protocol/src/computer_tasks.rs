//! Runtime callers can discover and request host-defined tasks. Approval remains owner-only.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum Request {
    List,
    Run {
        task: uuid::Uuid,
        request_id: uuid::Uuid,
    },
    Command {
        device_id: String,
        command: String,
        cwd: Option<String>,
        request_id: uuid::Uuid,
    },
    Results,
}
pub const METHOD: &str = "ComputerTasks";
