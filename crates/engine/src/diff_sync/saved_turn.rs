//! Latest-turn review against immutable archive objects, preserving capture omissions.
use super::*;
pub(crate) async fn capture_saved_turn_diff(
    root: &Path,
    turn: &TreeSnapshot,
    target: &str,
    branch: &str,
) -> Result<DiffSnapshot, EngineError> {
    // Add/delete representation avoids a rename across an omitted directory
    // pulling that excluded content back into the review.
    let names = capture_git_in(
        root,
        &[
            "diff",
            "--name-status",
            "-z",
            "--no-renames",
            turn.tree(),
            target,
            "--",
        ],
        2 * 1024 * 1024,
        Some(turn),
    )
    .await?;
    let nums = capture_git_in(
        root,
        &[
            "diff",
            "--numstat",
            "-z",
            "--no-renames",
            turn.tree(),
            target,
            "--",
        ],
        2 * 1024 * 1024,
        Some(turn),
    )
    .await?;
    let mut files = parse_name_status(&names.stdout);
    apply_numstat(&mut files, &nums.stdout);
    files.retain(|file| !turn.skipped_path(&file.path));
    let paths: Vec<Vec<u8>> = files
        .iter()
        .map(|file| file.path.as_bytes().to_vec())
        .collect();
    let mut patch = String::new();
    let mut truncated = names.truncated || nums.truncated;
    for batch in path_batches(&paths, MAX_PATH_ARGUMENT_BYTES) {
        let remaining = MAX_PATCH_BYTES.saturating_sub(patch.len());
        if remaining == 0 {
            truncated = true;
            break;
        }
        let mut args: Vec<OsString> = [
            "diff",
            "--no-ext-diff",
            "--no-color",
            "--no-renames",
            "--unified=3",
            turn.tree(),
            target,
            "--",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        args.extend(batch.iter().map(|path| path_argument(path)));
        let part = capture_git_in(root, &args, remaining, Some(turn)).await?;
        patch.push_str(&String::from_utf8_lossy(&part.stdout));
        if part.truncated {
            truncated = true;
            let boundary = patch.rfind('\n').unwrap_or(0);
            patch.truncate(boundary);
            break;
        }
    }
    let additions = files
        .iter()
        .fold(0u32, |n, f| n.saturating_add(f.additions));
    let deletions = files
        .iter()
        .fold(0u32, |n, f| n.saturating_add(f.deletions));
    let mut hash = Sha256::new();
    for part in [branch, turn.tree(), target, &patch] {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    hash.update(serde_json::to_vec(&files).map_err(|e| EngineError::Other(e.to_string()))?);
    hash.update([truncated as u8]);
    Ok(DiffSnapshot {
        git_status: None,
        branch: branch.into(),
        head_sha: Some(target.into()),
        patch,
        files,
        additions,
        deletions,
        truncated,
        checksum: crate::repos::hex(&hash.finalize()),
    })
}
