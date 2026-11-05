//! One refresh writer per connection; grants never contain renewable credentials.
use super::worker::{AuthDriver, AuthJob};
use super::*;
use tokio::sync::mpsc;

pub struct ExecutionCredential {
    pub secret: Zeroizing<String>,
    pub method: String,
    pub kind: String,
    pub account_id: Option<String>,
    pub expires_at: Option<i64>,
    pub authority_epoch: i64,
}
struct Record {
    method: String,
    ciphertext: String,
    version: i64,
    epoch: i64,
    expires_at: Option<i64>,
}
fn reconnect() -> ApiError {
    ApiError::unavailable("agent_connection_reconnect_required")
}
impl ConnectionControl {
    async fn credential_record(
        &self,
        owner: &str,
        id: &str,
        agent: &str,
    ) -> Result<Record, ApiError> {
        self.allowed(owner)?;
        let row:Option<(String,String,i64,i64,Option<i64>)>=sqlx::query_as("SELECT method,credential_ciphertext,version,authority_epoch,expires_at FROM cloud_agent_connections WHERE owner_id=$1 AND connection_id=$2 AND agent=$3 AND state='connected' AND credential_ciphertext IS NOT NULL").bind(owner).bind(id).bind(agent).fetch_optional(&self.pool).await?;
        let (method, ciphertext, version, epoch, expires_at) = row.ok_or_else(reconnect)?;
        Ok(Record {
            method,
            ciphertext,
            version,
            epoch,
            expires_at,
        })
    }
    pub async fn delivery_binding(
        &self,
        owner: &str,
        id: &str,
        agent: &str,
    ) -> Result<(String, i64), ApiError> {
        let record = self.credential_record(owner, id, agent).await?;
        Ok((id.to_owned(), record.epoch))
    }
    fn execution_credential(
        &self,
        owner: &str,
        id: &str,
        agent: &str,
        record: Record,
    ) -> Result<ExecutionCredential, ApiError> {
        let bytes = self
            .master
            .open_api(
                &format!("agent-connection:{owner}:{id}"),
                &record.ciphertext,
            )
            .map_err(|_| reconnect())?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| reconnect())?;
        if value["formatVersion"] != 1
            || value["agent"] != agent
            || value["method"] != record.method
        {
            return Err(reconnect());
        }
        let token = if record.method == "device-code" {
            value["accessToken"].as_str()
        } else {
            value["secret"].as_str()
        }
        .filter(|s| !s.is_empty() && s.len() <= 32768)
        .ok_or_else(reconnect)?;
        Ok(ExecutionCredential {
            secret: Zeroizing::new(token.into()),
            method: if record.method == "api-key" {
                "api-key"
            } else {
                "subscription"
            }
            .into(),
            kind: if record.method == "api-key" {
                "api-key"
            } else {
                "oauth-token"
            }
            .into(),
            account_id: value["providerAccountId"].as_str().map(str::to_owned),
            expires_at: record.expires_at,
            authority_epoch: record.epoch,
        })
    }
    pub async fn credential(
        &self,
        owner: &str,
        id: &str,
        agent: &str,
        force: bool,
        driver: &dyn AuthDriver,
    ) -> Result<ExecutionCredential, ApiError> {
        // Only forced Unauthorized refreshes need the pre-gate version to
        // coalesce another writer's completed rotation. Normal delivery still
        // reads the current connected credential after acquiring the same gate.
        let before_version = if force {
            Some(self.credential_record(owner, id, agent).await?.version)
        } else {
            None
        };
        let gate = {
            let mut gates = self.refresh_gates.lock().await;
            gates.retain(|_, value| value.strong_count() > 0);
            let key = format!("{owner}:{id}");
            if let Some(gate) = gates.get(&key).and_then(std::sync::Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(tokio::sync::Mutex::new(()));
                gates.insert(key, Arc::downgrade(&gate));
                gate
            }
        };
        let _guard = gate.lock().await;
        let mut record = self.credential_record(owner, id, agent).await?;
        let now = chrono::Utc::now().timestamp_millis();
        if record.method == "device-code"
            && self
                .recover_rotation(owner, id, record.version, record.epoch, now)
                .await?
        {
            record = self.credential_record(owner, id, agent).await?;
        }
        let force = force && before_version == Some(record.version);
        if !force
            && record
                .expires_at
                .is_none_or(|expiry| expiry > now + 120_000)
        {
            return self.execution_credential(owner, id, agent, record);
        }
        if record.method != "device-code" {
            sqlx::query("UPDATE cloud_agent_connections SET state='reconnect-required',authority_epoch=authority_epoch+1,version=version+1,last_error_code='authentication_required',updated_at=$3 WHERE owner_id=$1 AND connection_id=$2 AND version=$4 AND state='connected'").bind(owner).bind(id).bind(now).bind(record.version).execute(&self.pool).await?;
            return Err(reconnect());
        }
        let rotation = self
            .start_rotation(owner, id, agent, record.version, record.epoch, now)
            .await?;
        let Some(rotation) = rotation else {
            for _ in 0..12 {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                let refreshed = self.credential_record(owner, id, agent).await?;
                if refreshed.version != record.version
                    && refreshed.expires_at.is_some_and(|expiry| {
                        expiry > chrono::Utc::now().timestamp_millis() + 60_000
                    })
                {
                    return self.execution_credential(owner, id, agent, refreshed);
                }
            }
            return Err(ApiError::unavailable("agent_connection_refresh_pending"));
        };
        let bytes = self
            .master
            .open_api(
                &format!("agent-connection:{owner}:{id}"),
                &record.ciphertext,
            )
            .map_err(|_| reconnect())?;
        let canonical = Zeroizing::new(String::from_utf8(bytes.to_vec()).map_err(|_| reconnect())?);
        let (send, _receive) = mpsc::channel(1);
        let result = driver
            .execute(
                AuthJob {
                    agent: agent.into(),
                    method: record.method.clone(),
                    secret: canonical,
                    timeout_seconds: 15,
                    refresh: true,
                },
                send,
            )
            .await;
        let now = chrono::Utc::now().timestamp_millis();
        match result {
            Ok(verified) => {
                self.checkpoint_rotation(&rotation, &verified, now).await?;
                self.apply_rotation(&rotation, now).await?;
                record = self.credential_record(owner, id, agent).await?;
                self.execution_credential(owner, id, agent, record)
            }
            Err(_) => {
                self.uncertain_rotation(&rotation, now).await?;
                Err(reconnect())
            }
        }
    }
}
