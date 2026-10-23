//! Host-authorized control plane access. No access token crosses the viewport RPC boundary.
use crate::auth::Auth;
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};
use zuse_model::cloud::{Head, Workspace};

pub const LIST: &str = zuse_protocol::methods::CLOUD_WORKSPACES;
pub const READ: &str = zuse_protocol::methods::CLOUD_HISTORY;
pub const UNARCHIVE: &str = zuse_protocol::methods::CLOUD_UNARCHIVE;
pub fn handles(method: &str) -> bool {
    matches!(method, LIST | READ | UNARCHIVE)
}
pub struct CloudApi {
    origin: reqwest::Url,
    http: reqwest::Client,
    history: zuse_cloud_projection::reader::Reader,
    loopback: bool,
}
impl CloudApi {
    pub fn new(origin: &str) -> Result<Self> {
        let history = zuse_cloud_projection::reader::Reader::new(origin)?;
        let origin = reqwest::Url::parse(origin)?;
        let loopback = origin.host_str().is_some_and(|h| {
            h == "localhost"
                || h.parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        Ok(Self {
            origin,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(20))
                .build()?,
            history,
            loopback,
        })
    }
    fn shared() -> Result<Arc<Self>> {
        static API: OnceLock<Result<Arc<CloudApi>, String>> = OnceLock::new();
        API.get_or_init(|| {
            Self::new(
                &std::env::var("ZUSE_CLOUD_API_URL")
                    .unwrap_or_else(|_| "https://api-rust.zuse.sh".into()),
            )
            .map(Arc::new)
            .map_err(|_| "Cloud service configuration is invalid.".into())
        })
        .clone()
        .map_err(anyhow::Error::msg)
    }
    async fn request(&self, token: &str, path: &[&str], body: Option<Value>) -> Result<Value> {
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("Cloud service configuration is invalid."))?
            .clear()
            .extend(path.iter().copied());
        let request = if let Some(body) = body {
            self.http.post(url).json(&body)
        } else {
            self.http.get(url)
        };
        let mut response = request
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Cloud service is unavailable. Try again."))?;
        ensure!(
            response.status().is_success(),
            "{}",
            match response.status().as_u16() {
                401 => "Sign in to access cloud workspaces.",
                403 => "You do not have access to this workspace.",
                409 => "The workspace is busy. Wait for its current operation to finish.",
                _ => "Cloud service is unavailable. Try again.",
            }
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("Cloud service is unavailable."))?
        {
            ensure!(
                bytes.len() + chunk.len() <= 8 * 1024 * 1024,
                "Cloud response is too large."
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
    pub async fn handle(&self, auth: &Auth, method: &str, params: Value) -> Result<Value> {
        // A dev/local profile has no production cloud identity. Loopback is solely useful for local acceptance.
        if !auth.workos_enabled() && !self.loopback {
            return if method == LIST {
                Ok(json!({"accountId":auth.user_id(),"workspaces":[]}))
            } else {
                anyhow::bail!("Sign in to access cloud workspaces.")
            };
        }
        let account = auth
            .user_id()
            .ok_or_else(|| anyhow::anyhow!("Sign in to access cloud workspaces."))?;
        let token = auth
            .access_token()
            .await
            .map_err(|_| anyhow::anyhow!("Sign in to access cloud workspaces."))?;
        let result = match method {
            LIST => {
                let value = self
                    .request(&token, &["v1", "cloud", "workspaces"], None)
                    .await?;
                let mut rows: Vec<Workspace> = serde_json::from_value(value["workspaces"].clone())?;
                rows.retain(|row| row.rpc_protocol.as_deref() == Some("zuse-rpc-v1"));
                json!({"accountId":account,"workspaces":rows})
            }
            READ => {
                let id = params["workspaceId"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Select a workspace."))?;
                let value = self
                    .request(&token, &["v1", "cloud", "workspaces", id], None)
                    .await?;
                let workspace: Workspace = serde_json::from_value(value["workspace"].clone())
                    .or_else(|_| serde_json::from_value(value.clone()))?;
                ensure!(
                    workspace.archived(),
                    "This workspace is no longer archived."
                );
                let session = params["sessionId"]
                    .as_str()
                    .unwrap_or(&workspace.initial_session_id);
                let mut head: Head = self
                    .history
                    .head(&token, id, session)
                    .await
                    .map_err(|_| anyhow::anyhow!("Saved cloud history is unavailable. Try again."))?
                    .ok_or_else(|| anyhow::anyhow!("Saved cloud history is not available yet."))?;
                // Fully materialize saved pages for the existing transcript renderer, without opening an execution channel.
                let mut entries = match &head.transcript.frame {
                    zuse_model::TranscriptFrame::Reset { reset } => reset.clone(),
                    _ => anyhow::bail!("Saved history is invalid."),
                };
                let mut before = head.older_message_sequence;
                let mut bytes = serde_json::to_vec(&entries)?.len();
                while let Some(n) = before {
                    let page = self.history.page(&token, &head, n).await?.ok_or_else(|| {
                        anyhow::anyhow!("Saved history is still being uploaded. Try again.")
                    })?;
                    before = page.older_message_sequence;
                    bytes += serde_json::to_vec(&page.entries)?.len();
                    ensure!(
                        bytes <= 64 * 1024 * 1024,
                        "This conversation is too large to open in this view."
                    );
                    let mut older = page.entries;
                    older.append(&mut entries);
                    entries = older;
                }
                head.transcript.replay_baseline =
                    Some(zuse_model::TranscriptBaseline::capture(&entries));
                head.transcript.frame = zuse_model::TranscriptFrame::Reset { reset: entries };
                head.older_message_sequence = None;
                json!({"accountId":account,"workspace":workspace,"head":head})
            }
            UNARCHIVE => {
                let id = params["workspaceId"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Select a workspace."))?;
                let command = params["commandId"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Restore request is missing."))?;
                let value = self
                    .request(
                        &token,
                        &["v1", "cloud", "workspaces", id, "unarchive"],
                        Some(json!({"workspaceId":id,"commandId":command})),
                    )
                    .await?;
                json!({"accountId":account,"workspace":value})
            }
            _ => anyhow::bail!("Unknown cloud operation."),
        };
        ensure!(
            auth.user_id().as_deref() == Some(&account),
            "Your account changed. Reopen this view."
        );
        Ok(result)
    }
}
pub async fn handle(auth: &Auth, method: &str, params: Value) -> Result<Value> {
    CloudApi::shared()?.handle(auth, method, params).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_require_a_valid_secure_cloud_origin() {
        for origin in [
            "http://api-rust.zuse.sh",
            "https://user:secret@api-rust.zuse.sh",
            "https://api-rust.zuse.sh/v1",
            "https://api-rust.zuse.sh/?token=secret",
        ] {
            assert!(CloudApi::new(origin).is_err());
        }
        assert!(CloudApi::new("https://api-rust.zuse.sh").is_ok());
        assert!(CloudApi::new("http://127.0.0.1:27655").is_ok());
    }
    #[tokio::test]
    async fn local_profiles_never_send_synthetic_identity_to_production() {
        let dir = tempfile::tempdir().unwrap();
        let auth = Auth::new(crate::auth::AuthConfig::new(
            "http://127.0.0.1:1",
            dir.path(),
        ));
        let api = CloudApi::new("https://api-rust.zuse.sh").unwrap();
        assert_eq!(
            api.handle(&auth, LIST, json!({})).await.unwrap()["workspaces"],
            json!([])
        );
        assert!(
            api.handle(&auth, READ, json!({"workspaceId":"other-account"}))
                .await
                .is_err()
        );
        assert!(
            api.handle(
                &auth,
                UNARCHIVE,
                json!({"workspaceId":"other-account","commandId":"command"})
            )
            .await
            .is_err()
        );
    }
}
