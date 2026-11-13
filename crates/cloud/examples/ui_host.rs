//! Isolated account-bound host for compiled UI acceptance; never built by production targets.
use std::{path::PathBuf, sync::Arc};
use zuse_engine::{EngineCore, HarnessId, account::Account};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("profile directory"));
    let ready = PathBuf::from(args.next().expect("ready file"));
    let mut core = EngineCore::assemble(
        &root,
        Arc::new(zuse_engine::HarnessRegistry::new()),
        HarnessId::ClaudeCode,
        None,
    )?;
    let plugin_local = args.next().as_deref() == Some("--plugin-local");
    let (api, subject) = if plugin_local {
        ("http://127.0.0.1:8088", "local-user")
    } else {
        ("https://account.example.test", "user-bob")
    };
    core.account = Account::fixture_signed_in(&root, api.into(), subject);
    if plugin_local {
        core.plugins = zuse_engine::plugins::Plugins::open(
            &root,
            core.account.clone(),
            core.workspace.clone(),
            false,
        )?;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    std::fs::write(ready, listener.local_addr()?.port().to_string())?;
    zuse_rpc::serve_ws_listener(listener, core.rpc_service()).await;
    core.shutdown().await;
    Ok(())
}
