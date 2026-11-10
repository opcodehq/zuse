//! Install the selected native harness before checkout, using declared secrets.
use super::*;
use zuse_protocol::cloud_auth::CloudAuthProvider;

pub fn workload(
    identity: &Identity,
    boot: &CloudRuntimeBootstrap,
    authority: &Authority,
    updates: &tokio::sync::watch::Receiver<CloudRuntimeBootstrap>,
) -> Result<Arc<zuse_cloud_runtime::workload_secrets::Broker>> {
    let mut bindings = boot
        .extra
        .get("workloadSecretBindings")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if let Some(recipe) = boot.extra.get("environmentRecipe") {
        let recipe: zuse_protocol::cloud_computers::EnvironmentRecipe =
            serde_json::from_value(recipe.clone())?;
        bindings.extend(recipe.secret_bindings);
    }
    bindings.sort();
    bindings.dedup();
    zuse_cloud_runtime::workload_secrets::Broker::new(
        identity,
        boot,
        authority.clone(),
        updates.clone(),
        bindings,
    )
}

pub fn install(
    core: &EngineCore,
    boot: &CloudRuntimeBootstrap,
    data: &std::path::Path,
    provider: CloudAuthProvider,
    broker: Arc<zuse_cloud_runtime::grants::ProviderBroker>,
    workload: Arc<zuse_cloud_runtime::workload_secrets::Broker>,
) -> Result<()> {
    match provider {
        CloudAuthProvider::Codex => core.registry.register(Arc::new(
            zuse_providers::CodexHarness::new()
                .with_managed_startup()
                .with_managed_memory_sessions(
                    (boot.extra.get("internalCodexMemoryTransportReset")
                        == Some(&serde_json::json!(true)))
                    .then(|| broker.native_memory_sessions()),
                )
                .with_internal_tool_observations(
                    boot.extra.get("internalCodexToolObservations")
                        == Some(&serde_json::json!(true)),
                )
                .with_credentials(
                    zuse_cloud_runtime::workload_secrets::CodexCredentials::wrap(broker, workload),
                ),
        )),
        CloudAuthProvider::Claude => core.registry.register(Arc::new(
            zuse_providers::ClaudeHarness::new().with_credentials(
                zuse_cloud_runtime::workload_secrets::Credentials::wrap(broker, workload),
            ),
        )),
        CloudAuthProvider::Grok => {
            let helper = std::env::var_os("ZUSE_GROK_AUTH_HELPER")
                .map(PathBuf::from)
                .unwrap_or(std::env::current_exe()?.with_file_name("zuse-grok-auth"));
            let auth = zuse_cloud_runtime::grok_auth::LazyGrokAuth::new(
                broker,
                helper,
                data.join("grok-managed"),
            );
            core.registry.register(Arc::new(
                zuse_providers::AcpHarness::grok()
                    .with_sessions_root(data.join("grok-managed/sessions"))
                    .with_credentials(zuse_cloud_runtime::workload_secrets::Credentials::wrap(
                        auth, workload,
                    )),
            ));
        }
        _ => anyhow::bail!("runtime_agent_unsupported"),
    }
    if let Some(catalogs) = boot.extra.get("managedAgentCatalogs") {
        let catalogs: std::collections::BTreeMap<
            String,
            zuse_protocol::cloud_catalog::CloudAgentModels,
        > = serde_json::from_value(catalogs.clone())?;
        for catalog in catalogs.into_values() {
            core.registry.pin_models(catalog.id, catalog.models);
        }
    }
    Ok(())
}
