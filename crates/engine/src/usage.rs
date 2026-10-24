//! Account/profile-bound recorded usage; imports run on blocking workers and never start agents.
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use zuse_protocol::{
    Space,
    usage::{OVERVIEW, Request, SESSIONS},
};
use zuse_rpc::{RpcError, RpcReply};
use zuse_usage::{
    Data,
    aggregate::Selection,
    import::{Format, Importer, Source},
    pricing::Pricing,
};

#[derive(Clone)]
pub struct Usage {
    inner: Arc<Mutex<Cache>>,
    sources: Arc<Vec<Source>>,
    ledger: Option<Arc<PathBuf>>,
    pricing: Pricing,
}
struct Cache {
    importer: Importer,
    data: Option<Arc<Data>>,
    refreshed: Option<Instant>,
    ledger: Option<zuse_usage::ledger::Ledger>,
}
impl Usage {
    pub fn new(sources: Vec<Source>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Cache {
                importer: Importer::default(),
                data: None,
                refreshed: None,
                ledger: None,
            })),
            sources: Arc::new(sources),
            ledger: None,
            pricing: Pricing::bundled(),
        }
    }
    pub fn persistent(sources: Vec<Source>, ledger: PathBuf) -> Self {
        let mut usage = Self::new(sources);
        usage.ledger = Some(Arc::new(ledger));
        usage
    }
    pub fn with_pricing(mut self, pricing: Pricing) -> Self {
        self.pricing = pricing;
        self
    }
    pub fn local(journals: PathBuf) -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        let roots = |name: &str, fallback: Vec<PathBuf>| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(|value| std::env::split_paths(&value).collect())
                .unwrap_or(fallback)
        };
        let mut sources = vec![
            Source::new(
                "zuse",
                "Zuse (Beta)",
                Format::Native,
                vec![journals.clone()],
            ),
            Source::new(
                "claude",
                "Claude Code",
                Format::Claude,
                roots(
                    "CLAUDE_CONFIG_DIR",
                    vec![config.join("claude"), home.join(".claude")],
                )
                .into_iter()
                .map(|p| p.join("projects"))
                .collect(),
            ),
            Source::new(
                "codex",
                "Codex",
                Format::Codex,
                roots("CODEX_HOME", vec![home.join(".codex")])
                    .into_iter()
                    .map(|p| p.join("sessions"))
                    .collect(),
            ),
            Source::new(
                "opencode",
                "OpenCode",
                Format::OpenCode,
                roots("OPENCODE_DATA_DIR", vec![data.join("opencode")]),
            ),
            Source::new(
                "amp",
                "Amp",
                Format::Amp,
                roots("AMP_DATA_DIR", vec![data.join("amp")])
                    .into_iter()
                    .map(|p| p.join("threads"))
                    .collect(),
            ),
            Source::new(
                "pi",
                "pi-agent",
                Format::Pi,
                roots("PI_AGENT_DIR", vec![home.join(".pi/agent/sessions")]),
            ),
            Source::new(
                "grok",
                "Grok",
                Format::Grok,
                roots("GROK_DATA_DIR", vec![home.join(".grok/sessions")]),
            ),
        ];
        let legacy = std::env::var_os("ZUSE_USER_DATA_DIR")
            .map(PathBuf::from)
            .map(|p| vec![p.join("zuse.sqlite")])
            .unwrap_or_else(|| {
                if cfg!(target_os = "macos") {
                    vec![home.join("Library/Application Support/Zuse (Beta)/zuse.sqlite")]
                } else {
                    vec![data.join("zuse/zuse.sqlite")]
                }
            });
        sources.push(Source::new("zuse", "Zuse (Beta)", Format::Legacy, legacy));
        let profile = journals.parent().unwrap_or(&journals);
        Self::persistent(sources, profile.join("usage.sqlite"))
            .with_pricing(Pricing::cached(&profile.join("usage-pricing")))
    }
    async fn data(&self, force: bool) -> Result<Arc<Data>, RpcError> {
        let mut cache = self.inner.clone().lock_owned().await;
        if !force
            && cache
                .refreshed
                .is_some_and(|at| at.elapsed() < Duration::from_secs(60))
        {
            if let Some(data) = cache.data.clone() {
                return Ok(data);
            }
        }
        let sources = self.sources.clone();
        let ledger_path = self.ledger.clone();
        let prices = self.pricing.load(force).await;
        // Move the owned guard into the worker: disconnecting one caller cannot
        // start a second scan or lose the source cache halfway through a read.
        tokio::task::spawn_blocking(move || {
            if force {
                cache.importer.invalidate();
            }
            let mut data = cache.importer.scan(&sources);
            if let Some(path) = ledger_path {
                if cache.ledger.is_none() {
                    cache.ledger = Some(zuse_usage::ledger::Ledger::open(&path).map_err(|_| RpcError::Failed("Saved usage history could not be opened. Check profile storage and retry.".into()))?);
                }
                data = cache.ledger.as_mut().unwrap().merge(data).map_err(|_| RpcError::Failed("Saved usage history could not be updated. Check profile storage and retry.".into()))?;
            }
            // The ledger and import cache retain logged costs only, so a later
            // catalogue can reprice history even after its source file is pruned.
            prices.apply(&mut data);
            let data = Arc::new(data);
            cache.data = Some(data.clone());
            cache.refreshed = Some(Instant::now());
            Ok(data)
        })
        .await
        .map_err(|_| {
            RpcError::Failed("Local usage history could not be read. Retry to scan again.".into())
        })?
    }
    pub async fn handle(
        &self,
        method: &str,
        request: Request,
        spaces: &[Space],
        device_id: &str,
    ) -> Result<RpcReply, RpcError> {
        if !matches!(method, OVERVIEW | SESSIONS) {
            return Err(RpcError::UnknownMethod(method.into()));
        }
        let project_paths = match request.project_id.as_deref() {
            None => None,
            Some(id) => {
                let selected = spaces.iter().find(|space| space.id == id).ok_or_else(|| {
                    RpcError::BadParams("Usage project is unavailable on this host.".into())
                })?;
                let key = zuse_protocol::view::project_key(selected);
                Some(
                    spaces
                        .iter()
                        .filter(|space| {
                            space.device_id == device_id
                                && zuse_protocol::view::project_key(space) == key
                        })
                        .map(|space| space.path.clone())
                        .collect(),
                )
            }
        };
        let selection = Selection {
            since: request.since,
            until: request.until,
            project_paths,
            timezone: request.timezone.clone(),
        };
        selection.validate().map_err(RpcError::BadParams)?;
        let data = self.data(request.force_refresh).await?;
        if method == OVERVIEW {
            RpcReply::value(
                &data
                    .overview(&selection, chrono::Utc::now())
                    .map_err(RpcError::BadParams)?,
            )
        } else {
            RpcReply::value(
                &data
                    .sessions(&selection, &request)
                    .map_err(RpcError::BadParams)?,
            )
        }
    }
}
