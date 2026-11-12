use anyhow::Context;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use std::{net::SocketAddr, path::PathBuf};
use zuse_engine::{Engine, EngineConfig, EngineProfile};
use zuse_i18n::{LanguagePreference, Localizer, messages::cli as m};
use zuse_protocol::HarnessId;
use zuse_serve::{BrowserServer, TlsConfig};

mod remote;

#[derive(Parser)]
#[command(
    name = "zuse-server",
    about = "Serve the Zuse Rust browser client and local agent engine"
)]
struct Args {
    #[arg(long, env = "ZUSE_LANGUAGE", global = true, default_value = "system", value_parser = ["system", "en", "fr", "de", "zh-Hans", "zh-Hant", "ja", "ko"])]
    language: String,
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(long, default_value = "127.0.0.1:27656")]
    listen: SocketAddr,
    /// Loopback RPC endpoint used by agent tools; 0 chooses an available port.
    #[arg(long, global = true, default_value_t = 27654)]
    ipc_port: u16,
    #[arg(long, env = "ZUSE_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[arg(long)]
    assets: Option<PathBuf>,
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
    /// Let paired devices on this computer's local network and tailnet connect
    /// directly. Traffic stays end-to-end encrypted; no certificate is needed.
    #[arg(long)]
    lan: bool,
    /// Port for --lan connections.
    #[arg(long, requires = "lan")]
    lan_port: Option<u16>,
    /// Also publish --lan over Tailscale HTTPS, reachable from browsers.
    #[arg(long, requires = "lan")]
    tailscale_serve: bool,
    /// Sign in and link this computer when the server starts.
    #[arg(long)]
    account: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Serve agent tools over stdio, forwarding to the local engine.
    Mcp,
    /// Create a private link for another device.
    Pair {
        /// Name shown in Connected devices.
        #[arg(long)]
        label: Option<String>,
        /// Show the QR code for the Zuse app instead of a browser.
        #[arg(long)]
        app: bool,
    },
    /// Sign in to your Zuse account.
    Login,
    /// Sign out of your Zuse account.
    Logout,
    /// Make this computer available through your Zuse account.
    Link {
        /// Computer name shown in your account.
        #[arg(long)]
        name: Option<String>,
    },
    /// Stop making this computer available through your account.
    Unlink,
    /// Show account and network status.
    Status,
}

fn main() -> anyhow::Result<()> {
    if zuse_engine::command_host::requested() {
        zuse_engine::command_host::run_blocking()?;
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .thread_stack_size(zuse_engine::RUNTIME_STACK_SIZE)
        .enable_all()
        .build()?
        .block_on(run())
}
fn language_from(
    arguments: &[std::ffi::OsString],
    environment: Option<String>,
    system: &[String],
) -> Localizer {
    let mut selected = environment.unwrap_or_else(|| "system".into());
    let mut arguments = arguments.iter().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--" {
            break;
        }
        if argument == "--language" {
            if let Some(value) = arguments.next() {
                selected = value.to_string_lossy().into_owned();
            }
        } else if let Some(value) = argument
            .to_str()
            .and_then(|value| value.strip_prefix("--language="))
        {
            selected = value.into();
        }
    }
    Localizer::new(LanguagePreference::from(selected).resolve(system))
}

fn localized_command(l10n: &Localizer) -> clap::Command {
    let template = format!(
        "{{about-with-newline}}\n{}: {{usage}}\n\n{}:\n{{subcommands}}\n{}:\n{{options}}",
        m::usage(l10n),
        m::commands(l10n),
        m::options(l10n)
    );
    let help = clap::Arg::new("help")
        .short('h')
        .long("help")
        .action(clap::ArgAction::Help)
        .help(m::help(l10n));
    let mut command = Args::command()
        .about(m::about(l10n))
        .help_template(template.clone())
        .disable_help_flag(true)
        .arg(help.clone())
        .disable_help_subcommand(true);
    for (id, help) in [
        ("language", m::language(l10n)),
        ("listen", m::listen(l10n)),
        ("ipc_port", m::ipc_port(l10n)),
        ("data_dir", m::data_dir(l10n)),
        ("assets", m::assets(l10n)),
        ("token", m::token(l10n)),
        ("public_origin", m::public_origin(l10n)),
        ("tls_cert", m::tls_cert(l10n)),
        ("tls_key", m::tls_key(l10n)),
        ("relay_origin", m::relay_origin(l10n)),
        ("relay_host_token_file", m::relay_host_token_file(l10n)),
        ("account", m::account(l10n)),
    ] {
        command = command.mut_arg(id, |arg| arg.help(help));
    }
    for (name, about) in [
        ("mcp", m::mcp(l10n)),
        ("pair", m::pair(l10n)),
        ("login", m::login(l10n)),
        ("logout", m::logout(l10n)),
        ("link", m::link(l10n)),
        ("unlink", m::unlink(l10n)),
        ("status", m::status(l10n)),
    ] {
        command = command.mut_subcommand(name, |subcommand| {
            subcommand
                .about(about)
                .help_template(template.clone())
                .disable_help_flag(true)
                .arg(help.clone())
        });
    }
    command
        .mut_subcommand("pair", |pair| {
            pair.mut_arg("label", |arg| arg.help(m::pair_label(l10n)))
                .mut_arg("app", |arg| arg.help(m::pair_app(l10n)))
        })
        .mut_subcommand("link", |link| {
            link.mut_arg("name", |arg| arg.help(m::link_name(l10n)))
        })
}

fn parse_arguments(arguments: Vec<std::ffi::OsString>, l10n: &Localizer) -> Args {
    let mut command = localized_command(l10n);
    match command.try_get_matches_from_mut(arguments) {
        Ok(matches) => Args::from_arg_matches(&matches).expect("Clap validated arguments"),
        Err(error) => {
            use clap::error::ErrorKind;
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                let _ = error.print();
                std::process::exit(0);
            }
            let detail = match error.kind() {
                ErrorKind::UnknownArgument | ErrorKind::InvalidSubcommand => {
                    m::unknown_argument(l10n)
                }
                ErrorKind::MissingRequiredArgument | ErrorKind::MissingSubcommand => {
                    m::missing_argument(l10n)
                }
                _ => m::invalid_arguments(l10n),
            };
            let argument = error
                .get(clap::error::ContextKind::InvalidArg)
                .or_else(|| error.get(clap::error::ContextKind::InvalidSubcommand))
                .map(|value| format!("\n{value}"))
                .unwrap_or_default();
            eprintln!("{detail}{argument}\n\n{}", command.render_help());
            std::process::exit(error.exit_code());
        }
    }
}

async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let arguments = std::env::args_os().collect::<Vec<_>>();
    let l10n = language_from(
        &arguments,
        std::env::var("ZUSE_LANGUAGE").ok(),
        &zuse_i18n::system_languages(),
    );
    let mut args = parse_arguments(arguments, &l10n);
    match args.command.take() {
        Some(Command::Mcp) => {
            zuse_mcp::run(zuse_mcp::McpConfig::from_env()).await?;
            return Ok(());
        }
        Some(command) => return remote::run(command, args.ipc_port, &l10n).await,
        None => {}
    }
    let relay = match (&args.relay_origin, &args.relay_host_token_file) {
        (Some(origin), Some(path)) => {
            Some((zuse_serve::relay::validate_origin(origin)?, path.clone()))
        }
        (None, None) => None,
        _ => anyhow::bail!(m::relay_requires_credentials(&l10n)),
    };
    let shutdown = tokio_util::sync::CancellationToken::new();
    let tls = match (&args.tls_cert, &args.tls_key) {
        (Some(certificate), Some(key)) => Some(TlsConfig::from_pem_files(certificate, key)?),
        (None, None) => None,
        _ => anyhow::bail!(m::https_requires_keys(&l10n)),
    };
    anyhow::ensure!(
        args.listen.ip().is_loopback() || tls.is_some() || args.public_origin.is_some(),
        m::https_required(&l10n)
    );
    let data_dir = match args.data_dir {
        Some(path) => path,
        None => PathBuf::from(std::env::var_os("HOME").context(m::missing_home(&l10n))?)
            .join(".zuse-native"),
    };
    std::fs::create_dir_all(&data_dir).context(m::create_data_failed(&l10n))?;
    let data_dir = std::fs::canonicalize(data_dir).context(m::resolve_data_failed(&l10n))?;
    // Bind before starting engine workers, so port conflicts leave no live runtime.
    let ipc = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, args.ipc_port))
        .await
        .context(m::bind_failed(&l10n))?;
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
            eprintln!("{}", m::assets_missing(&l10n));
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
        remote::plain(m::listening(&l10n, ipc_port.to_string()))
    );
    println!(
        "{}",
        remote::plain(m::open(&l10n, format!("{origin}/#token={token}")))
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

#[cfg(test)]
mod localization_tests {
    use super::*;
    use zuse_i18n::Locale;
    fn arguments(values: &[&str]) -> Vec<std::ffi::OsString> {
        values.iter().map(Into::into).collect()
    }
    #[test]
    fn language_precedence_is_flag_then_environment_then_system() {
        let system = vec!["zh-TW".into()];
        assert_eq!(
            language_from(&arguments(&["zuse-server"]), None, &system).locale(),
            Locale::ZhHant
        );
        assert_eq!(
            language_from(&arguments(&["zuse-server"]), Some("de".into()), &system).locale(),
            Locale::De
        );
        assert_eq!(
            language_from(
                &arguments(&["zuse-server", "mcp", "--language=fr"]),
                Some("de".into()),
                &system
            )
            .locale(),
            Locale::Fr
        );
        assert_eq!(
            language_from(
                &arguments(&["zuse-server", "--language", "en"]),
                Some("de".into()),
                &system
            )
            .locale(),
            Locale::En
        );
    }
    #[test]
    fn help_is_localized_and_command_spelling_is_stable() {
        for locale in Locale::ALL {
            let localizer = Localizer::new(locale);
            let mut command = localized_command(&localizer);
            let help = command.render_long_help().to_string();
            assert!(help.contains(&m::about(&localizer)));
            assert!(help.contains("--language"));
            assert!(help.contains("--tls-cert"));
            assert!(help.contains("mcp"));
            assert!(help.contains("ZUSE_LANGUAGE"));
            let matches = command
                .try_get_matches_from(["zuse-server", "mcp", "--language", locale.code()])
                .unwrap();
            assert_eq!(
                Args::from_arg_matches(&matches).unwrap().language,
                locale.code()
            );
        }
    }
    #[test]
    fn invalid_language_is_a_parser_error_and_mcp_help_uses_its_own_description() {
        let localizer = Localizer::new(Locale::ZhHans);
        let error = localized_command(&localizer)
            .try_get_matches_from(["zuse-server", "--language", "unknown"])
            .unwrap_err();
        assert_eq!(error.exit_code(), 2);
        let error = localized_command(&localizer)
            .try_get_matches_from(["zuse-server", "mcp", "--help"])
            .unwrap_err();
        assert_eq!(error.exit_code(), 0);
        assert!(error.to_string().contains(&m::mcp(&localizer)));
    }
    #[test]
    fn remote_commands_are_localized_and_reach_the_chosen_engine() {
        for locale in Locale::ALL {
            let localizer = Localizer::new(locale);
            let error = localized_command(&localizer)
                .try_get_matches_from(["zuse-server", "pair", "--help"])
                .unwrap_err();
            assert_eq!(error.exit_code(), 0);
            assert!(error.to_string().contains(&m::pair(&localizer)));
            assert!(error.to_string().contains(&m::pair_label(&localizer)));
        }
        let matches = localized_command(&Localizer::new(Locale::En))
            .try_get_matches_from(["zuse-server", "pair", "--ipc-port", "4000", "--app"])
            .unwrap();
        let args = Args::from_arg_matches(&matches).unwrap();
        assert_eq!(args.ipc_port, 4000);
        assert!(matches!(
            args.command,
            Some(Command::Pair {
                label: None,
                app: true
            })
        ));
    }
}
