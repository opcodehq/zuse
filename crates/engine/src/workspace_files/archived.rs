//! Lazy, read-only workspace views over a saved checkpoint and retained context.
use super::*;
use crate::archives::{Archives, CheckoutArchive};
use zuse_protocol::Chat;

pub(crate) struct ArchivedWorkspace {
    archives: Archives,
    chat: Chat,
    record: CheckoutArchive,
}
fn archive_error(err: impl std::fmt::Display) -> WorkspaceFilesError {
    WorkspaceFilesError::Io(err.to_string())
}
impl WorkspaceFiles {
    pub(super) fn target_is_archived(
        &self,
        target: &WorkspaceTarget,
    ) -> Result<bool, WorkspaceFilesError> {
        target
            .chat_id
            .as_deref()
            .map(|id| {
                self.inner
                    .workspace
                    .chat(id)
                    .map(|chat| chat.is_some_and(|chat| chat.archived))
                    .map_err(archive_error)
            })
            .unwrap_or(Ok(false))
    }
    pub(super) fn require_active_target(
        &self,
        target: &WorkspaceTarget,
    ) -> Result<(), WorkspaceFilesError> {
        if let Some(id) = target.chat_id.as_deref() {
            if self
                .inner
                .workspace
                .chat(id)
                .map_err(archive_error)?
                .is_some_and(|chat| chat.archived)
            {
                return Err(WorkspaceFilesError::Authorization(
                    "Unarchive this thread before changing its files.".into(),
                ));
            }
        }
        Ok(())
    }
    pub(crate) fn archived_target(
        &self,
        target: &WorkspaceTarget,
    ) -> Result<Option<ArchivedWorkspace>, WorkspaceFilesError> {
        let Some(id) = target.chat_id.as_deref() else {
            return Ok(None);
        };
        if target.space_id.is_some() || target.checkout_path.is_some() {
            return Err(bad_path(
                "chat target cannot include spaceId or checkoutPath",
            ));
        }
        let chat = self
            .inner
            .workspace
            .chat(id)
            .map_err(archive_error)?
            .ok_or_else(|| WorkspaceFilesError::NotFound("Thread not found.".into()))?;
        if chat.device_id != self.inner.device_id {
            return Err(WorkspaceFilesError::Authorization(
                "Thread belongs to another computer.".into(),
            ));
        }
        let Some(cwd) = chat.cwd.as_deref() else {
            return Ok(None);
        };
        let archives = Archives::new(self.inner.repos.data_dir());
        let Some(record) = archives
            .review_record(&chat, Path::new(cwd))
            .map_err(archive_error)?
        else {
            return Ok(None);
        };
        Ok(Some(ArchivedWorkspace {
            archives,
            chat,
            record,
        }))
    }
}
impl ArchivedWorkspace {
    fn checkout_id(&self) -> String {
        self.chat.checkout_id.clone().unwrap_or_else(|| {
            format!(
                "archive:{}",
                crate::repos::hex(&Sha256::digest(
                    self.record.checkout.as_os_str().as_encoded_bytes()
                ))
            )
        })
    }
    fn relative_file(&self, path: &str) -> Result<WorkspaceRelativePath, WorkspaceFilesError> {
        if path.starts_with('/') {
            let absolute = Path::new(path);
            for root in [
                self.chat.cwd.as_deref().map(Path::new),
                Some(self.record.checkout.as_path()),
            ]
            .into_iter()
            .flatten()
            {
                if let Ok(relative) = absolute.strip_prefix(root) {
                    return WorkspaceRelativePath::file(&path_to_wire(relative)?);
                }
            }
            return Err(WorkspaceFilesError::Authorization(
                "Only saved workspace paths can be read from an archive.".into(),
            ));
        }
        WorkspaceRelativePath::file(path)
    }
    async fn entries(
        &self,
        directory: &WorkspaceRelativePath,
        include_ignored: bool,
    ) -> Result<(Vec<WorkspaceEntry>, bool), WorkspaceFilesError> {
        let wire = directory.wire_path();
        let tree = if wire.is_empty() {
            self.record.checkpoint.clone()
        } else {
            format!("{}:{}", self.record.checkpoint, wire)
        };
        let args = ["ls-tree", "-z", "-l", &tree];
        let captured =
            crate::diff_sync::capture_git(&self.record.repo, &args, 8 * 1024 * 1024).await;
        let saved = self.archives.retained_root(&self.record);
        let saved_directory = checked_directory(&saved, directory).ok();
        let mut entries = std::collections::BTreeMap::new();
        let mut truncated = false;
        match captured {
            Ok(captured) => {
                truncated = captured.truncated;
                let complete = if captured.truncated {
                    captured
                        .stdout
                        .iter()
                        .rposition(|byte| *byte == 0)
                        .map_or(0, |end| end + 1)
                } else {
                    captured.stdout.len()
                };
                for bytes in captured.stdout[..complete]
                    .split(|b| *b == 0)
                    .filter(|bytes| !bytes.is_empty())
                {
                    let Some(tab) = bytes.iter().position(|b| *b == b'\t') else {
                        truncated = true;
                        continue;
                    };
                    let header = String::from_utf8_lossy(&bytes[..tab]);
                    let fields = header.split_whitespace().collect::<Vec<_>>();
                    let Ok(name) = std::str::from_utf8(&bytes[tab + 1..]) else {
                        truncated = true;
                        continue;
                    };
                    if fields.len() != 4 {
                        truncated = true;
                        continue;
                    }
                    let path = if wire.is_empty() {
                        name.to_owned()
                    } else {
                        format!("{wire}/{name}")
                    };
                    if WorkspaceRelativePath::file(&path).is_err()
                        || is_internal_temp_wire_path(&path)
                    {
                        continue;
                    }
                    let kind = match fields[0] {
                        "040000" => WorkspaceEntryKind::Directory,
                        "120000" => WorkspaceEntryKind::Symlink,
                        _ => WorkspaceEntryKind::File,
                    };
                    entries.insert(
                        name.to_owned(),
                        WorkspaceEntry {
                            mutation_revision: None,
                            path,
                            name: name.to_owned(),
                            kind,
                            size: fields[3].parse().ok(),
                            modified_at: None,
                            ignored: false,
                            read_only: true,
                        },
                    );
                    if entries.len() == MAX_DIRECTORY_ENTRIES {
                        truncated = true;
                        break;
                    }
                }
            }
            Err(err) if saved_directory.is_none() => return Err(archive_error(err)),
            Err(_) => {}
        }
        if include_ignored {
            if let Some(saved_directory) = saved_directory {
                for entry in std::fs::read_dir(saved_directory).map_err(archive_error)? {
                    if entries.len() >= MAX_DIRECTORY_ENTRIES {
                        truncated = true;
                        break;
                    }
                    let entry = entry.map_err(archive_error)?;
                    let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                        truncated = true;
                        continue;
                    };
                    if entries.contains_key(&name) {
                        continue;
                    }
                    let path = if wire.is_empty() {
                        name.clone()
                    } else {
                        format!("{wire}/{name}")
                    };
                    if WorkspaceRelativePath::file(&path).is_err()
                        || is_internal_temp_wire_path(&path)
                    {
                        continue;
                    }
                    let metadata =
                        std::fs::symlink_metadata(entry.path()).map_err(archive_error)?;
                    let kind = if metadata.file_type().is_symlink() {
                        WorkspaceEntryKind::Symlink
                    } else if metadata.is_dir() {
                        WorkspaceEntryKind::Directory
                    } else {
                        WorkspaceEntryKind::File
                    };
                    entries.insert(
                        name.clone(),
                        WorkspaceEntry {
                            mutation_revision: None,
                            path,
                            name,
                            kind,
                            size: metadata.is_file().then_some(metadata.len()),
                            modified_at: metadata.modified().ok().map(chrono::DateTime::from),
                            ignored: true,
                            read_only: true,
                        },
                    );
                    if entries.len() == MAX_DIRECTORY_ENTRIES {
                        truncated = true;
                        break;
                    }
                }
            }
        }
        let mut entries = entries.into_values().collect::<Vec<_>>();
        entries.sort_by(|a, b| {
            entry_group(a.kind)
                .cmp(&entry_group(b.kind))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.path.cmp(&b.path))
        });
        Ok((entries, truncated))
    }
    pub(super) async fn list_directory(
        &self,
        request: ListWorkspaceDirectoryRequest,
    ) -> Result<WorkspaceDirectoryPage, WorkspaceFilesError> {
        let directory = WorkspaceRelativePath::directory(&request.directory)?;
        let (entries, truncated) = self.entries(&directory, request.include_ignored).await?;
        let fingerprint = format!(
            "{}:{}",
            self.record.checkpoint,
            directory_fingerprint(&entries)
        );
        let offset = if let Some(cursor) = request.cursor.as_deref() {
            let cursor = decode_cursor(cursor)?;
            if cursor.version != 1
                || cursor.directory != directory.wire_path()
                || cursor.include_ignored != request.include_ignored
                || cursor.fingerprint != fingerprint
            {
                return Err(bad_path("Archive listing changed; restart pagination"));
            }
            cursor.offset
        } else {
            0
        };
        if offset > entries.len() {
            return Err(bad_path("Archive cursor is out of range"));
        }
        let end = offset
            .saturating_add(DIRECTORY_PAGE_SIZE)
            .min(entries.len());
        let next_cursor = (end < entries.len()).then(|| {
            encode_cursor(&DirectoryCursor {
                version: 1,
                directory: directory.wire_path(),
                include_ignored: request.include_ignored,
                offset: end,
                fingerprint,
            })
        });
        Ok(WorkspaceDirectoryPage {
            checkout_id: Some(self.checkout_id()),
            mutation_capabilities: Some(zuse_protocol::WorkspaceMutationCapabilities {
                move_entry: false,
                delete_entry: false,
            }),
            directory: directory.wire_path(),
            entries: entries[offset..end].to_vec(),
            next_cursor,
            truncated,
        })
    }
    async fn entry(
        &self,
        relative: &WorkspaceRelativePath,
    ) -> Result<WorkspaceEntry, WorkspaceFilesError> {
        let parent = relative.as_path().parent().unwrap_or(Path::new(""));
        let parent = WorkspaceRelativePath::directory(&path_to_wire(parent)?)?;
        let (entries, _) = self.entries(&parent, true).await?;
        entries
            .into_iter()
            .find(|entry| entry.path == relative.wire_path())
            .ok_or_else(|| {
                WorkspaceFilesError::NotFound("File is not in this saved checkpoint.".into())
            })
    }
    async fn materialized(
        &self,
        relative: &WorkspaceRelativePath,
    ) -> Result<(PathBuf, WorkspaceRelativePath), WorkspaceFilesError> {
        let original = self.record.checkout.join(relative.as_path());
        let (file, root) = self
            .archives
            .attachment(&original)
            .await
            .map_err(archive_error)?
            .ok_or_else(|| WorkspaceFilesError::NotFound("Saved file is unavailable.".into()))?;
        let path = file.strip_prefix(&root).map_err(archive_error)?;
        Ok((
            root.clone(),
            WorkspaceRelativePath::file(&path_to_wire(path)?)?,
        ))
    }
    pub(super) async fn read_file(
        &self,
        request: ReadWorkspaceFileRequest,
    ) -> Result<WorkspaceFileText, WorkspaceFilesError> {
        let relative = self.relative_file(&request.path)?;
        let entry = self.entry(&relative).await?;
        let unsupported = if entry.kind == WorkspaceEntryKind::Symlink {
            Some(WorkspaceReadOnlyReason::Symlink)
        } else if entry.kind != WorkspaceEntryKind::File {
            Some(WorkspaceReadOnlyReason::NotRegularFile)
        } else if entry.size.is_some_and(|size| size > MAX_PREVIEW_FILE_BYTES) {
            Some(WorkspaceReadOnlyReason::TooLarge)
        } else {
            None
        };
        if let Some(reason) = unsupported {
            return Ok(WorkspaceFileText {
                checkout_id: self.checkout_id(),
                path: relative.wire_path(),
                text: None,
                content_hash: None,
                size: entry.size.unwrap_or(0),
                modified_at: None,
                encoding: WorkspaceTextEncoding::Unsupported,
                line_ending: None,
                read_only_reason: Some(reason),
                truncated: reason == WorkspaceReadOnlyReason::TooLarge,
            });
        }
        let (root, path) = self.materialized(&relative).await?;
        let mut file = tokio::task::spawn_blocking(move || {
            read_file_blocking(&root, &path, &AtomicBool::new(false))
        })
        .await
        .map_err(archive_error)??;
        file.checkout_id = self.checkout_id();
        file.path = relative.wire_path();
        file.modified_at = None;
        file.read_only_reason = file
            .read_only_reason
            .or(Some(WorkspaceReadOnlyReason::Archived));
        Ok(file)
    }
    pub(super) async fn read_image(
        &self,
        request: zuse_protocol::ReadWorkspaceImageRequest,
    ) -> Result<zuse_protocol::WorkspaceImageChunk, WorkspaceFilesError> {
        if request.expected_checkout_id != self.checkout_id() {
            return Err(WorkspaceFilesError::Authorization(
                "Saved checkout identity changed.".into(),
            ));
        }
        let relative = self.relative_file(&request.path)?;
        let entry = self.entry(&relative).await?;
        if entry.kind != WorkspaceEntryKind::File
            || entry
                .size
                .is_some_and(|size| size > zuse_protocol::MAX_WORKSPACE_IMAGE_BYTES as u64)
        {
            return Err(bad_path("Saved image is not a supported regular file"));
        }
        let (root, relative) = self.materialized(&relative).await?;
        tokio::task::spawn_blocking(move || read_image_blocking(&root, &relative, &request))
            .await
            .map_err(archive_error)?
    }
    pub(super) async fn search(
        &self,
        request: SearchWorkspaceFilesRequest,
    ) -> Result<Vec<WorkspaceFileSearchMatch>, WorkspaceFilesError> {
        validate_name_search_query(&request.query, request.include_ignored)?;
        let limit =
            usize::from(request.limit.unwrap_or(MAX_SEARCH_RESULTS as u16)).min(MAX_SEARCH_RESULTS);
        if limit == 0 {
            return Ok(vec![]);
        }
        let query = normalize_name_query(&request.query).trim().to_lowercase();
        let mut pending = vec![WorkspaceRelativePath::directory("")?];
        let mut matches = Vec::new();
        let mut scanned = 0;
        while let Some(directory) = pending.pop() {
            let (entries, truncated) = self.entries(&directory, request.include_ignored).await?;
            if truncated {
                return Err(bad_path(
                    "Archive name search exceeded its listing limit. Browse a smaller folder.",
                ));
            }
            for entry in entries {
                scanned += 1;
                if scanned > MAX_DIRECTORY_ENTRIES {
                    return Err(bad_path("Archive name search exceeded its file limit"));
                }
                if entry.kind == WorkspaceEntryKind::Directory {
                    pending.push(WorkspaceRelativePath::directory(&entry.path)?);
                }
                let lower = entry.path.to_lowercase();
                let mut chars = lower.chars();
                if query
                    .chars()
                    .all(|needle| chars.by_ref().any(|value| value == needle))
                {
                    let score =
                        if lower.contains(&query) { 1000 } else { 0 } - entry.path.len() as i64;
                    matches.push(WorkspaceFileSearchMatch {
                        path: entry.path,
                        name: entry.name,
                        kind: entry.kind,
                        score,
                    });
                    matches.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
                    matches.truncate(limit);
                }
            }
        }
        Ok(matches)
    }
}

impl ArchivedWorkspace {
    pub(super) async fn search_content(
        &self,
        request: SearchWorkspaceContentRequest,
    ) -> Result<WorkspaceContentSearchResult, WorkspaceFilesError> {
        validate_workspace_search_query(&request.query)?;
        let limit = usize::from(request.limit.unwrap_or(DEFAULT_CONTENT_RESULTS as u16))
            .min(MAX_CONTENT_RESULTS);
        let per_file = usize::from(
            request
                .per_file_limit
                .unwrap_or(DEFAULT_CONTENT_RESULTS_PER_FILE as u16),
        )
        .min(limit);
        let mut result = WorkspaceContentSearchResult {
            matches: vec![],
            truncated: false,
            indexing: false,
        };
        if limit == 0 || per_file == 0 {
            return Ok(result);
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut pending = vec![WorkspaceRelativePath::directory("")?];
        let sensitive = request.query.chars().any(char::is_uppercase);
        let needle = if sensitive {
            request.query.clone()
        } else {
            request.query.to_lowercase()
        };
        while let Some(directory) = pending.pop() {
            if Instant::now() >= deadline {
                result.truncated = true;
                break;
            }
            let (entries, partial) = self.entries(&directory, false).await?;
            result.truncated |= partial;
            for entry in entries {
                if Instant::now() >= deadline {
                    result.truncated = true;
                    return Ok(result);
                }
                if entry.kind == WorkspaceEntryKind::Directory {
                    pending.push(WorkspaceRelativePath::directory(&entry.path)?);
                    continue;
                }
                if entry.kind != WorkspaceEntryKind::File
                    || entry.size.is_some_and(|size| size > MAX_PREVIEW_FILE_BYTES)
                {
                    continue;
                }
                let object = format!("{}:{}", self.record.checkpoint, entry.path);
                let args = ["show", object.as_str()];
                let capture = crate::diff_sync::capture_git(
                    &self.record.repo,
                    &args,
                    MAX_PREVIEW_FILE_BYTES as usize,
                )
                .await
                .map_err(archive_error)?;
                if capture.truncated || capture.stdout.contains(&0) {
                    continue;
                }
                let Ok(text) = std::str::from_utf8(&capture.stdout) else {
                    continue;
                };
                let mut file_matches = 0;
                for (line_index, line) in text.lines().enumerate() {
                    let haystack = if sensitive {
                        std::borrow::Cow::Borrowed(line)
                    } else {
                        std::borrow::Cow::Owned(line.to_lowercase())
                    };
                    let spans = haystack
                        .match_indices(&needle)
                        .map(|(start, value)| {
                            if sensitive {
                                (start, start + value.len())
                            } else {
                                (
                                    source_offset(line, start, false),
                                    source_offset(line, start + value.len(), true),
                                )
                            }
                        })
                        .take(512)
                        .collect::<Vec<_>>();
                    let Some(&(first, _)) = spans.first() else {
                        continue;
                    };
                    if file_matches == per_file {
                        result.truncated = true;
                        break;
                    }
                    if result.matches.len() == limit {
                        result.truncated = true;
                        return Ok(result);
                    }
                    let indent = line.len() - line.trim_start().len();
                    let mut start = indent.max(first.saturating_sub(128));
                    while !line.is_char_boundary(start) {
                        start -= 1;
                    }
                    let mut end = (start + 512).min(line.len());
                    while !line.is_char_boundary(end) {
                        end -= 1;
                    }
                    let ranges = spans
                        .into_iter()
                        .filter(|(a, b)| *a < end && *b > start)
                        .map(|(a, b)| (a.saturating_sub(start) as u32, (b.min(end) - start) as u32))
                        .collect();
                    result.matches.push(WorkspaceContentMatch {
                        path: entry.path.clone(),
                        line: line_index as u64 + 1,
                        column: first as u64,
                        preview: line[start..end].to_owned(),
                        ranges,
                    });
                    file_matches += 1;
                }
            }
        }
        Ok(result)
    }
}
// Lowercasing may expand a scalar (for example dotted I). Map matched byte
// offsets back to the original UTF-8 line before exposing columns/ranges.
fn source_offset(line: &str, folded_offset: usize, end: bool) -> usize {
    let mut folded = 0;
    for (offset, value) in line.char_indices() {
        let size = value.to_lowercase().map(char::len_utf8).sum::<usize>();
        if folded_offset < folded + size {
            return if end && folded_offset > folded {
                offset + value.len_utf8()
            } else {
                offset
            };
        }
        folded += size;
    }
    line.len()
}
