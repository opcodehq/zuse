//! Leased verification/device login. A revoked or superseded connection cannot be promoted.
use super::*;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::mpsc,
};

pub struct AuthJob {
    pub agent: String,
    pub method: String,
    pub secret: Zeroizing<String>,
    pub timeout_seconds: u64,
    pub refresh: bool,
}
pub struct DeviceChallenge {
    pub url: String,
    pub code: String,
}
pub struct VerifiedConnection {
    pub credential: Zeroizing<String>,
    pub account_label: Option<String>,
    pub account_id: Option<String>,
    pub expires_at: Option<i64>,
}
#[async_trait::async_trait]
pub trait AuthDriver: Send + Sync {
    async fn execute(
        &self,
        job: AuthJob,
        events: mpsc::Sender<DeviceChallenge>,
    ) -> Result<VerifiedConnection, &'static str>;
}
pub struct CommandDriver {
    script: PathBuf,
}
impl CommandDriver {
    pub fn deployment_ready(script: &Path) -> anyhow::Result<()> {
        Self::new(script)?;
        for agent in ["CLAUDE", "CODEX", "GROK"] {
            let binary = std::env::var_os(format!("ZUSE_AUTH_{agent}_BINARY"))
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("Pinned authority executable is not configured"))?;
            anyhow::ensure!(
                binary.is_absolute() && binary.is_file(),
                "Pinned authority executable is missing"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                anyhow::ensure!(
                    binary.metadata()?.permissions().mode() & 0o111 != 0,
                    "Authority executable is not executable"
                );
            }
        }
        Ok(())
    }
    pub fn new(script: &Path) -> anyhow::Result<Self> {
        anyhow::ensure!(
            script.is_absolute() && script.is_file(),
            "Credential authority requires an installed auth driver"
        );
        Ok(Self {
            script: script.to_owned(),
        })
    }
}
struct ProcessGroup {
    id: Option<u32>,
    finished: bool,
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if !self.finished {
            if let Some(id) = self.id {
                let _ = std::process::Command::new("/bin/kill")
                    .args(["-TERM", "--", &format!("-{id}")])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
    }
}
fn challenge(agent: &str, url: &str, code: &str) -> Result<DeviceChallenge, &'static str> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "invalid_verification_url")?;
    let host = parsed.host_str().unwrap_or_default();
    let allowed = match agent {
        "codex" => matches!(host, "auth.openai.com" | "chatgpt.com"),
        "grok" => matches!(host, "auth.x.ai" | "accounts.x.ai" | "grok.com"),
        _ => false,
    };
    if !allowed
        || parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
        || parsed.port().is_some_and(|p| p != 443)
        || !(4..=32).contains(&code.len())
        || !code.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        return Err("invalid_verification_url");
    }
    Ok(DeviceChallenge {
        url: url.into(),
        code: code.into(),
    })
}
fn verified(job: &AuthJob, credential: Value) -> Result<VerifiedConnection, &'static str> {
    if credential["formatVersion"] != 1
        || credential["agent"] != job.agent
        || credential["method"] != job.method
    {
        return Err("auth_credentials_invalid");
    }
    if job.method == "device-code" {
        for key in ["accessToken", "refreshToken", "providerAccountId"] {
            if !credential[key]
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.len() <= 32768)
            {
                return Err("auth_credentials_invalid");
            }
        }
        if !credential["expiresAt"]
            .as_i64()
            .is_some_and(|t| t > chrono::Utc::now().timestamp_millis() + 60_000)
        {
            return Err("invalid_auth_expiry");
        }
    } else if !credential["secret"]
        .as_str()
        .is_some_and(|s| !s.is_empty() && s.len() <= 32768)
    {
        return Err("auth_credentials_invalid");
    }
    let label = credential["accountLabel"]
        .as_str()
        .map(str::to_owned)
        .filter(|s| s.len() <= 300);
    let account = credential["providerAccountId"]
        .as_str()
        .map(str::to_owned)
        .filter(|s| s.len() <= 256);
    Ok(VerifiedConnection {
        account_label: label,
        account_id: account,
        expires_at: credential["expiresAt"].as_i64(),
        credential: Zeroizing::new(credential.to_string()),
    })
}
#[async_trait::async_trait]
impl AuthDriver for CommandDriver {
    async fn execute(
        &self,
        job: AuthJob,
        events: mpsc::Sender<DeviceChallenge>,
    ) -> Result<VerifiedConnection, &'static str> {
        let mut command = tokio::process::Command::new("/usr/bin/python3");
        command
            .arg(&self.script)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("LANG", "C.UTF-8")
            .current_dir("/tmp")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        for name in [
            "ZUSE_AUTH_CODEX_BINARY",
            "ZUSE_AUTH_CLAUDE_BINARY",
            "ZUSE_AUTH_GROK_BINARY",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.as_std_mut().process_group(0);
        }
        let mut child = command.spawn().map_err(|_| "auth_tool_unavailable")?;
        let mut group = ProcessGroup {
            id: child.id(),
            finished: false,
        };
        let request = if job.refresh {
            let credential: Value =
                serde_json::from_str(&job.secret).map_err(|_| "auth_credentials_invalid")?;
            json!({"agent":job.agent,"method":job.method,"action":"refresh","credential":credential,"timeoutSeconds":job.timeout_seconds})
        } else {
            json!({"agent":job.agent,"method":job.method,"secret":job.secret.as_str(),"timeoutSeconds":job.timeout_seconds})
        };
        let input = Zeroizing::new(request.to_string());
        let mut stdin = child.stdin.take().ok_or("auth_driver_unavailable")?;
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|_| "auth_driver_unavailable")?;
        stdin
            .shutdown()
            .await
            .map_err(|_| "auth_driver_unavailable")?;
        drop(stdin);
        let stdout = child.stdout.take().ok_or("auth_driver_unavailable")?;
        let mut lines = BufReader::new(stdout.take(262144)).lines();
        let execution = async {
            let mut result = None;
            while let Some(line) = lines
                .next_line()
                .await
                .map_err(|_| "auth_driver_unavailable")?
            {
                if line.len() > 131072 {
                    return Err("auth_driver_response_invalid");
                }
                let value: Value =
                    serde_json::from_str(&line).map_err(|_| "auth_driver_response_invalid")?;
                match value["event"].as_str() {
                    Some("device-code") => {
                        let event = challenge(
                            &job.agent,
                            value["verificationUrl"]
                                .as_str()
                                .ok_or("invalid_verification_url")?,
                            value["verificationCode"]
                                .as_str()
                                .ok_or("invalid_verification_url")?,
                        )?;
                        events.send(event).await.map_err(|_| "auth_cancelled")?;
                    }
                    Some("verified") => {
                        if result.is_some() {
                            return Err("auth_driver_response_invalid");
                        }
                        result = Some(verified(&job, value["credential"].clone())?);
                    }
                    Some("error") => {
                        return Err(match value["code"].as_str() {
                            Some("auth_tool_unavailable") => "auth_tool_unavailable",
                            Some("auth_login_expired") => "auth_login_expired",
                            Some("invalid_auth_expiry") => "invalid_auth_expiry",
                            _ => "auth_verification_failed",
                        });
                    }
                    _ => return Err("auth_driver_response_invalid"),
                }
            }
            let status = child.wait().await.map_err(|_| "auth_driver_unavailable")?;
            if !status.success() {
                return Err("auth_verification_failed");
            }
            group.finished = true;
            result.ok_or("auth_credentials_missing")
        };
        tokio::time::timeout(Duration::from_secs(job.timeout_seconds + 5), execution)
            .await
            .map_err(|_| "auth_operation_timeout")?
    }
}
impl ConnectionControl {
    pub async fn work_once(&self, driver: &dyn AuthDriver) -> Result<bool, ApiError> {
        let now = chrono::Utc::now().timestamp_millis();
        self.cleanup_rotations(now).await?;
        self.expire_rotation_dispatches(now).await?;
        let accounts = self.accounts.iter().cloned().collect::<Vec<_>>();
        // Recover dead claims without launching a second login against an uncertain process.
        sqlx::query("UPDATE cloud_auth_operations SET state='error',last_error_code='auth_worker_interrupted',operation_ciphertext=NULL,updated_at=$1 WHERE state='authorizing' AND lease_expires_at<=$1 AND owner_id=ANY($2)").bind(now).bind(&accounts).execute(&self.pool).await?;
        sqlx::query("UPDATE cloud_auth_operations SET state='expired',operation_ciphertext=NULL,updated_at=$1 WHERE state IN ('queued','authorizing') AND expires_at<=$1 AND owner_id=ANY($2)").bind(now).bind(&accounts).execute(&self.pool).await?;
        sqlx::query("UPDATE cloud_agent_connections c SET state='error',credential_ciphertext=NULL,last_error_code='auth_operation_expired',updated_at=$1 WHERE state='pending' AND owner_id=ANY($2) AND EXISTS(SELECT 1 FROM cloud_auth_operations o WHERE o.connection_id=c.connection_id AND o.state IN ('error','expired','cancelled')) AND NOT EXISTS(SELECT 1 FROM cloud_auth_operations o WHERE o.connection_id=c.connection_id AND o.state IN ('queued','authorizing'))").bind(now).bind(&accounts).execute(&self.pool).await?;
        let mut tx = self.pool.begin().await?;
        let row:Option<(String,String,String,String,String,Option<String>,i64,i64,i64,Option<String>)>=sqlx::query_as("SELECT o.operation_id,o.owner_id,o.connection_id,c.agent,c.method,c.credential_ciphertext,c.version,o.expires_at,c.authority_epoch,o.operation_ciphertext FROM cloud_auth_operations o JOIN cloud_agent_connections c ON c.connection_id=o.connection_id AND c.owner_id=o.owner_id WHERE (o.state='queued' OR (o.state='authorizing' AND o.lease_id='credential-proposal-v1')) AND o.expires_at>$1 AND c.state<>'revoked' AND o.owner_id=ANY($2) ORDER BY o.created_at,o.operation_id FOR UPDATE OF o SKIP LOCKED LIMIT 1").bind(now).bind(&accounts).fetch_optional(&mut *tx).await?;
        let Some((
            operation,
            owner,
            connection,
            agent,
            method,
            encrypted,
            version,
            expiry,
            epoch,
            operation_encrypted,
        )) = row
        else {
            tx.commit().await?;
            return Ok(false);
        };
        if !self.accounts.contains(&owner) {
            tx.rollback().await?;
            return Ok(false);
        }
        let now: i64 =
            sqlx::query_scalar("SELECT (extract(epoch from clock_timestamp())*1000)::bigint")
                .fetch_one(&mut *tx)
                .await?;
        if expiry <= now {
            sqlx::query("UPDATE cloud_auth_operations SET state='expired',operation_ciphertext=NULL,lease_id=NULL,lease_expires_at=NULL,updated_at=$2 WHERE operation_id=$1").bind(&operation).bind(now).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(true);
        }
        let proposal: Option<super::reconfiguration::Proposal> =
            if operation.starts_with(super::reconfiguration::PREFIX) {
                let encrypted = operation_encrypted.as_deref().ok_or_else(unavailable)?;
                let bytes = self
                    .master
                    .open_api(&format!("agent-login:{owner}:{operation}"), encrypted)
                    .map_err(|_| unavailable())?;
                Some(serde_json::from_slice(&bytes).map_err(|_| unavailable())?)
            } else {
                None
            };
        if proposal.as_ref().is_some_and(|p| {
            p.version != 1
                || p.owner != owner
                || p.connection != connection
                || p.agent != agent
                || p.method != method
                || p.base_version != version
                || p.base_epoch != epoch
                || method == "device-code"
        }) {
            sqlx::query("UPDATE cloud_auth_operations SET state='error',operation_ciphertext=NULL,last_error_code='auth_connection_changed',lease_id=NULL,lease_expires_at=NULL,updated_at=$2 WHERE operation_id=$1").bind(&operation).bind(now).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(true);
        }
        let expected_agent = agent.clone();
        let expected_method = method.clone();
        let lease = uuid::Uuid::new_v4().to_string();
        sqlx::query("UPDATE cloud_auth_operations SET state='authorizing',lease_id=$2,lease_expires_at=$3,updated_at=$4 WHERE operation_id=$1").bind(&operation).bind(&lease).bind(now+15_000).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        let secret = if let Some(proposal) = &proposal {
            Zeroizing::new(proposal.secret.clone())
        } else if let Some(encrypted) = encrypted {
            let bytes = self
                .master
                .open_api(
                    &format!("agent-connection:{owner}:{connection}"),
                    &encrypted,
                )
                .map_err(|_| unavailable())?;
            Zeroizing::new(String::from_utf8(bytes.to_vec()).map_err(|_| unavailable())?)
        } else {
            Zeroizing::new(String::new())
        };
        let job = AuthJob {
            refresh: false,
            agent,
            method,
            secret,
            timeout_seconds: ((expiry - now) / 1000).clamp(1, 900) as u64,
        };
        let result = {
            let (send, mut receive) = mpsc::channel(4);
            let execute = driver.execute(job, send);
            tokio::pin!(execute);
            let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
            loop {
                tokio::select! {
                    result=&mut execute=>break result,
                    Some(event)=receive.recv()=>{
                        if proposal.is_some() {break Err("auth_unexpected_challenge");}
                        let encrypted=self.master.seal_api(&format!("agent-login:{owner}:{operation}"),&json!({"verificationUrl":event.url,"verificationCode":event.code}).to_string()).map_err(|_|unavailable())?;
                        sqlx::query("UPDATE cloud_auth_operations SET operation_ciphertext=$3,updated_at=$4 WHERE operation_id=$1 AND lease_id=$2 AND state='authorizing'").bind(&operation).bind(&lease).bind(encrypted).bind(chrono::Utc::now().timestamp_millis()).execute(&self.pool).await?;
                    },
                    _=heartbeat.tick()=>{
                        let now=chrono::Utc::now().timestamp_millis();
                        let live=sqlx::query("UPDATE cloud_auth_operations o SET lease_expires_at=$3 WHERE o.operation_id=$1 AND o.lease_id=$2 AND o.state='authorizing' AND o.expires_at>$4 AND EXISTS(SELECT 1 FROM cloud_agent_connections c WHERE c.connection_id=o.connection_id AND c.version=$5 AND c.authority_epoch=$6 AND c.state<>'revoked')").bind(&operation).bind(&lease).bind(now+15_000).bind(now).bind(version).bind(epoch).execute(&self.pool).await?.rows_affected();
                        if live==0 {break Err("auth_cancelled");}
                    }
                }
            }
        };
        let mut tx = self.pool.begin().await?;
        let current:Option<(i64,String,Option<String>,String,i64,String)>=sqlx::query_as("SELECT version,state,provider_account_id,method,authority_epoch,agent FROM cloud_agent_connections WHERE owner_id=$1 AND connection_id=$2 FOR UPDATE").bind(&owner).bind(&connection).fetch_optional(&mut *tx).await?;
        let claim:Option<(String,Option<String>,Option<i64>,i64)>=sqlx::query_as("SELECT state,lease_id,lease_expires_at,expires_at FROM cloud_auth_operations WHERE operation_id=$1 FOR UPDATE").bind(&operation).fetch_optional(&mut *tx).await?;
        let now: i64 =
            sqlx::query_scalar("SELECT (extract(epoch from clock_timestamp())*1000)::bigint")
                .fetch_one(&mut *tx)
                .await?;
        let claimed = claim.is_some_and(|(state, id, until, expiry)| {
            state == "authorizing"
                && id.as_deref() == Some(&lease)
                && until.is_some_and(|until| until > now)
                && expiry > now
        });
        if current
            .as_ref()
            .is_none_or(|(v, state, _, method, e, agent)| {
                *v != version
                    || *e != epoch
                    || method != &expected_method
                    || agent != &expected_agent
                    || state == "revoked"
            })
            || !claimed
        {
            if proposal.is_some() && claimed {
                sqlx::query("UPDATE cloud_auth_operations SET state='error',operation_ciphertext=NULL,lease_id=NULL,lease_expires_at=NULL,last_error_code='auth_connection_changed',updated_at=$2 WHERE operation_id=$1").bind(&operation).bind(now).execute(&mut *tx).await?;
                tx.commit().await?;
            } else {
                tx.rollback().await?;
            }
            return Ok(true);
        }
        match result {
            Ok(verified) => {
                let (prior_version, prior_state, prior_account, method, prior_epoch, _) =
                    current.as_ref().ok_or_else(unavailable)?;
                let identity_error = if proposal.is_some()
                    && prior_account
                        .as_ref()
                        .is_some_and(|prior| Some(prior) != verified.account_id.as_ref())
                {
                    Some(if verified.account_id.is_some() {
                        "auth_account_mismatch"
                    } else {
                        "auth_identity_review_required"
                    })
                } else if method == "device-code" {
                    if prior_account.is_none()
                        && !(*prior_version == 1
                            && *prior_epoch == 1
                            && matches!(prior_state.as_str(), "pending" | "error"))
                    {
                        Some("auth_identity_review_required")
                    } else if verified.account_id.as_deref().is_none_or(str::is_empty) {
                        Some("auth_identity_review_required")
                    } else if prior_account
                        .as_ref()
                        .is_some_and(|prior| Some(prior) != verified.account_id.as_ref())
                    {
                        Some("auth_account_mismatch")
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(code) = identity_error {
                    if *prior_version == 1
                        && *prior_epoch == 1
                        && prior_state == "pending"
                        && prior_account.is_none()
                    {
                        sqlx::query("UPDATE cloud_agent_connections SET state='error',credential_ciphertext=NULL,last_error_code=$3,updated_at=$4 WHERE owner_id=$1 AND connection_id=$2 AND state='pending'").bind(&owner).bind(&connection).bind(code).bind(now).execute(&mut *tx).await?;
                    }
                    // Existing tasks pin this connection. A wrong-account sign-in
                    // must not replace its identity, ciphertext, version or epoch.
                    sqlx::query("UPDATE cloud_auth_operations SET state='error',operation_ciphertext=NULL,lease_id=NULL,lease_expires_at=NULL,last_error_code=$2,updated_at=$3 WHERE operation_id=$1").bind(&operation).bind(code).bind(now).execute(&mut *tx).await?;
                    tx.commit().await?;
                    return Ok(true);
                }
                let encrypted = self
                    .master
                    .seal_api(
                        &format!("agent-connection:{owner}:{connection}"),
                        &verified.credential,
                    )
                    .map_err(|_| unavailable())?;
                sqlx::query("UPDATE cloud_agent_connections SET state='connected',credential_ciphertext=$3,account_label=$4,provider_account_id=$5,expires_at=$6,version=version+1,authority_epoch=authority_epoch+1,refresh_lease_id=NULL,refresh_lease_expires_at=NULL,last_error_code=NULL,updated_at=$7 WHERE owner_id=$1 AND connection_id=$2").bind(&owner).bind(&connection).bind(encrypted).bind(verified.account_label).bind(verified.account_id).bind(verified.expires_at).bind(now).execute(&mut *tx).await?;
                sqlx::query("UPDATE cloud_auth_operations SET state='connected',operation_ciphertext=NULL,lease_id=NULL,lease_expires_at=NULL,updated_at=$2 WHERE operation_id=$1").bind(&operation).bind(now).execute(&mut *tx).await?;
            }
            Err(code) => {
                sqlx::query("UPDATE cloud_auth_operations SET state='error',last_error_code=$2,operation_ciphertext=NULL,lease_id=NULL,lease_expires_at=NULL,updated_at=$3 WHERE operation_id=$1").bind(&operation).bind(code).bind(now).execute(&mut *tx).await?;
                if proposal.is_none() {
                    sqlx::query("UPDATE cloud_agent_connections SET state='error',credential_ciphertext=NULL,last_error_code=$3,updated_at=$4 WHERE owner_id=$1 AND connection_id=$2 AND state='pending'").bind(&owner).bind(&connection).bind(code).bind(now).execute(&mut *tx).await?;
                }
            }
        }
        tx.commit().await?;
        Ok(true)
    }
}

#[cfg(test)]
#[path = "worker_reauth_tests.rs"]
mod reauth_tests;
