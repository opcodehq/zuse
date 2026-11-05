//! Grok's external-auth command receives access-only grants through a private socket.
//! The authority retains refresh tokens; no credential is written by this bridge.
use crate::grants::{Credential, ProviderBroker};
use anyhow::{Result, ensure};
use async_trait::async_trait;
use std::{os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};
use zeroize::Zeroizing;
use zuse_providers::{
    HarnessError,
    credentials::{ChildEnvironment, CredentialResolver},
};

/// The bridge serves access credentials only. A source owns renewal and account
/// fencing; the guest broker and isolated authority discovery use the same wire.
#[async_trait]
pub trait GrokCredentialSource: Send + Sync {
    async fn token(&self, expired: bool) -> Result<Credential>;
    async fn cached(&self) -> Result<Credential> {
        self.token(false).await
    }
    async fn resume(&self) -> Result<()> {
        Ok(())
    }
}
#[async_trait]
impl GrokCredentialSource for ProviderBroker {
    async fn token(&self, expired: bool) -> Result<Credential> {
        self.resolve_credential(expired, None).await
    }
    async fn cached(&self) -> Result<Credential> {
        self.credential().await
    }
    async fn resume(&self) -> Result<()> {
        self.resume_credential().await
    }
}

pub struct GrokAuth {
    source: Arc<dyn GrokCredentialSource>,
    directory: PathBuf,
    socket: PathBuf,
    helper: PathBuf,
    profile: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

/// Registering another harness must not create a Grok profile or fetch its grant.
pub struct LazyGrokAuth {
    source: Arc<ProviderBroker>,
    helper: PathBuf,
    profile: PathBuf,
    auth: tokio::sync::OnceCell<Arc<GrokAuth>>,
}
impl LazyGrokAuth {
    pub fn new(source: Arc<ProviderBroker>, helper: PathBuf, profile: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            source,
            helper,
            profile,
            auth: tokio::sync::OnceCell::new(),
        })
    }
    async fn auth(&self) -> std::result::Result<&Arc<GrokAuth>, HarnessError> {
        self.auth
            .get_or_try_init(|| async {
                GrokAuth::start(
                    self.source.clone(),
                    self.helper.clone(),
                    self.profile.clone(),
                )
                .map_err(|_| HarnessError::Protocol("grok-auth-reconnecting".into()))
            })
            .await
    }
}
#[async_trait]
impl CredentialResolver for LazyGrokAuth {
    async fn before_turn(&self) -> std::result::Result<ChildEnvironment, HarnessError> {
        self.auth().await?.before_turn().await
    }
    async fn environment(&self) -> std::result::Result<ChildEnvironment, HarnessError> {
        self.auth().await?.environment().await
    }
}
impl Drop for GrokAuth {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_dir(&self.directory);
    }
}
impl GrokAuth {
    pub fn start(
        source: Arc<ProviderBroker>,
        helper: PathBuf,
        profile: PathBuf,
    ) -> Result<Arc<Self>> {
        Self::start_with_source(source, helper, profile)
    }
    pub fn start_with_source(
        source: Arc<dyn GrokCredentialSource>,
        helper: PathBuf,
        profile: PathBuf,
    ) -> Result<Arc<Self>> {
        ensure!(
            helper.is_absolute() && helper.is_file(),
            "grok_auth_helper_missing"
        );
        ensure!(profile.is_absolute(), "grok_profile_path_invalid");
        std::fs::create_dir_all(&profile)?;
        ensure!(
            !std::fs::symlink_metadata(&profile)?
                .file_type()
                .is_symlink(),
            "grok_profile_path_invalid"
        );
        std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o700))?;
        let directory = std::env::temp_dir().join(format!("zuse-auth-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let socket = directory.join("grok.sock");
        let listener = match UnixListener::bind(&socket) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = std::fs::remove_dir(&directory);
                return Err(error.into());
            }
        };
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        let task = tokio::spawn(serve(listener, source.clone()));
        Ok(Arc::new(Self {
            source,
            directory,
            socket,
            helper,
            profile,
            task,
        }))
    }
    pub fn sessions_root(&self) -> PathBuf {
        self.profile.join("sessions")
    }
}

use std::os::unix::fs::DirBuilderExt;
async fn serve(listener: UnixListener, source: Arc<dyn GrokCredentialSource>) {
    // Acquire before accept: stalled callers cannot create unbounded tasks.
    let slots = Arc::new(tokio::sync::Semaphore::new(4));
    let mut requests = tokio::task::JoinSet::new();
    loop {
        let Ok(permit) = slots.clone().acquire_owned().await else {
            break;
        };
        let Ok((stream, _)) = listener.accept().await else {
            break;
        };
        let source = source.clone();
        requests.spawn(async move {
            let _permit = permit;
            let _ = tokio::time::timeout(Duration::from_secs(4), respond(stream, source)).await;
        });
        while requests.try_join_next().is_some() {}
    }
}
async fn respond(mut stream: UnixStream, source: Arc<dyn GrokCredentialSource>) -> Result<()> {
    let mode = stream.read_u8().await?;
    ensure!(mode <= 1, "invalid_auth_request");
    let credential = source.token(mode == 1).await?;
    ensure!(
        credential.kind == "oauth-token" && credential.method == "subscription",
        "grok_auth_method_mismatch"
    );
    let ttl = (credential.expires_at - crate::now_ms()) / 1000;
    ensure!(
        ttl > 60 && credential.secret.len() <= 32768,
        "grok_auth_expired"
    );
    let output = Zeroizing::new(
        serde_json::json!({
            "access_token": credential.secret.as_str(), "expires_in": ttl,
        })
        .to_string(),
    );
    stream.write_all(output.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

#[async_trait]
impl CredentialResolver for GrokAuth {
    async fn before_turn(&self) -> std::result::Result<ChildEnvironment, HarnessError> {
        self.source
            .resume()
            .await
            .map_err(|_| HarnessError::Protocol("grok-auth-reconnecting".into()))?;
        self.environment().await
    }
    async fn environment(&self) -> std::result::Result<ChildEnvironment, HarnessError> {
        let failure = || HarnessError::Protocol("grok-auth-reconnecting".into());
        let credential = self.source.cached().await.map_err(|_| failure())?;
        let environment = match (credential.method.as_str(), credential.kind.as_str()) {
            ("api-key", "api-key") => Ok(ChildEnvironment::new("XAI_API_KEY", credential.secret)),
            ("subscription", "oauth-token") => Ok(ChildEnvironment::new(
                "GROK_AUTH_PROVIDER_COMMAND",
                Zeroizing::new(format!(
                    "exec '{}'",
                    self.helper.to_string_lossy().replace('\'', "'\\''")
                )),
            )
            .and(
                "ZUSE_GROK_AUTH_SOCKET",
                Zeroizing::new(self.socket.to_string_lossy().into_owned()),
            )
            .and("BROWSER", Zeroizing::new("/bin/false".into()))),
            _ => Err(failure()),
        }?;
        Ok(environment
            .and(
                "GROK_HOME",
                Zeroizing::new(self.profile.to_string_lossy().into_owned()),
            )
            .and("BROWSER", Zeroizing::new("/bin/false".into())))
    }
}

/// Called by the installed helper before runtime tracing/CLI initialization.
pub async fn helper(socket: PathBuf, expired: bool) -> Result<Zeroizing<Vec<u8>>> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut stream = UnixStream::connect(socket).await?;
        stream.write_u8(u8::from(expired)).await?;
        stream.shutdown().await?;
        let mut bytes = Zeroizing::new(Vec::new());
        stream.take(65537).read_to_end(&mut bytes).await?;
        ensure!(bytes.len() <= 65536, "grok_auth_response_invalid");
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        ensure!(
            value["access_token"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
                && value["expires_in"].as_i64().is_some_and(|ttl| ttl > 60)
                && value.get("refresh_token").is_none(),
            "grok_auth_response_invalid"
        );
        Ok(bytes)
    })
    .await
    .map_err(|_| anyhow::anyhow!("grok_auth_timeout"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Source {
        modes: Mutex<Vec<bool>>,
        fail_refresh: bool,
    }
    #[async_trait]
    impl GrokCredentialSource for Source {
        async fn token(&self, expired: bool) -> Result<Credential> {
            self.modes.lock().unwrap().push(expired);
            ensure!(!(expired && self.fail_refresh), "revoked_test_connection");
            Ok(Credential {
                method: "subscription".into(),
                kind: "oauth-token".into(),
                secret: Zeroizing::new("synthetic-execution-token".into()),
                expires_at: crate::now_ms() + 600_000,
                account_id: Some("selected-account".into()),
                plan_type: None,
            })
        }
    }

    #[tokio::test]
    async fn external_auth_returns_access_only_and_failed_renewal_never_returns_stale_token() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("auth.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let source = Arc::new(Source {
            modes: Mutex::new(vec![]),
            fail_refresh: true,
        });
        let server =
            tokio_util::task::AbortOnDropHandle::new(tokio::spawn(serve(listener, source.clone())));
        let output = helper(socket.clone(), false).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["access_token"], "synthetic-execution-token");
        assert!(value["expires_in"].as_i64().unwrap() > 60);
        assert_eq!(value.as_object().unwrap().len(), 2);
        assert!(helper(socket, true).await.is_err());
        assert_eq!(*source.modes.lock().unwrap(), vec![false, true]);
        // This bridge creates only a socket, never auth.json or a refresh-token file.
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        drop(server);
    }

    #[tokio::test]
    async fn malformed_local_request_cannot_trigger_authority_refresh() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("auth.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let source = Arc::new(Source {
            modes: Mutex::new(vec![]),
            fail_refresh: false,
        });
        let server =
            tokio_util::task::AbortOnDropHandle::new(tokio::spawn(serve(listener, source.clone())));
        let mut client = UnixStream::connect(&socket).await.unwrap();
        client.write_u8(2).await.unwrap();
        client.shutdown().await.unwrap();
        let mut result = vec![];
        client.read_to_end(&mut result).await.unwrap();
        assert!(result.is_empty());
        assert!(source.modes.lock().unwrap().is_empty());
        assert!(helper(socket, false).await.is_ok());
        drop(server);
    }
}
