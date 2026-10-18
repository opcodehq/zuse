//! Existing account-control API; account, transport and host authority stay separate.
pub mod host;
mod identity;
mod proof;
use crate::account::Account;
use futures::StreamExt;
pub use identity::HostIdentity;
use proof::ProofKey;
use reqwest::Method;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Sign in to your Zuse account first.")]
    SignedOut,
    #[error("Your account changed during the remote access request.")]
    Changed,
    #[error("The remote access service is unavailable. Retry when connected.")]
    Unavailable,
    #[error("The remote access service rejected this request.")]
    Rejected,
    #[error("The remote access service returned an invalid response.")]
    Invalid,
    #[error(
        "The saved remote host identity is invalid. Restore the profile identity before linking."
    )]
    Identity,
    #[error("Remote access identity could not be saved.")]
    Storage,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub environment_id: String,
    pub label: Option<String>,
    pub provider_kind: String,
    pub endpoint: Option<Endpoint>,
    pub linked_at: i64,
    pub runtime_version: Option<String>,
    pub wire_protocol_version: Option<u32>,
    pub service_state: Option<String>,
    pub capabilities: Option<Value>,
    pub endpoint_health: Option<Value>,
    pub last_heartbeat: Option<i64>,
    pub environment_public_key: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub http_base_url: String,
    pub ws_base_url: String,
}
impl Endpoint {
    fn managed_origin(&self) -> Result<String> {
        if !(self.http_base_url.starts_with("https://")
            || self.http_base_url.starts_with("http://"))
            || !(self.ws_base_url.starts_with("wss://") || self.ws_base_url.starts_with("ws://"))
        {
            return Err(Error::Invalid);
        }
        let origin =
            zuse_remote::normalized_origin(&self.http_base_url).map_err(|_| Error::Invalid)?;
        if zuse_remote::normalized_origin(&self.ws_base_url).map_err(|_| Error::Invalid)? != origin
        {
            return Err(Error::Invalid);
        }
        Ok(origin)
    }
}
#[derive(Serialize, Deserialize)]
pub struct Environments {
    pub environments: Vec<Environment>,
}
#[derive(Serialize, Deserialize)]
pub struct Clients {
    pub clients: Vec<AuthorizedClient>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizedClient {
    pub client_id: String,
    pub platform: String,
    pub label: Option<String>,
    pub last_seen_at: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Challenge {
    pub challenge_id: String,
    pub challenge: String,
    pub api_issuer: String,
    pub expires_at: i64,
}
/// Secret-bearing receipt. Intentionally has no Debug or Serialize implementation.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkReceipt {
    pub environment_id: String,
    pub endpoint: Endpoint,
    pub api_issuer: String,
    pub environment_credential: String,
    pub mint_public_key: String,
    pub tunnel_hostname: Option<String>,
    pub connector_token: Option<String>,
}
/// A client-bound, short-lived bootstrap receipt. Never ordinary UI metadata.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectGrant {
    pub endpoint: Endpoint,
    pub endpoint_candidates: Option<Vec<EndpointCandidate>>,
    pub connect_token: String,
    pub expires_at: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointCandidate {
    pub kind: String,
    pub endpoint: Endpoint,
}
struct Access {
    token: String,
    expires: Instant,
    wall_expiry: i64,
}
struct Session {
    subject: String,
    cancel: CancellationToken,
    key: ProofKey,
    access: Option<Access>,
    registered: Option<String>,
}
#[derive(Clone)]
pub struct AccountClient {
    account: Account,
    http: reqwest::Client,
    session: Arc<Mutex<Option<Session>>>,
}
struct Lease {
    subject: String,
    cancel: CancellationToken,
}
impl Lease {
    fn check(&self, account: &Account) -> Result<()> {
        if self.cancel.is_cancelled()
            || account.status().user.as_ref().map(|u| u.id.as_str()) != Some(self.subject.as_str())
        {
            Err(Error::Changed)
        } else {
            Ok(())
        }
    }
}
impl AccountClient {
    pub fn new(account: Account) -> Result<Self> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| Error::Unavailable)?;
        Ok(Self {
            account,
            http,
            session: Arc::new(Mutex::new(None)),
        })
    }
    async fn lease(&self) -> Result<Lease> {
        if self.account.status().user.is_none() {
            return Err(Error::SignedOut);
        }
        let cancel = self.account.session_cancellation();
        let credential = tokio::select! {biased;_=cancel.cancelled()=>return Err(Error::Changed),v=self.account.credential()=>v.map_err(|e|match e{crate::account::AccountError::SignedOut=>Error::SignedOut,crate::account::AccountError::Changed=>Error::Changed,_=>Error::Unavailable})?};
        let lease = Lease {
            subject: credential.subject,
            cancel,
        };
        lease.check(&self.account)?;
        Ok(lease)
    }
    async fn authorization(&self, lease: &Lease) -> Result<String> {
        lease.check(&self.account)?;
        let credential = tokio::select! {biased;_=lease.cancel.cancelled()=>return Err(Error::Changed),value=self.account.credential()=>value.map_err(|_|Error::Unavailable)?};
        lease.check(&self.account)?;
        if credential.subject != lease.subject {
            return Err(Error::Changed);
        }
        Ok(format!("Bearer {}", credential.access_token))
    }
    async fn send<T: DeserializeOwned>(
        &self,
        lease: &Lease,
        method: Method,
        path: &str,
        body: Option<&Value>,
        authorization: &str,
        proof: Option<&str>,
    ) -> Result<T> {
        lease.check(&self.account)?;
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.account.api_url()))
            .header(reqwest::header::AUTHORIZATION, authorization);
        if let Some(proof) = proof {
            request = request.header("dpop", proof);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let task = async {
            let response = request.send().await.map_err(|_| Error::Unavailable)?;
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(Error::SignedOut);
            }
            if !status.is_success() {
                return Err(
                    if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS
                    {
                        Error::Unavailable
                    } else {
                        Error::Rejected
                    },
                );
            }
            if response.content_length().is_some_and(|n| n > 1024 * 1024) {
                return Err(Error::Invalid);
            }
            let mut bytes = vec![];
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| Error::Unavailable)?;
                if bytes.len() + chunk.len() > 1024 * 1024 {
                    return Err(Error::Invalid);
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)
        };
        let value = tokio::select! {biased;_=lease.cancel.cancelled()=>return Err(Error::Changed),value=task=>value?};
        lease.check(&self.account)?;
        Ok(value)
    }
    async fn bearer<T: DeserializeOwned>(
        &self,
        lease: &Lease,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<T> {
        self.send(
            lease,
            method,
            path,
            body,
            &self.authorization(lease).await?,
            None,
        )
        .await
    }
    pub async fn environments(&self) -> Result<Environments> {
        let lease = self.lease().await?;
        let result: Environments = self
            .bearer(&lease, Method::GET, "/v1/environments", None)
            .await?;
        if result.environments.len() > 512
            || result.environments.iter().any(|e| {
                !valid_id(&e.environment_id) || e.label.as_ref().is_some_and(|v| v.len() > 1024)
            })
        {
            return Err(Error::Invalid);
        }
        Ok(result)
    }
    pub async fn clients(&self) -> Result<Clients> {
        let lease = self.lease().await?;
        let result: Clients = self
            .bearer(&lease, Method::GET, "/v1/clients", None)
            .await?;
        if result.clients.len() > 1024 || result.clients.iter().any(|c| !valid_id(&c.client_id)) {
            return Err(Error::Invalid);
        }
        Ok(result)
    }
    pub async fn revoke_client(&self, id: &str) -> Result<()> {
        if !valid_id(id) {
            return Err(Error::Invalid);
        }
        let lease = self.lease().await?;
        let _: Value = self
            .bearer(&lease, Method::DELETE, &format!("/v1/clients/{id}"), None)
            .await?;
        Ok(())
    }
    // The owner must retain one account lifetime through challenge + proof + link.
    // This closure prevents a re-login between those stages from using the first
    // account's signed challenge for the second account.
    pub async fn link(
        &self,
        identity: &HostIdentity,
        label: &str,
        port: u16,
    ) -> Result<LinkReceipt> {
        let label = label.trim();
        if label.is_empty() || label.len() > 128 || label.chars().any(char::is_control) || port == 0
        {
            return Err(Error::Invalid);
        }
        let endpoint = Endpoint {
            http_base_url: format!("http://127.0.0.1:{port}"),
            ws_base_url: format!("ws://127.0.0.1:{port}"),
        };
        let lease = self.lease().await?;
        let challenge: Challenge = self
            .bearer(
                &lease,
                Method::POST,
                "/v1/client/environment-link-challenges",
                None,
            )
            .await?;
        if challenge.expires_at <= chrono::Utc::now().timestamp_millis()
            || !valid_id(&challenge.challenge_id)
        {
            return Err(Error::Invalid);
        }
        let proof = identity.link_proof(&challenge.challenge, &challenge.api_issuer)?;
        let body = json!({"challengeId":challenge.challenge_id,"proof":proof,"environmentId":identity.device(),"environmentPublicKey":identity.public_jwk(),"providerKind":"desktop","endpoint":endpoint,"label":label,"runtimeVersion":env!("CARGO_PKG_VERSION"),"managedTunnel":true,"origin":{"localHttpHost":"127.0.0.1","localHttpPort":port}});
        let result: LinkReceipt = self
            .bearer(
                &lease,
                Method::POST,
                "/v1/client/environment-links",
                Some(&body),
            )
            .await?;
        if result.environment_id != identity.device()
            || result.environment_credential.is_empty()
            || result.environment_credential.len() > 8192
            || result
                .connector_token
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 16384)
        {
            return Err(Error::Invalid);
        }
        zuse_remote::normalized_origin(&result.api_issuer).map_err(|_| Error::Invalid)?;
        let key: Value =
            serde_json::from_str(&result.mint_public_key).map_err(|_| Error::Invalid)?;
        if key["kty"] != "OKP"
            || key["crv"] != "Ed25519"
            || !key["d"].is_null()
            || result.mint_public_key.len() > 4096
        {
            return Err(Error::Invalid);
        }
        let jwk: jsonwebtoken::jwk::Jwk =
            serde_json::from_value(key).map_err(|_| Error::Invalid)?;
        jsonwebtoken::DecodingKey::from_jwk(&jwk).map_err(|_| Error::Invalid)?;
        let origin = result.endpoint.managed_origin()?;
        let hostname = result
            .tunnel_hostname
            .as_deref()
            .filter(|v| !v.is_empty() && v.len() <= 253)
            .ok_or(Error::Invalid)?;
        if zuse_remote::normalized_origin(&format!("https://{hostname}"))
            .map_err(|_| Error::Invalid)?
            != origin
            || result.connector_token.is_none()
        {
            return Err(Error::Invalid);
        }
        Ok(result)
    }
    async fn dpop<T: DeserializeOwned>(&self, path: &str, body: Value, device: &str) -> Result<T> {
        if !valid_id(device) {
            return Err(Error::Invalid);
        }
        let lease = self.lease().await?;
        let mut state = tokio::select! {biased;_=lease.cancel.cancelled()=>return Err(Error::Changed),state=self.session.lock()=>state};
        if state
            .as_ref()
            .is_none_or(|s| s.subject != lease.subject || s.cancel.is_cancelled())
        {
            *state = Some(Session {
                subject: lease.subject.clone(),
                cancel: lease.cancel.clone(),
                key: ProofKey::new()?,
                access: None,
                registered: None,
            });
        }
        let session = state.as_mut().unwrap();
        for attempt in 0..2 {
            lease.check(&self.account)?;
            if session.access.as_ref().is_none_or(|access| {
                access.expires <= Instant::now() + Duration::from_secs(30)
                    || access.wall_expiry <= chrono::Utc::now().timestamp_millis() + 30000
            }) {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Grant {
                    access_token: String,
                    expires_in: u64,
                }
                let url = format!("{}/v1/client/dpop-token", self.account.api_url());
                let proof = session.key.sign("POST", &url, None)?;
                let grant: Grant = self
                    .send(
                        &lease,
                        Method::POST,
                        "/v1/client/dpop-token",
                        None,
                        &self.authorization(&lease).await?,
                        Some(&proof),
                    )
                    .await?;
                if grant.access_token.is_empty()
                    || grant.access_token.len() > 16384
                    || !(31000..=86400000).contains(&grant.expires_in)
                {
                    return Err(Error::Invalid);
                }
                session.access = Some(Access {
                    token: grant.access_token,
                    expires: Instant::now() + Duration::from_millis(grant.expires_in),
                    wall_expiry: chrono::Utc::now().timestamp_millis() + grant.expires_in as i64,
                });
            }
            let access = &session.access.as_ref().unwrap().token;
            let authorization = format!("DPoP {access}");
            let operation=async {
                if session.registered.as_deref()!=Some(device){
                    let path="/v1/mobile/devices";let url=format!("{}{path}",self.account.api_url());let proof=session.key.sign("POST",&url,Some(access))?;
                    let _:Value=self.send(&lease,Method::POST,path,Some(&json!({"deviceId":format!("desktop-{device}"),"platform":"desktop","dpopJwk":session.key.jwk})),&authorization,Some(&proof)).await?;
                    session.registered=Some(device.into());
                }
                let url=format!("{}{path}",self.account.api_url());let proof=session.key.sign("POST",&url,Some(access))?;
                self.send(&lease,Method::POST,path,Some(&body),&authorization,Some(&proof)).await
            }.await;
            match operation {
                Err(Error::SignedOut) if attempt == 0 => session.access = None,
                value => return value,
            }
        }
        Err(Error::Rejected)
    }
    pub async fn connect(
        &self,
        environment: &str,
        device: &str,
        wire_protocol_version: u32,
    ) -> Result<ConnectGrant> {
        if !valid_id(environment) || wire_protocol_version == 0 {
            return Err(Error::Invalid);
        }
        let result: ConnectGrant = self
            .dpop(
                &format!("/v1/environments/{environment}/connect"),
                json!({"requireManaged":true,"wireProtocolVersion":wire_protocol_version}),
                device,
            )
            .await?;
        if result.connect_token.is_empty()
            || result.connect_token.len() > 16384
            || result.expires_at <= chrono::Utc::now().timestamp_millis()
        {
            return Err(Error::Invalid);
        }
        result.endpoint.managed_origin()?;
        if result
            .endpoint_candidates
            .as_ref()
            .is_some_and(|routes| routes.len() > 16)
        {
            return Err(Error::Invalid);
        }
        Ok(result)
    }
    pub async fn unlink(&self, environment: &str) -> Result<()> {
        if !valid_id(environment) {
            return Err(Error::Invalid);
        }
        let lease = self.lease().await?;
        let _: Value = self
            .bearer(
                &lease,
                Method::POST,
                "/v1/client/environment-unlink",
                Some(&json!({"environmentId":environment})),
            )
            .await?;
        Ok(())
    }
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}

#[cfg(test)]
mod tests;
