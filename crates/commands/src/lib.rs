//! The `zuse` command line, shared by the desktop app and the standalone
//! binary for machines without a display. Only the window itself, Appshot and
//! opening links live in the desktop app.

mod auth;
mod cli;
pub mod config;
mod daemon;
pub mod legacy;
mod logging;
pub mod paths;
mod remote;
mod serve;
mod ssh_proxy;
mod sync;
mod update;

pub use cli::{
    Command, DEFAULT_IPC_PORT, DaemonCommand, DaemonInstall, Global, ServeArgs, default_ipc_port,
    is_language, language_from, localize, localizer, parse,
};
pub use logging::{Logging, init_logging};
use zuse_i18n::{Localizer, messages::cli as m};

impl Command {
    pub fn logging(&self) -> Logging {
        match self {
            Command::Headless => Logging::Headless,
            Command::Serve(_) => Logging::Serve,
            Command::Mcp | Command::SshProxy { .. } => Logging::Stdio,
            _ => Logging::Brief,
        }
    }
}

/// Run one command. Logging is the caller's, set from [`Command::logging`].
pub fn run(global: Global, command: Command, l10n: &Localizer) -> anyhow::Result<()> {
    let port = global.ipc_port();
    match command {
        Command::Serve(args) => serve::main(*args, port, l10n),
        Command::Headless => zuse_engine::runtime()?.block_on(async {
            zuse_engine::Engine::new(config::engine_config(port))
                .run()
                .await
        }),
        Command::Pair { label, app } => {
            block_on(remote::run(remote::Action::Pair { label, app }, port, l10n))
        }
        Command::Link { name } => block_on(remote::run(remote::Action::Link { name }, port, l10n)),
        Command::Unlink => block_on(remote::run(remote::Action::Unlink, port, l10n)),
        Command::Status => {
            let healthy = block_on(async {
                let healthy = auth::status(&config::engine_config(port)).await?;
                remote::status_section(port, l10n).await;
                anyhow::Ok(healthy)
            })?;
            if !healthy {
                std::process::exit(1);
            }
            Ok(())
        }
        Command::Login { no_browser } => {
            block_on(auth::login(config::engine_config(port), no_browser))
        }
        Command::Logout => block_on(auth::logout(config::engine_config(port))),
        Command::Sync => block_on(sync::run(port)),
        Command::Mcp => mcp(),
        Command::SshProxy { host } => block_on(ssh_proxy::run(port, &host)),
        Command::Update { check } => block_on(update::update(&config::edge_url_from_env(), check)),
        Command::Daemon { command } => match command {
            DaemonCommand::Install(install) => {
                let service = daemon_service(&global, install, l10n);
                daemon::install(&paths::data_dir(), &service)
            }
            DaemonCommand::Uninstall => daemon::uninstall(),
            DaemonCommand::Start => daemon::start(),
            DaemonCommand::Stop => daemon::stop(),
            DaemonCommand::Restart => daemon::restart(),
            DaemonCommand::Status => daemon::status(),
        },
    }
}

/// The service's arguments. Serve flags are checked now rather than in a
/// restart loop, and written as given: values from the environment stay in
/// the captured environment.
fn daemon_service(global: &Global, install: DaemonInstall, l10n: &Localizer) -> daemon::Service {
    let mut arguments = vec![];
    if let Some(port) = global.ipc_port {
        arguments.extend(["--ipc-port".into(), port.to_string()]);
    }
    if install.serve {
        #[derive(clap::Parser)]
        #[command(name = "zuse")]
        struct Check {
            #[command(flatten)]
            global: Global,
            #[command(subcommand)]
            command: Command,
        }
        let check = ["zuse", "serve"]
            .into_iter()
            .map(Into::into)
            .chain(install.serve_args.iter().map(Into::into))
            .collect();
        let _: Check = parse(check, l10n);
        arguments.push("serve".into());
        arguments.extend(install.serve_args);
    } else {
        arguments.push("headless".into());
    }
    daemon::Service { arguments }
}

/// The standalone binary started without a command, where the desktop app
/// would open its window.
pub fn desktop_missing(mut command: clap::Command, l10n: &Localizer) -> ! {
    eprintln!("{}\n", m::desktop_missing(l10n));
    eprintln!("{}", command.render_help());
    std::process::exit(2)
}

/// One bridge per agent session, and it only relays: stdio requests out to
/// the engine over IPC and replies back. Two workers keep a burst of parallel
/// tool calls (large replies to encode) close to per-core speed without a
/// worker per core in every agent's bridge.
pub(crate) fn mcp() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(zuse_mcp::run(zuse_mcp::McpConfig::from_env()))
}

pub(crate) fn block_on<T>(future: impl Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    tokio::runtime::Runtime::new()?.block_on(future)
}
