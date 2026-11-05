//! Account-owned credential records. Pending input is encrypted before durable admission.
use crate::{crypto::MasterKey, error::ApiError, store::Store};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{collections::HashSet, sync::Arc};
use zeroize::Zeroizing;
use zuse_protocol::cloud_connections::*;
pub mod credentials;
pub mod discovery;
mod reconfiguration;
mod refresh_journal;
pub mod renewal;
pub mod worker;

pub struct ConnectionControl {
    pub(crate) pool: PgPool,
    pub(crate) master: MasterKey,
    accounts: HashSet<String>,
    refresh_gates: tokio::sync::Mutex<
        std::collections::HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>,
    >,
}
fn unavailable() -> ApiError {
    ApiError::unavailable("cloud_agent_connection_unavailable")
}
fn public(row: Value) -> Result<AgentConnection, ApiError> {
    let mut result: AgentConnection = serde_json::from_value(json!({"connectionId":row["connection_id"],"agent":row["agent"],"method":row["method"],"state":row["state"],"accountLabel":row["account_label"],"expiresAt":row["expires_at"],"version":row["version"],"errorCode":row["last_error_code"],"credentialUpdate":row["credential_update"],"credentialUpdateKey":row["credential_update_key"]})).map_err(|_|unavailable())?;
    if result.credential_update.as_ref().is_some_and(|operation| {
        matches!(
            operation.state,
            LoginState::Queued | LoginState::Authorizing
        )
    }) {
        result.state = ConnectionState::Pending;
    }
    Ok(result)
}
impl ConnectionControl {
    /// Read-only deployment check: never claims or advances a user's login.
    pub async fn ready(&self) -> Result<(), ApiError> {
        sqlx::query("SELECT rotation_id,phase,result_ciphertext FROM zuse_rust_credential_rotations LIMIT 0").execute(&self.pool).await?;
        sqlx::query("SELECT connection_id,authority_epoch,refresh_lease_id FROM cloud_agent_connections LIMIT 0")
            .execute(&self.pool).await?;
        sqlx::query("SELECT operation_id,actor_id,lease_id FROM cloud_auth_operations LIMIT 0")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn connect(
        url: &str,
        master: MasterKey,
        accounts: Vec<String>,
    ) -> anyhow::Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .after_connect(|connection, _| {
                Box::pin(async move {
                    sqlx::query("SET statement_timeout='10s'")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
            .map_err(|_| anyhow::anyhow!("Cannot connect credential authority database"))?;
        Ok(Self::from_pool(pool, master, accounts))
    }
    pub fn from_pool(pool: PgPool, master: MasterKey, accounts: Vec<String>) -> Self {
        Self {
            pool,
            master,
            accounts: accounts.into_iter().collect(),
            refresh_gates: Default::default(),
        }
    }
    fn allowed(&self, owner: &str) -> Result<(), ApiError> {
        if self.accounts.contains(owner) {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "cloud_agent_connection_writes_disabled",
            ))
        }
    }
    pub async fn configure(
        &self,
        owner: &str,
        actor: &str,
        mut request: ConfigureConnection,
        now: i64,
    ) -> Result<AgentConnection, ApiError> {
        self.allowed(owner)?;
        let input = Zeroizing::new(std::mem::take(&mut request.secret));
        let secret = Zeroizing::new(
            if request.agent == zuse_protocol::cloud_computers::AgentId::Claude
                && request.method == ConnectionMethod::SetupToken
            {
                input.chars().filter(|c| !c.is_whitespace()).collect()
            } else {
                input.trim().to_owned()
            },
        );
        if request.method == ConnectionMethod::DeviceCode
            || !request.method.supported(request.agent)
            || !(8..=32768).contains(&secret.len())
            || secret.contains(['\n', '\r', '\0'])
            || request.idempotency_key.is_empty()
            || request.idempotency_key.len() > 200
        {
            return Err(ApiError::bad_request("invalid_agent_connection"));
        }
        if request.connection_id.is_some() {
            return self
                .configure_existing(owner, actor, &request, &secret, now)
                .await;
        }
        let digest = self.master.digest_api(
            &format!(
                "agent-connection:{owner}:{}:{}",
                request.agent.as_str(),
                request.method.as_str()
            ),
            &secret,
        );
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("connection:{owner}:{}", request.idempotency_key))
            .execute(&mut *tx)
            .await?;
        let prior:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(c) FROM cloud_agent_connections c WHERE owner_id=$1 AND idempotency_key=$2 FOR UPDATE").bind(owner).bind(&request.idempotency_key).fetch_optional(&mut *tx).await?;
        if let Some(prior) = prior {
            if prior["request_digest"] != digest {
                return Err(ApiError::conflict("agent_connection_idempotency_conflict"));
            }
            tx.commit().await?;
            return public(prior);
        }
        let prior_operation:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM cloud_auth_operations WHERE owner_id=$1 AND idempotency_key=$2)").bind(owner).bind(format!("verify:{}",request.idempotency_key)).fetch_one(&mut *tx).await?;
        if prior_operation {
            return Err(ApiError::conflict("agent_connection_idempotency_conflict"));
        }
        let id = format!("connection_{}", uuid::Uuid::new_v4().simple());
        let encrypted = self
            .master
            .seal_api(&format!("agent-connection:{owner}:{id}"), &secret)
            .map_err(|_| unavailable())?;
        let row:Value=sqlx::query_scalar("INSERT INTO cloud_agent_connections AS c(connection_id,owner_id,idempotency_key,request_digest,agent,method,state,credential_ciphertext,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,'pending',$7,$8,$8) RETURNING to_jsonb(c)")
            .bind(&id).bind(owner).bind(&request.idempotency_key).bind(digest).bind(request.agent.as_str()).bind(request.method.as_str()).bind(encrypted).bind(now).fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO cloud_auth_operations(operation_id,owner_id,actor_id,connection_id,idempotency_key,state,expires_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,'queued',$6,$7,$7)")
            .bind(format!("auth_{}",uuid::Uuid::new_v4().simple())).bind(owner).bind(actor).bind(&id).bind(format!("verify:{}",request.idempotency_key)).bind(now+120_000).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        public(row)
    }
    pub async fn start_login(
        &self,
        owner: &str,
        actor: &str,
        request: StartConnectionLogin,
        now: i64,
    ) -> Result<LoginOperation, ApiError> {
        self.allowed(owner)?;
        if !ConnectionMethod::DeviceCode.supported(request.agent)
            || request.idempotency_key.is_empty()
            || request.idempotency_key.len() > 200
        {
            return Err(ApiError::bad_request("invalid_device_login"));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("login:{owner}:{}", request.idempotency_key))
            .execute(&mut *tx)
            .await?;
        let prior:Option<(String,String)>=sqlx::query_as("SELECT operation_id,connection_id FROM cloud_auth_operations WHERE owner_id=$1 AND idempotency_key=$2 FOR UPDATE").bind(owner).bind(&request.idempotency_key).fetch_optional(&mut *tx).await?;
        if let Some((operation, id)) = prior {
            if request.connection_id.as_ref().is_some_and(|c| *c != id) {
                return Err(ApiError::conflict("agent_connection_idempotency_conflict"));
            }
            tx.commit().await?;
            let result = self.operation(owner, &operation, now).await?;
            if result.agent != request.agent {
                return Err(ApiError::conflict("agent_connection_idempotency_conflict"));
            }
            return Ok(result);
        }
        let id = if let Some(id) = request.connection_id {
            let existing:Option<(String,String)>=sqlx::query_as("SELECT agent,method FROM cloud_agent_connections WHERE owner_id=$1 AND connection_id=$2 FOR UPDATE").bind(owner).bind(&id).fetch_optional(&mut *tx).await?;
            if existing != Some((request.agent.as_str().into(), "device-code".into())) {
                return Err(ApiError::not_found("agent_connection_not_found"));
            }
            let pending:Option<String>=sqlx::query_scalar("SELECT operation_id FROM cloud_auth_operations WHERE owner_id=$1 AND connection_id=$2 AND state IN ('queued','authorizing') AND expires_at>$3 LIMIT 1").bind(owner).bind(&id).bind(now).fetch_optional(&mut *tx).await?;
            if let Some(pending) = pending {
                tx.commit().await?;
                return self.operation(owner, &pending, now).await;
            }
            sqlx::query("UPDATE cloud_auth_operations SET state='expired',updated_at=$3 WHERE owner_id=$1 AND connection_id=$2 AND state IN ('queued','authorizing') AND expires_at<=$3").bind(owner).bind(&id).bind(now).execute(&mut *tx).await?;
            id
        } else {
            let id = format!("connection_{}", uuid::Uuid::new_v4().simple());
            sqlx::query("INSERT INTO cloud_agent_connections(connection_id,owner_id,idempotency_key,request_digest,agent,method,state,created_at,updated_at) VALUES($1,$2,$3,'device-code',$4,'device-code','pending',$5,$5)").bind(&id).bind(owner).bind(format!("device:{}",request.idempotency_key)).bind(request.agent.as_str()).bind(now).execute(&mut *tx).await?;
            id
        };
        let operation = format!("auth_{}", uuid::Uuid::new_v4().simple());
        sqlx::query("INSERT INTO cloud_auth_operations(operation_id,owner_id,actor_id,connection_id,idempotency_key,state,expires_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,'queued',$6,$7,$7)").bind(&operation).bind(owner).bind(actor).bind(id).bind(request.idempotency_key).bind(now+15*60_000).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        self.operation(owner, &operation, now).await
    }
    pub async fn operation(
        &self,
        owner: &str,
        id: &str,
        now: i64,
    ) -> Result<LoginOperation, ApiError> {
        let row:Option<(String,String,String,i64,Option<String>,Option<String>,Option<String>)>=sqlx::query_as("SELECT o.connection_id,c.agent,o.state,o.expires_at,o.operation_ciphertext,o.last_error_code,o.lease_id FROM cloud_auth_operations o JOIN cloud_agent_connections c ON c.connection_id=o.connection_id AND c.owner_id=o.owner_id WHERE o.owner_id=$1 AND o.operation_id=$2")
            .bind(owner).bind(id).fetch_optional(&self.pool).await?;
        let (connection, agent, state, expiry, encrypted, error, lease) =
            row.ok_or_else(|| ApiError::not_found("agent_login_not_found"))?;
        let mut value = Value::Null;
        if let Some(encrypted) = encrypted.filter(|_| !id.starts_with(reconfiguration::PREFIX)) {
            let bytes = self
                .master
                .open_api(&format!("agent-login:{owner}:{id}"), &encrypted)
                .map_err(|_| unavailable())?;
            value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        }
        let state = if expiry <= now && matches!(state.as_str(), "queued" | "authorizing") {
            "expired"
        } else if lease.as_deref() == Some(reconfiguration::WAITING_LEASE) {
            "queued"
        } else {
            state.as_str()
        };
        let pending = matches!(state, "queued" | "authorizing");
        serde_json::from_value(json!({"operationId":id,"connectionId":connection,"agent":agent,"state":state,"verificationUrl":if pending{value["verificationUrl"].clone()}else{Value::Null},"verificationCode":if pending{value["verificationCode"].clone()}else{Value::Null},"expiresAt":expiry,"errorCode":error})).map_err(|_|unavailable())
    }
    pub async fn cancel(
        &self,
        owner: &str,
        id: &str,
        now: i64,
    ) -> Result<LoginOperation, ApiError> {
        self.allowed(owner)?;
        let mut tx = self.pool.begin().await?;
        let connection:Option<String>=sqlx::query_scalar("SELECT c.connection_id FROM cloud_agent_connections c JOIN cloud_auth_operations o ON o.connection_id=c.connection_id AND o.owner_id=c.owner_id WHERE o.owner_id=$1 AND o.operation_id=$2 FOR UPDATE OF c").bind(owner).bind(id).fetch_optional(&mut *tx).await?;
        let connection = connection.ok_or_else(|| ApiError::not_found("agent_login_not_found"))?;
        let cancelled=sqlx::query("UPDATE cloud_auth_operations SET state='cancelled',operation_ciphertext=NULL,updated_at=$3 WHERE owner_id=$1 AND operation_id=$2 AND state IN ('queued','authorizing')").bind(owner).bind(id).bind(now).execute(&mut *tx).await?.rows_affected();
        if cancelled > 0 {
            sqlx::query("UPDATE cloud_agent_connections c SET state='error',credential_ciphertext=NULL,last_error_code='auth_cancelled',updated_at=$3 WHERE owner_id=$1 AND connection_id=$2 AND state='pending' AND NOT EXISTS(SELECT 1 FROM cloud_auth_operations o WHERE o.connection_id=c.connection_id AND o.state IN ('queued','authorizing'))").bind(owner).bind(&connection).bind(now).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        self.operation(owner, id, now).await
    }
    pub async fn disconnect(&self, owner: &str, id: &str, now: i64) -> Result<(), ApiError> {
        self.allowed(owner)?;
        let mut tx = self.pool.begin().await?;
        let affected=sqlx::query("UPDATE cloud_agent_connections SET state='revoked',credential_ciphertext=NULL,version=version+1,authority_epoch=authority_epoch+1,refresh_lease_id=NULL,refresh_lease_expires_at=NULL,updated_at=$3 WHERE owner_id=$1 AND connection_id=$2").bind(owner).bind(id).bind(now).execute(&mut *tx).await?.rows_affected();
        if affected == 0 {
            return Err(ApiError::not_found("agent_connection_not_found"));
        }
        sqlx::query("UPDATE cloud_auth_operations SET state='cancelled',operation_ciphertext=NULL,updated_at=$3 WHERE owner_id=$1 AND connection_id=$2 AND state IN ('queued','authorizing')").bind(owner).bind(id).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
impl Store {
    pub fn with_shared_connections(mut self, control: Arc<ConnectionControl>) -> Self {
        self.connections = Some(control);
        self
    }
    pub fn with_connections(mut self, control: ConnectionControl) -> Self {
        self.connections = Some(Arc::new(control));
        self
    }
    pub async fn agent_connections(&self, owner: &str) -> Result<Vec<AgentConnection>, ApiError> {
        if self.connections.is_none() {
            return Err(ApiError::unavailable(
                "cloud_agent_connection_writes_disabled",
            ));
        }
        let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(c) || jsonb_build_object('credential_update_key',(SELECT substring(o.idempotency_key from 8) FROM cloud_auth_operations o WHERE o.owner_id=c.owner_id AND o.connection_id=c.connection_id AND left(o.operation_id,9)='auth_cfg_' ORDER BY (o.state IN ('queued','authorizing')) DESC,o.created_at DESC,o.operation_id DESC LIMIT 1),'credential_update',(SELECT jsonb_build_object('operationId',o.operation_id,'connectionId',c.connection_id,'agent',c.agent,'state',CASE WHEN o.state IN ('queued','authorizing') AND o.expires_at<=(extract(epoch from clock_timestamp())*1000)::bigint THEN 'expired' WHEN o.lease_id='credential-proposal-v1' THEN 'queued' ELSE o.state END,'expiresAt',o.expires_at,'errorCode',o.last_error_code) FROM cloud_auth_operations o WHERE o.owner_id=c.owner_id AND o.connection_id=c.connection_id AND left(o.operation_id,9)='auth_cfg_' ORDER BY (o.state IN ('queued','authorizing')) DESC,o.created_at DESC,o.operation_id DESC LIMIT 1)) FROM cloud_agent_connections c WHERE owner_id=$1 AND state<>'revoked' ORDER BY created_at,connection_id").bind(owner).fetch_all(&self.pool).await?;
        rows.into_iter().map(public).collect()
    }
}
