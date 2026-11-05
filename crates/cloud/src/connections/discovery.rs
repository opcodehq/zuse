//! Operator-only selected-account discovery through the production harness protocol.
use super::{ConnectionControl, worker::CommandDriver};
use crate::error::ApiError;
use std::{path::Path, sync::Arc};
use zeroize::Zeroizing;
use zuse_protocol::{HarnessId, cloud_catalog::CloudAgentModels};
use zuse_providers::{
    Harness, HarnessError,
    codex::{
        CodexHarness,
        auth::{Credential, CredentialResolver},
    },
    credentials::ChildEnvironment,
};

mod grok;

fn failed() -> ApiError {
    ApiError::unavailable("agent_model_discovery_failed")
}
fn auth_failed() -> HarnessError {
    HarnessError::Protocol("selected-account-discovery-unavailable".into())
}
fn private_home() -> Result<tempfile::TempDir, ApiError> {
    let home = tempfile::Builder::new()
        .prefix("zuse-codex-discovery-")
        .tempdir()
        .map_err(|_| failed())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(|_| failed())?;
    }
    // Codex rejects a configured CODEX_HOME that does not already exist.
    // Create only empty private profile directories, never import host auth.
    for name in [".codex", ".config"] {
        let mut directory = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directory.mode(0o700);
        }
        directory
            .create(home.path().join(name))
            .map_err(|_| failed())?;
    }
    Ok(home)
}
struct Selected {
    control: Arc<ConnectionControl>,
    driver: Arc<CommandDriver>,
    owner: String,
    connection: String,
    account: Option<String>,
    home: std::path::PathBuf,
}
#[async_trait::async_trait]
impl CredentialResolver for Selected {
    async fn environment(&self) -> Result<ChildEnvironment, HarnessError> {
        Ok(ChildEnvironment::new(
            "HOME",
            Zeroizing::new(self.home.to_string_lossy().into_owned()),
        )
        .and(
            "CODEX_HOME",
            Zeroizing::new(self.home.join(".codex").to_string_lossy().into_owned()),
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
    async fn resolve(
        &self,
        unauthorized: bool,
        previous: Option<&str>,
    ) -> Result<Credential, HarnessError> {
        let value = self
            .control
            .credential(
                &self.owner,
                &self.connection,
                "codex",
                unauthorized,
                self.driver.as_ref(),
            )
            .await
            .map_err(|_| auth_failed())?;
        if value.account_id != self.account
            || previous.is_some_and(|p| Some(p) != self.account.as_deref())
        {
            return Err(auth_failed());
        }
        if value.method == "api-key" {
            return Ok(Credential::ApiKey(value.secret));
        }
        Ok(Credential::ChatGpt {
            access_token: value.secret,
            account_id: value.account_id.ok_or_else(auth_failed)?,
            plan_type: None,
        })
    }
}
impl ConnectionControl {
    pub async fn discover_codex_catalog(
        self: &Arc<Self>,
        owner: &str,
        connection: &str,
        binary: &Path,
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
        if !binary.is_absolute() || !binary.is_file() {
            return Err(failed());
        }
        let credential = self
            .credential(owner, connection, "codex", false, driver.as_ref())
            .await?;
        let epoch = credential.authority_epoch;
        let home = private_home()?;
        let selected = Selected {
            control: self.clone(),
            driver,
            owner: owner.into(),
            connection: connection.into(),
            account: credential.account_id.clone(),
            home: home.path().into(),
        };
        drop(credential);
        let harness = CodexHarness::new()
            .with_executable(binary)
            .with_credentials(Arc::new(selected));
        // model_catalog never uses models()'s static fallback.
        let catalog = tokio::time::timeout(
            std::time::Duration::from_secs(45),
            harness.model_catalog(true),
        )
        .await
        .map_err(|_| failed())?
        .map_err(|_| failed())?;
        if self.delivery_binding(owner, connection, "codex").await?.1 != epoch {
            return Err(failed());
        }
        let default_model = catalog.models.first().ok_or_else(failed)?.id.clone();
        Ok(CloudAgentModels {
            id: HarnessId::Codex,
            name: "Codex".into(),
            default_model,
            models: catalog.models,
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn codex_discovery_creates_empty_private_profile_before_native_startup() {
        let home = private_home().unwrap();
        let child=std::process::Command::new("/bin/sh")
            .args(["-c", "test -d \"$CODEX_HOME\" && test -d \"$XDG_CONFIG_HOME\" && test ! -e \"$CODEX_HOME/auth.json\""])
            .env_clear().env("HOME",home.path()).env("CODEX_HOME",home.path().join(".codex"))
            .env("XDG_CONFIG_HOME",home.path().join(".config")).status().unwrap();
        assert!(
            child.success(),
            "native startup requires an existing isolated profile"
        );
        for path in [
            home.path().to_path_buf(),
            home.path().join(".codex"),
            home.path().join(".config"),
        ] {
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
                0
            );
        }
        assert_eq!(
            std::fs::read_dir(home.path().join(".codex"))
                .unwrap()
                .count(),
            0
        );
    }
}
