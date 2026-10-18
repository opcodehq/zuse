use anyhow::Context;
use clap::{Parser, Subcommand};
use std::{net::SocketAddr, path::PathBuf};
use zuse_engine::{Engine, EngineConfig, EngineProfile};
use zuse_protocol::HarnessId;
use zuse_serve::{BrowserServer, TlsConfig};

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
    /// Loopback RPC endpoint used by agent tools; 0 chooses an available port.
    #[arg(long, default_value_t = 27654)]
    ipc_port: u16,
    #[arg(long, env = "ZUSE_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[arg(long, default_value = "dist/web")]
    assets: PathBuf,
    #[arg(long, env = "ZUSE_BROWSER_TOKEN", hide_env_values = true)]
    token: Option<String>,
    #[arg(long, env = "ZUSE_PUBLIC_ORIGIN")]
    public_origin: Option<String>,
    /// PEM certificate chain for direct HTTPS, trusted by connecting browsers.
    #[arg(long, env = "ZUSE_TLS_CERT", requires = "tls_key")]
    tls_cert: Option<PathBuf>,
    /// Matching PEM private key for direct HTTPS.
    #[arg(long, env = "ZUSE_TLS_KEY", requires = "tls_cert")]
    tls_key: Option<PathBuf>,
    /// Public relay origin. Pairing keys stay on this host.
    #[arg(long, requires = "relay_host_token_file")]
    relay_origin: Option<String>,
    /// Operator-issued relay routing credential, independent of pairing keys.
    #[arg(long, requires = "relay_origin")]
    relay_host_token_file: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Serve agent tools over stdio, forwarding to the local engine.
    Mcp,
}

fn main() -> anyhow::Result<()> {
    if zuse_engine::command_host::requested() {
        zuse_engine::command_host::run_blocking()?;
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}
async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args = Args::parse();
    if matches!(args.command, Some(Command::Mcp)) {
        zuse_mcp::run(zuse_mcp::McpConfig::from_env()).await?;
        return Ok(());
    }
    let relay = match (&args.relay_origin, &args.relay_host_token_file) {
        (Some(origin), Some(path)) => {
            Some((zuse_serve::relay::validate_origin(origin)?, path.clone()))
        }
        (None, None) => None,
        _ => anyhow::bail!("Relay access requires its origin and host token file"),
    };
    let shutdown = tokio_util::sync::CancellationToken::new();
    let tls = match (&args.tls_cert, &args.tls_key) {
        (Some(certificate), Some(key)) => Some(TlsConfig::from_pem_files(certificate, key)?),
        (None, None) => None,
        _ => anyhow::bail!("Direct HTTPS requires both --tls-cert and --tls-key"),
    };
    anyhow::ensure!(
        args.listen.ip().is_loopback() || tls.is_some() || args.public_origin.is_some(),
        "Non-loopback listening requires --tls-cert/--tls-key or --public-origin for an HTTPS reverse proxy"
    );
    let data_dir = args.data_dir.unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").expect("HOME or --data-dir is required"))
            .join(".zuse-native")
    });
    std::fs::create_dir_all(&data_dir).context("Could not create the local data directory")?;
    let data_dir =
        std::fs::canonicalize(data_dir).context("Could not resolve the local data directory")?;
    // Bind before starting engine workers, so port conflicts leave no live runtime.
    let ipc = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, args.ipc_port))
        .await
        .context("Could not bind the agent RPC port; choose another --ipc-port")?;
    let ipc_port = ipc.local_addr()?.port();
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    let address = listener.local_addr()?;
    let token = match args.token {
        Some(token) => token,
        None => BrowserServer::load_or_create_token(&data_dir)?,
    };

    let config = EngineConfig {
        data_dir: data_dir.clone(),
        edge_url: "http://127.0.0.1:27655".into(),
        edge_token: None,
        ipc_port,
        default_harness: HarnessId::ClaudeCode,
        org_id: None,
        workos_client_id: None,
    };
    let auth = Engine::build_auth(&config).await;
    let runtime = Engine::assemble_runtime(&config, auth, EngineProfile::local(&data_dir)?).await?;
    // Agent-origin attribution and plugin tools must dial this engine's real port.
    runtime.core().sessions.set_ipc_port(ipc_port);
    let browser = match BrowserServer::new(
        runtime.core().rpc_service(),
        token.clone(),
        args.public_origin.clone(),
    ) {
        Ok(browser) => browser
            .with_managed_authority(std::sync::Arc::new(runtime.core().managed_host.control()))
            .with_pairings(
                runtime.core().pairings.clone(),
                runtime.core().device_id.clone(),
            )
            .with_pairing_slots(runtime.core().browser_access.control().pairing_slots())
            .with_shutdown_token(shutdown.clone()),
        Err(error) => {
            runtime.shutdown().await;
            return Err(error);
        }
    };
    let ipc_task = tokio::spawn(zuse_rpc::serve_ws_listener(
        ipc,
        runtime.core().rpc_service(),
    ));
    let origin = args.public_origin.unwrap_or_else(|| {
        format!(
            "{}://{address}",
            if tls.is_some() { "https" } else { "http" }
        )
    });
    runtime
        .core()
        .browser_access
        .control()
        .register_external(address, origin.clone(), tls.is_none())
        .await;
    println!("Agent tools listen on 127.0.0.1:{ipc_port}");
    println!("Open {origin}/#token={token}");
    if let Some((origin, host_token_file)) = relay {
        // Explicit command-line configuration replaces saved startup settings.
        runtime.core().relay_access.control().stop().await;
        if let Err(error) = runtime
            .core()
            .relay_access
            .control()
            .start(
                zuse_engine::relay_access::Start {
                    origin,
                    host_token_file,
                },
                runtime.core().rpc_service(),
            )
            .await
        {
            ipc_task.abort();
            runtime.shutdown().await;
            return Err(error);
        }
    }
    let result = match tls {
        Some(tls) => browser.serve_tls(listener, args.assets, tls).await,
        None => browser.serve(listener, args.assets).await,
    };
    shutdown.cancel();
    ipc_task.abort();
    runtime.shutdown().await;
    result
}
