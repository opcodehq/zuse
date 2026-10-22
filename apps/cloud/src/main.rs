use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::{env, sync::Arc};
use zuse_cloud::{
    auth::WorkosIdentity,
    config::Config,
    routes::{AppState, router},
    store::Store,
};

#[derive(Parser)]
#[command(about = "Zuse hosted cloud control plane")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Serve authenticated read-only catalog and archive status endpoints.
    Serve,
    /// Inspect existing schema/data counts without changing ownership or data.
    Inspect,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args = Args::parse();
    match args.command.unwrap_or(Command::Serve) {
        Command::Inspect => {
            let url = env::var("ZUSE_CLOUD_DATABASE_URL")
                .context("ZUSE_CLOUD_DATABASE_URL is required")?;
            let store = Store::connect_read_only(&url).await?;
            println!(
                "{}",
                serde_pretty(
                    &store
                        .migration_report()
                        .await
                        .map_err(|error| anyhow::anyhow!("{}", error.code))?
                )?
            );
        }
        Command::Serve => {
            let config = Config::from_env()?;
            let state = AppState {
                store: Store::connect_read_only(&config.database_url).await?,
                identity: WorkosIdentity::new(&config)?,
                organizations_enabled: config.organizations_enabled,
                organization_ids: Arc::new(config.organization_ids),
            };
            let origins = config
                .allowed_origins
                .iter()
                .map(|origin| origin.parse())
                .collect::<Result<Vec<_>, _>>()?;
            let listener = tokio::net::TcpListener::bind(config.listen).await?;
            tracing::info!(address = %listener.local_addr()?, "Zuse cloud API listening (read-only)");
            axum_serve(listener, router(state, origins)).await?;
        }
    }
    Ok(())
}

fn serde_pretty(value: &serde_json::Value) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)?)
}
async fn axum_serve(listener: tokio::net::TcpListener, app: axum::Router) -> Result<()> {
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            #[cfg(unix)]
            {
                let mut terminate =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("install SIGTERM handler");
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
        })
        .await?;
    Ok(())
}
