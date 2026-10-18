//! Read-only review from pinned Git objects; never rehydrates a checkout.
use super::*;
use crate::diff_sync::{self, DiffSnapshot};
use zuse_protocol::{Chat, CheckoutDiff, CheckoutFileDiffText, GetCheckoutFileDiffTextRequest};

impl Archives {
    pub(crate) fn review_record(
        &self,
        chat: &Chat,
        cwd: &Path,
    ) -> Result<Option<CheckoutArchive>, EngineError> {
        if !chat.archived {
            return Ok(None);
        }
        let Some(chat_cwd) = chat.cwd.as_deref() else {
            return Ok(None);
        };
        let Some(record) = self.record(Path::new(chat_cwd))? else {
            return Ok(None);
        };
        if cwd != Path::new(chat_cwd) && cwd != record.checkout {
            return Err(error(
                "The review path does not belong to this archived thread.",
            ));
        }
        // Shared/retained checkouts still use the ordinary live review path.
        if record.phase == Phase::Restored
            || (record.phase == Phase::Prepared && record.checkout.exists())
        {
            return Ok(None);
        }
        Ok(Some(record))
    }

    async fn review_snapshot(
        &self,
        record: &CheckoutArchive,
        mode: &str,
        base_ref: Option<&str>,
        commit: Option<&str>,
    ) -> Result<(DiffSnapshot, String, String), EngineError> {
        let found = text(&record.repo, &["rev-parse", "--verify", &record.reference]).await?;
        if found != record.checkpoint {
            return Err(error(
                "The saved checkpoint reference changed. Review was stopped.",
            ));
        }
        let (base, target) = match mode {
            "branch" => {
                let reference = base_ref.ok_or_else(|| error("baseRef required"))?;
                // Use the archived HEAD, never the main checkout's current HEAD.
                let base =
                    text(&record.repo, &["merge-base", "--", reference, &record.head]).await?;
                (base, record.checkpoint.clone())
            }
            "commit" => {
                let sha = commit.ok_or_else(|| error("commitSha required"))?;
                let sha = text(
                    &record.repo,
                    &[
                        "rev-parse",
                        "--verify",
                        "--end-of-options",
                        &format!("{sha}^{{commit}}"),
                    ],
                )
                .await?;
                (diff_sync::commit_diff_base(&record.repo, &sha).await, sha)
            }
            "turn" => {
                return Err(error(
                    "Latest-turn review is unavailable for a reclaimed checkout. Select Working tree or Branch changes.",
                ));
            }
            // Preserve the live RPC's default/legacy working-tree modes.
            _ => (record.head.clone(), record.checkpoint.clone()),
        };
        let snapshot = diff_sync::capture_revision_diff(
            &record.repo,
            &base,
            &target,
            record.branch.as_deref().unwrap_or("HEAD"),
        )
        .await?;
        Ok((snapshot, base, target))
    }

    pub(crate) async fn review_diff(
        &self,
        chat: &Chat,
        record: &CheckoutArchive,
        mode: &str,
        base_ref: Option<&str>,
        commit: Option<&str>,
    ) -> Result<CheckoutDiff, EngineError> {
        let (snapshot, _, _) = self.review_snapshot(record, mode, base_ref, commit).await?;
        Ok(CheckoutDiff {
            checkout_id: chat
                .checkout_id
                .clone()
                .unwrap_or_else(|| format!("archive:{}", key(&record.checkout))),
            device_id: chat.device_id.clone(),
            cwd: chat
                .cwd
                .clone()
                .unwrap_or_else(|| record.checkout.to_string_lossy().into_owned()),
            patch: snapshot.patch,
            files: snapshot.files,
            additions: snapshot.additions,
            deletions: snapshot.deletions,
            truncated: snapshot.truncated,
            checksum: snapshot.checksum,
            updated_at: chrono::Utc::now(),
        })
    }

    pub(crate) async fn review_file(
        &self,
        chat: &Chat,
        record: &CheckoutArchive,
        request: &GetCheckoutFileDiffTextRequest,
    ) -> Result<CheckoutFileDiffText, EngineError> {
        let expected = chat
            .checkout_id
            .clone()
            .unwrap_or_else(|| format!("archive:{}", key(&record.checkout)));
        if request.checkout_id != expected {
            return Err(error("checkoutId does not match the saved checkout."));
        }
        let (snapshot, base, target) = self
            .review_snapshot(
                record,
                &request.mode,
                request.base_ref.as_deref(),
                request.commit_sha.as_deref(),
            )
            .await?;
        if snapshot.checksum != request.diff_checksum {
            return Ok(CheckoutFileDiffText {
                diff_checksum: request.diff_checksum.clone(),
                old_text: None,
                new_text: None,
                old_content_hash: None,
                new_content_hash: None,
                binary: false,
                truncated: false,
                stale: true,
            });
        }
        let file = snapshot
            .files
            .iter()
            .find(|file| file.path == request.path)
            .ok_or_else(|| error("Path is not part of the saved diff snapshot."))?;
        let pair =
            diff_sync::read_diff_file_text_at(&record.repo, &base, None, Some(&target), file)
                .await?;
        Ok(CheckoutFileDiffText {
            diff_checksum: snapshot.checksum,
            old_text: pair.old_text,
            new_text: pair.new_text,
            old_content_hash: pair.old_content_hash,
            new_content_hash: pair.new_content_hash,
            binary: pair.binary,
            truncated: pair.truncated,
            stale: false,
        })
    }
}
