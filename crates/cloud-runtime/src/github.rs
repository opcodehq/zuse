//! Task-local GitHub credential broker. Only short-lived repository-scoped tokens
//! leave the authority. Tokens live in memory, never Git URLs/config/argv or disks.
mod cli;
pub use cli::{managed_cli_receipt, managed_cli_version, run_cli, run_profile};

use crate::{Authority, now_ms};
use anyhow::{Result, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use rsa::{Oaep, RsaPrivateKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{Mutex, watch},
};
use zeroize::Zeroizing;
use zuse_protocol::cloud::CloudRuntimeBootstrap;

pub struct GitRecipient {
    key: RsaPrivateKey,
    public_jwk: String,
    thumb: String,
    workspace: String,
    owner: String,
    actor: String,
    generation: i64,
}
impl GitRecipient {
    pub(crate) fn new(
        bootstrap: &CloudRuntimeBootstrap,
        key: RsaPrivateKey,
        public_jwk: String,
    ) -> Result<Self> {
        let jwk: Value = serde_json::from_str(&public_jwk)?;
        let thumb = B64.encode(Sha256::digest(serde_json::to_vec(
            &json!({"e":jwk["e"],"kty":"RSA","n":jwk["n"]}),
        )?));
        let actor = bootstrap
            .extra
            .get("actorId")
            .and_then(Value::as_str)
            .or_else(|| {
                (!bootstrap.zuse_account_id.starts_with("organization:"))
                    .then_some(bootstrap.zuse_account_id.as_str())
            })
            .ok_or_else(|| anyhow::anyhow!("github_task_actor_missing"))?;
        Ok(Self {
            key,
            public_jwk,
            thumb,
            workspace: bootstrap.workspace_id.clone(),
            owner: bootstrap.zuse_account_id.clone(),
            actor: actor.to_owned(),
            generation: bootstrap.runtime_generation,
        })
    }
    fn open(&self, response: &Value, request: &str, repository: &str) -> Result<Credential> {
        ensure!(
            response["protocolVersion"] == 1
                && response["requestId"] == request
                && response["runtimeGeneration"] == self.generation,
            "github_grant_binding_invalid"
        );
        let sealed = response["sealedGrant"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("github_grant_invalid"))?;
        let raw = open_secret(&self.key, sealed)?;
        let mut value: Value =
            serde_json::from_slice(&raw).map_err(|_| anyhow::anyhow!("github_grant_invalid"))?;
        let token = match value.get_mut("token").map(Value::take) {
            Some(Value::String(token)) if !token.is_empty() && token.len() <= 32768 => {
                Zeroizing::new(token)
            }
            _ => return Err(anyhow::anyhow!("github_grant_invalid")),
        };
        ensure!(
            value["protocolVersion"] == 1
                && value["requestId"] == request
                && value["workspaceId"] == self.workspace
                && value["ownerId"] == self.owner
                && value["actorId"] == self.actor
                && value["runtimeGeneration"] == self.generation
                && value["credentialKeyThumbprint"] == self.thumb
                && value["repository"] == repository,
            "github_grant_binding_invalid"
        );
        let expiry = value["expiresAtMs"]
            .as_i64()
            .filter(|e| *e > now_ms() + 5000 && *e <= now_ms() + 3_600_000)
            .ok_or_else(|| anyhow::anyhow!("github_grant_expired"))?;
        ensure!(
            response["expiresAtMs"] == expiry,
            "github_grant_binding_invalid"
        );
        let login = value["login"]
            .as_str()
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
            .ok_or_else(|| anyhow::anyhow!("github_grant_invalid"))?
            .to_owned();
        let email = value["email"]
            .as_str()
            .filter(|s| {
                s.len() <= 256
                    && s.ends_with("@users.noreply.github.com")
                    && !s.chars().any(char::is_control)
            })
            .ok_or_else(|| anyhow::anyhow!("github_grant_invalid"))?
            .to_owned();
        Ok(Credential {
            token,
            expiry,
            admitted_at: now_ms(),
            login,
            email,
        })
    }
}
pub(crate) fn open_secret(key: &RsaPrivateKey, sealed: &str) -> Result<Zeroizing<Vec<u8>>> {
    use aes_gcm::{
        Aes256Gcm, KeyInit, Nonce,
        aead::{Aead, Payload},
    };
    ensure!(sealed.len() <= 65536, "runtime_sealed_secret_invalid");
    let parts: Vec<_> = sealed.split('.').collect();
    ensure!(parts.len() == 5, "runtime_sealed_secret_invalid");
    let decode = |part: &str| {
        B64.decode(part)
            .map_err(|_| anyhow::anyhow!("runtime_sealed_secret_invalid"))
    };
    let header: Value = serde_json::from_slice(&decode(parts[0])?)
        .map_err(|_| anyhow::anyhow!("runtime_sealed_secret_invalid"))?;
    ensure!(
        header["alg"] == "RSA-OAEP-256"
            && header["enc"] == "A256GCM"
            && header.get("crit").is_none()
            && header.get("zip").is_none(),
        "runtime_sealed_secret_invalid"
    );
    let cek = Zeroizing::new(
        key.decrypt(Oaep::new::<Sha256>(), &decode(parts[1])?)
            .map_err(|_| anyhow::anyhow!("runtime_sealed_secret_invalid"))?,
    );
    let iv = decode(parts[2])?;
    let tag = decode(parts[4])?;
    ensure!(
        cek.len() == 32 && iv.len() == 12 && tag.len() == 16,
        "runtime_sealed_secret_invalid"
    );
    let mut ciphertext = decode(parts[3])?;
    ciphertext.extend_from_slice(&tag);
    Ok(Zeroizing::new(
        Aes256Gcm::new_from_slice(&cek)
            .map_err(|_| anyhow::anyhow!("runtime_sealed_secret_invalid"))?
            .decrypt(
                Nonce::from_slice(&iv),
                Payload {
                    msg: &ciphertext,
                    aad: parts[0].as_bytes(),
                },
            )
            .map_err(|_| anyhow::anyhow!("runtime_sealed_secret_invalid"))?,
    ))
}
struct Credential {
    token: Zeroizing<String>,
    expiry: i64,
    admitted_at: i64,
    login: String,
    email: String,
}
struct Broker {
    recipient: GitRecipient,
    authority: Authority,
    bootstrap: watch::Receiver<CloudRuntimeBootstrap>,
    repository: String,
    cached: Mutex<Option<Arc<Credential>>>,
    #[cfg(feature = "engine")]
    secret_filter: std::sync::OnceLock<Arc<zuse_providers::credentials::redaction::Secrets>>,
}
impl Broker {
    async fn credential(&self) -> Result<Arc<Credential>> {
        let bootstrap = self.bootstrap.borrow().clone();
        ensure!(
            bootstrap.workspace_id == self.recipient.workspace
                && bootstrap.zuse_account_id == self.recipient.owner
                && bootstrap.runtime_generation == self.recipient.generation
                && bootstrap
                    .extra
                    .get("actorId")
                    .and_then(Value::as_str)
                    .unwrap_or(&bootstrap.zuse_account_id)
                    == self.recipient.actor,
            "github_runtime_fenced"
        );
        let mut cache = self.cached.lock().await;
        if let Some(known) = cache
            .as_ref()
            .filter(|c| c.expiry > now_ms() + 60_000 && c.admitted_at > now_ms() - 10_000)
        {
            return Ok(known.clone());
        }
        let request = uuid::Uuid::new_v4().to_string();
        let response=self.authority.post(&bootstrap.workspace_id,&["runtime","github","grant"],&bootstrap.runtime_credential,&json!({"protocolVersion":1,"requestId":request,"runtimeGeneration":self.recipient.generation,"credentialPublicJwk":self.recipient.public_jwk})).await?;
        let credential = Arc::new(self.recipient.open(&response, &request, &self.repository)?);
        #[cfg(feature = "engine")]
        if let Some(filter) = self.secret_filter.get() {
            filter.record(&credential.token)?;
        }
        *cache = Some(credential.clone());
        Ok(credential)
    }
    async fn respond(&self, input: &[u8], operation: &str) -> Result<Zeroizing<Vec<u8>>> {
        let fields = parse_input(input)?;
        ensure!(
            fields.get("protocol").map(String::as_str) == Some("https")
                && fields.get("host").map(String::as_str) == Some("github.com")
                && fields.get("path").is_some_and(|path| path
                    .strip_suffix(".git")
                    .unwrap_or(path)
                    .eq_ignore_ascii_case(&self.repository)),
            "github_credential_scope_mismatch"
        );
        match operation {
            "get" => {
                let credential = self.credential().await?;
                ensure!(
                    fields
                        .get("username")
                        .is_none_or(|username| username == &credential.login),
                    "github_credential_scope_mismatch"
                );
                Ok(Zeroizing::new(
                    format!(
                        "username={}\npassword={}\n\n",
                        credential.login,
                        credential.token.as_str()
                    )
                    .into_bytes(),
                ))
            }
            "erase" => {
                *self.cached.lock().await = None;
                Ok(Zeroizing::new(vec![]))
            }
            "store" => Ok(Zeroizing::new(vec![])),
            _ => Err(anyhow::anyhow!("github_credential_operation_invalid")),
        }
    }
}
pub struct GithubAuth {
    broker: Arc<Broker>,
    socket: PathBuf,
    directory: PathBuf,
    helper: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for GithubAuth {
    fn drop(&mut self) {
        self.task.abort();
        cli::clear_profile(&self.directory);
        let _ = std::fs::remove_file(&self.socket);
        // Directory is generated privately by this owner; includes only helper
        // scripts and an isolated gh config, never reusable task artifacts.
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
impl GithubAuth {
    #[cfg(feature = "engine")]
    pub fn set_secret_filter(
        &self,
        filter: Arc<zuse_providers::credentials::redaction::Secrets>,
    ) -> Result<()> {
        if let Ok(cache) = self.broker.cached.try_lock() {
            if let Some(credential) = cache.as_ref() {
                filter.record(&credential.token)?;
            }
        }
        let _ = self.broker.secret_filter.set(filter);
        Ok(())
    }
    pub fn start(
        recipient: GitRecipient,
        authority: Authority,
        bootstrap: watch::Receiver<CloudRuntimeBootstrap>,
        repository: &str,
        executable: &Path,
    ) -> Result<Self> {
        ensure!(
            executable.is_absolute() && executable.is_file(),
            "github_credential_helper_missing"
        );
        ensure!(valid_repository(repository), "github_repository_invalid");
        let directory = std::env::temp_dir().join(format!("zuse-git-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let socket = directory.join("git.sock");
        let listener = UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        let broker = Arc::new(Broker {
            recipient,
            authority,
            bootstrap,
            repository: repository.into(),
            cached: Mutex::new(None),
            #[cfg(feature = "engine")]
            secret_filter: std::sync::OnceLock::new(),
        });
        let source = broker.clone();
        let task = tokio::spawn(async move {
            let slots = Arc::new(tokio::sync::Semaphore::new(4));
            let mut requests = tokio::task::JoinSet::new();
            loop {
                let Ok(permit) = slots.clone().acquire_owned().await else {
                    break;
                };
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let source = source.clone();
                requests.spawn(async move {
                    let _permit = permit;
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                        let length = stream.read_u32().await? as usize;
                        ensure!(length <= 8192, "github_credential_request_too_large");
                        let mut bytes = Zeroizing::new(vec![0; length]);
                        stream.read_exact(&mut bytes).await?;
                        let value: Value = serde_json::from_slice(&bytes)?;
                        let input = value["input"]
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("github_credential_request_invalid"))?;
                        let operation = value["operation"].as_str().unwrap_or("");
                        let response = source.respond(input.as_bytes(), operation).await?;
                        stream.write_u32(response.len() as u32).await?;
                        stream.write_all(&response).await?;
                        stream.shutdown().await?;
                        Ok::<(), anyhow::Error>(())
                    })
                    .await;
                });
                while requests.try_join_next().is_some() {}
            }
        });
        let helper = format!(
            "!{} github-credential --socket {}",
            shell_quote(executable),
            shell_quote(&socket)
        );
        Ok(Self {
            broker,
            socket,
            directory,
            helper,
            task,
        })
    }
    pub fn helper_command(&self) -> &str {
        &self.helper
    }
    pub async fn verify(&self) -> Result<()> {
        self.broker.credential().await.map(|_| ())
    }
    pub async fn configure_checkout(&self, checkout: &Path) -> Result<()> {
        let credential = self.broker.credential().await?;
        for args in [
            vec![
                "config",
                "--local",
                "--replace-all",
                "credential.helper",
                "",
            ],
            vec![
                "config",
                "--local",
                "--add",
                "credential.helper",
                self.helper.as_str(),
            ],
            vec!["config", "--local", "credential.useHttpPath", "true"],
            vec!["config", "--local", "user.name", credential.login.as_str()],
            vec!["config", "--local", "user.email", credential.email.as_str()],
        ] {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(checkout)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()?;
            ensure!(status.success(), "github_credential_configuration_failed");
        }
        Ok(())
    }
}
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}
fn valid_repository(repository: &str) -> bool {
    let parts: Vec<_> = repository.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|s| {
            !s.is_empty()
                && *s != "."
                && *s != ".."
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}
fn parse_input(input: &[u8]) -> Result<std::collections::BTreeMap<String, String>> {
    ensure!(input.len() <= 8192, "github_credential_request_too_large");
    let text = std::str::from_utf8(input)?;
    let mut fields = std::collections::BTreeMap::new();
    for line in text.lines() {
        if line.is_empty() {
            break;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("github_credential_request_invalid"))?;
        ensure!(
            !value.contains(['\0', '\r']),
            "github_credential_request_invalid"
        );
        if matches!(key, "protocol" | "host" | "path" | "username") {
            ensure!(
                !fields.contains_key(key),
                "github_credential_request_invalid"
            );
            fields.insert(key.to_owned(), value.to_owned());
        }
    }
    Ok(fields)
}
pub fn repository_from_url(value: &str) -> Result<String> {
    let url = reqwest::Url::parse(value)?;
    ensure!(
        url.scheme() == "https"
            && url.host_str() == Some("github.com")
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "github_repository_invalid"
    );
    let path = url.path().trim_start_matches('/');
    let repository = path.strip_suffix(".git").unwrap_or(path);
    ensure!(valid_repository(repository), "github_repository_invalid");
    Ok(repository.to_owned())
}
pub fn run_helper_cli(socket: &Path, operation: &str) -> Result<()> {
    use std::io::{Read, Write};
    let mut input = Zeroizing::new(Vec::new());
    std::io::stdin().take(8193).read_to_end(&mut input)?;
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(helper(socket, operation, &input))
        .map_err(|_| anyhow::anyhow!("github_credential_unavailable"))?;
    std::io::stdout().lock().write_all(&result)?;
    Ok(())
}
pub async fn helper(socket: &Path, operation: &str, input: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(
        matches!(operation, "get" | "store" | "erase"),
        "github_credential_operation_invalid"
    );
    parse_input(input)?;
    tokio::time::timeout(std::time::Duration::from_secs(35), async {
        let request = Zeroizing::new(serde_json::to_vec(
            &json!({"operation":operation,"input":std::str::from_utf8(input)?}),
        )?);
        ensure!(request.len() <= 8192, "github_credential_request_too_large");
        let mut stream = UnixStream::connect(socket).await?;
        stream.write_u32(request.len() as u32).await?;
        stream.write_all(&request).await?;
        stream.shutdown().await?;
        let length = stream.read_u32().await? as usize;
        ensure!(length <= 65536, "github_credential_response_invalid");
        let mut response = Zeroizing::new(vec![0; length]);
        stream.read_exact(&mut response).await?;
        Ok(response)
    })
    .await
    .map_err(|_| anyhow::anyhow!("github_credential_timeout"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::{RsaPublicKey, traits::PublicKeyParts};
    fn seal(key: &RsaPrivateKey, value: &Value) -> String {
        use aes_gcm::{
            Aes256Gcm, KeyInit, Nonce,
            aead::{Aead, Payload},
        };
        let header =
            B64.encode(serde_json::to_vec(&json!({"alg":"RSA-OAEP-256","enc":"A256GCM"})).unwrap());
        let cek = [52u8; 32];
        let nonce = [53u8; 12];
        let wrapped = RsaPublicKey::from(key)
            .encrypt(&mut rand::thread_rng(), Oaep::new::<Sha256>(), &cek)
            .unwrap();
        let sealed = Aes256Gcm::new_from_slice(&cek)
            .unwrap()
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &serde_json::to_vec(value).unwrap(),
                    aad: header.as_bytes(),
                },
            )
            .unwrap();
        format!(
            "{}.{}.{}.{}.{}",
            header,
            B64.encode(wrapped),
            B64.encode(nonce),
            B64.encode(&sealed[..sealed.len() - 16]),
            B64.encode(&sealed[sealed.len() - 16..])
        )
    }
    fn bootstrap() -> CloudRuntimeBootstrap {
        serde_json::from_value(json!({"workspaceId":"task","zuseAccountId":"organization:team","actorId":"actor","chatId":"chat","initialSessionId":"session","runtimeCredential":"runtime-secret","runtimeGeneration":4,"gatewayEpoch":4,"runtimeCredentialExpiresAt":now_ms()+3600000,"gatewayUrl":"wss://api.example.test/v1/cloud/workspaces/task/gateway","gatewayProtocol":"zuse-cloud-gateway-v3","sealedTranscriptKey":"unused"})).unwrap()
    }
    fn recipient() -> GitRecipient {
        let key = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
        let public = json!({"kty":"RSA","n":B64.encode(key.n().to_bytes_be()),"e":B64.encode(key.e().to_bytes_be())});
        GitRecipient::new(&bootstrap(), key, public.to_string()).unwrap()
    }
    fn payload(recipient: &GitRecipient) -> Value {
        json!({"protocolVersion":1,"requestId":"request","workspaceId":"task","ownerId":"organization:team","actorId":"actor","repository":"team/project","runtimeGeneration":4,"credentialKeyThumbprint":recipient.thumb,"expiresAtMs":now_ms()+600000,"token":"scoped-token","login":"actor","email":"42+actor@users.noreply.github.com"})
    }
    #[test]
    fn git_grants_bind_recipient_actor_generation_repository_and_expiry() {
        let recipient = recipient();
        let valid = payload(&recipient);
        let response = |value: &Value| json!({"protocolVersion":1,"requestId":"request","runtimeGeneration":4,"expiresAtMs":value["expiresAtMs"],"sealedGrant":seal(&recipient.key,value)});
        let credential = recipient
            .open(&response(&valid), "request", "team/project")
            .unwrap();
        assert_eq!(credential.token.as_str(), "scoped-token");
        for (field, value) in [
            ("actorId", json!("installing-admin")),
            ("ownerId", json!("foreign-account")),
            ("repository", json!("team/other")),
            ("runtimeGeneration", json!(3)),
            ("credentialKeyThumbprint", json!("foreign-key")),
            ("expiresAtMs", json!(now_ms() - 1)),
        ] {
            let mut altered = valid.clone();
            altered[field] = value;
            assert!(
                recipient
                    .open(&response(&altered), "request", "team/project")
                    .is_err()
            );
        }
        let mut tampered = response(&valid);
        let mut encoded = tampered["sealedGrant"].as_str().unwrap().to_owned();
        encoded.push('a');
        tampered["sealedGrant"] = json!(encoded);
        assert!(
            recipient
                .open(&tampered, "request", "team/project")
                .is_err()
        );
    }
    #[test]
    fn git_helper_rejects_ambiguous_scope_and_never_retains_incoming_password() {
        let fields=parse_input(b"protocol=https\nhost=github.com\npath=team/project.git\npassword=NEVER-PERSIST\nwwwauth[]=first\nwwwauth[]=second\n\n").unwrap();
        assert_eq!(fields.len(), 3);
        assert!(!fields.contains_key("password"));
        assert!(parse_input(b"protocol=https\nhost=github.com\nhost=evil.example\n\n").is_err());
        assert!(parse_input(&vec![b'a'; 8193]).is_err());
        for repository in [
            "../project",
            "team/../project",
            "team/",
            "/project",
            "team/project?token=bad",
        ] {
            assert!(!valid_repository(repository));
        }
        assert!(valid_repository("team/project"));
    }
    #[tokio::test]
    async fn credential_helper_socket_only_delivers_the_pinned_repository_and_does_not_store() {
        let recipient_key = recipient();
        let credential = Arc::new(Credential {
            token: Zeroizing::new("scoped-token".into()),
            expiry: now_ms() + 600000,
            admitted_at: now_ms(),
            login: "actor".into(),
            email: "42+actor@users.noreply.github.com".into(),
        });
        let (_, updates) = watch::channel(bootstrap());
        let auth = GithubAuth::start(
            recipient(),
            Authority::new("https://api.example.test/").unwrap(),
            updates.clone(),
            "team/project",
            &std::env::current_exe().unwrap(),
        )
        .unwrap();
        *auth.broker.cached.lock().await = Some(credential.clone());
        let input = b"protocol=https\nhost=github.com\npath=team/project.git\n\n";
        let over_socket = helper(&auth.socket, "get", input).await.unwrap();
        assert_eq!(
            over_socket.as_slice(),
            b"username=actor\npassword=scoped-token\n\n"
        );
        let checkout = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .arg("init")
                .arg(checkout.path())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        );
        auth.configure_checkout(checkout.path()).await.unwrap();
        let config = std::fs::read_to_string(checkout.path().join(".git/config")).unwrap();
        assert!(!config.contains("scoped-token"));
        assert!(config.contains("github-credential"));
        let broker = Broker {
            recipient: recipient_key,
            authority: Authority::new("https://api.example.test/").unwrap(),
            bootstrap: updates,
            repository: "team/project".into(),
            cached: Mutex::new(Some(credential)),
            #[cfg(feature = "engine")]
            secret_filter: std::sync::OnceLock::new(),
        };
        let response = broker
            .respond(
                b"protocol=https\nhost=github.com\npath=team/project.git\n\n",
                "get",
            )
            .await
            .unwrap();
        assert_eq!(
            response.as_slice(),
            b"username=actor\npassword=scoped-token\n\n"
        );
        for input in [
            b"protocol=https\nhost=evil.example\npath=team/project.git\n\n".as_slice(),
            b"protocol=http\nhost=github.com\npath=team/project.git\n\n",
            b"protocol=https\nhost=github.com\npath=team/other.git\n\n",
            b"protocol=https\nhost=github.com\n\n",
        ] {
            assert!(broker.respond(input, "get").await.is_err());
        }
        broker.respond(b"protocol=https\nhost=github.com\npath=team/project.git\npassword=DO-NOT-STORE\n\n","store").await.unwrap();
        broker
            .respond(
                b"protocol=https\nhost=github.com\npath=team/project.git\n\n",
                "erase",
            )
            .await
            .unwrap();
        assert!(broker.cached.lock().await.is_none());
    }
}
