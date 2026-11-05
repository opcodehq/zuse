//! Zuse cloud identity for hosted plugins and provider connections.
//!
//! Native clients on this machine sign in through the browser: AuthKit redirects
//! back to a loopback callback and the engine redeems the code with PKCE. Remote
//! browser clients, and machines where the callback cannot bind, use device
//! authorization instead. Credentials stay in the engine; views receive only
//! profile and sign-in progress metadata.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio_util::sync::CancellationToken;
use zuse_protocol::UserProfile;

pub const API_URL: &str = crate::api_origins::DEFAULT_API_URL;
pub const PUBLIC_CLIENT_ID: &str = "client_01KWGQ818571ARFATQ3G9AR2Y2";
const IDENTITY_URL: &str = "https://api.workos.com";
const MAX_RESPONSE: usize = 256 * 1024;
/// How long a browser sign-in waits for AuthKit to redirect back.
const BROWSER_SIGN_IN_SECS: i64 = 10 * 60;

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("Sign in to your Zuse account first.")]
    SignedOut,
    #[error("Your account changed while the request was running. Retry from the current account.")]
    Changed,
    #[error("Account service is unavailable. Check your connection and retry.")]
    Unavailable,
    #[error("The account service returned an invalid response.")]
    InvalidResponse,
    #[error("Sign-in was cancelled.")]
    Cancelled,
    #[error("Sign-in expired. Start again.")]
    Expired,
    #[error("Sign-in was denied.")]
    Denied,
    #[error("The saved account could not be updated. Check the profile folder permissions.")]
    Storage,
    #[error("Account service URLs must be HTTPS origins.")]
    InvalidEndpoint,
}

/// Deliberately has no Debug implementation.
pub struct Credential {
    pub subject: String,
    pub access_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginProgress {
    pub url: String,
    /// The code to confirm on the device authorization page; empty for a
    /// browser sign-in, which returns to this machine on its own.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user_code: String,
    pub expires_at: i64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub user: Option<UserProfile>,
    pub login: Option<LoginProgress>,
    pub error: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedSession {
    refresh_token: String,
    user: UserProfile,
}
struct Access {
    token: String,
    expires_at: i64,
}
struct State {
    retired: bool,
    generation: u64,
    saved: Option<SavedSession>,
    access: Option<Access>,
    login: Option<LoginProgress>,
    error: Option<String>,
    login_cancel: CancellationToken,
    session_cancel: CancellationToken,
    retry_after: i64,
}
struct Inner {
    http: reqwest::Client,
    client_id: String,
    identity_url: String,
    api_url: String,
    store: Option<PathBuf>,
    state: Mutex<State>,
    refresh: AsyncMutex<()>,
    status: watch::Sender<AccountStatus>,
}
#[derive(Clone)]
pub struct Account(Arc<Inner>);

#[derive(Deserialize)]
struct DeviceGrant {
    device_code: String,
    user_code: String,
    verification_uri_complete: String,
    expires_in: u64,
    interval: u64,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    user: IdentityUser,
}
#[derive(Deserialize)]
struct IdentityUser {
    id: String,
    email: String,
    first_name: Option<String>,
    last_name: Option<String>,
}

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn profile(user: IdentityUser) -> UserProfile {
    let name = [user.first_name, user.last_name]
        .into_iter()
        .flatten()
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    UserProfile {
        id: user.id,
        email: user.email,
        name: (!name.is_empty()).then_some(name),
    }
}
fn claims(token: &str) -> Result<Value, AccountError> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or(AccountError::InvalidResponse)?;
    let data = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| AccountError::InvalidResponse)?;
    serde_json::from_slice(&data).map_err(|_| AccountError::InvalidResponse)
}
fn endpoint(value: &str) -> Result<String, AccountError> {
    let url = reqwest::Url::parse(value).map_err(|_| AccountError::InvalidEndpoint)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(AccountError::InvalidEndpoint);
    }
    Ok(value.trim_end_matches('/').into())
}

impl Account {
    pub fn open(root: &Path) -> Result<Self, AccountError> {
        let api_url = crate::api_origins::from_env().general;
        let client_id = std::env::var("ZUSE_WORKOS_CLIENT_ID")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| PUBLIC_CLIENT_ID.into());
        Self::configured(root, endpoint(&api_url)?, client_id, IDENTITY_URL.into())
    }

    /// [`Self::open`] with explicit service endpoints (deployments and tests).
    pub fn configured(
        root: &Path,
        api_url: String,
        client_id: String,
        identity_url: String,
    ) -> Result<Self, AccountError> {
        Self::with_storage(
            Some(root.join("account").join("session.json")),
            api_url,
            client_id,
            identity_url,
        )
    }

    /// An isolated hosted-browser account. Refresh credentials remain in server
    /// memory; this never loads a desktop session or writes a profile/cache.
    pub fn ephemeral(api_url: &str, client_id: &str) -> Result<Self, AccountError> {
        if client_id.trim().is_empty() {
            return Err(AccountError::InvalidEndpoint);
        }
        Self::with_storage(
            None,
            endpoint(api_url)?,
            client_id.into(),
            IDENTITY_URL.into(),
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn ephemeral_fixture(
        api_url: &str,
        identity_url: &str,
        client_id: &str,
    ) -> Result<Self, AccountError> {
        fn fixture_endpoint(value: &str) -> Result<String, AccountError> {
            let url = reqwest::Url::parse(value).map_err(|_| AccountError::InvalidEndpoint)?;
            if url.scheme() == "http"
                && url.host_str().is_some_and(|host| {
                    host.parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
                })
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
            {
                Ok(value.trim_end_matches('/').into())
            } else {
                endpoint(value)
            }
        }
        if client_id.trim().is_empty() {
            return Err(AccountError::InvalidEndpoint);
        }
        Self::with_storage(
            None,
            fixture_endpoint(api_url)?,
            client_id.into(),
            fixture_endpoint(identity_url)?,
        )
    }

    fn with_storage(
        store: Option<PathBuf>,
        api_url: String,
        client_id: String,
        identity_url: String,
    ) -> Result<Self, AccountError> {
        // A damaged cloud session must not prevent local agent work.
        let loaded = store.as_ref().map(std::fs::read).transpose();
        let (saved, storage_error) = match loaded {
            Ok(None) => (None, None),
            Ok(Some(bytes)) => match serde_json::from_slice::<Option<SavedSession>>(&bytes) {
                Ok(saved) => (saved, None),
                Err(_) => (None, Some(AccountError::Storage.to_string())),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, None),
            Err(_) => (None, Some(AccountError::Storage.to_string())),
        };
        let initial = AccountStatus {
            user: saved.as_ref().map(|s| s.user.clone()),
            error: storage_error.clone(),
            ..Default::default()
        };
        let (status, _) = watch::channel(initial);
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| AccountError::Unavailable)?;
        Ok(Self(Arc::new(Inner {
            http,
            client_id,
            identity_url,
            api_url,
            store,
            state: Mutex::new(State {
                retired: false,
                generation: 0,
                saved,
                access: None,
                login: None,
                error: storage_error,
                login_cancel: CancellationToken::new(),
                session_cancel: CancellationToken::new(),
                retry_after: 0,
            }),
            refresh: AsyncMutex::new(()),
            status,
        })))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn fixture_signed_in(root: &Path, api_url: String, subject: &str) -> Self {
        let account = Self::configured(
            root,
            api_url,
            "fixture-client".into(),
            "http://127.0.0.1:1".into(),
        )
        .unwrap();
        account.fixture_install(subject);
        account
    }
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn fixture_install(&self, subject: &str) {
        let payload = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&json!({"sub":subject,"exp":now()+3600})).unwrap());
        let generation = lock(&self.0.state).generation;
        self.install(
            TokenResponse {
                access_token: format!("fixture.{payload}.signature"),
                refresh_token: format!("refresh-{subject}"),
                user: IdentityUser {
                    id: subject.into(),
                    email: format!("{subject}@example.test"),
                    first_name: None,
                    last_name: None,
                },
            },
            generation,
            None,
        )
        .unwrap();
    }

    pub fn api_url(&self) -> &str {
        &self.0.api_url
    }
    pub fn watch(&self) -> watch::Receiver<AccountStatus> {
        self.0.status.subscribe()
    }
    pub fn status(&self) -> AccountStatus {
        self.0.status.borrow().clone()
    }
    pub fn session_cancellation(&self) -> CancellationToken {
        lock(&self.0.state).session_cancel.clone()
    }

    fn publish(&self, state: &State) {
        self.0.status.send_replace(AccountStatus {
            user: state.saved.as_ref().map(|s| s.user.clone()),
            login: state.login.clone(),
            error: state.error.clone(),
        });
    }

    /// Retire any sign-in in flight and open a new attempt.
    fn begin_login(&self) -> Result<(u64, CancellationToken), AccountError> {
        let mut state = lock(&self.0.state);
        if state.retired {
            return Err(AccountError::Changed);
        }
        state.generation = state.generation.wrapping_add(1);
        state.login_cancel.cancel();
        state.login_cancel = CancellationToken::new();
        state.login = None;
        state.error = None;
        self.publish(&state);
        Ok((state.generation, state.login_cancel.clone()))
    }

    /// Publish an attempt's progress unless a newer attempt replaced it.
    fn show_login(&self, generation: u64, progress: &LoginProgress) -> Result<(), AccountError> {
        let mut state = lock(&self.0.state);
        if state.retired || state.generation != generation {
            return Err(AccountError::Cancelled);
        }
        state.login = Some(progress.clone());
        self.publish(&state);
        Ok(())
    }

    /// Finish an attempt in the background; failures surface on the status.
    fn complete_login(
        &self,
        generation: u64,
        completion: impl std::future::Future<Output = Result<(), AccountError>> + Send + 'static,
    ) {
        let account = self.clone();
        tokio::spawn(async move {
            if let Err(error) = completion.await {
                let mut state = lock(&account.0.state);
                if state.generation == generation {
                    state.login = None;
                    if !matches!(error, AccountError::Cancelled) {
                        state.error = Some(error.to_string());
                    }
                    account.publish(&state);
                }
            }
        });
    }

    /// Sign in through this machine's browser: AuthKit redirects to a loopback
    /// callback and the code is redeemed with PKCE, so there is no code to
    /// confirm. Falls back to [`Self::sign_in`] (device authorization) when the
    /// callback cannot bind or the identity provider rejects its redirect.
    /// Only for clients on the engine's own machine.
    pub async fn sign_in_in_browser(&self) -> Result<LoginProgress, AccountError> {
        let listener = match tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await
        {
            Ok(listener) => listener,
            Err(error) => {
                tracing::info!(%error, "account: loopback sign-in unavailable; using device authorization");
                return self.sign_in().await;
            }
        };
        let port = listener
            .local_addr()
            .map_err(|_| AccountError::Unavailable)?
            .port();
        let (generation, cancel) = self.begin_login()?;
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");
        let expected_state = uuid::Uuid::new_v4().simple().to_string();
        let verifier = {
            use rand_core::RngCore;
            let mut bytes = [0u8; 32];
            rand_core::OsRng.fill_bytes(&mut bytes);
            URL_SAFE_NO_PAD.encode(bytes)
        };
        let challenge = {
            use sha2::Digest;
            URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
        };
        let url = reqwest::Url::parse_with_params(
            &format!("{}/user_management/authorize", self.0.identity_url),
            [
                ("response_type", "code"),
                ("client_id", self.0.client_id.as_str()),
                ("redirect_uri", redirect_uri.as_str()),
                ("provider", "authkit"),
                ("state", expected_state.as_str()),
                ("code_challenge", challenge.as_str()),
                ("code_challenge_method", "S256"),
            ],
        )
        .map_err(|_| AccountError::InvalidEndpoint)?;
        let rejected = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AccountError::Cancelled),
            rejected = crate::auth::loopback_redirect_rejected(url.as_str()) => rejected,
        };
        if rejected {
            tracing::info!("account: loopback redirect rejected; using device authorization");
            return self.sign_in_attempt(generation, cancel).await;
        }
        let progress = LoginProgress {
            url: url.into(),
            user_code: String::new(),
            expires_at: now() + BROWSER_SIGN_IN_SECS,
        };
        self.show_login(generation, &progress)?;
        let callback = BrowserCallback {
            port,
            state: expected_state,
            verifier,
        };
        let account = self.clone();
        self.complete_login(generation, async move {
            account
                .await_callback(listener, callback, generation, cancel)
                .await
        });
        Ok(progress)
    }

    async fn await_callback(
        &self,
        listener: tokio::net::TcpListener,
        callback: BrowserCallback,
        generation: u64,
        cancel: CancellationToken,
    ) -> Result<(), AccountError> {
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(BROWSER_SIGN_IN_SECS as u64);
        loop {
            let mut stream = tokio::select! {
                _ = cancel.cancelled() => return Err(AccountError::Cancelled),
                _ = tokio::time::sleep_until(deadline) => return Err(AccountError::Expired),
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => stream,
                    Err(_) => continue,
                },
            };
            let Some(request) = read_request_head(&mut stream).await else {
                continue;
            };
            let outcome = match callback.parse(&request) {
                CallbackRequest::Ignored(status, message) => {
                    respond(&mut stream, status, message).await;
                    continue;
                }
                CallbackRequest::Denied => Err(AccountError::Denied),
                CallbackRequest::Code(code) => tokio::select! {
                    _ = cancel.cancelled() => Err(AccountError::Cancelled),
                    result = self.redeem(&code, &callback.verifier, generation) => result,
                },
            };
            let (status, message) = match &outcome {
                Ok(()) => (
                    "200 OK",
                    "Signed in. You can close this tab and return to Zuse.",
                ),
                Err(AccountError::Denied) => {
                    ("200 OK", "Sign-in was denied. Return to Zuse to try again.")
                }
                Err(_) => (
                    "502 Bad Gateway",
                    "Sign-in could not be completed. Return to Zuse and retry.",
                ),
            };
            respond(&mut stream, status, message).await;
            return outcome;
        }
    }

    /// Redeem an authorization code from the browser redirect.
    async fn redeem(
        &self,
        code: &str,
        verifier: &str,
        generation: u64,
    ) -> Result<(), AccountError> {
        let response = self
            .0
            .http
            .post(format!(
                "{}/user_management/authenticate",
                self.0.identity_url
            ))
            .json(&json!({
                "client_id": self.0.client_id,
                "grant_type": "authorization_code",
                "code": code,
                "code_verifier": verifier,
            }))
            .send()
            .await
            .map_err(|_| AccountError::Unavailable)?;
        let success = response.status().is_success();
        let body = read_json(response).await?;
        if !success {
            return Err(AccountError::Unavailable);
        }
        let tokens: TokenResponse =
            serde_json::from_value(body).map_err(|_| AccountError::InvalidResponse)?;
        self.install(tokens, generation, None)
    }

    /// Sign in with device authorization: the user confirms a code on the
    /// identity provider's page, from any device.
    pub async fn sign_in(&self) -> Result<LoginProgress, AccountError> {
        let (generation, cancel) = self.begin_login()?;
        self.sign_in_attempt(generation, cancel).await
    }

    async fn sign_in_attempt(
        &self,
        generation: u64,
        cancel: CancellationToken,
    ) -> Result<LoginProgress, AccountError> {
        let response = tokio::select! {
            biased;
            _=cancel.cancelled()=>return Err(AccountError::Cancelled),
            response=self.0.http.post(format!("{}/user_management/authorize/device",self.0.identity_url))
                .form(&[("client_id",self.0.client_id.as_str())]).send()=>response.map_err(|_|AccountError::Unavailable)?,
        };
        if !response.status().is_success() {
            return Err(AccountError::Unavailable);
        }
        let grant: DeviceGrant = serde_json::from_value(read_json(response).await?)
            .map_err(|_| AccountError::InvalidResponse)?;
        if grant.device_code.is_empty()
            || grant.user_code.is_empty()
            || grant.interval == 0
            || grant.expires_in == 0
            || grant.expires_in > 3600
            || !grant.verification_uri_complete.starts_with("https://")
        {
            return Err(AccountError::InvalidResponse);
        }
        let verification = reqwest::Url::parse(&grant.verification_uri_complete)
            .map_err(|_| AccountError::InvalidResponse)?;
        if verification.scheme() != "https"
            || verification.host_str().is_none()
            || !verification.username().is_empty()
            || verification.password().is_some()
        {
            return Err(AccountError::InvalidResponse);
        }
        let progress = LoginProgress {
            url: grant.verification_uri_complete.clone(),
            user_code: grant.user_code.clone(),
            expires_at: now() + grant.expires_in as i64,
        };
        self.show_login(generation, &progress)?;
        let account = self.clone();
        self.complete_login(generation, async move {
            account.poll_login(grant, generation, cancel).await
        });
        Ok(progress)
    }

    async fn poll_login(
        &self,
        grant: DeviceGrant,
        generation: u64,
        cancel: CancellationToken,
    ) -> Result<(), AccountError> {
        let deadline = now() + grant.expires_in as i64;
        let mut interval = grant.interval.clamp(1, 60);
        while now() < deadline {
            tokio::select! {_ = cancel.cancelled()=>return Err(AccountError::Cancelled),_ = tokio::time::sleep(Duration::from_secs(interval))=>{}}
            if now() >= deadline {
                return Err(AccountError::Expired);
            }
            let response = tokio::select! {
                _=cancel.cancelled()=>return Err(AccountError::Cancelled),
                response=self.0.http.post(format!("{}/user_management/authenticate",self.0.identity_url))
                    .form(&[("client_id",self.0.client_id.as_str()),("grant_type","urn:ietf:params:oauth:grant-type:device_code"),("device_code",grant.device_code.as_str())]).send()=>response.map_err(|_|AccountError::Unavailable)?,
            };
            let success = response.status().is_success();
            let body = read_json(response).await?;
            if success {
                let tokens: TokenResponse =
                    serde_json::from_value(body).map_err(|_| AccountError::InvalidResponse)?;
                return self.install(tokens, generation, None);
            }
            match body["error"].as_str() {
                Some("authorization_pending") => {}
                Some("slow_down") => interval = (interval + 5).min(60),
                Some("access_denied") => return Err(AccountError::Denied),
                Some("expired_token") => return Err(AccountError::Expired),
                _ => return Err(AccountError::Unavailable),
            }
        }
        Err(AccountError::Expired)
    }

    fn install(
        &self,
        tokens: TokenResponse,
        generation: u64,
        expected_subject: Option<&str>,
    ) -> Result<(), AccountError> {
        let user = profile(tokens.user);
        let payload = claims(&tokens.access_token)?;
        let expires_at = payload["exp"]
            .as_i64()
            .filter(|at| *at > now())
            .ok_or(AccountError::InvalidResponse)?;
        if user.id.is_empty()
            || tokens.refresh_token.is_empty()
            || payload["sub"].as_str() != Some(user.id.as_str())
        {
            return Err(AccountError::InvalidResponse);
        }
        if expected_subject.is_some_and(|subject| subject != user.id) {
            return Err(AccountError::Changed);
        }
        let mut state = lock(&self.0.state);
        if state.retired || state.generation != generation {
            return Err(AccountError::Changed);
        }
        let saved = SavedSession {
            user,
            refresh_token: tokens.refresh_token,
        };
        self.persist(&saved)?;
        if state
            .saved
            .as_ref()
            .is_some_and(|s| s.user.id != saved.user.id)
            || expected_subject.is_none()
        {
            state.session_cancel.cancel();
            state.session_cancel = CancellationToken::new();
        }
        state.saved = Some(saved);
        state.access = Some(Access {
            token: tokens.access_token,
            expires_at,
        });
        if expected_subject.is_none() {
            // Retire refreshes that began while interactive sign-in was pending.
            state.generation = state.generation.wrapping_add(1);
            state.login_cancel.cancel();
            state.login = None;
        }
        state.error = None;
        state.retry_after = 0;
        self.publish(&state);
        Ok(())
    }

    pub async fn credential(&self) -> Result<Credential, AccountError> {
        let _gate = self.0.refresh.lock().await;
        let (generation, saved, cancel) = {
            let state = lock(&self.0.state);
            if state.retired {
                return Err(AccountError::Changed);
            }
            let saved = state.saved.clone().ok_or(AccountError::SignedOut)?;
            if let Some(access) = &state.access {
                if access.expires_at > now() + 30 {
                    return Ok(Credential {
                        subject: saved.user.id,
                        access_token: access.token.clone(),
                    });
                }
            }
            if state.retry_after > now() {
                return Err(AccountError::Unavailable);
            }
            (state.generation, saved, state.session_cancel.clone())
        };
        let response = tokio::select! {
            _=cancel.cancelled()=>return Err(AccountError::Changed),
            response=self.0.http.post(format!("{}/user_management/authenticate",self.0.identity_url))
                .json(&json!({"client_id":self.0.client_id,"grant_type":"refresh_token","refresh_token":saved.refresh_token})).send()=>response,
        };
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                self.cooldown(generation);
                return Err(AccountError::Unavailable);
            }
        };
        let status = response.status();
        let body = match read_json(response).await {
            Ok(body) => body,
            Err(error) => {
                self.cooldown(generation);
                return Err(error);
            }
        };
        if !status.is_success() {
            if matches!(
                body["error"].as_str().or_else(|| body["code"].as_str()),
                Some("invalid_grant")
            ) {
                let mut state = lock(&self.0.state);
                if state.generation == generation {
                    state.generation = state.generation.wrapping_add(1);
                    state.session_cancel.cancel();
                    state.login_cancel.cancel();
                    state.login = None;
                    state.saved = None;
                    state.access = None;
                    state.error = Some("Your Zuse sign-in expired. Sign in again.".into());
                    if self.persist(&Value::Null).is_err() {
                        state.error = Some(AccountError::Storage.to_string());
                    }
                    self.publish(&state);
                }
                return Err(AccountError::SignedOut);
            }
            self.cooldown(generation);
            return Err(AccountError::Unavailable);
        }
        let tokens: TokenResponse = serde_json::from_value(body).map_err(|_| {
            self.cooldown(generation);
            AccountError::InvalidResponse
        })?;
        self.install(tokens, generation, Some(&saved.user.id))
            .inspect_err(|_| self.cooldown(generation))?;
        let state = lock(&self.0.state);
        if state.retired || state.generation != generation {
            return Err(AccountError::Changed);
        }
        let saved = state.saved.as_ref().ok_or(AccountError::SignedOut)?;
        let access = state.access.as_ref().ok_or(AccountError::SignedOut)?;
        Ok(Credential {
            subject: saved.user.id.clone(),
            access_token: access.token.clone(),
        })
    }

    fn cooldown(&self, generation: u64) {
        let mut state = lock(&self.0.state);
        if state.generation == generation {
            state.retry_after = now() + 5;
            state.error = Some(AccountError::Unavailable.to_string());
            self.publish(&state);
        }
    }

    /// Revoke a retired runtime without deleting the account saved for next boot.
    pub fn retire(&self) {
        let mut state = lock(&self.0.state);
        state.retired = true;
        state.generation = state.generation.wrapping_add(1);
        state.login_cancel.cancel();
        state.session_cancel.cancel();
        state.login = None;
        state.access = None;
        state.saved = None;
        self.publish(&state);
    }

    fn persist(&self, value: &impl Serialize) -> Result<(), AccountError> {
        match &self.0.store {
            Some(path) => persist(path, value),
            None => Ok(()),
        }
    }
    pub fn sign_out(&self) -> Result<(), AccountError> {
        let mut state = lock(&self.0.state);
        state.generation = state.generation.wrapping_add(1);
        state.login_cancel.cancel();
        state.session_cancel.cancel();
        state.saved = None;
        state.access = None;
        state.login = None;
        state.error = None;
        state.retry_after = 0;
        self.publish(&state);
        // Persist a signed-out tombstone atomically, including directory fsync.
        let result = self.persist(&Value::Null);
        #[cfg(not(target_arch = "wasm32"))]
        if self.0.store.is_some() && crate::cloud::clear_native_history_cache().is_err() {
            state.error = Some("Signed out. Local history cache cleanup could not finish.".into());
            self.publish(&state);
        }
        if result.is_err() {
            state.error = Some(AccountError::Storage.to_string());
            self.publish(&state);
        }
        result
    }
}

/// The browser sign-in's loopback redirect target and PKCE secret.
struct BrowserCallback {
    port: u16,
    state: String,
    verifier: String,
}

enum CallbackRequest {
    /// Not this sign-in's redirect; answer and keep waiting.
    Ignored(&'static str, &'static str),
    Denied,
    Code(String),
}

impl BrowserCallback {
    /// Classify a request head received on the loopback listener.
    fn parse(&self, head: &str) -> CallbackRequest {
        let invalid = CallbackRequest::Ignored(
            "400 Bad Request",
            "Invalid or expired sign-in link. Start again from Zuse.",
        );
        let host = format!("127.0.0.1:{}", self.port);
        let host_matches = head.lines().skip(1).any(|line| {
            line.split_once(':').is_some_and(|(name, value)| {
                name.trim().eq_ignore_ascii_case("host") && value.trim() == host
            })
        });
        let mut request_line = head.lines().next().unwrap_or_default().split_whitespace();
        let (Some("GET"), Some(target)) = (request_line.next(), request_line.next()) else {
            return invalid;
        };
        let Ok(url) = reqwest::Url::parse(&format!("http://{host}{target}")) else {
            return invalid;
        };
        if url.path() != "/callback" {
            return CallbackRequest::Ignored("404 Not Found", "Not found.");
        }
        if !host_matches {
            return invalid;
        }
        let param = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        if param("state").as_deref() != Some(self.state.as_str()) {
            return invalid;
        }
        if param("error").is_some() {
            return CallbackRequest::Denied;
        }
        match param("code").filter(|code| !code.is_empty() && code.len() <= 1024) {
            Some(code) => CallbackRequest::Code(code),
            None => invalid,
        }
    }
}

/// Read an HTTP request head (bounded); `None` when the client goes away.
async fn read_request_head(stream: &mut tokio::net::TcpStream) -> Option<String> {
    use tokio::io::AsyncReadExt;
    let mut head = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        if head.len() > 16 * 1024 {
            return None;
        }
        let read = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut chunk));
        match read.await {
            Ok(Ok(n)) if n > 0 => head.extend_from_slice(&chunk[..n]),
            _ => return None,
        }
    }
    Some(String::from_utf8_lossy(&head).into_owned())
}

async fn respond(stream: &mut tokio::net::TcpStream, status: &str, message: &str) {
    use tokio::io::AsyncWriteExt;
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Zuse</title><h1>Zuse</h1><p>{message}</p>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncache-control: no-store\r\n\
         referrer-policy: no-referrer\r\ncontent-security-policy: default-src 'none'\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn read_json(response: reqwest::Response) -> Result<Value, AccountError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE as u64)
    {
        return Err(AccountError::InvalidResponse);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| AccountError::Unavailable)?;
        if bytes.len() + chunk.len() > MAX_RESPONSE {
            return Err(AccountError::InvalidResponse);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| AccountError::InvalidResponse)
}

fn persist(path: &Path, session: &impl Serialize) -> Result<(), AccountError> {
    let dir = path.parent().ok_or(AccountError::Storage)?;
    std::fs::create_dir_all(dir).map_err(|_| AccountError::Storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| AccountError::Storage)?;
    }
    let temporary = dir.join(format!(".session-{}.tmp", uuid::Uuid::new_v4()));
    let write = (|| -> Result<(), AccountError> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|_| AccountError::Storage)?;
        let bytes = serde_json::to_vec(session).map_err(|_| AccountError::Storage)?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| AccountError::Storage)?;
        std::fs::rename(&temporary, path).map_err(|_| AccountError::Storage)?;
        #[cfg(unix)]
        {
            std::fs::File::open(dir)
                .and_then(|d| d.sync_all())
                .map_err(|_| AccountError::Storage)?;
        }
        Ok(())
    })();
    if write.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    write
}

#[cfg(test)]
mod tests {
    use super::*;
    fn token(subject: &str, expires: i64) -> TokenResponse {
        let payload = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&json!({"sub":subject,"exp":expires})).unwrap());
        TokenResponse {
            access_token: format!("header.{payload}.signature"),
            refresh_token: format!("refresh-{subject}"),
            user: IdentityUser {
                id: subject.into(),
                email: format!("{subject}@example.test"),
                first_name: None,
                last_name: None,
            },
        }
    }
    #[test]
    fn damaged_cloud_session_keeps_local_engine_available() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("account");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("session.json"), "broken").unwrap();
        let account = Account::open(dir.path()).unwrap();
        assert!(account.status().user.is_none());
        assert!(account.status().error.is_some());
    }

    #[test]
    fn sign_out_fences_late_authorization_and_revokes_existing_requests() {
        let dir = tempfile::tempdir().unwrap();
        let account = Account::open(dir.path()).unwrap();
        account.install(token("a", now() + 3600), 0, None).unwrap();
        let active = account.session_cancellation();
        account.sign_out().unwrap();
        assert!(active.is_cancelled());
        assert!(matches!(
            account.install(token("a", now() + 3600), 0, None),
            Err(AccountError::Changed)
        ));
        assert!(account.status().user.is_none());
        assert_eq!(
            std::fs::read_to_string(account.0.store.as_ref().unwrap()).unwrap(),
            "null"
        );
        assert!(Account::open(dir.path()).unwrap().status().user.is_none());
    }
    #[test]
    fn account_switch_rejects_old_refresh_and_never_saves_access_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let account = Account::open(dir.path()).unwrap();
        account.install(token("a", now() + 3600), 0, None).unwrap();
        let active = account.session_cancellation();
        account.install(token("b", now() + 3600), 1, None).unwrap();
        assert!(active.is_cancelled());
        assert!(matches!(
            account.install(token("a", now() + 3600), 1, Some("a")),
            Err(AccountError::Changed)
        ));
        let bytes = std::fs::read_to_string(account.0.store.as_ref().unwrap()).unwrap();
        assert!(!bytes.contains("access_token"));
        assert!(!bytes.contains("signature"));
        let reopened = Account::open(dir.path()).unwrap();
        assert_eq!(reopened.status().user.unwrap().id, "b");
        assert!(lock(&reopened.0.state).access.is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(account.0.store.as_ref().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[tokio::test]
    async fn concurrent_agents_share_one_refresh_and_rotation() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut buf = [0u8; 1024];
                let n = socket.read(&mut buf).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let size = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap();
                    if bytes.len() >= end + 4 + size {
                        let body: Value =
                            serde_json::from_slice(&bytes[end + 4..end + 4 + size]).unwrap();
                        assert_eq!(body["grant_type"], "refresh_token");
                        break;
                    }
                }
            }
            let fresh = token("a", now() + 3600);
            let body=json!({"access_token":fresh.access_token,"refresh_token":"rotated-refresh","user":{"id":"a","email":"a@example.test"}}).to_string();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).as_bytes()).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let account = Account::configured(
            dir.path(),
            API_URL.into(),
            "test-client".into(),
            format!("http://{address}"),
        )
        .unwrap();
        account.install(token("a", now() + 10), 0, None).unwrap();
        let results = futures::future::join_all((0..16).map(|_| account.credential())).await;
        for result in results {
            assert_eq!(result.unwrap().subject, "a");
        }
        server.await.unwrap();
        assert_eq!(
            lock(&account.0.state).saved.as_ref().unwrap().refresh_token,
            "rotated-refresh"
        );
    }
}
