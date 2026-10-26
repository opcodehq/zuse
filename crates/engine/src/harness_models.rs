//! Model inventory, account affinity and shared native model transport for Zuse.
use crate::model_connections::ModelConnections;
use async_trait::async_trait;
use futures::{StreamExt, future::join_all};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex as AsyncMutex, broadcast};
use tokio_util::sync::CancellationToken;
use zuse_harness::{
    Error, ModelInput, ModelOutput,
    broker::{AccountBroker, BrokerHost, Connection as BrokerConnection},
    transport::{Credential as TransportCredential, ModelTransport, NativeTransport, Route},
};
use zuse_model_accounts::{Connection, Credential, Provider};
const INVENTORY_LIMIT: usize = 2 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessModel {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_compact_token_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_context_window_percent: Option<f64>,
}
#[derive(Debug, Clone)]
pub struct AccountNotice {
    pub root_id: String,
    pub connection_id: String,
    pub name: String,
    pub switched: bool,
}
#[async_trait]
trait Accounts: Send + Sync {
    fn lifetime(&self, _: &str) -> CancellationToken {
        CancellationToken::new()
    }
    async fn list(&self) -> Result<Vec<Connection>, Error>;
    async fn credential(&self, id: &str) -> Result<Credential, Error>;
}
#[async_trait]
impl Accounts for ModelConnections {
    fn lifetime(&self, id: &str) -> CancellationToken {
        self.lifetime(id)
    }
    async fn list(&self) -> Result<Vec<Connection>, Error> {
        Ok(self.status().await.connections)
    }
    async fn credential(&self, id: &str) -> Result<Credential, Error> {
        self.credential(id).await.map_err(account_error)
    }
}
struct AccountCredential {
    connection_id: String,
    provider: String,
    access_token: String,
    authorization_generation: u64,
    route: Option<Route>,
}
#[async_trait]
trait Wire: Send + Sync {
    async fn request(
        &self,
        input: ModelInput,
        credential: AccountCredential,
    ) -> Result<ModelOutput, Error>;
    async fn close(&self);
}
#[async_trait]
impl Wire for NativeTransport {
    async fn request(
        &self,
        input: ModelInput,
        credential: AccountCredential,
    ) -> Result<ModelOutput, Error> {
        let route = match credential.route {
            Some(route) => route,
            None => Route::subscription(&credential.provider)?,
        };
        ModelTransport::request(
            self,
            input,
            TransportCredential {
                connection_id: credential.connection_id,
                access_token: credential.access_token,
                authorization_generation: credential.authorization_generation,
                route,
            },
        )
        .await
    }
    async fn close(&self) {
        ModelTransport::close(self).await;
    }
}
fn account_error(error: zuse_model_accounts::Error) -> Error {
    match error {
        zuse_model_accounts::Error::Cancelled | zuse_model_accounts::Error::Changed => {
            Error::Interrupted
        }
        zuse_model_accounts::Error::Reauthorization | zuse_model_accounts::Error::Verification => {
            Error::Model {
                code: "model_reauthorization_required".into(),
                status: Some(401),
            }
        }
        zuse_model_accounts::Error::RefreshNotAvailable => {
            Error::paused("The provider refresh window has not opened. Retry after it opens.")
        }
        zuse_model_accounts::Error::Unavailable => failure("model_account_unavailable", Some(503)),
        _ => Error::AccountsUnavailable,
    }
}
fn failure(code: &str, status: Option<u16>) -> Error {
    Error::Model {
        code: code.into(),
        status,
    }
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
fn parse_model(id: &str) -> Result<(Provider, &str), Error> {
    let (provider, model) = id
        .split_once('/')
        .ok_or_else(|| failure("choose_a_zuse_model", None))?;
    if model.is_empty() || model.contains('/') || model.chars().any(char::is_whitespace) {
        return Err(failure("choose_a_zuse_model", None));
    }
    let provider = match provider {
        "chatgpt" => Provider::Chatgpt,
        "supergrok" => Provider::Supergrok,
        _ => return Err(failure("choose_a_zuse_model", None)),
    };
    Ok((provider, model))
}
fn inventory(provider: Provider, body: Value) -> Result<Vec<HarnessModel>, Error> {
    let rows = body[if provider == Provider::Chatgpt {
        "models"
    } else {
        "data"
    }]
    .as_array()
    .ok_or_else(|| failure("invalid_model_inventory", None))?;
    if rows.len() > 2048 {
        return Err(failure("invalid_model_inventory", None));
    }
    let mut models = BTreeMap::new();
    for row in rows {
        if provider == Provider::Chatgpt && row["visibility"] != "list" {
            continue;
        }
        let Some(id) = row[if provider == Provider::Chatgpt {
            "slug"
        } else {
            "id"
        }]
        .as_str()
        .filter(|s| !s.is_empty() && !s.contains('/') && !s.chars().any(char::is_whitespace)) else {
            continue;
        };
        if provider == Provider::Supergrok {
            let lower = id.to_ascii_lowercase();
            if ["image", "video", "imagine", "embed", "voice", "audio"]
                .iter()
                .any(|term| lower.contains(term))
            {
                continue;
            }
        }
        let positive = |key: &str| {
            row[key]
                .as_u64()
                .filter(|n| *n > 0 && *n <= 9007199254740991)
        };
        let qualified = format!("{}/{id}", provider.as_str());
        models.insert(
            qualified.clone(),
            HarnessModel {
                id: qualified,
                label: row["display_name"]
                    .as_str()
                    .unwrap_or(id)
                    .chars()
                    .take(512)
                    .collect(),
                context_window_tokens: positive("context_window")
                    .or_else(|| positive("max_context_window")),
                auto_compact_token_limit: positive("auto_compact_token_limit"),
                effective_context_window_percent: row["effective_context_window_percent"]
                    .as_f64()
                    .filter(|n| n.is_finite() && *n > 0. && *n <= 100.),
            },
        );
    }
    Ok(models.into_values().collect())
}
#[derive(Default)]
struct Cached {
    value: Option<Vec<HarnessModel>>,
    loaded: Option<Instant>,
    error_until: Option<Instant>,
}
struct CacheSlot {
    value: Arc<AsyncMutex<Cached>>,
    used: Instant,
}
struct Transport {
    api: crate::harness_api::ApiModels,
    accounts: Arc<dyn Accounts>,
    wire: Arc<dyn Wire>,
    http: reqwest::Client,
    endpoints: [String; 2],
    caches: Mutex<HashMap<String, CacheSlot>>,
    versions: Mutex<HashMap<String, (u64, bool)>>,
    names: Mutex<HashMap<String, String>>,
    broker: OnceLock<Weak<AccountBroker>>,
    notices: broadcast::Sender<AccountNotice>,
    stop: CancellationToken,
}
#[derive(Clone)]
pub struct HarnessModels {
    transport: Arc<Transport>,
    broker: Arc<AccountBroker>,
}
impl HarnessModels {
    pub fn new(accounts: ModelConnections, directory: PathBuf) -> Result<Self, Error> {
        let transport = NativeTransport::new()?;
        Self::configured_api(
            crate::harness_api::ApiModels::load(&directory.join("providers.json"))?,
            Arc::new(accounts),
            Arc::new(transport),
            [
                "https://api.openai.com/v1/models".into(),
                "https://api.x.ai/v1/models".into(),
            ],
        )
    }
    #[cfg(test)]
    fn configured(
        accounts: Arc<dyn Accounts>,
        wire: Arc<dyn Wire>,
        endpoints: [String; 2],
    ) -> Result<Self, Error> {
        Self::configured_api(Default::default(), accounts, wire, endpoints)
    }
    fn configured_api(
        api: crate::harness_api::ApiModels,
        accounts: Arc<dyn Accounts>,
        wire: Arc<dyn Wire>,
        endpoints: [String; 2],
    ) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| failure("model_inventory_unavailable", None))?;
        let transport = Arc::new(Transport {
            api,
            accounts,
            wire,
            http,
            endpoints,
            caches: Mutex::new(HashMap::new()),
            versions: Mutex::new(HashMap::new()),
            names: Mutex::new(HashMap::new()),
            broker: OnceLock::new(),
            notices: broadcast::channel(128).0,
            stop: CancellationToken::new(),
        });
        let broker = Arc::new(AccountBroker::new(transport.clone()));
        let _ = transport.broker.set(Arc::downgrade(&broker));
        Ok(Self { transport, broker })
    }
    pub fn subscribe(&self) -> broadcast::Receiver<AccountNotice> {
        self.transport.notices.subscribe()
    }
    pub fn remove_root(&self, id: &str) {
        self.broker.remove_root(id);
    }
    pub async fn models(
        &self,
        force: bool,
        cancel: &CancellationToken,
    ) -> Result<Vec<HarnessModel>, Error> {
        if force {
            lock(&self.transport.caches).clear();
        }
        let collect = async {
            let rows = self.transport.sync().await?;
            let candidates = rows.iter().filter(|c| c.authorized).collect::<Vec<_>>();
            if candidates.is_empty() {
                return Ok(self.transport.api.models());
            }
            let inventories = join_all(
                candidates
                    .into_iter()
                    .map(|connection| self.transport.models(connection)),
            )
            .await;
            let mut result = BTreeMap::new();
            let mut succeeded = false;
            for inventory in inventories {
                if let Ok(models) = inventory {
                    succeeded = true;
                    for model in models {
                        result.entry(model.id.clone()).or_insert(model);
                    }
                }
            }
            if !succeeded && self.transport.api.models().is_empty() {
                return Err(failure("model_inventory_unavailable", None));
            }
            for model in self.transport.api.models() {
                result.insert(model.id.clone(), model);
            }
            Ok(result.into_values().collect())
        };
        tokio::select! {biased;_=self.transport.stop.cancelled()=>Err(Error::Interrupted),_=cancel.cancelled()=>Err(Error::Interrupted),result=collect=>result}
    }
    pub async fn request(&self, input: ModelInput) -> Result<ModelOutput, Error> {
        tokio::select! {biased;_=self.transport.stop.cancelled()=>Err(Error::Interrupted),result=self.broker.request(input)=>result}
    }
    pub async fn shutdown(&self) {
        self.transport.stop.cancel();
        self.transport.wire.close().await;
        lock(&self.transport.caches).clear();
    }
}
impl Transport {
    async fn sync(&self) -> Result<Vec<Connection>, Error> {
        let rows = self.accounts.list().await?;
        let current = rows
            .iter()
            .map(|r| (r.id.clone(), (r.authorization_generation, r.authorized)))
            .collect::<HashMap<_, _>>();
        let changed = {
            let mut versions = lock(&self.versions);
            let changed = versions
                .iter()
                .filter_map(|(id, prior)| (current.get(id) != Some(prior)).then_some(id.clone()))
                .collect::<Vec<_>>();
            *versions = current;
            changed
        };
        for id in changed {
            lock(&self.caches).remove(&id);
            if let Some(broker) = self.broker.get().and_then(Weak::upgrade) {
                broker.forget(&id);
            }
        }
        *lock(&self.names) = rows
            .iter()
            .map(|r| {
                (
                    r.id.clone(),
                    if r.name.contains('@') {
                        match r.provider {
                            Provider::Chatgpt => "ChatGPT".into(),
                            Provider::Supergrok => "SuperGrok".into(),
                        }
                    } else {
                        r.name.clone()
                    },
                )
            })
            .collect();
        Ok(rows)
    }
    fn slot(&self, id: &str) -> Result<Arc<AsyncMutex<Cached>>, Error> {
        let mut caches = lock(&self.caches);
        if let Some(slot) = caches.get_mut(id) {
            slot.used = Instant::now();
            return Ok(slot.value.clone());
        }
        if caches.len() >= 128 {
            let oldest = caches
                .iter()
                .filter(|(_, slot)| Arc::strong_count(&slot.value) == 1)
                .min_by_key(|(_, slot)| slot.used)
                .map(|(id, _)| id.clone())
                .ok_or_else(|| failure("model_inventory_busy", None))?;
            caches.remove(&oldest);
        }
        let value = Arc::new(AsyncMutex::new(Cached::default()));
        caches.insert(
            id.into(),
            CacheSlot {
                value: value.clone(),
                used: Instant::now(),
            },
        );
        Ok(value)
    }
    async fn models(&self, connection: &Connection) -> Result<Vec<HarnessModel>, Error> {
        let slot = self.slot(&connection.id)?;
        let mut cache = slot.lock().await;
        if cache
            .loaded
            .is_some_and(|at| at.elapsed() < Duration::from_secs(15 * 60))
        {
            if let Some(models) = &cache.value {
                return Ok(models.clone());
            }
        }
        if cache.error_until.is_some_and(|at| at > Instant::now()) {
            return Err(failure("model_inventory_unavailable", None));
        }
        let lifetime = self.accounts.lifetime(&connection.id);
        let load = async {
            let credential = self.accounts.credential(&connection.id).await?;
            if credential.provider != connection.provider {
                return Err(failure("model_provider_mismatch", None));
            }
            let response = self
                .http
                .get(&self.endpoints[usize::from(connection.provider == Provider::Supergrok)])
                .bearer_auth(credential.access_token)
                .send()
                .await
                .map_err(|_| failure("model_inventory_unavailable", None))?;
            if !response.status().is_success() {
                return Err(failure(
                    "model_inventory_unavailable",
                    Some(response.status().as_u16()),
                ));
            }
            if response
                .content_length()
                .is_some_and(|n| n > INVENTORY_LIMIT as u64)
            {
                return Err(failure("invalid_model_inventory", None));
            }
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| failure("model_inventory_unavailable", None))?;
                if bytes.len() + chunk.len() > INVENTORY_LIMIT {
                    return Err(failure("invalid_model_inventory", None));
                }
                bytes.extend_from_slice(&chunk);
            }
            inventory(
                connection.provider,
                serde_json::from_slice(&bytes)
                    .map_err(|_| failure("invalid_model_inventory", None))?,
            )
        };
        let result = tokio::select! {biased;_=lifetime.cancelled()=>Err(Error::Interrupted),result=load=>result};
        match result {
            Ok(models) => {
                cache.loaded = Some(Instant::now());
                cache.error_until = None;
                cache.value = Some(models.clone());
                Ok(models)
            }
            Err(error) => {
                cache.error_until = Some(Instant::now() + Duration::from_secs(15));
                Err(error)
            }
        }
    }
}
#[async_trait]
impl BrokerHost for Transport {
    async fn connections(&self, input: &ModelInput) -> Result<Vec<BrokerConnection>, Error> {
        if input.model.starts_with("api/") {
            let id = self.api.connection(&input.model)?;
            return Ok(vec![BrokerConnection {
                id,
                authorized: true,
                preferred: true,
            }]);
        }
        let (provider, _) = parse_model(&input.model)?;
        let collect = async {
            let rows = self.sync().await?;
            Ok(join_all(
                rows.into_iter()
                    .filter(|r| r.authorized && r.provider == provider)
                    .map(|row| async move {
                        let authorized = self
                            .models(&row)
                            .await
                            .is_ok_and(|models| models.iter().any(|model| model.id == input.model));
                        BrokerConnection {
                            id: row.id,
                            authorized,
                            preferred: row.preferred,
                        }
                    }),
            )
            .await)
        };
        tokio::select! {biased;_=self.stop.cancelled()=>Err(Error::Interrupted),_=input.cancel.cancelled()=>Err(Error::Interrupted),result=collect=>result}
    }
    async fn request(
        &self,
        connection_id: &str,
        mut input: ModelInput,
    ) -> Result<ModelOutput, Error> {
        if input.model.starts_with("api/") {
            let (model, credential) = self.api.credential(&input.model)?;
            if credential.connection_id != connection_id {
                return Err(Error::AccountsUnavailable);
            }
            input.model = model;
            return self
                .wire
                .request(
                    input,
                    AccountCredential {
                        connection_id: credential.connection_id,
                        provider: "api".into(),
                        access_token: credential.access_token,
                        route: Some(credential.route),
                        authorization_generation: credential.authorization_generation,
                    },
                )
                .await;
        }
        let (provider, model) = parse_model(&input.model)?;
        let model = model.to_owned();
        let lifetime = self.accounts.lifetime(connection_id);
        let credential = tokio::select! {biased;_=lifetime.cancelled()=>return Err(Error::Interrupted),credential=self.accounts.credential(connection_id)=>credential?};
        if credential.provider != provider {
            return Err(failure("model_provider_mismatch", None));
        }
        if input.cancel.is_cancelled() || self.stop.is_cancelled() {
            return Err(Error::Interrupted);
        }
        input.model = model;
        let authorization_generation = lock(&self.versions)
            .get(connection_id)
            .map_or(0, |(generation, _)| *generation);
        tokio::select! {biased;_=lifetime.cancelled()=>Err(Error::Interrupted),result=self.wire
        .request(
            input,
            AccountCredential {
                connection_id: credential.connection_id,
                provider: provider.as_str().into(),
                access_token: credential.access_token,
                route: None,
                authorization_generation,
            },
        )=>result}
    }
    fn switched(&self, root_id: &str, previous: Option<&str>, next: &str) {
        let name = lock(&self.names)
            .get(next)
            .cloned()
            .unwrap_or_else(|| "Connected account".into());
        let _ = self.notices.send(AccountNotice {
            root_id: root_id.into(),
            connection_id: next.into(),
            name,
            switched: previous.is_some(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, http::HeaderMap, routing::get};
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
    use zuse_harness::{RequestBudget, Tools, Usage};
    use zuse_model_accounts::ConnectionState;
    struct Source {
        rows: Mutex<Vec<Connection>>,
        mismatch: AtomicBool,
        lifetime: CancellationToken,
    }
    #[async_trait]
    impl Accounts for Source {
        fn lifetime(&self, _: &str) -> CancellationToken {
            self.lifetime.clone()
        }
        async fn list(&self) -> Result<Vec<Connection>, Error> {
            Ok(lock(&self.rows).clone())
        }
        async fn credential(&self, id: &str) -> Result<Credential, Error> {
            let rows = lock(&self.rows);
            let row = rows
                .iter()
                .find(|row| row.id == id && row.authorized)
                .ok_or(Error::AccountsUnavailable)?;
            Ok(Credential {
                connection_id: id.into(),
                provider: if self.mismatch.load(Ordering::Acquire) {
                    Provider::Supergrok
                } else {
                    row.provider
                },
                access_token: format!("token-{id}"),
            })
        }
    }
    #[derive(Default)]
    struct MockWire {
        calls: Mutex<Vec<(String, String)>>,
        fail_a: AtomicU8,
        closed: AtomicBool,
        slow: AtomicBool,
        started: tokio::sync::Notify,
        active: AtomicUsize,
    }
    #[async_trait]
    impl Wire for MockWire {
        async fn request(
            &self,
            input: ModelInput,
            credential: AccountCredential,
        ) -> Result<ModelOutput, Error> {
            struct Active<'a>(&'a AtomicUsize);
            impl Drop for Active<'_> {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::SeqCst);
                }
            }
            self.active.fetch_add(1, Ordering::SeqCst);
            let _active = Active(&self.active);
            assert_eq!(
                credential.access_token,
                format!("token-{}", credential.connection_id)
            );
            assert!(!input.model.contains('/'));
            assert!(matches!(
                credential.provider.as_str(),
                "chatgpt" | "supergrok"
            ));
            lock(&self.calls).push((credential.connection_id.clone(), input.model));
            if self.slow.load(Ordering::Acquire) {
                self.started.notify_one();
                std::future::pending::<()>().await;
            }
            if credential.connection_id == "a" {
                match self.fail_a.load(Ordering::Acquire) {
                    1 => {
                        return Err(failure(
                            "subscription_sharing_usage_limit_exceeded",
                            Some(429),
                        ));
                    }
                    2 => return Err(failure("http_401", Some(401))),
                    _ => {}
                }
            }
            Ok(ModelOutput {
                messages: vec![],
                calls: vec![],
                text: credential.connection_id.clone(),
                usage: Usage {
                    connection_id: credential.connection_id,
                    ..Default::default()
                },
            })
        }
        async fn close(&self) {
            self.closed.store(true, Ordering::Release);
        }
    }
    #[derive(Default)]
    struct Budget(AtomicUsize);
    #[async_trait]
    impl RequestBudget for Budget {
        async fn reserve(&self) -> Result<(), Error> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    fn input(root: &str, agent: &str, model: &str, budget: Arc<Budget>) -> ModelInput {
        ModelInput {
            root_id: root.into(),
            agent_id: agent.into(),
            model: model.into(),
            reasoning: None,
            messages: Arc::new(vec![]),
            tools: Arc::new(Tools::new()),
            purpose: Default::default(),
            retry_index: 0,
            cancel: CancellationToken::new(),
            text: Arc::new(|_| {}),
            budget,
            report: Arc::new(|_| {}),
        }
    }
    fn row(id: &str, provider: Provider, preferred: bool) -> Connection {
        Connection {
            provider,
            storage: "local".into(),
            id: id.into(),
            client_id: "fixture-client".into(),
            name: if id == "a" {
                "personal@example.test".into()
            } else {
                id.into()
            },
            email: None,
            created_at: 1,
            preferred,
            authorized: true,
            status: ConnectionState::Connected,
            plan_notice_seen: true,
            authorization_generation: 1,
        }
    }
    struct Fixture {
        models: HarnessModels,
        source: Arc<Source>,
        wire: Arc<MockWire>,
        http_calls: Arc<AtomicUsize>,
        gate: Arc<AtomicBool>,
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        server: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.models.transport.stop.cancel();
            self.server.abort();
        }
    }
    impl Fixture {
        async fn new() -> Self {
            let source = Arc::new(Source {
                rows: Mutex::new(vec![
                    row("a", Provider::Chatgpt, true),
                    row("b", Provider::Chatgpt, false),
                    row("g", Provider::Supergrok, true),
                ]),
                mismatch: AtomicBool::new(false),
                lifetime: CancellationToken::new(),
            });
            let wire = Arc::new(MockWire::default());
            let http_calls = Arc::new(AtomicUsize::new(0));
            let calls = http_calls.clone();
            let gate = Arc::new(AtomicBool::new(false));
            let started = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let gated = gate.clone();
            let entered = started.clone();
            let resume = release.clone();
            let router=Router::new().route("/openai",get(move |headers:HeaderMap|{
                let calls=calls.clone();let gate=gated.clone();let started=entered.clone();let release=resume.clone();
                async move {
                    let token=headers.get("authorization").unwrap().to_str().unwrap();assert!(matches!(token,"Bearer token-a"|"Bearer token-b"|"Bearer token-limited"));
                    calls.fetch_add(1,Ordering::SeqCst);started.notify_one();if gate.load(Ordering::Acquire){release.notified().await;}
                    let mut rows=vec![json!({"slug":"beta","visibility":"list"}),json!({"slug":"hidden","visibility":"hide"}),json!({"slug":"bad/id","visibility":"list"})];
                    if token!="Bearer token-limited" {rows.push(json!({"slug":"alpha","display_name":"Alpha","visibility":"list","context_window":32000,"auto_compact_token_limit":24000,"effective_context_window_percent":95.5}));}
                    Json(json!({"models":rows}))
                }
            })).route("/xai",get({let calls=http_calls.clone();move |headers:HeaderMap|{let calls=calls.clone();async move {
                assert_eq!(headers.get("authorization").unwrap(),"Bearer token-g");calls.fetch_add(1,Ordering::SeqCst);
                Json(json!({"data":[{"id":"gamma","max_context_window":64000},{"id":"image-fixture"},{"id":"voice-fixture"}]}))
            }}}));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            });
            let models = HarnessModels::configured(
                source.clone(),
                wire.clone(),
                [
                    format!("http://{address}/openai"),
                    format!("http://{address}/xai"),
                ],
            )
            .unwrap();
            Self {
                models,
                source,
                wire,
                http_calls,
                gate,
                started,
                release,
                server,
            }
        }
    }
    #[tokio::test]
    async fn account_inventories_are_filtered_and_concurrent_discovery_shares_fetches() {
        let fixture = Fixture::new().await;
        let cancel = CancellationToken::new();
        let results = join_all((0..8).map(|_| fixture.models.models(false, &cancel))).await;
        for result in results {
            let models = result.unwrap();
            assert_eq!(
                models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
                ["chatgpt/alpha", "chatgpt/beta", "supergrok/gamma"]
            );
            let alpha = &models[0];
            assert_eq!(alpha.context_window_tokens, Some(32000));
            assert_eq!(alpha.auto_compact_token_limit, Some(24000));
            assert_eq!(alpha.effective_context_window_percent, Some(95.5));
        }
        assert_eq!(fixture.http_calls.load(Ordering::SeqCst), 3);
        fixture.models.models(true, &cancel).await.unwrap();
        assert_eq!(fixture.http_calls.load(Ordering::SeqCst), 6);
    }
    #[tokio::test]
    async fn account_fallback_preserves_child_affinity_and_reconnect_clears_old_health() {
        let fixture = Fixture::new().await;
        let budget = Arc::new(Budget::default());
        let mut notices = fixture.models.subscribe();
        lock(&fixture.source.rows).insert(0, row("limited", Provider::Chatgpt, true));
        fixture.wire.fail_a.store(1, Ordering::Release);
        let first = fixture
            .models
            .request(input("root", "root", "chatgpt/alpha", budget.clone()))
            .await
            .unwrap();
        assert_eq!(first.text, "b");
        assert_eq!(budget.0.load(Ordering::SeqCst), 1);
        assert_eq!(
            fixture
                .models
                .request(input("root", "child", "chatgpt/alpha", budget.clone()))
                .await
                .unwrap()
                .text,
            "b"
        );
        assert!(
            lock(&fixture.wire.calls)
                .iter()
                .all(|(id, _)| id != "limited")
        );
        let initial = notices.recv().await.unwrap();
        assert_eq!(initial.name, "ChatGPT");
        assert!(!initial.switched);
        assert!(notices.recv().await.unwrap().switched);
        fixture.wire.fail_a.store(0, Ordering::Release);
        lock(&fixture.source.rows)
            .iter_mut()
            .find(|r| r.id == "a")
            .unwrap()
            .authorization_generation += 1;
        assert_eq!(
            fixture
                .models
                .request(input(
                    "new-root",
                    "new-root",
                    "chatgpt/alpha",
                    budget.clone()
                ))
                .await
                .unwrap()
                .text,
            "a"
        );
        assert_eq!(
            fixture
                .models
                .request(input("grok-root", "grok-root", "supergrok/gamma", budget))
                .await
                .unwrap()
                .text,
            "g"
        );
        fixture.models.remove_root("root");
    }
    #[tokio::test]
    async fn authentication_failure_pauses_then_a_new_authorization_generation_recovers() {
        let fixture = Fixture::new().await;
        let budget = Arc::new(Budget::default());
        fixture.wire.fail_a.store(2, Ordering::Release);
        assert!(matches!(
            fixture
                .models
                .request(input("root", "root", "chatgpt/alpha", budget.clone()))
                .await,
            Err(Error::Model {
                status: Some(401),
                ..
            })
        ));
        assert_eq!(
            *lock(&fixture.wire.calls),
            vec![("a".into(), "alpha".into())]
        );
        fixture.wire.fail_a.store(0, Ordering::Release);
        lock(&fixture.source.rows)[0].authorization_generation += 1;
        assert_eq!(
            fixture
                .models
                .request(input("root", "root", "chatgpt/alpha", budget))
                .await
                .unwrap()
                .text,
            "a"
        );
    }
    #[tokio::test]
    async fn a_provider_mismatch_cannot_send_credentials_to_another_model_endpoint() {
        let fixture = Fixture::new().await;
        fixture.source.mismatch.store(true, Ordering::Release);
        let connection = lock(&fixture.source.rows)[0].clone();
        assert!(fixture.models.transport.models(&connection).await.is_err());
        assert!(
            fixture
                .models
                .transport
                .request(
                    "a",
                    input("root", "root", "chatgpt/alpha", Arc::new(Budget::default()))
                )
                .await
                .is_err()
        );
        assert_eq!(fixture.http_calls.load(Ordering::SeqCst), 0);
        assert!(lock(&fixture.wire.calls).is_empty());
    }
    #[tokio::test]
    async fn cancelled_discovery_does_not_poison_inventory_and_shutdown_retires_transport() {
        let fixture = Fixture::new().await;
        lock(&fixture.source.rows).truncate(1);
        fixture.gate.store(true, Ordering::Release);
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let models = fixture.models.clone();
        let request = tokio::spawn(async move { models.models(false, &token).await });
        tokio::time::timeout(Duration::from_secs(3), fixture.started.notified())
            .await
            .unwrap();
        cancel.cancel();
        assert!(matches!(request.await.unwrap(), Err(Error::Interrupted)));
        fixture.gate.store(false, Ordering::Release);
        fixture.release.notify_waiters();
        assert!(
            !fixture
                .models
                .models(false, &CancellationToken::new())
                .await
                .unwrap()
                .is_empty()
        );
        fixture.models.shutdown().await;
        assert!(fixture.wire.closed.load(Ordering::Acquire));
        assert!(matches!(
            fixture
                .models
                .request(input(
                    "root",
                    "root",
                    "chatgpt/alpha",
                    Arc::new(Budget::default())
                ))
                .await,
            Err(Error::Interrupted)
        ));
    }
    #[tokio::test]
    async fn identity_retirement_cancels_an_active_model_stream() {
        let fixture = Fixture::new().await;
        fixture.wire.slow.store(true, Ordering::Release);
        let models = fixture.models.clone();
        let pending = tokio::spawn(async move {
            models
                .request(input(
                    "root",
                    "root",
                    "chatgpt/alpha",
                    Arc::new(Budget::default()),
                ))
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), fixture.wire.started.notified())
            .await
            .unwrap();
        fixture.source.lifetime.cancel();
        assert!(matches!(pending.await.unwrap(), Err(Error::Interrupted)));
        assert_eq!(fixture.wire.active.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn unqualified_and_unentitled_models_never_reach_the_sdk() {
        let fixture = Fixture::new().await;
        let budget = Arc::new(Budget::default());
        for model in [
            "alpha",
            "other/alpha",
            "chatgpt/alpha/extra",
            "chatgpt/has space",
        ] {
            assert!(
                fixture
                    .models
                    .request(input("root", "root", model, budget.clone()))
                    .await
                    .is_err()
            );
        }
        assert_eq!(fixture.http_calls.load(Ordering::SeqCst), 0);
        assert!(matches!(
            fixture
                .models
                .request(input("root", "root", "chatgpt/not-entitled", budget))
                .await,
            Err(Error::AccountsUnavailable)
        ));
        assert!(lock(&fixture.wire.calls).is_empty());
    }
}
