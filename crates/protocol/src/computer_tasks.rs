//! Runtime callers can discover and request host-defined tasks. Approval remains owner-only.
use serde::{Deserialize, Serialize};

/// Deadline for a command that does not ask for one.
pub const DEFAULT_TIMEOUT_SECS: u32 = 600;
/// The longest deadline a command may request.
pub const MAX_TIMEOUT_SECS: u32 = 3600;
/// How often a running command reports output and rechecks its approval.
pub const HEARTBEAT_SECS: u64 = 2;
/// A claimed command silent for this long shows "Outcome unknown".
pub const UNKNOWN_AFTER_SECS: u64 = 80;
/// The longest a long-poll (a connector's claim, a runtime's [`Request::Wait`])
/// is held open before it answers "nothing yet"; callers ask again.
pub const MAX_WAIT_MS: u32 = 8000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum Request {
    /// Host-defined tasks offered by attached computers.
    List,
    /// Attached computers, including offline ones.
    Computers,
    Run {
        task: uuid::Uuid,
        request_id: uuid::Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u32>,
    },
    /// A shell command. Without `device_id` it runs on the chat's primary
    /// computer (the one the owner last sent from); a secondary must be named.
    Command {
        #[serde(default)]
        device_id: Option<String>,
        command: String,
        cwd: Option<String>,
        request_id: uuid::Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u32>,
    },
    Results,
    /// Like [`Request::Results`], answered once request `request_id` has
    /// settled (finished, denied, stopped or unknown) or after `wait_ms`
    /// (at most [`MAX_WAIT_MS`]), whichever comes first. Servers from before
    /// this request refuse it; callers then poll `Results`.
    Wait {
        request_id: uuid::Uuid,
        wait_ms: u32,
    },
}
pub const METHOD: &str = "ComputerTasks";

/// A request state that will not change any more (see the cloud's results).
pub fn settled(state: &str) -> bool {
    matches!(state, "finished" | "denied" | "cancelled" | "unknown")
}

/// The deadline in milliseconds for a requested timeout, or `None` when the
/// request is out of range.
pub fn timeout_ms(requested: Option<u32>) -> Option<i32> {
    match requested {
        None => Some((DEFAULT_TIMEOUT_SECS * 1000) as i32),
        Some(seconds) if (1..=MAX_TIMEOUT_SECS).contains(&seconds) => Some((seconds * 1000) as i32),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_defaults_and_rejects_out_of_range_requests() {
        assert_eq!(timeout_ms(None), Some(600_000));
        assert_eq!(timeout_ms(Some(1)), Some(1_000));
        assert_eq!(timeout_ms(Some(3600)), Some(3_600_000));
        assert_eq!(timeout_ms(Some(0)), None);
        assert_eq!(timeout_ms(Some(3601)), None);
        assert_eq!(timeout_ms(Some(u32::MAX)), None);
    }

    /// Runtimes released before per-request deadlines send this shape.
    #[test]
    fn requests_without_a_timeout_still_parse() {
        let request: Request = serde_json::from_value(serde_json::json!({
            "action": "command",
            "device_id": "mac",
            "command": "pwd",
            "cwd": null,
            "request_id": "6b1f4a52-37c9-4e57-9a8b-7b5e1c2d3f40"
        }))
        .unwrap();
        assert!(matches!(
            request,
            Request::Command {
                timeout_seconds: None,
                ..
            }
        ));
    }
}
