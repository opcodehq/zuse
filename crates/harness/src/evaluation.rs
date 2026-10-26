//! Durable physical-request limits for controlled evaluations.
use crate::{Error, RequestBudget};
use async_trait::async_trait;
use std::{
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
};
/// All compared processes must share this ledger. Missing instrumentation means
/// a runner is ineligible for a capped evaluation, never an unmetered fallback.
#[derive(Clone)]
pub struct RequestLedger {
    path: PathBuf,
    limit: u64,
}
impl RequestLedger {
    pub fn new(path: impl Into<PathBuf>, limit: u64) -> Result<Self, Error> {
        if limit == 0 || limit > 300 {
            return Err(Error::paused(
                "Evaluation request limit must be between 1 and 300",
            ));
        }
        Ok(Self {
            path: path.into(),
            limit,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    fn reserve_sync(&self) -> Result<(), Error> {
        let parent = self.path.parent().ok_or(Error::Storage)?;
        std::fs::create_dir_all(parent).map_err(|_| Error::Storage)?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&self.path).map_err(|_| Error::Storage)?;
        file.lock().map_err(|_| Error::Storage)?;
        if file.metadata().map_err(|_| Error::Storage)?.len() > 32768 {
            return Err(Error::Storage);
        }
        let mut text = String::new();
        file.read_to_string(&mut text).map_err(|_| Error::Storage)?;
        if text.is_empty() {
            writeln!(file, "limit={}", self.limit)
                .and_then(|_| file.sync_all())
                .map_err(|_| Error::Storage)?;
            #[cfg(unix)]
            std::fs::File::open(parent)
                .and_then(|f| f.sync_all())
                .map_err(|_| Error::Storage)?;
        } else if !text.ends_with('\n')
            || text.lines().next() != Some(format!("limit={}", self.limit).as_str())
        {
            return Err(Error::Storage);
        }
        let mut count = 0;
        for line in text.lines().skip(1) {
            count += 1;
            if line.parse::<u64>().ok() != Some(count) {
                return Err(Error::Storage);
            }
        }
        if count >= self.limit {
            return Err(Error::Model {
                code: "evaluation_budget_exhausted".into(),
                status: None,
            });
        }
        file.seek(std::io::SeekFrom::End(0))
            .map_err(|_| Error::Storage)?;
        writeln!(file, "{}", count + 1)
            .and_then(|_| file.sync_all())
            .map_err(|_| Error::Storage)
    }
}
#[async_trait]
impl RequestBudget for RequestLedger {
    async fn reserve(&self) -> Result<(), Error> {
        let ledger = self.clone();
        tokio::task::spawn_blocking(move || ledger.reserve_sync())
            .await
            .map_err(|_| Error::Storage)?
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn shared_request_limit_survives_reopen_and_counts_concurrent_admissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("budget");
        let mut tasks = Vec::new();
        for _ in 0..20 {
            let ledger = RequestLedger::new(&path, 6).unwrap();
            tasks.push(tokio::spawn(async move { ledger.reserve().await.is_ok() }));
        }
        let mut accepted = 0;
        for task in tasks {
            accepted += usize::from(task.await.unwrap());
        }
        assert_eq!(accepted, 6);
        assert!(
            RequestLedger::new(&path, 6)
                .unwrap()
                .reserve()
                .await
                .is_err()
        );
        assert!(
            RequestLedger::new(&path, 7)
                .unwrap()
                .reserve()
                .await
                .is_err()
        );
        std::fs::write(&path, "limit=6\n1\n2").unwrap();
        assert!(
            RequestLedger::new(&path, 6)
                .unwrap()
                .reserve()
                .await
                .is_err()
        );
    }
}
