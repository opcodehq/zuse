use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::{env, sync::Arc};
use zuse_cloud::{
    auth::WorkosIdentity,
    config::Config,
    routes::{AppState, router},
    runtime::{RuntimeConfig, RuntimeControl},
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
    /// Serve the cloud API; runtime control writes require explicit configuration.
    Serve,
    /// Inspect existing schema/data counts without changing ownership or data.
    Inspect,
    /// Admit initial allocation for an already Rust-owned workspace and approved prepared image.
    Allocate {
        #[arg(long)]
        workspace_id: String,
        #[arg(long)]
        snapshot: String,
    },
    /// Reconcile allocations and retained-machine lifecycle independently of API viewers.
    Worker {
        #[arg(long, default_value_t = false)]
        once: bool,
    },
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
        Command::Allocate {
            workspace_id,
            snapshot,
        } => {
            let runtime = writable_runtime().await?;
            let operation = runtime
                .queue_allocation(&workspace_id, &snapshot, now_ms())
                .await
                .map_err(|error| anyhow::anyhow!("{}", error.code))?;
            println!("{operation}");
        }
        Command::Worker { once } => {
            let runtime = writable_runtime().await?;
            let provider = Arc::new(
                zuse_cloud::providers::boxd::Boxd::new(
                    env::var("BOXD_API_KEY").context("Allocation worker requires BOXD_API_KEY")?,
                    env::var("BOXD_ORG").ok(),
                )
                .map_err(|_| anyhow::anyhow!("Invalid Boxd worker configuration"))?,
            );
            let recovery_task = if once {
                None
            } else {
                let recovery_runtime = runtime.clone();
                let recovery_provider = provider.clone();
                Some(tokio::spawn(async move {
                    loop {
                        if let Err(error) = recovery_runtime
                            .recovery_once(&*recovery_provider, now_ms())
                            .await
                        {
                            tracing::warn!(code = error.code, "Recovery stage deferred");
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    }
                }))
            };
            loop {
                if let Err(error) = runtime.allocate_once(&*provider, now_ms()).await {
                    tracing::warn!(code = error.code, "Allocation stage deferred");
                    if once {
                        anyhow::bail!("{}", error.code);
                    }
                }
                if let Err(error) = runtime.lifecycle_once(&*provider, now_ms()).await {
                    tracing::warn!(code = error.code, "Retained lifecycle stage deferred");
                    if once {
                        anyhow::bail!("{}", error.code);
                    }
                }
                if once {
                    runtime
                        .recovery_once(&*provider, now_ms())
                        .await
                        .map_err(|e| anyhow::anyhow!("{}", e.code))?;
                    break;
                }
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {},
                }
            }
            if let Some(task) = recovery_task {
                task.abort();
            }
        }
        Command::Serve => {
            let config = Config::from_env()?;
            let runtime = match RuntimeConfig::from_env()? {
                Some(settings) => {
                    Some(RuntimeControl::connect(&config.database_url, settings).await?)
                }
                None => None,
            };
            let runtime_enabled = runtime.is_some();
            let state = AppState {
                store: Store::connect_read_only(&config.database_url)
                    .await?
                    .configure_transcripts()?,
                identity: WorkosIdentity::new(&config)?,
                organizations_enabled: config.organizations_enabled,
                organization_ids: Arc::new(config.organization_ids),
                runtime,
                gateway: Arc::new(zuse_cloud::gateway::Hub::default()),
            };
            let origins = config
                .allowed_origins
                .iter()
                .map(|origin| origin.parse())
                .collect::<Result<Vec<_>, _>>()?;
            let listener = tokio::net::TcpListener::bind(config.listen).await?;
            tracing::info!(address = %listener.local_addr()?,runtime_control=runtime_enabled, "Zuse cloud API listening");
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

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
async fn writable_runtime() -> Result<Arc<RuntimeControl>> {
    let settings = RuntimeConfig::from_env()?
        .context("This command requires ZUSE_CLOUD_RUNTIME_WRITES=true")?;
    let url = env::var("ZUSE_CLOUD_DATABASE_URL").context("ZUSE_CLOUD_DATABASE_URL is required")?;
    let runtime = RuntimeControl::connect(&url, settings).await?;
    runtime.allocation_ready().await.map_err(|error| {
        anyhow::anyhow!(
            "{}; apply migration 0004_allocations.sql before running the allocation worker",
            error.code
        )
    })?;
    runtime
        .lifecycle_ready()
        .await
        .map_err(|error| anyhow::anyhow!("{}; apply migration 0005_lifecycle.sql", error.code))?;
    Ok(runtime)
}
