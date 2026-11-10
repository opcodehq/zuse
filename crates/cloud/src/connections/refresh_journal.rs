//! Encrypted refresh checkpoints. A vendor call is never replayed after an uncertain dispatch.
use super::{ConnectionControl, worker::VerifiedConnection};
use crate::error::ApiError;
use zeroize::Zeroizing;
fn unavailable() -> ApiError {
    ApiError::unavailable("agent_connection_refresh_uncertain")
}
pub(super) struct Rotation {
    pub id: String,
    pub lease: String,
    pub owner: String,
    pub connection: String,
    pub version: i64,
    pub epoch: i64,
}
impl Rotation {
    fn aad(&self) -> String {
        format!(
            "agent-rotation:{}:{}:{}:{}:{}",
            self.owner, self.connection, self.epoch, self.version, self.id
        )
    }
}
impl ConnectionControl {
    pub(super) async fn recover_rotation(
        &self,
        owner: &str,
        connection: &str,
        version: i64,
        epoch: i64,
        now: i64,
    ) -> Result<bool, ApiError> {
        let row:Option<(String,String,String,i64)>=sqlx::query_as("SELECT rotation_id,lease_id,phase,lease_expires_at FROM zuse_rust_credential_rotations WHERE owner_id=$1 AND connection_id=$2 AND source_version=$3 AND authority_epoch=$4").bind(owner).bind(connection).bind(version).bind(epoch).fetch_optional(&self.pool).await?;
        let Some((id, lease, phase, until)) = row else {
            return Ok(false);
        };
        let rotation = Rotation {
            id,
            lease,
            owner: owner.into(),
            connection: connection.into(),
            version,
            epoch,
        };
        match phase.as_str() {
            "result" => {
                self.apply_rotation(&rotation, now).await?;
                Ok(true)
            }
            "dispatching" if until > now => {
                Err(ApiError::unavailable("agent_connection_refresh_pending"))
            }
            "dispatching" | "uncertain" => {
                self.uncertain_rotation(&rotation, now).await?;
                Err(unavailable())
            }
            "superseded" => Err(unavailable()),
            _ => Ok(false),
        }
    }
    pub(super) async fn start_rotation(
        &self,
        owner: &str,
        connection: &str,
        agent: &str,
        version: i64,
        epoch: i64,
        now: i64,
    ) -> Result<Option<Rotation>, ApiError> {
        let rotation = Rotation {
            id: uuid::Uuid::new_v4().to_string(),
            lease: uuid::Uuid::new_v4().to_string(),
            owner: owner.into(),
            connection: connection.into(),
            version,
            epoch,
        };
        let mut tx = self.pool.begin().await?;
        let changed=sqlx::query("UPDATE cloud_agent_connections SET refresh_lease_id=$3,refresh_lease_expires_at=$4 WHERE owner_id=$1 AND connection_id=$2 AND agent=$8 AND method='device-code' AND authority_epoch=$7 AND version=$5 AND state='connected' AND (refresh_lease_id IS NULL OR refresh_lease_expires_at<=$6)").bind(owner).bind(connection).bind(&rotation.lease).bind(now+60_000).bind(version).bind(now).bind(epoch).bind(agent).execute(&mut *tx).await?.rows_affected();
        if changed == 0 {
            return Ok(None);
        }
        // Insert and connection lease commit together before native code can use a refresh token.
        let inserted=sqlx::query("INSERT INTO zuse_rust_credential_rotations(rotation_id,owner_id,connection_id,agent,authority_epoch,source_version,lease_id,phase,started_at,lease_expires_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'dispatching',$8,$9,$8) ON CONFLICT(owner_id,connection_id,source_version) DO NOTHING").bind(&rotation.id).bind(owner).bind(connection).bind(agent).bind(epoch).bind(version).bind(&rotation.lease).bind(now).bind(now+60_000).execute(&mut *tx).await?.rows_affected();
        if inserted == 0 {
            return Err(unavailable());
        }
        tx.commit().await?;
        Ok(Some(rotation))
    }
    pub(super) async fn checkpoint_rotation(
        &self,
        rotation: &Rotation,
        verified: &VerifiedConnection,
        now: i64,
    ) -> Result<(), ApiError> {
        let ciphertext = self
            .master
            .seal_api(&rotation.aad(), &verified.credential)
            .map_err(|_| unavailable())?;
        let changed=sqlx::query("UPDATE zuse_rust_credential_rotations SET phase='result',result_ciphertext=$3,provider_account_id=$4,account_label=$5,credential_expires_at=$6,updated_at=$7 WHERE rotation_id=$1 AND lease_id=$2 AND phase='dispatching'").bind(&rotation.id).bind(&rotation.lease).bind(ciphertext).bind(&verified.account_id).bind(&verified.account_label).bind(verified.expires_at).bind(now).execute(&self.pool).await?.rows_affected();
        if changed != 1 {
            return Err(unavailable());
        }
        Ok(())
    }
    pub(super) async fn apply_rotation(
        &self,
        rotation: &Rotation,
        now: i64,
    ) -> Result<(), ApiError> {
        let mut tx = self.pool.begin().await?;
        let current:Option<(i64,i64,String,Option<String>)>=sqlx::query_as("SELECT version,authority_epoch,state,provider_account_id FROM cloud_agent_connections WHERE owner_id=$1 AND connection_id=$2 FOR UPDATE").bind(&rotation.owner).bind(&rotation.connection).fetch_optional(&mut *tx).await?;
        let result:Option<(String,Option<String>,Option<String>,Option<i64>)>=sqlx::query_as("SELECT result_ciphertext,provider_account_id,account_label,credential_expires_at FROM zuse_rust_credential_rotations WHERE rotation_id=$1 AND lease_id=$2 AND phase='result' FOR UPDATE").bind(&rotation.id).bind(&rotation.lease).fetch_optional(&mut *tx).await?;
        let Some((ciphertext, account, label, expiry)) = result else {
            return Err(unavailable());
        };
        if current.as_ref().is_none_or(|(version, epoch, state, id)| {
            *version != rotation.version
                || *epoch != rotation.epoch
                || state != "connected"
                || id != &account
        }) {
            sqlx::query("UPDATE cloud_agent_connections SET state='reconnect-required',authority_epoch=authority_epoch+1,version=version+1,last_error_code='auth_refresh_uncertain',updated_at=$5 WHERE owner_id=$1 AND connection_id=$2 AND version=$3 AND authority_epoch=$4 AND state='connected'").bind(&rotation.owner).bind(&rotation.connection).bind(rotation.version).bind(rotation.epoch).bind(now).execute(&mut *tx).await?;
            sqlx::query("UPDATE zuse_rust_credential_rotations SET phase='superseded',result_ciphertext=NULL,updated_at=$2 WHERE rotation_id=$1").bind(&rotation.id).bind(now).execute(&mut *tx).await?;
            tx.commit().await?;
            return Err(unavailable());
        }
        let bytes = self
            .master
            .open_api(&rotation.aad(), &ciphertext)
            .map_err(|_| unavailable())?;
        let text = Zeroizing::new(String::from_utf8(bytes.to_vec()).map_err(|_| unavailable())?);
        let canonical = self
            .master
            .seal_api(
                &format!(
                    "agent-connection:{}:{}",
                    rotation.owner, rotation.connection
                ),
                &text,
            )
            .map_err(|_| unavailable())?;
        sqlx::query("UPDATE cloud_agent_connections SET credential_ciphertext=$4,provider_account_id=$5,account_label=COALESCE($6,account_label),expires_at=$7,refresh_lease_id=NULL,refresh_lease_expires_at=NULL,version=version+1,updated_at=$8 WHERE owner_id=$1 AND connection_id=$2 AND version=$3").bind(&rotation.owner).bind(&rotation.connection).bind(rotation.version).bind(canonical).bind(account).bind(label).bind(expiry).bind(now).execute(&mut *tx).await?;
        sqlx::query("UPDATE zuse_rust_credential_rotations SET phase='applied',result_ciphertext=NULL,updated_at=$2 WHERE rotation_id=$1").bind(&rotation.id).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub(super) async fn uncertain_rotation(
        &self,
        rotation: &Rotation,
        now: i64,
    ) -> Result<(), ApiError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT connection_id FROM cloud_agent_connections WHERE owner_id=$1 AND connection_id=$2 FOR UPDATE").bind(&rotation.owner).bind(&rotation.connection).execute(&mut *tx).await?;
        let phase: Option<String> = sqlx::query_scalar(
            "SELECT phase FROM zuse_rust_credential_rotations WHERE rotation_id=$1 FOR UPDATE",
        )
        .bind(&rotation.id)
        .fetch_optional(&mut *tx)
        .await?;
        if !phase
            .as_deref()
            .is_some_and(|p| matches!(p, "dispatching" | "uncertain"))
        {
            tx.commit().await?;
            return Ok(());
        }
        // Revocation/new credentials win; only the original version can be fenced.
        sqlx::query("UPDATE cloud_agent_connections SET state='reconnect-required',authority_epoch=authority_epoch+1,version=version+1,refresh_lease_id=NULL,refresh_lease_expires_at=NULL,last_error_code='auth_refresh_uncertain',updated_at=$5 WHERE owner_id=$1 AND connection_id=$2 AND version=$3 AND authority_epoch=$4 AND state='connected'").bind(&rotation.owner).bind(&rotation.connection).bind(rotation.version).bind(rotation.epoch).bind(now).execute(&mut *tx).await?;
        sqlx::query("UPDATE zuse_rust_credential_rotations SET phase='uncertain',result_ciphertext=NULL,updated_at=$2 WHERE rotation_id=$1 AND phase IN ('dispatching','uncertain')").bind(&rotation.id).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub(super) async fn expire_rotation_dispatches(&self, now: i64) -> Result<(), ApiError> {
        let owners = self.accounts.iter().cloned().collect::<Vec<_>>();
        let rows:Vec<(String,String,String,String,i64,i64)>=sqlx::query_as("SELECT rotation_id,lease_id,owner_id,connection_id,source_version,authority_epoch FROM zuse_rust_credential_rotations WHERE phase='dispatching' AND lease_expires_at<=$1 AND owner_id=ANY($2) ORDER BY lease_expires_at LIMIT 16").bind(now).bind(&owners).fetch_all(&self.pool).await?;
        for (id, lease, owner, connection, version, epoch) in rows {
            self.uncertain_rotation(
                &Rotation {
                    id,
                    lease,
                    owner,
                    connection,
                    version,
                    epoch,
                },
                now,
            )
            .await?;
        }
        Ok(())
    }
    pub(super) async fn cleanup_rotations(&self, now: i64) -> Result<(), ApiError> {
        let owners = self.accounts.iter().cloned().collect::<Vec<_>>();
        sqlx::query("UPDATE zuse_rust_credential_rotations SET phase='superseded',result_ciphertext=NULL,updated_at=$1 WHERE rotation_id IN (SELECT r.rotation_id FROM zuse_rust_credential_rotations r WHERE r.phase='result' AND r.owner_id=ANY($2) AND NOT EXISTS(SELECT 1 FROM cloud_agent_connections c WHERE c.owner_id=r.owner_id AND c.connection_id=r.connection_id AND c.version=r.source_version AND c.authority_epoch=r.authority_epoch AND c.state='connected') ORDER BY r.updated_at LIMIT 32)").bind(now).bind(&owners).execute(&self.pool).await?;
        sqlx::query("DELETE FROM zuse_rust_credential_rotations WHERE rotation_id IN (SELECT rotation_id FROM zuse_rust_credential_rotations WHERE phase IN ('applied','superseded','uncertain') AND owner_id=ANY($2) AND updated_at<$1 ORDER BY updated_at LIMIT 32)").bind(now-7*86_400_000).bind(&owners).execute(&self.pool).await?;
        Ok(())
    }
}

#[cfg(all(test, unix))]
#[path = "refresh_journal_tests.rs"]
mod tests;
