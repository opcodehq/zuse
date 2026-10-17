use crate::{vault::Vault, *};
use aes_gcm::aead::{OsRng, rand_core::RngCore};
use axum::{
    Router,
    extract::{OriginalUri, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::{FutureExt, StreamExt, future::BoxFuture};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, jwk::JwkSet};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::{
    net::TcpListener,
    sync::{Mutex as AsyncMutex, oneshot, watch},
};
use tokio_util::sync::CancellationToken;

const XAI_CLIENT: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const MAX_RESPONSE: usize = 1024 * 1024;
type Key = (Provider, String);
struct Active {
    generation: String,
    cancel: CancellationToken,
    events: watch::Sender<Option<SignInEvent>>,
}
struct Inner {
    vault: Vault,
    http: reqwest::Client,
    active: Mutex<HashMap<Key, Active>>,
    stop: CancellationToken,
    keys: [AsyncMutex<Option<(Instant, Arc<JwkSet>)>>; 2],
    issuers: [String; 2],
    xai_client: String,
}
#[derive(Clone)]
pub struct ModelAccounts(Arc<Inner>);
pub struct Login {
    pub id: String,
    pub event: SignInEvent,
    pub events: watch::Receiver<Option<SignInEvent>>,
}
#[derive(serde::Deserialize)]
struct Tokens {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    scope: Option<String>,
    token_type: Option<String>,
    expires_in: Option<u64>,
    earliest_refresh_at: Option<Value>,
}
#[derive(serde::Deserialize)]
struct Device {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    #[serde(default = "default_interval")]
    interval: u64,
}
fn default_interval() -> u64 {
    5
}
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
fn random() -> Result<String, Error> {
    let mut bytes = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| Error::Unavailable)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn index(p: Provider) -> usize {
    usize::from(p == Provider::Supergrok)
}
fn endpoint(provider: Provider, path: &str) -> String {
    format!("{}{path}", provider.issuer())
}
fn early(value: Option<&Value>) -> Result<Option<u64>, Error> {
    let Some(value) = value else {
        return Ok(None);
    };
    let number = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()));
    if let Some(number) = number {
        let at = if number < 1e12 {
            number * 1000.
        } else {
            number
        };
        return if at.is_finite() && at >= 0. && at <= u64::MAX as f64 {
            Ok(Some(at as u64))
        } else {
            Err(Error::Verification)
        };
    }
    value
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .and_then(|v| v.timestamp_millis().try_into().ok())
        .map(Some)
        .ok_or(Error::Verification)
}
impl ModelAccounts {
    pub fn new(vault: Vault) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| Error::Unavailable)?;
        Ok(Self(Arc::new(Inner {
            vault,
            http,
            active: Mutex::new(HashMap::new()),
            stop: CancellationToken::new(),
            keys: [AsyncMutex::new(None), AsyncMutex::new(None)],
            issuers: [
                Provider::Chatgpt.issuer().into(),
                Provider::Supergrok.issuer().into(),
            ],
            xai_client: std::env::var("ZUSE_XAI_OAUTH_CLIENT_ID")
                .unwrap_or_else(|_| XAI_CLIENT.into()),
        })))
    }
    pub fn vault(&self) -> &Vault {
        &self.0.vault
    }
    fn issuer(&self, provider: Provider) -> &str {
        &self.0.issuers[index(provider)]
    }
    pub fn retire(&self) {
        self.0.stop.cancel();
        for login in lock(&self.0.active).values() {
            login.cancel.cancel();
        }
    }
    pub fn cancel(&self, provider: Provider, id: &str) {
        if let Some(login) = lock(&self.0.active).get(&(provider, id.into())) {
            login.cancel.cancel();
        }
    }
    pub fn watch_login(
        &self,
        provider: Provider,
        id: &str,
    ) -> Option<watch::Receiver<Option<SignInEvent>>> {
        lock(&self.0.active)
            .get(&(provider, id.into()))
            .map(|s| s.events.subscribe())
    }
    pub async fn list(&self) -> Result<Vec<Connection>, Error> {
        let mut connections = Vec::new();
        for provider in [Provider::Chatgpt, Provider::Supergrok] {
            let lease = self.0.vault.acquire(provider).await?;
            let active = lease.registrations().await?;
            for pending in lease.pending_all().await? {
                if !active.iter().any(|r| r.id == pending.id) {
                    connections.push(Connection {
                        provider,
                        storage: self.0.vault.storage().into(),
                        id: pending.id,
                        client_id: pending.client_id,
                        name: "ChatGPT".into(),
                        email: None,
                        created_at: pending.created_at,
                        preferred: false,
                        authorized: false,
                        status: ConnectionState::Pending,
                        plan_notice_seen: false,
                    });
                }
            }
            connections.extend(
                active
                    .iter()
                    .map(|r| r.metadata(provider, self.0.vault.storage())),
            );
            lease.finish().await;
        }
        connections.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        Ok(connections)
    }
    pub async fn connect(
        &self,
        provider: Provider,
        reconnect: Option<String>,
    ) -> Result<Login, Error> {
        if self.0.stop.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let id = reconnect.clone().unwrap_or_else(|| {
            if provider == Provider::Supergrok {
                format!("supergrok:{}", uuid::Uuid::new_v4())
            } else {
                uuid::Uuid::new_v4().to_string()
            }
        });
        let lease = self.0.vault.acquire(provider).await?;
        let existing = lease.registration(&id).await?;
        let pending = lease.pending(&id).await?;
        if reconnect.is_some() && existing.is_none() && pending.is_none() {
            return Err(Error::Unknown);
        }
        lease.finish().await;
        let cancel = self.0.stop.child_token();
        let generation = uuid::Uuid::new_v4().to_string();
        let (events, receiver) = watch::channel(None);
        let key = (provider, id.clone());
        {
            let mut active = lock(&self.0.active);
            if active.len() >= 32 && !active.contains_key(&key) {
                return Err(Error::Busy);
            }
            if let Some(old) = active.insert(
                key.clone(),
                Active {
                    generation: generation.clone(),
                    cancel: cancel.clone(),
                    events: events.clone(),
                },
            ) {
                old.cancel.cancel();
            }
        }
        let mut start_guard = StartGuard {
            manager: self.clone(),
            key: key.clone(),
            generation: generation.clone(),
            cancel: cancel.clone(),
            armed: true,
        };
        let account_cancel = self.0.vault.cancellation();
        let started = tokio::select! {biased;_=cancel.cancelled()=>Err(Error::Cancelled),_=account_cancel.cancelled()=>Err(Error::Changed),result=async {
            match provider {
                Provider::Chatgpt=>self.chatgpt(id.clone(),existing,pending,cancel.clone()).await,
                Provider::Supergrok=>self.supergrok(id.clone(),existing,cancel.clone()).await,
            }
        }=>result};
        let (event, flow) = match started {
            Ok(flow) => flow,
            Err(error) => {
                cancel.cancel();
                self.remove(&key, &generation);
                return Err(error);
            }
        };
        events.send_replace(Some(event.clone()));
        let manager = self.clone();
        tokio::spawn(async move {
            let result = tokio::select! {biased;_=cancel.cancelled()=>Err(Error::Cancelled),_=account_cancel.cancelled()=>Err(Error::Changed),_=events.closed()=>Err(Error::Cancelled),_=tokio::time::sleep(Duration::from_secs(3600))=>Err(Error::Expired),result=flow=>result};
            match result {
                Ok(connection) => {
                    events.send_replace(Some(SignInEvent::Connected { connection }));
                }
                Err(error) => {
                    events.send_replace(Some(SignInEvent::Error {
                        message: error.to_string(),
                    }));
                }
            }
            cancel.cancel();
            manager.remove(&key, &generation);
        });
        start_guard.armed = false;
        Ok(Login {
            id,
            event,
            events: receiver,
        })
    }
    fn remove(&self, key: &Key, generation: &str) {
        let mut active = lock(&self.0.active);
        if active.get(key).is_some_and(|a| a.generation == generation) {
            active.remove(key);
        }
    }
    async fn chatgpt(
        &self,
        id: String,
        existing: Option<Registration>,
        pending: Option<PendingRegistration>,
        cancel: CancellationToken,
    ) -> Result<(SignInEvent, BoxFuture<'static, Result<Connection, Error>>), Error> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| Error::Unavailable)?;
        let port = listener
            .local_addr()
            .map_err(|_| Error::Unavailable)?
            .port();
        let redirect = format!("http://127.0.0.1:{port}/auth/callback");
        let state = random()?;
        let verifier = random()?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let nonce = random()?;
        let saved_client = existing
            .as_ref()
            .map(|r| r.client_id.clone())
            .or_else(|| pending.as_ref().map(|p| p.client_id.clone()));
        let mut url = reqwest::Url::parse(&format!(
            "{}/api/accounts/authorize",
            self.issuer(Provider::Chatgpt)
        ))
        .map_err(|_| Error::Unavailable)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair(
                "client_id",
                saved_client.as_deref().unwrap_or("dynamic_agent_client"),
            );
            if saved_client.is_none() {
                query.append_pair("agent_name_hint", "Zuse");
            } else if let Some(email) = existing.as_ref().and_then(|r| r.email.as_deref()) {
                query.append_pair("login_hint", email);
            }
            query
                .append_pair("ext_agent_host_id", self.0.vault.host_id())
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", &redirect)
                .append_pair("scope", Provider::Chatgpt.scope())
                .append_pair("resource", "https://api.openai.com/v1")
                .append_pair("state", &state)
                .append_pair("nonce", &nonce)
                .append_pair("code_challenge", &challenge)
                .append_pair("code_challenge_method", "S256");
            if existing.as_ref().is_some_and(|r| {
                !r.scope
                    .split_whitespace()
                    .any(|s| s == Provider::Chatgpt.required_scope())
            }) {
                query.append_pair("prompt", "consent");
            }
        }
        let (done, rx) = oneshot::channel();
        let callback_state = Arc::new(Callback {
            manager: self.clone(),
            id,
            existing,
            pending,
            saved_client,
            state,
            nonce,
            verifier,
            redirect,
            port,
            cancel: cancel.clone(),
            done: Mutex::new(Some(done)),
        });
        let router = Router::new()
            .route("/auth/callback", get(callback))
            .with_state(callback_state);
        let flow=async move {
            let shutdown=cancel.clone();tokio::spawn(async move{let _=axum::serve(listener,router).with_graceful_shutdown(shutdown.cancelled_owned()).await;});
            tokio::select!{biased;_=cancel.cancelled()=>Err(Error::Cancelled),_=tokio::time::sleep(Duration::from_secs(300))=>Err(Error::Expired),result=rx=>result.unwrap_or(Err(Error::Cancelled))}
        }.boxed();
        Ok((SignInEvent::Url { url: url.into() }, flow))
    }
    async fn supergrok(
        &self,
        id: String,
        existing: Option<Registration>,
        cancel: CancellationToken,
    ) -> Result<(SignInEvent, BoxFuture<'static, Result<Connection, Error>>), Error> {
        let client = existing
            .as_ref()
            .map(|r| r.client_id.clone())
            .unwrap_or_else(|| self.0.xai_client.clone());
        if client.is_empty() {
            return Err(Error::Unavailable);
        }
        let (status, body) = self
            .post(
                Provider::Supergrok,
                "/oauth2/device/code",
                vec![
                    ("client_id", client.clone()),
                    ("scope", Provider::Supergrok.scope().into()),
                ],
            )
            .await?;
        if !status.is_success() {
            return Err(Error::Unavailable);
        }
        let device: Device = serde_json::from_value(body).map_err(|_| Error::Verification)?;
        let url = reqwest::Url::parse(&device.verification_uri).map_err(|_| Error::Verification)?;
        if url.scheme() != "https"
            || !matches!(url.host_str(), Some("auth.x.ai" | "accounts.x.ai"))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || device.expires_in == 0
            || device.expires_in > 3600
            || device.interval == 0
            || device.interval > 60
            || device.device_code.is_empty()
            || device.user_code.is_empty()
            || device.user_code.len() > 128
        {
            return Err(Error::Verification);
        }
        let expires = now_ms().saturating_add(device.expires_in * 1000);
        let event = SignInEvent::Device {
            user_code: device.user_code.clone(),
            verification_url: device.verification_uri.clone(),
            expires_at: expires,
        };
        let manager = self.clone();
        let flow=async move {
            let mut interval=device.interval;
            while now_ms()<expires {
                tokio::select!{biased;_=cancel.cancelled()=>return Err(Error::Cancelled),_=tokio::time::sleep(Duration::from_millis((interval*1000).min(expires.saturating_sub(now_ms()))))=>{}}
                if now_ms()>=expires{break;}
                let (status,body)=manager.post(Provider::Supergrok,"/oauth2/token",vec![("client_id",client.clone()),("grant_type","urn:ietf:params:oauth:grant-type:device_code".into()),("device_code",device.device_code.clone())]).await?;
                if !status.is_success(){match body["error"].as_str(){Some("authorization_pending")=>continue,Some("slow_down")=>{interval=(interval+5).min(60);continue;},Some("access_denied")=>return Err(Error::Denied),Some("expired_token")=>return Err(Error::Expired),_=>return Err(Error::Verification)}}
                let tokens=manager.tokens(Provider::Supergrok,body,false)?;
                return manager.install(Provider::Supergrok,&id,&client,existing.as_ref(),tokens,None,&cancel).await;
            }Err(Error::Expired)
        }.boxed();
        Ok((event, flow))
    }
    async fn post(
        &self,
        provider: Provider,
        path: &str,
        fields: Vec<(&str, String)>,
    ) -> Result<(reqwest::StatusCode, Value), Error> {
        let response = self
            .0
            .http
            .post(format!("{}{path}", self.issuer(provider)))
            .header(header::ACCEPT, "application/json")
            .form(&fields)
            .send()
            .await
            .map_err(|_| Error::Unavailable)?;
        let status = response.status();
        Ok((status, read_json(response).await?))
    }
    fn tokens(&self, provider: Provider, body: Value, refresh: bool) -> Result<Tokens, Error> {
        let tokens: Tokens = serde_json::from_value(body).map_err(|_| Error::Verification)?;
        if tokens.access_token.as_ref().is_some_and(|s| s.is_empty())
            || tokens.refresh_token.as_ref().is_some_and(|s| s.is_empty())
            || tokens.id_token.as_ref().is_some_and(|s| s.is_empty())
        {
            return Err(Error::Verification);
        }
        if tokens.access_token.is_some()
            && (tokens
                .token_type
                .as_ref()
                .is_none_or(|t| !t.eq_ignore_ascii_case("bearer"))
                || tokens.expires_in.is_none_or(|v| v == 0))
        {
            return Err(Error::Verification);
        }
        let granted = tokens.scope.as_deref().unwrap_or("");
        if (refresh
            || provider == Provider::Supergrok
            || granted
                .split_whitespace()
                .any(|s| s == "offline_access" || s == provider.required_scope()))
            && (tokens.access_token.is_none()
                || tokens.refresh_token.is_none() && provider == Provider::Chatgpt)
        {
            return Err(Error::Verification);
        }
        early(tokens.earliest_refresh_at.as_ref())?;
        Ok(tokens)
    }
    async fn verify(
        &self,
        provider: Provider,
        token: &str,
        client: &str,
        nonce: Option<&str>,
    ) -> Result<Value, Error> {
        let header = jsonwebtoken::decode_header(token).map_err(|_| Error::Verification)?;
        let allowed = match provider {
            Provider::Supergrok => header.alg == Algorithm::ES256,
            Provider::Chatgpt => matches!(
                header.alg,
                Algorithm::RS256
                    | Algorithm::RS384
                    | Algorithm::RS512
                    | Algorithm::PS256
                    | Algorithm::PS384
                    | Algorithm::PS512
                    | Algorithm::ES256
                    | Algorithm::ES384
                    | Algorithm::EdDSA
            ),
        };
        if !allowed {
            return Err(Error::Verification);
        }
        let kid = header.kid.as_deref().ok_or(Error::Verification)?;
        let mut keys = self.0.keys[index(provider)].lock().await;
        if keys.as_ref().is_none_or(|(at, set)| {
            at.elapsed() > Duration::from_secs(600) || set.find(kid).is_none()
        }) {
            let endpoint = match provider {
                Provider::Supergrok => format!("{}/.well-known/jwks.json", self.issuer(provider)),
                Provider::Chatgpt => {
                    let response = self
                        .0
                        .http
                        .get(format!(
                            "{}/.well-known/openid-configuration",
                            self.issuer(provider)
                        ))
                        .send()
                        .await
                        .map_err(|_| Error::Unavailable)?;
                    if !response.status().is_success() {
                        return Err(Error::Unavailable);
                    }
                    let discovery = read_json(response).await?;
                    if discovery["issuer"] != self.issuer(provider) {
                        return Err(Error::Verification);
                    }
                    let endpoint = discovery["jwks_uri"].as_str().ok_or(Error::Verification)?;
                    let url = reqwest::Url::parse(endpoint).map_err(|_| Error::Verification)?;
                    let issuer = reqwest::Url::parse(self.issuer(provider))
                        .map_err(|_| Error::Verification)?;
                    if url.origin() != issuer.origin()
                        || !url.username().is_empty()
                        || url.password().is_some()
                    {
                        return Err(Error::Verification);
                    }
                    endpoint.to_owned()
                }
            };
            let response = self
                .0
                .http
                .get(endpoint)
                .send()
                .await
                .map_err(|_| Error::Unavailable)?;
            if !response.status().is_success() {
                return Err(Error::Unavailable);
            }
            let set: JwkSet = serde_json::from_value(read_json(response).await?)
                .map_err(|_| Error::Verification)?;
            if set.keys.len() > 64 {
                return Err(Error::Verification);
            }
            *keys = Some((Instant::now(), Arc::new(set)));
        }
        let jwk = keys
            .as_ref()
            .unwrap()
            .1
            .find(kid)
            .ok_or(Error::Verification)?;
        let key = DecodingKey::from_jwk(jwk).map_err(|_| Error::Verification)?;
        let mut validation = Validation::new(header.alg);
        validation.set_audience(&[client]);
        validation.set_issuer(&[self.issuer(provider)]);
        validation.set_required_spec_claims(&["sub", "exp", "iss", "aud"]);
        validation.validate_nbf = true;
        validation.leeway = 30;
        let claims = jsonwebtoken::decode::<Value>(token, &key, &validation)
            .map_err(|_| Error::Verification)?
            .claims;
        if claims["sub"].as_str().is_none_or(|s| s.is_empty())
            || claims["iat"]
                .as_u64()
                .is_none_or(|at| at > now_ms() / 1000 + 30)
        {
            return Err(Error::Verification);
        }
        if let Some(nonce) = nonce {
            let actual = claims["nonce"].as_str().ok_or(Error::Verification)?;
            if actual.as_bytes().ct_eq(nonce.as_bytes()).unwrap_u8() != 1 {
                return Err(Error::Verification);
            }
        }
        Ok(claims)
    }
    async fn install(
        &self,
        provider: Provider,
        id: &str,
        client: &str,
        previous: Option<&Registration>,
        tokens: Tokens,
        nonce: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Connection, Error> {
        if provider == Provider::Supergrok && tokens.refresh_token.is_none() {
            return Err(Error::Verification);
        }
        let id_token = tokens.id_token.as_deref().ok_or(Error::Verification)?;
        let claims = self.verify(provider, id_token, client, nonce).await?;
        let subject = claims["sub"].as_str().ok_or(Error::Verification)?;
        if previous.is_some_and(|p| p.subject != subject) {
            return Err(Error::Changed);
        }
        let lease = self.0.vault.acquire(provider).await?;
        let current = lease.registration(id).await?;
        if cancel.is_cancelled()
            || self.0.stop.is_cancelled()
            || current.as_ref().map_or(0, |r| r.authorization_generation)
                != previous.map_or(0, |r| r.authorization_generation)
        {
            return Err(Error::Cancelled);
        }
        let source = current.as_ref().or(previous);
        let created_at = source.map_or(now_ms(), |r| r.created_at);
        let registration = Registration {
            id: id.into(),
            client_id: client.into(),
            subject: subject.into(),
            email: claims["email"]
                .as_str()
                .filter(|_| provider == Provider::Chatgpt || claims["email_verified"] == true)
                .map(str::to_owned),
            name: source.map(|r| r.name.clone()).unwrap_or_else(|| {
                if provider == Provider::Chatgpt {
                    "ChatGPT".into()
                } else {
                    "SuperGrok".into()
                }
            }),
            created_at,
            preferred: source.is_some_and(|r| r.preferred),
            plan_notice_seen: provider == Provider::Supergrok
                || source.is_some_and(|r| r.plan_notice_seen),
            authorization_generation: current
                .as_ref()
                .map_or(1, |r| r.authorization_generation.saturating_add(1)),
            scope: tokens.scope.unwrap_or_default(),
            id_token: tokens.id_token,
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_at: tokens
                .expires_in
                .map(|s| now_ms().saturating_add(s.saturating_mul(1000))),
            earliest_refresh_at: early(tokens.earliest_refresh_at.as_ref())?,
        };
        lease.save(&registration).await?;
        lease.remove_pending(id).await?;
        let metadata = registration.metadata(provider, self.0.vault.storage());
        lease.finish().await;
        Ok(metadata)
    }
    pub async fn credential(&self, provider: Provider, id: &str) -> Result<Credential, Error> {
        if self.0.stop.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let lease = self.0.vault.acquire(provider).await?;
        let mut record = lease.registration(id).await?.ok_or(Error::Unknown)?;
        if !record.authorized(provider) {
            return Err(Error::Reauthorization);
        }
        let now = now_ms();
        if record.earliest_refresh_at.unwrap_or(0) > now && record.expires_at.unwrap_or(0) <= now {
            return Err(Error::RefreshNotAvailable);
        }
        if record.expires_at.unwrap_or(0) <= now + 60000
            && record.earliest_refresh_at.unwrap_or(0) <= now
        {
            let mut fields = vec![
                ("grant_type", "refresh_token".into()),
                ("client_id", record.client_id.clone()),
                (
                    "refresh_token",
                    record.refresh_token.clone().ok_or(Error::Reauthorization)?,
                ),
            ];
            if provider == Provider::Chatgpt {
                fields.push(("resource", "https://api.openai.com/v1".into()));
            }
            let path = if provider == Provider::Chatgpt {
                "/api/accounts/oauth/token"
            } else {
                "/oauth2/token"
            };
            let (status, body) = tokio::select! {biased;_=self.0.stop.cancelled()=>return Err(Error::Cancelled),result=self.post(provider,path,fields)=>result?};
            if !status.is_success() {
                return Err(if status.is_server_error() {
                    Error::Unavailable
                } else {
                    Error::Reauthorization
                });
            }
            let tokens = self.tokens(provider, body, true)?;
            record.access_token = tokens.access_token;
            record.refresh_token = tokens.refresh_token.or(record.refresh_token);
            record.expires_at = tokens
                .expires_in
                .map(|s| now_ms().saturating_add(s.saturating_mul(1000)));
            record.earliest_refresh_at = early(tokens.earliest_refresh_at.as_ref())?;
            if let Some(scope) = tokens.scope {
                record.scope = scope;
            }
            if !record.authorized(provider) {
                return Err(Error::Reauthorization);
            }
            lease.save(&record).await?;
        }
        if self.0.stop.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let credential = Credential {
            connection_id: id.into(),
            provider,
            access_token: record.access_token.ok_or(Error::Reauthorization)?,
        };
        lease.finish().await;
        Ok(credential)
    }
    pub async fn rename(&self, provider: Provider, id: &str, name: &str) -> Result<(), Error> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 200 {
            return Err(Error::InvalidName);
        }
        let lease = self.0.vault.acquire(provider).await?;
        let mut record = lease.registration(id).await?.ok_or(Error::Unknown)?;
        record.name = name.into();
        lease.save(&record).await?;
        lease.finish().await;
        Ok(())
    }
    pub async fn acknowledge_plan(&self, provider: Provider, id: &str) -> Result<(), Error> {
        let lease = self.0.vault.acquire(provider).await?;
        let mut record = lease.registration(id).await?.ok_or(Error::Unknown)?;
        record.plan_notice_seen = true;
        lease.save(&record).await?;
        lease.finish().await;
        Ok(())
    }
    pub async fn preferred(&self, provider: Provider, id: &str) -> Result<(), Error> {
        let lease = self.0.vault.acquire(provider).await?;
        let mut records = lease.registrations().await?;
        if !records.iter().any(|r| r.id == id) {
            return Err(Error::Unknown);
        }
        for record in &mut records {
            if record.id != id && record.preferred {
                record.preferred = false;
                lease.save(record).await?;
            }
        }
        let target = records.iter_mut().find(|r| r.id == id).unwrap();
        target.preferred = true;
        lease.save(target).await?;
        lease.finish().await;
        Ok(())
    }
    pub async fn disconnect(&self, provider: Provider, id: &str) -> Result<bool, Error> {
        self.cancel(provider, id);
        let lease = self.0.vault.acquire(provider).await?;
        let Some(mut record) = lease.registration(id).await? else {
            lease.finish().await;
            return Ok(true);
        };
        let refresh = record.refresh_token.take();
        record.access_token = None;
        record.id_token = None;
        record.expires_at = None;
        record.earliest_refresh_at = None;
        record.authorization_generation = record.authorization_generation.saturating_add(1);
        lease.save(&record).await?;
        let revoked = if let Some(refresh) = refresh {
            self.revoke(provider, &record.client_id, refresh)
                .await
                .unwrap_or(false)
        } else {
            true
        };
        lease.finish().await;
        Ok(revoked)
    }
    async fn revoke(
        &self,
        provider: Provider,
        client: &str,
        refresh: String,
    ) -> Result<bool, Error> {
        let url = if provider == Provider::Supergrok {
            format!("{}/oauth2/revoke", self.issuer(provider))
        } else {
            let response = self
                .0
                .http
                .get(format!(
                    "{}/.well-known/openid-configuration",
                    self.issuer(provider)
                ))
                .send()
                .await
                .map_err(|_| Error::Unavailable)?;
            if !response.status().is_success() {
                return Err(Error::Unavailable);
            }
            let body = read_json(response).await?;
            let uri = body["revocation_endpoint"]
                .as_str()
                .ok_or(Error::Unavailable)?;
            let url = reqwest::Url::parse(uri).map_err(|_| Error::Verification)?;
            let issuer =
                reqwest::Url::parse(self.issuer(provider)).map_err(|_| Error::Verification)?;
            if url.origin() != issuer.origin()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(Error::Verification);
            }
            uri.to_owned()
        };
        let response = self
            .0
            .http
            .post(url)
            .form(&[
                ("token", refresh.as_str()),
                ("token_type_hint", "refresh_token"),
                ("client_id", client),
            ])
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|_| Error::Unavailable)?;
        Ok(response.status().is_success())
    }
}
struct StartGuard {
    manager: ModelAccounts,
    key: Key,
    generation: String,
    cancel: CancellationToken,
    armed: bool,
}
impl Drop for StartGuard {
    fn drop(&mut self) {
        if self.armed {
            self.cancel.cancel();
            self.manager.remove(&self.key, &self.generation);
        }
    }
}

struct Callback {
    manager: ModelAccounts,
    id: String,
    existing: Option<Registration>,
    pending: Option<PendingRegistration>,
    saved_client: Option<String>,
    state: String,
    nonce: String,
    verifier: String,
    redirect: String,
    port: u16,
    cancel: CancellationToken,
    done: Mutex<Option<oneshot::Sender<Result<Connection, Error>>>>,
}
async fn callback(
    State(flow): State<Arc<Callback>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    let expected = format!("127.0.0.1:{}", flow.port);
    if headers.get(header::HOST).and_then(|v| v.to_str().ok()) != Some(expected.as_str())
        || uri.to_string().len() > 16384
    {
        return page(StatusCode::BAD_REQUEST, false);
    }
    let Ok(url) = reqwest::Url::parse(&format!("http://{expected}{uri}")) else {
        return page(StatusCode::BAD_REQUEST, false);
    };
    let pairs = url.query_pairs().collect::<Vec<_>>();
    let values = |name: &str| {
        pairs
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.as_ref())
            .collect::<Vec<_>>()
    };
    let states = values("state");
    if states.len() != 1
        || states[0]
            .as_bytes()
            .ct_eq(flow.state.as_bytes())
            .unwrap_u8()
            != 1
    {
        return page(StatusCode::BAD_REQUEST, false);
    }
    let sender = lock(&flow.done).take();
    let Some(sender) = sender else {
        return page(StatusCode::BAD_REQUEST, false);
    };
    let result = tokio::select! {biased;_=flow.cancel.cancelled()=>Err(Error::Cancelled),result=async {
        if !values("error").is_empty(){return Err(Error::Denied);}let codes=values("code");let clients=values("client_id");
        if codes.len()!=1 || codes[0].is_empty() || clients.len()>1{return Err(Error::Verification);}
        let client=clients.first().copied().or(flow.saved_client.as_deref()).filter(|c|!c.is_empty() && *c!="dynamic_agent_client").ok_or(Error::Verification)?;
        if flow.saved_client.as_ref().is_some_and(|saved|saved!=client){return Err(Error::Changed);}
        if flow.existing.is_none(){
            let lease=flow.manager.0.vault.acquire(Provider::Chatgpt).await?;
            if flow.cancel.is_cancelled(){return Err(Error::Cancelled);}
            lease.save_pending(&PendingRegistration{id:flow.id.clone(),client_id:client.into(),created_at:flow.pending.as_ref().map_or(now_ms(),|p|p.created_at)}).await?;lease.finish().await;
        }
        let (status,body)=flow.manager.post(Provider::Chatgpt,"/api/accounts/oauth/token",vec![("grant_type","authorization_code".into()),("client_id",client.into()),("code",codes[0].into()),("code_verifier",flow.verifier.clone()),("redirect_uri",flow.redirect.clone()),("resource","https://api.openai.com/v1".into())]).await?;
        if !status.is_success(){return Err(if status.is_server_error(){Error::Unavailable}else{Error::Reauthorization});}
        let tokens=flow.manager.tokens(Provider::Chatgpt,body,false)?;if tokens.scope.is_none(){return Err(Error::Verification);}
        flow.manager.install(Provider::Chatgpt,&flow.id,client,flow.existing.as_ref(),tokens,Some(&flow.nonce),&flow.cancel).await
    }=>result};
    let success = result.is_ok();
    let _ = sender.send(result);
    page(
        if success {
            StatusCode::OK
        } else {
            StatusCode::BAD_REQUEST
        },
        success,
    )
}
fn page(status: StatusCode, success: bool) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; frame-ancestors 'none'",
            ),
        ],
        if success {
            "Connected to Zuse. Return to the application."
        } else {
            "Sign-in could not be completed. Return to Zuse and retry."
        },
    )
        .into_response()
}
async fn read_json(response: reqwest::Response) -> Result<Value, Error> {
    if response
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE as u64)
    {
        return Err(Error::Verification);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| Error::Unavailable)?;
        if body.len() + chunk.len() > MAX_RESPONSE {
            return Err(Error::Verification);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| Error::Verification)
}
