//! Standalone opaque relay; it never starts an agent engine or sees pairing keys.
use clap::Parser;
use std::{net::SocketAddr, path::PathBuf};
use zuse_serve::{
    TlsConfig,
    relay::{HostToken, RelayBroker},
};
#[derive(Parser)]
#[command(
    name = "zuse-relay",
    about = "Serve encrypted Zuse rendezvous and browser assets"
)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:27657")]
    listen: SocketAddr,
    #[arg(long)]
    host_token_file: PathBuf,
    #[arg(long, default_value = "dist/web")]
    assets: PathBuf,
    #[arg(long, requires = "tls_key")]
    tls_cert: Option<PathBuf>,
    #[arg(long, requires = "tls_cert")]
    tls_key: Option<PathBuf>,
    /// Explicit HTTPS origin when TLS terminates at a reverse proxy.
    #[arg(long)]
    public_origin: Option<String>,
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let token = HostToken::read_file(&args.host_token_file)?;
    let tls = match (args.tls_cert, args.tls_key) {
        (Some(cert), Some(key)) => Some(TlsConfig::from_pem_files(&cert, &key)?),
        (None, None) => None,
        _ => anyhow::bail!("Choose both TLS certificate and key"),
    };
    if let Some(origin) = &args.public_origin {
        zuse_serve::relay::validate_origin(origin)?;
    }
    anyhow::ensure!(
        args.listen.ip().is_loopback()
            || tls.is_some()
            || args
                .public_origin
                .as_ref()
                .is_some_and(|origin| origin.starts_with("https://")),
        "Remote relay listening requires direct TLS or an explicit HTTPS proxy origin"
    );
    anyhow::ensure!(
        args.assets.join("index.html").is_file(),
        "Build or install the browser assets first"
    );
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    println!("Zuse relay listening on {}", listener.local_addr()?);
    RelayBroker::new(token)
        .serve_with_shutdown(listener, args.assets, tls, async {
            #[cfg(unix)]
            {
                if let Ok(mut signal) =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                {
                    tokio::select! {_=tokio::signal::ctrl_c()=>{},_=signal.recv()=>{}}
                    return;
                }
            }
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
}
