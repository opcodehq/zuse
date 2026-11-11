//! `zuse serve`: the browser client and a local engine in one process.
use crate::{cli::ServeArgs, remote};
use anyhow::Context;
use std::path::PathBuf;
use zuse_engine::{Engine, EngineConfig, EngineProfile};
use zuse_i18n::{Localizer, messages::cli as m};
use zuse_protocol::HarnessId;
use zuse_serve::{BrowserServer, TlsConfig};

pub fn main(args: ServeArgs, ipc_port: u16, l10n: &Localizer) -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .thread_stack_size(zuse_engine::RUNTIME_STACK_SIZE)
        .enable_all()
        .build()?
        .block_on(serve(args, ipc_port, l10n))
}

async fn serve(args: ServeArgs, ipc_port: u16, l10n: &Localizer) -> anyhow::Result<()> {
    let relay = match (&args.relay_origin, &args.relay_host_token_file) {
        (Some(origin), Some(path)) => {
            Some((zuse_serve::relay::validate_origin(origin)?, path.clone()))
        }
        (None, None) => None,
        _ => anyhow::bail!(m::relay_requires_credentials(l10n)),
    };
    let shutdown = tokio_util::sync::CancellationToken::new();
    let tls = match (&args.tls_cert, &args.tls_key) {
        (Some(certificate), Some(key)) => Some(TlsConfig::from_pem_files(certificate, key)?),
        (None, None) => None,
        _ => anyhow::bail!(m::https_requires_keys(l10n)),
    };
    anyhow::ensure!(
        args.listen.ip().is_loopback() || tls.is_some() || args.public_origin.is_some(),
        m::https_required(l10n)
    );
    let data_dir = match args.data_dir {
        Some(path) => path,
        None => crate::paths::default_data_dir().context(m::missing_home(l10n))?,
    };
    std::fs::create_dir_all(&data_dir).context(m::create_data_failed(l10n))?;
    let data_dir = std::fs::canonicalize(data_dir).context(m::resolve_data_failed(l10n))?;
    // Bind before starting engine workers, so port conflicts leave no live runtime.
    let ipc = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, ipc_port))
        .await
        .context(m::bind_failed(l10n))?;
    let ipc_port = ipc.local_addr()?.port();
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    let address = listener.local_addr()?;
    let token = match args.token {
        Some(token) => token,
        None => BrowserServer::load_or_create_token(&data_dir)?,
    };
    let assets = args
        .assets
        .or_else(zuse_engine::browser_access::discover_assets)
        .unwrap_or_else(|| {
            eprintln!("{}", m::assets_missing(l10n));
            PathBuf::from("dist/web")
        });

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
            .with_paired_sessions(runtime.core().browser_access.control().paired_sessions())
            .with_advertised_routes(
                runtime
                    .core()
                    .browser_access
                    .control()
                    .advertised_routes()
                    .subscribe(),
            )
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
    let public_origin = args.public_origin.is_some();
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
    println!(
        "{}",
        remote::plain(m::listening(l10n, ipc_port.to_string()))
    );
    println!(
        "{}",
        remote::plain(m::open(l10n, format!("{origin}/#token={token}")))
    );
    if args.lan {
        match runtime
            .core()
            .direct_access
            .control()
            .start(
                zuse_engine::direct_access::Start {
                    port: args.lan_port,
                    tailscale_serve: args.tailscale_serve,
                },
                runtime.core().rpc_service(),
            )
            .await
        {
            Ok(status) => {
                for route in status.routes {
                    println!("{}: {}", route.kind.label(), route.origin);
                }
            }
            Err(error) => {
                ipc_task.abort();
                runtime.shutdown().await;
                return Err(error);
            }
        }
    }
    let relayed = relay.is_some();
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
    let managed = runtime.core().managed_host.control().status();
    let reachable =
        args.lan || relayed || public_origin || (managed.enabled && managed.account_matches);
    // Linking needs the listener below to be serving, so this cannot run first.
    let remote_setup = (reachable || args.account).then(|| {
        let client = zuse_rpc::memory_client(runtime.core().rpc_service());
        let l10n = Localizer::new(l10n.locale());
        let account = args.account;
        tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            if account
                && let Err(error) = async {
                    remote::sign_in(&client, &l10n).await?;
                    remote::link(&client, None, &l10n).await
                }
                .await
            {
                eprintln!("{error}");
            }
            if let Err(error) = remote::invite(&client, None, false, &l10n).await {
                eprintln!("{error}");
            }
        }))
    });
    let result = match tls {
        Some(tls) => browser.serve_tls(listener, assets, tls).await,
        None => browser.serve(listener, assets).await,
    };
    drop(remote_setup);
    shutdown.cancel();
    ipc_task.abort();
    runtime.shutdown().await;
    result
}
