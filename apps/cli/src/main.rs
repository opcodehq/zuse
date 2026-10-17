use clap::{Parser, Subcommand};
use std::{net::SocketAddr, path::PathBuf};
use zuse_engine::{Engine, EngineConfig, EngineProfile};
use zuse_protocol::HarnessId;
use zuse_serve::BrowserServer;

#[derive(Parser)]
#[command(
    name = "zuse-server",
    about = "Serve the Zuse Rust browser client and local agent engine"
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(long, default_value = "127.0.0.1:27656")]
    listen: SocketAddr,
    #[arg(long, env = "ZUSE_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[arg(long, default_value = "dist/web")]
    assets: PathBuf,
    #[arg(long, env = "ZUSE_BROWSER_TOKEN", hide_env_values = true)]
    token: Option<String>,
    #[arg(long, env = "ZUSE_PUBLIC_ORIGIN")]
    public_origin: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Serve agent tools over stdio, forwarding to the local engine.
    Mcp,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args = Args::parse();
    if matches!(args.command, Some(Command::Mcp)) {
        zuse_mcp::run(zuse_mcp::McpConfig::from_env()).await?;
        return Ok(());
    }
    let data_dir = args.data_dir.unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").expect("HOME or --data-dir is required"))
            .join(".zuse-native")
    });
    let config = EngineConfig {
        data_dir: data_dir.clone(),
        edge_url: "http://127.0.0.1:27655".into(),
        edge_token: None,
        ipc_port: 27654,
        default_harness: HarnessId::ClaudeCode,
        org_id: None,
        workos_client_id: None,
    };
    let auth = Engine::build_auth(&config).await;
    let runtime = Engine::assemble_runtime(&config, auth, EngineProfile::local(&data_dir)?).await?;
    let ipc =
        tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, config.ipc_port)).await?;
    let ipc_task = tokio::spawn(zuse_rpc::serve_ws_listener(
        ipc,
        runtime.core().rpc_service(),
    ));
    let token = args.token.unwrap_or_else(BrowserServer::random_token);
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    let address = listener.local_addr()?;
    let browser = BrowserServer::new(
        runtime.core().rpc_service(),
        token.clone(),
        args.public_origin.clone(),
    )?;
    let origin = args
        .public_origin
        .unwrap_or_else(|| format!("http://{address}"));
    println!("Open {origin}/#token={token}");
    browser.serve(listener, args.assets).await?;
    ipc_task.abort();
    runtime.shutdown().await;
    Ok(())
}
