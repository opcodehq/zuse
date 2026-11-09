//! Computer controls follow current task authority, separately from a diagnostic channel.
use serde_json::Value;
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Controls {
    pub stop: bool,
    pub sleep: bool,
    pub resume: bool,
    pub delete: bool,
}
pub(super) fn editable(target: &Value) -> bool {
    if target["taskCommandCanEdit"].is_boolean() {
        return target["taskCommandCanEdit"] == true;
    }
    target["runtimeRepairCanEdit"] == true || target["computerRecreationCanEdit"] == true
}

pub(super) fn available(
    target: &Value,
    live: bool,
    api_commands: bool,
    selected: bool,
    busy: bool,
) -> Controls {
    let state = target["state"].as_str().unwrap_or_default();
    let deleted = matches!(state, "deleting" | "deleted" | "archiving" | "archived");
    let held = ["runtimeRepair", "computerRecreation"].iter().any(|key| {
        target[*key]["operationId"].is_string()
            && target[*key]["state"]
                .as_str()
                .is_some_and(|state| state != "ready")
    });
    let edit = editable(target) && !deleted;
    let preparing = matches!(state, "queued" | "provisioning" | "setup" | "starting");
    Controls {
        stop: edit
            && !held
            && !busy
            && selected
            && (api_commands && (preparing || state == "ready") || live && state == "ready"),
        sleep: edit && !held && !busy && live && state == "ready",
        resume: edit
            && !held
            && !busy
            && !live
            && matches!(state, "paused" | "blocked" | "stopped" | "failed")
            && super::preparation::issue(target["statusCode"].as_str().unwrap_or_default())
                .is_none(),
        delete: edit && !busy,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn admitted_preparation_can_stop_without_sleeping_a_diagnostic_connection() {
        let row = json!({"state":"setup","taskCommandCanEdit":true});
        assert_eq!(
            available(&row, true, true, true, false),
            Controls {
                stop: true,
                sleep: false,
                resume: false,
                delete: true
            }
        );
        assert!(!available(&row, true, false, true, false).stop);
    }
    #[test]
    fn viewers_and_held_operations_do_not_offer_mutating_lifecycle_controls() {
        let mut row = json!({"state":"ready"});
        assert_eq!(
            available(&row, true, true, true, false),
            Controls {
                stop: false,
                sleep: false,
                resume: false,
                delete: false
            }
        );
        row["runtimeRepairCanEdit"] = json!(true);
        row["runtimeRepair"] = json!({"operationId":"repair","state":"attention-required"});
        let controls = available(&row, true, true, true, false);
        assert!(!controls.stop && !controls.sleep && !controls.resume);
        assert!(controls.delete, "Explicit cleanup remains available");
    }
    #[test]
    fn resume_does_not_replace_a_specific_setup_recovery() {
        let mut row = json!({"state":"paused","runtimeRepairCanEdit":true});
        assert!(available(&row, false, false, true, false).resume);
        row["statusCode"] = json!("codex_memory_transport_recovery_required");
        assert!(!available(&row, false, false, true, false).resume);
    }
}
