//! Profile-scoped, stale-while-refresh catalogue loading. Only public rate data is fetched.
use super::Catalogue;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::{
    io::AsyncWriteExt,
    sync::{Mutex, watch},
};

const URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_BYTES: usize = 16 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct Pricing {
    fallback: Arc<Catalogue>,
    remote: Option<Arc<Remote>>,
}
struct Remote {
    path: PathBuf,
    url: String,
    client: reqwest::Client,
    state: Mutex<State>,
}
struct State {
    loaded: bool,
    table: Arc<Catalogue>,
    fresh_at: Option<SystemTime>,
    flight: Option<watch::Receiver<Option<Arc<Catalogue>>>>,
}
impl Pricing {
    /// Deterministic offline pricing, also used by isolated import/report consumers.
    pub fn fixed(table: Arc<Catalogue>) -> Self {
        Self {
            fallback: table,
            remote: None,
        }
    }
    pub fn bundled() -> Self {
        Self::fixed(Catalogue::bundled())
    }
    pub fn cached(directory: &Path) -> Self {
        Self::remote(directory, URL, TIMEOUT)
    }
    fn remote(directory: &Path, url: &str, timeout: Duration) -> Self {
        let fallback = Catalogue::bundled();
        Self {
            fallback: fallback.clone(),
            remote: Some(Arc::new(Remote {
                path: directory.join("model-prices.json"),
                url: url.into(),
                client: reqwest::Client::builder()
                    .timeout(timeout)
                    .build()
                    .expect("pricing HTTP client"),
                state: Mutex::new(State {
                    loaded: false,
                    table: fallback,
                    fresh_at: None,
                    flight: None,
                }),
            })),
        }
    }
    /// Normal opens return an available snapshot; forced reports wait for a stale refresh.
    /// Fresh prices still have a 24-hour lifetime, independent of the report's one-minute cache.
    pub async fn load(&self, force: bool) -> Arc<Catalogue> {
        let Some(remote) = self.remote.clone() else {
            return self.fallback.clone();
        };
        let mut state = remote.state.lock().await;
        if !state.loaded {
            if let Some((table, modified)) = read_cache(&remote.path).await {
                state.table = table;
                state.fresh_at = Some(modified);
            }
            state.loaded = true;
        }
        if state
            .fresh_at
            .is_some_and(|at| at.elapsed().unwrap_or_default() < TTL)
        {
            return state.table.clone();
        }
        let available = state.table.clone();
        let mut done = if let Some(flight) = &state.flight {
            flight.clone()
        } else {
            let (sender, receiver) = watch::channel(None);
            state.flight = Some(receiver.clone());
            let remote = remote.clone();
            tokio::spawn(async move {
                let refreshed = remote.fetch().await;
                let mut state = remote.state.lock().await;
                if let Some(table) = refreshed {
                    state.table = table;
                    state.fresh_at = Some(SystemTime::now());
                }
                state.flight = None;
                let _ = sender.send(Some(state.table.clone()));
            });
            receiver
        };
        drop(state);
        if !force {
            return available;
        }
        loop {
            if let Some(table) = done.borrow().clone() {
                return table;
            }
            if done.changed().await.is_err() {
                return available;
            }
        }
    }
}
async fn read_cache(path: &Path) -> Option<(Arc<Catalogue>, SystemTime)> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    if metadata.len() > MAX_BYTES as u64 {
        return None;
    }
    let modified = metadata.modified().ok()?;
    let bytes = tokio::fs::read(path).await.ok()?;
    let table = tokio::task::spawn_blocking(move || Catalogue::parse(&bytes))
        .await
        .ok()?
        .ok()?;
    Some((Arc::new(table), modified))
}
impl Remote {
    async fn fetch(&self) -> Option<Arc<Catalogue>> {
        let mut response = self
            .client
            .get(&self.url)
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?;
        if response
            .content_length()
            .is_some_and(|size| size > MAX_BYTES as u64)
        {
            return None;
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
                return None;
            }
            bytes.extend_from_slice(&chunk);
        }
        let (table, bytes) = tokio::task::spawn_blocking(move || {
            Catalogue::parse(&bytes).map(|table| (Arc::new(table), bytes))
        })
        .await
        .ok()?
        .ok()?;
        // A storage failure must not discard a successfully downloaded catalogue.
        let _ = write_cache(&self.path, &bytes).await;
        Some(table)
    }
}
async fn write_cache(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = path.parent().expect("pricing cache has a directory");
    tokio::fs::create_dir_all(directory).await?;
    let temp = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .await?;
        file.write_all(bytes).await?;
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&temp, path).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
    }
    result
}

#[cfg(test)]
mod tests;
