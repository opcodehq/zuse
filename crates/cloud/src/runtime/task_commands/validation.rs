use super::*;
use std::path::{Component, Path};
pub(super) fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-:.".contains(&b))
}
pub(super) fn relative(value: &str) -> bool {
    value.len() <= 4096
        && !value.contains('\\')
        && !value.contains('\0')
        && !Path::new(value).is_absolute()
        && Path::new(value)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}
pub(super) fn input(input: &wire::Input) -> Result<(), ApiError> {
    if input.text.len() > wire::MAX_TEXT_BYTES
        || input.attachments.len() + input.computer_files.len()
            > zuse_protocol::cloud_assets::MAX_ASSETS
        || !computer_files(&input.computer_files)
        || input.file_refs.len() > 64
        || input.skill_refs.len() > 64
        || input.annotations.len() > 64
        || input.file_refs.iter().any(|v| !relative(v))
        || input
            .skill_refs
            .iter()
            .any(|v| v.is_empty() || v.len() > 4096)
        || input.annotations.iter().any(|v| v.len() > 16384)
        || (input.text.trim().is_empty()
            && input.attachments.is_empty()
            && input.computer_files.is_empty()
            && input.file_refs.is_empty()
            && input.skill_refs.is_empty())
    {
        return Err(ApiError::bad_request("cloud_task_input_invalid"));
    }
    if input.run_selection.as_ref().is_some_and(|s| {
        s.model.is_empty()
            || s.model.len() > 512
            || s.model.chars().any(char::is_control)
            || s.model_options.len() > 64
    }) {
        return Err(ApiError::bad_request("cloud_task_queue_selection_invalid"));
    }
    if input.attachments.iter().any(|a| {
        !id(&a.asset_id)
            || a.size_bytes > zuse_protocol::cloud_assets::MAX_BYTES
            || !zuse_protocol::cloud_assets::supported_mime(&a.mime_type)
            || a.original_name.len() > 512
            || a.sha256.len() > 128
    }) {
        return Err(ApiError::bad_request("cloud_task_asset_invalid"));
    }
    Ok(())
}
pub(super) fn request(request: &wire::Request) -> Result<(), ApiError> {
    if !id(&request.command_id) || !id(&request.session_id) {
        return Err(ApiError::bad_request("cloud_task_command_identity_invalid"));
    }
    if serde_json::to_vec(request).map_or(true, |v| v.len() > 196_608) {
        return Err(ApiError::bad_request("cloud_task_command_too_large"));
    }
    match &request.action {
        wire::Action::CreateSession { title } => {
            if title
                .as_ref()
                .is_some_and(|v| v.len() > 512 || v.chars().any(char::is_control))
            {
                return Err(ApiError::bad_request("cloud_task_session_title_invalid"));
            }
        }
        wire::Action::PauseQueue { .. } => {}
        wire::Action::ReviewUnknown {
            interrupted_command_id,
        } => {
            if !id(interrupted_command_id) {
                return Err(ApiError::bad_request("cloud_task_command_identity_invalid"));
            }
        }
        wire::Action::Queue {
            queue_id,
            message_id,
            input: body,
            ..
        } => {
            if !id(queue_id) || !id(message_id) {
                return Err(ApiError::bad_request("cloud_task_queue_identity_invalid"));
            }
            input(body)?;
        }
        wire::Action::Edit {
            queue_id,
            revision,
            input: body,
            ..
        } => {
            if !id(queue_id) || *revision < 1 {
                return Err(ApiError::bad_request("cloud_task_queue_revision_invalid"));
            }
            input(body)?;
        }
        wire::Action::Delete {
            queue_id, revision, ..
        }
        | wire::Action::Move {
            queue_id, revision, ..
        }
        | wire::Action::SendNow { queue_id, revision }
        | wire::Action::SteerNow {
            queue_id, revision, ..
        }
        | wire::Action::BeginEdit {
            queue_id, revision, ..
        } => {
            if !id(queue_id) || *revision < 1 {
                return Err(ApiError::bad_request("cloud_task_queue_revision_invalid"));
            }
        }
        wire::Action::RenewEdit { queue_id, lease_id }
        | wire::Action::CancelEdit { queue_id, lease_id } => {
            if !id(queue_id) || !id(lease_id) {
                return Err(ApiError::bad_request("cloud_task_queue_lease_invalid"));
            }
        }
        wire::Action::Submit {
            message_id,
            payload,
            attachments,
            ..
        } => {
            if !id(message_id) || attachments.len() > zuse_protocol::cloud_assets::MAX_ASSETS {
                return Err(ApiError::bad_request("cloud_task_message_invalid"));
            }
            if let zuse_protocol::session_commands::SessionCommandPayload::Run {
                request,
                message_id: inner,
            } = payload
            {
                if inner != message_id
                    || request.prompt.len() > wire::MAX_TEXT_BYTES
                    || !relative(&request.cwd)
                    || request.resume.is_some()
                    || request.worktree.is_some()
                    || request.mcp.is_some()
                    || !computer_files(&request.attachments)
                {
                    return Err(ApiError::bad_request("cloud_task_runtime_binding_required"));
                }
            }
        }
    }
    let can_wake = matches!(
        &request.action,
        wire::Action::Submit {
            payload: zuse_protocol::session_commands::SessionCommandPayload::Run { .. },
            ..
        } | wire::Action::Queue { .. }
            | wire::Action::SendNow { .. }
    );
    if !can_wake && request.wake != wire::WakePolicy::PreserveSleep {
        return Err(ApiError::bad_request("cloud_task_control_cannot_wake"));
    }
    Ok(())
}

/// Files a run or queued message carries are paths already uploaded to the
/// computer (the server never sees their bytes); the computer checks they are
/// its uploads.
fn computer_files(paths: &[String]) -> bool {
    paths.len() <= zuse_protocol::cloud_assets::MAX_ASSETS
        && paths.iter().all(|path| {
            path.starts_with('/')
                && path.len() <= 1024
                && !path.chars().any(char::is_control)
                && !path.split('/').any(|part| part == "..")
        })
}

#[cfg(test)]
mod computer_file_tests {
    #[test]
    fn a_run_may_carry_computer_upload_paths_but_nothing_else() {
        assert!(super::computer_files(&[]));
        assert!(super::computer_files(&[
            "/home/zuse/.zuse-data/uploads/a/shot.png".into()
        ]));
        assert!(!super::computer_files(&["relative.png".into()]));
        assert!(!super::computer_files(&[
            "/home/zuse/../../etc/passwd".into()
        ]));
        assert!(!super::computer_files(&vec!["/u/a".to_owned(); 9]));
    }

    fn queued(attachments: usize, computer_files: Vec<String>) -> super::wire::Input {
        let asset = zuse_protocol::cloud_assets::Asset {
            asset_id: "asset-1".into(),
            mime_type: "image/png".into(),
            original_name: "shot.png".into(),
            size_bytes: 3,
            sha256: "hash".into(),
        };
        super::wire::Input {
            run_selection: None,
            text: String::new(),
            attachments: vec![asset; attachments],
            computer_files,
            file_refs: vec![],
            skill_refs: vec![],
            annotations: vec![],
            as_goal: false,
        }
    }

    #[test]
    fn a_queued_message_may_carry_only_files_already_on_the_computer() {
        let path = "/home/zuse/.zuse-data/uploads/a/shot.png".to_owned();
        // Files alone are a message, like a run with only files.
        assert!(super::input(&queued(0, vec![path.clone()])).is_ok());
        assert!(super::input(&queued(0, vec!["shot.png".into()])).is_err());
        assert!(super::input(&queued(0, vec!["/a/../../etc/passwd".into()])).is_err());
        // Computer files and server-bound files share the per-message limit.
        assert!(super::input(&queued(4, vec![path.clone(); 4])).is_ok());
        assert!(super::input(&queued(4, vec![path; 5])).is_err());
    }

    #[test]
    fn computer_files_stay_out_of_queued_input_that_has_none() {
        let encoded = serde_json::to_value(queued(0, vec![])).unwrap();
        assert!(encoded.get("computerFiles").is_none());
        let decoded: super::wire::Input = serde_json::from_value(
            serde_json::json!({"text":"later","computerFiles":["/u/a.png"]}),
        )
        .unwrap();
        assert_eq!(decoded.computer_files, vec!["/u/a.png".to_owned()]);
    }
}
