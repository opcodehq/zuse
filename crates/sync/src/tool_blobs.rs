//! Profile-local tool payloads, separate from compact transcript snapshots.
use super::*;
const MAX_BLOB_BYTES: usize = 32 * 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolBlobPage {
    pub text: String,
    pub next_offset: usize,
    pub total_bytes: usize,
    pub done: bool,
}
fn invalid(message: &str) -> StoreError {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message).into()
}
impl DocsStore {
    /// Both output and diff commit before their references enter the transcript.
    pub fn save_tool_blobs(
        &self,
        doc_id: &str,
        blobs: &[(String, String)],
    ) -> Result<(), StoreError> {
        if blobs.iter().any(|(_, text)| text.len() > MAX_BLOB_BYTES) {
            return Err(invalid("tool output exceeds the local storage limit"));
        }
        store_blocking(|| {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            for (key, text) in blobs {
                tx.execute("INSERT INTO tool_blobs(blob_ref,doc_id,bytes) VALUES(?1,?2,?3) ON CONFLICT(blob_ref) DO UPDATE SET bytes=excluded.bytes WHERE tool_blobs.doc_id=excluded.doc_id",params![key,doc_id,text.as_bytes()])?;
            }
            tx.commit()?;
            Ok(())
        })
    }
    /// Page bytes without loading the entire result. Cursors are UTF-8 boundaries.
    pub fn read_tool_blob(
        &self,
        blob_ref: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Option<ToolBlobPage>, StoreError> {
        if !(4..=MAX_BLOB_BYTES).contains(&limit) {
            return Err(invalid("invalid tool output page size"));
        }
        store_blocking(|| {
            let conn = self.conn();
            let total: Option<usize> = conn
                .query_row(
                    "SELECT length(bytes) FROM tool_blobs WHERE blob_ref=?1",
                    params![blob_ref],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(total) = total else {
                return Ok(None);
            };
            if offset > total {
                return Err(invalid("tool output cursor exceeds its size"));
            }
            let bytes: Vec<u8> = conn.query_row(
                "SELECT substr(bytes,?2,?3) FROM tool_blobs WHERE blob_ref=?1",
                params![blob_ref, offset + 1, limit],
                |row| row.get(0),
            )?;
            let valid = match std::str::from_utf8(&bytes) {
                Ok(text) => text.len(),
                Err(error) if error.error_len().is_none() => error.valid_up_to(),
                Err(_) => return Err(invalid("tool output cursor is not a UTF-8 boundary")),
            };
            let text = std::str::from_utf8(&bytes[..valid])
                .map_err(|_| invalid("invalid tool output encoding"))?
                .to_owned();
            let next_offset = offset + valid;
            Ok(Some(ToolBlobPage {
                text,
                next_offset,
                total_bytes: total,
                done: next_offset == total,
            }))
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persisted_unicode_output_pages_exactly_and_deletes_with_its_chat() {
        let dir = tempfile::tempdir().unwrap();
        let key = "chat/part";
        let text = "ab🙂café\n".repeat(40);
        {
            let store = DocsStore::open(dir.path()).unwrap();
            store
                .save_tool_blobs("chat", &[(key.into(), text.clone())])
                .unwrap();
        }
        let store = DocsStore::open(dir.path()).unwrap();
        let mut actual = String::new();
        let mut offset = 0;
        loop {
            let page = store.read_tool_blob(key, offset, 7).unwrap().unwrap();
            assert!(page.next_offset > offset);
            actual.push_str(&page.text);
            offset = page.next_offset;
            if page.done {
                break;
            }
        }
        assert_eq!(actual, text);
        assert!(store.read_tool_blob(key, 3, 7).is_err());
        assert!(store.read_tool_blob(key, text.len() + 1, 7).is_err());
        store.delete_snapshot("chat").unwrap();
        assert!(store.read_tool_blob(key, 0, 7).unwrap().is_none());
    }
    #[test]
    fn profile_isolation_and_oversize_batches_cannot_partially_commit() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let first = DocsStore::open(a.path()).unwrap();
        let second = DocsStore::open(b.path()).unwrap();
        first
            .save_tool_blobs("a", &[("a/key".into(), "private".into())])
            .unwrap();
        assert!(second.read_tool_blob("a/key", 0, 8).unwrap().is_none());
        assert!(
            first
                .save_tool_blobs(
                    "a",
                    &[
                        ("a/small".into(), "small".into()),
                        ("a/large".into(), "x".repeat(MAX_BLOB_BYTES + 1))
                    ]
                )
                .is_err()
        );
        assert!(first.read_tool_blob("a/small", 0, 8).unwrap().is_none());
    }
    #[test]
    fn storage_failure_rolls_back_the_whole_result() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        store.conn().execute_batch("CREATE TRIGGER reject_diff BEFORE INSERT ON tool_blobs WHEN NEW.blob_ref='chat/diff' BEGIN SELECT RAISE(FAIL,'disk fixture'); END;").unwrap();
        assert!(
            store
                .save_tool_blobs(
                    "chat",
                    &[
                        ("chat/output".into(), "output".into()),
                        ("chat/diff".into(), "diff".into())
                    ]
                )
                .is_err()
        );
        assert!(store.read_tool_blob("chat/output", 0, 8).unwrap().is_none());
    }
}
