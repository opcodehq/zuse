//! Durable account events. The outbox and replay store never touch a computer.
pub mod cursor;
pub mod hint;
pub mod hub;
pub mod store;
pub mod tickets;

impl crate::store::Store {
    /// Opt in only after the event migration is installed. Reuse the runtime's
    /// bounded writable pool instead of allocating another database pool.
    pub async fn with_account_events_from_env(
        mut self,
        runtime: Option<&std::sync::Arc<crate::runtime::RuntimeControl>>,
    ) -> anyhow::Result<Self> {
        if std::env::var("ZUSE_CLOUD_ACCOUNT_EVENTS").as_deref() != Ok("true") {
            return Ok(self);
        }
        let runtime =
            runtime.ok_or_else(|| anyhow::anyhow!("Account events require runtime authority"))?;
        let raw = std::env::var("CLOUD_DATA_ENCRYPTION_KEY")
            .or_else(|_| std::env::var("CLOUD_CREDENTIAL_VAULT_KEY"))
            .map_err(|_| anyhow::anyhow!("Account events require encryption configuration"))?;
        let key = crate::crypto::MasterKey::parse(&raw)
            .map_err(|_| anyhow::anyhow!("Invalid account event encryption configuration"))?;
        let issuer = std::env::var("API_ISSUER")
            .map_err(|_| anyhow::anyhow!("Account events require an API issuer"))?;
        let url = reqwest::Url::parse(&issuer)
            .map_err(|_| anyhow::anyhow!("Invalid account event issuer"))?;
        anyhow::ensure!(
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "Invalid account event issuer"
        );
        self.events = Some(
            hub::Hub::start(runtime.shared_pool(), key, issuer)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.code))?,
        );
        Ok(self)
    }
}
