//! Discover the selected Grok account through the same access-only bridge as a
//! computer. The authority retains renewable credentials; no profile is imported.
use super::*;
use zuse_cloud_runtime::{
    grants::Credential as AccessCredential,
    grok_auth::{GrokAuth, GrokCredentialSource},
};
use zuse_providers::{acp::AcpHarness, credentials::CredentialResolver};

struct SelectedGrok {
    control: Arc<ConnectionControl>,
    driver: Arc<CommandDriver>,
    owner: String,
    connection: String,
    account: Option<String>,
    epoch: i64,
}
impl SelectedGrok {
    fn access(
        &self,
        credential: crate::connections::credentials::ExecutionCredential,
    ) -> anyhow::Result<AccessCredential> {
        anyhow::ensure!(
            credential.account_id == self.account && credential.authority_epoch == self.epoch,
            "selected_grok_account_changed"
        );
        Ok(AccessCredential {
            method: credential.method,
            kind: credential.kind,
            secret: credential.secret,
            expires_at: credential
                .expires_at
                .unwrap_or_else(|| chrono::Utc::now().timestamp_millis() + 300_000),
            account_id: credential.account_id,
            plan_type: None,
        })
    }
}
#[async_trait::async_trait]
impl GrokCredentialSource for SelectedGrok {
    async fn token(&self, expired: bool) -> anyhow::Result<AccessCredential> {
        let credential = self
            .control
            .credential(
                &self.owner,
                &self.connection,
                "grok",
                expired,
                self.driver.as_ref(),
            )
            .await
            .map_err(|_| anyhow::anyhow!("selected_grok_connection_unavailable"))?;
        self.access(credential)
    }
}

struct IsolatedGrok {
    bridge: Arc<GrokAuth>,
    home: std::path::PathBuf,
}
#[async_trait::async_trait]
impl CredentialResolver for IsolatedGrok {
    async fn environment(&self) -> Result<ChildEnvironment, HarnessError> {
        Ok(self
            .bridge
            .environment()
            .await?
            .and(
                "HOME",
                Zeroizing::new(self.home.to_string_lossy().into_owned()),
            )
            .and(
                "XDG_CONFIG_HOME",
                Zeroizing::new(self.home.join(".config").to_string_lossy().into_owned()),
            )
            .and("ZUSE_CLOUD_DATABASE_URL", Zeroizing::new(String::new()))
            .and(
                "ZUSE_CLOUD_RELEASE_DATABASE_URL",
                Zeroizing::new(String::new()),
            )
            .and("DATABASE_URL", Zeroizing::new(String::new())))
    }
}

impl ConnectionControl {
    pub async fn discover_grok_catalog(
        self: &Arc<Self>,
        owner: &str,
        connection: &str,
        binary: &Path,
        helper: &Path,
        driver: Arc<CommandDriver>,
    ) -> Result<CloudAgentModels, ApiError> {
        self.allowed(owner)?;
        let schema: String = sqlx::query_scalar("SELECT current_schema()")
            .fetch_one(&self.pool)
            .await?;
        if schema != "zuse_staging"
            || std::env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").as_deref() != Ok("staging")
        {
            return Err(ApiError::forbidden("model_discovery_staging_required"));
        }
        if !binary.is_absolute()
            || !binary.is_file()
            || !helper.is_absolute()
            || !helper.is_file()
            || helper.file_name().and_then(|v| v.to_str()) != Some("zuse-grok-auth")
        {
            return Err(failed());
        }
        let credential = self
            .credential(owner, connection, "grok", false, driver.as_ref())
            .await?;
        let epoch = credential.authority_epoch;
        let source = SelectedGrok {
            control: self.clone(),
            driver,
            owner: owner.into(),
            connection: connection.into(),
            account: credential.account_id.clone(),
            epoch,
        };
        drop(credential);
        let home = private_home()?;
        let bridge =
            GrokAuth::start_with_source(Arc::new(source), helper.into(), home.path().join("grok"))
                .map_err(|_| failed())?;
        let harness = AcpHarness::grok()
            .with_executable(binary)
            .with_credentials(Arc::new(IsolatedGrok {
                bridge,
                home: home.path().into(),
            }));
        let catalog = tokio::time::timeout(
            std::time::Duration::from_secs(45),
            harness.model_catalog(true),
        )
        .await
        .map_err(|_| failed())?
        .map_err(|_| failed())?;
        if self.delivery_binding(owner, connection, "grok").await?.1 != epoch {
            return Err(failed());
        }
        let default_model = catalog.models.first().ok_or_else(failed)?.id.clone();
        Ok(CloudAgentModels {
            id: HarnessId::Grok,
            name: "Grok".into(),
            default_model,
            models: catalog.models,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::credentials::ExecutionCredential;

    #[tokio::test]
    async fn grok_discovery_access_is_fenced_to_the_selected_identity_and_epoch() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/unused")
            .unwrap();
        let source = SelectedGrok {
            control: Arc::new(ConnectionControl::from_pool(
                pool,
                crate::crypto::MasterKey::parse("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
                    .unwrap(),
                vec![],
            )),
            driver: Arc::new(CommandDriver::new(Path::new("/bin/true")).unwrap()),
            owner: "selected-owner".into(),
            connection: "selected-connection".into(),
            account: Some("selected-account".into()),
            epoch: 7,
        };
        let credential = |account: &str, epoch| ExecutionCredential {
            secret: Zeroizing::new("synthetic-access-only".into()),
            method: "subscription".into(),
            kind: "oauth-token".into(),
            account_id: Some(account.into()),
            expires_at: Some(chrono::Utc::now().timestamp_millis() + 600_000),
            authority_epoch: epoch,
        };
        assert!(source.access(credential("another-account", 7)).is_err());
        assert!(source.access(credential("selected-account", 8)).is_err());
        let access = source.access(credential("selected-account", 7)).unwrap();
        assert_eq!(access.secret.as_str(), "synthetic-access-only");
        assert_eq!(access.account_id.as_deref(), Some("selected-account"));
    }
}
