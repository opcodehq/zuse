//! The `zuse` command tree. The desktop app adds headed mode and Appshot on
//! top; the standalone binary parses it as is.
use clap::{Args, CommandFactory, FromArgMatches, Subcommand};
use std::{ffi::OsString, net::SocketAddr, path::PathBuf};
use zuse_i18n::{LanguagePreference, Localizer, messages::cli as m};

pub const DEFAULT_IPC_PORT: u16 = 27654;
const LANGUAGES: [&str; 8] = ["system", "en", "fr", "de", "zh-Hans", "zh-Hant", "ja", "ko"];

/// Whether `value` names a supported interface language preference.
pub fn is_language(value: &str) -> bool {
    LANGUAGES.contains(&value)
}

/// Flags accepted by every command.
#[derive(Args, Debug, Clone)]
pub struct Global {
    #[arg(long, env = "ZUSE_LANGUAGE", global = true, default_value = "system", value_parser = LANGUAGES)]
    pub language: String,
    /// Loopback RPC port of the engine; defaults to ZUSE_IPC_PORT or 27654.
    #[arg(long, global = true, value_name = "PORT")]
    pub ipc_port: Option<u16>,
}
impl Global {
    pub fn ipc_port(&self) -> u16 {
        self.ipc_port.unwrap_or_else(default_ipc_port)
    }
}

/// `ZUSE_IPC_PORT`, read leniently so a stray value never blocks the app.
pub fn default_ipc_port() -> u16 {
    std::env::var("ZUSE_IPC_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(DEFAULT_IPC_PORT)
}

/// `zuse serve`, formerly `zuse-server`.
#[derive(Args, Debug, Clone)]
pub struct ServeArgs {
    #[arg(long, default_value = "127.0.0.1:27656")]
    pub listen: SocketAddr,
    #[arg(long, env = "ZUSE_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
    #[arg(long)]
    pub assets: Option<PathBuf>,
    #[arg(long, env = "ZUSE_BROWSER_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
    #[arg(long, env = "ZUSE_PUBLIC_ORIGIN")]
    pub public_origin: Option<String>,
    #[arg(long, env = "ZUSE_TLS_CERT", requires = "tls_key")]
    pub tls_cert: Option<PathBuf>,
    #[arg(long, env = "ZUSE_TLS_KEY", requires = "tls_cert")]
    pub tls_key: Option<PathBuf>,
    #[arg(long, requires = "relay_host_token_file")]
    pub relay_origin: Option<String>,
    #[arg(long, requires = "relay_origin")]
    pub relay_host_token_file: Option<PathBuf>,
    #[arg(long)]
    pub lan: bool,
    #[arg(long, requires = "lan")]
    pub lan_port: Option<u16>,
    #[arg(long, requires = "lan")]
    pub tailscale_serve: bool,
    #[arg(long)]
    pub account: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Serve(Box<ServeArgs>),
    Headless,
    Pair {
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        app: bool,
    },
    Link {
        #[arg(long)]
        name: Option<String>,
    },
    Unlink,
    Status,
    Login {
        /// For SSH sessions and machines without a browser.
        #[arg(long)]
        no_browser: bool,
    },
    Logout,
    Sync,
    Mcp,
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// SSH ProxyCommand for `ssh zuse-<workspace>`, through the running engine.
    #[command(hide = true)]
    SshProxy {
        /// The host alias ssh connects to (`zuse-<workspace id>`).
        host: String,
    },
    Update {
        #[arg(long)]
        check: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum DaemonCommand {
    Install(DaemonInstall),
    Uninstall,
    Start,
    Stop,
    Restart,
    Status,
}

#[derive(Args, Debug)]
pub struct DaemonInstall {
    #[arg(long)]
    pub serve: bool,
    #[arg(
        requires = "serve",
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "SERVE_ARGS"
    )]
    pub serve_args: Vec<String>,
}

/// Flag, then `ZUSE_LANGUAGE`, then the system languages. Read before parsing
/// so parser errors are localized too.
pub fn language_from(
    arguments: &[OsString],
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

pub fn localizer(arguments: &[OsString]) -> Localizer {
    language_from(
        arguments,
        std::env::var("ZUSE_LANGUAGE").ok(),
        &zuse_i18n::system_languages(),
    )
}

/// Keyed by `parent child` so `daemon status` and `status` differ.
fn subcommand_about(path: &str, l10n: &Localizer) -> Option<String> {
    Some(match path {
        "zuse serve" => m::about(l10n),
        "zuse headless" => m::headless(l10n),
        "zuse pair" => m::pair(l10n),
        "zuse link" => m::link(l10n),
        "zuse unlink" => m::unlink(l10n),
        "zuse status" => m::overview_status(l10n),
        "zuse login" => m::session_login(l10n),
        "zuse logout" => m::session_logout(l10n),
        "zuse sync" => m::sync(l10n),
        "zuse mcp" => m::mcp(l10n),
        "zuse daemon" => m::daemon(l10n),
        "zuse update" => m::update(l10n),
        "zuse appshot" => m::appshot(l10n),
        "daemon install" => m::daemon_install(l10n),
        "daemon uninstall" => m::daemon_uninstall(l10n),
        "daemon start" => m::daemon_start(l10n),
        "daemon stop" => m::daemon_stop(l10n),
        "daemon restart" => m::daemon_restart(l10n),
        "daemon status" => m::daemon_status(l10n),
        _ => return None,
    })
}

pub(crate) fn argument_help(id: &str, l10n: &Localizer) -> Option<String> {
    Some(match id {
        "language" => m::language(l10n),
        "listen" => m::listen(l10n),
        "data_dir" => m::data_dir(l10n),
        "assets" => m::assets(l10n),
        "token" => m::token(l10n),
        "public_origin" => m::public_origin(l10n),
        "tls_cert" => m::tls_cert(l10n),
        "tls_key" => m::tls_key(l10n),
        "relay_origin" => m::relay_origin(l10n),
        "relay_host_token_file" => m::relay_host_token_file(l10n),
        "lan" => m::lan(l10n),
        "lan_port" => m::lan_port(l10n),
        "tailscale_serve" => m::tailscale_serve(l10n),
        "account" => m::account(l10n),
        "label" => m::pair_label(l10n),
        "app" => m::pair_app(l10n),
        "name" => m::link_name(l10n),
        "no_browser" => m::login_no_browser(l10n),
        "serve" => m::daemon_serve(l10n),
        "serve_args" => m::daemon_serve_args(l10n),
        "check" => m::update_check(l10n),
        "open_url" => m::open_url(l10n),
        "paired_link_file" => m::paired_link_file(l10n),
        _ => return None,
    })
}

pub(crate) type Describe = fn(&str, &Localizer) -> Option<String>;

/// Shared presentation: localized headings, help and version flags, then
/// `about` for each subcommand and `help` for each argument.
pub(crate) fn localize_frame(
    mut command: clap::Command,
    l10n: &Localizer,
    about: Describe,
    help: Describe,
) -> clap::Command {
    let mut sections = vec![];
    if command.has_subcommands() {
        sections.push(format!("{}:\n{{subcommands}}", m::commands(l10n)));
    }
    if command.get_positionals().next().is_some() {
        sections.push(format!("{}:\n{{positionals}}", m::arguments(l10n)));
    }
    sections.push(format!("{}:\n{{options}}", m::options(l10n)));
    let template = format!(
        "{{about-with-newline}}\n{}: {{usage}}\n\n{}",
        m::usage(l10n),
        sections.join("\n\n")
    );
    command = command
        .help_template(template)
        .disable_help_flag(true)
        .disable_help_subcommand(true)
        .arg(
            clap::Arg::new("help")
                .short('h')
                .long("help")
                .action(clap::ArgAction::Help)
                .help(m::help(l10n)),
        )
        .mut_args(|arg| match help(arg.get_id().as_str(), l10n) {
            Some(text) => arg.help(text),
            None => arg,
        });
    if command.get_version().is_some() {
        command = command.disable_version_flag(true).arg(
            clap::Arg::new("version")
                .short('V')
                .long("version")
                .action(clap::ArgAction::Version)
                .help(m::version(l10n)),
        );
    }
    let parent = command.get_name().to_owned();
    command.mut_subcommands(|subcommand| {
        let path = format!("{parent} {}", subcommand.get_name());
        let subcommand = match about(&path, l10n) {
            Some(text) => subcommand.about(text),
            None => subcommand,
        };
        localize_frame(subcommand, l10n, about, help)
    })
}

/// Localize a command tree built from [`Command`] and [`Global`].
pub fn localize(command: clap::Command, l10n: &Localizer) -> clap::Command {
    localize_frame(
        command.about(m::zuse_about(l10n)),
        l10n,
        subcommand_about,
        |id, l10n| {
            (id == "ipc_port")
                .then(|| m::engine_port(l10n))
                .or_else(|| argument_help(id, l10n))
        },
    )
}

/// Parse with localized help and errors; exits on help, version or misuse.
pub fn parse<T: CommandFactory + FromArgMatches>(arguments: Vec<OsString>, l10n: &Localizer) -> T {
    parse_with(localize(T::command(), l10n), arguments, l10n)
}

pub(crate) fn parse_with<T: FromArgMatches>(
    mut command: clap::Command,
    arguments: Vec<OsString>,
    l10n: &Localizer,
) -> T {
    match command.try_get_matches_from_mut(arguments) {
        Ok(matches) => T::from_arg_matches(&matches).expect("Clap validated arguments"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use zuse_i18n::Locale;

    #[derive(Parser, Debug)]
    #[command(name = "zuse", version)]
    struct Cli {
        #[command(flatten)]
        global: Global,
        #[command(subcommand)]
        command: Option<Command>,
    }

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(Into::into).collect()
    }
    fn command(l10n: &Localizer) -> clap::Command {
        localize(Cli::command(), l10n)
    }
    fn parse(values: &[&str]) -> Cli {
        let matches = command(&Localizer::new(Locale::En))
            .try_get_matches_from(values)
            .unwrap();
        Cli::from_arg_matches(&matches).unwrap()
    }

    #[test]
    fn language_precedence_is_flag_then_environment_then_system() {
        let system = vec!["zh-TW".into()];
        assert_eq!(
            language_from(&arguments(&["zuse"]), None, &system).locale(),
            Locale::ZhHant
        );
        assert_eq!(
            language_from(&arguments(&["zuse"]), Some("de".into()), &system).locale(),
            Locale::De
        );
        assert_eq!(
            language_from(
                &arguments(&["zuse", "serve", "--language=fr"]),
                Some("de".into()),
                &system
            )
            .locale(),
            Locale::Fr
        );
    }

    #[test]
    fn every_command_and_flag_is_described_in_every_language() {
        fn walk(command: &clap::Command, path: &str) {
            let path = format!("{path} {}", command.get_name());
            if !command.is_hide_set() && path != " zuse" {
                assert!(command.get_about().is_some(), "{path} has no description");
            }
            for arg in command.get_arguments() {
                if !arg.is_hide_set() {
                    assert!(
                        arg.get_help().is_some(),
                        "{path} {} has no help",
                        arg.get_id()
                    );
                }
            }
            for subcommand in command.get_subcommands() {
                walk(subcommand, &path);
            }
        }
        for locale in Locale::ALL {
            let l10n = Localizer::new(locale);
            let mut command = command(&l10n);
            command.build();
            walk(&command, "");
            let help = command.render_long_help().to_string();
            assert!(help.contains(&m::zuse_about(&l10n)));
            assert!(help.contains(&m::about(&l10n)), "serve is listed");
            assert!(help.contains("--ipc-port"));
            let serve = command.find_subcommand_mut("serve").unwrap();
            let help = serve.render_long_help().to_string();
            assert!(help.contains("--tls-cert") && help.contains("ZUSE_LANGUAGE"));
            assert!(help.contains(&m::engine_port(&l10n)));
        }
    }

    #[test]
    fn serve_takes_the_old_server_flags_and_the_global_engine_port() {
        let cli = parse(&[
            "zuse",
            "serve",
            "--listen",
            "127.0.0.1:0",
            "--ipc-port",
            "0",
            "--lan",
            "--tailscale-serve",
        ]);
        assert_eq!(cli.global.ipc_port, Some(0));
        let Some(Command::Serve(serve)) = cli.command else {
            panic!("expected serve")
        };
        assert_eq!(serve.listen.port(), 0);
        assert!(serve.lan && serve.tailscale_serve);
        let error = command(&Localizer::new(Locale::En))
            .try_get_matches_from(["zuse", "serve", "--tailscale-serve"])
            .unwrap_err();
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn remote_commands_reach_the_chosen_engine_from_either_side() {
        let cli = parse(&["zuse", "pair", "--ipc-port", "4000", "--app"]);
        assert_eq!(cli.global.ipc_port(), 4000);
        assert!(matches!(
            cli.command,
            Some(Command::Pair {
                label: None,
                app: true
            })
        ));
        let cli = parse(&["zuse", "--ipc-port", "4001", "ssh-proxy", "zuse-w1"]);
        assert_eq!(cli.global.ipc_port(), 4001);
        // The ProxyCommand the app writes puts the port after the subcommand.
        let cli = parse(&["zuse", "ssh-proxy", "--ipc-port", "4002", "zuse-w1"]);
        assert_eq!(cli.global.ipc_port(), 4002);
        assert!(matches!(cli.command, Some(Command::SshProxy { host }) if host == "zuse-w1"));
    }

    #[test]
    fn daemon_install_passes_everything_after_serve_through() {
        let cli = parse(&["zuse", "daemon", "install"]);
        let Some(Command::Daemon {
            command: DaemonCommand::Install(install),
        }) = cli.command
        else {
            panic!("expected install")
        };
        assert!(!install.serve && install.serve_args.is_empty());
        let cli = parse(&[
            "zuse",
            "daemon",
            "install",
            "--serve",
            "--lan",
            "--listen",
            "0.0.0.0:27656",
        ]);
        let Some(Command::Daemon {
            command: DaemonCommand::Install(install),
        }) = cli.command
        else {
            panic!("expected install")
        };
        assert!(install.serve);
        assert_eq!(install.serve_args, ["--lan", "--listen", "0.0.0.0:27656"]);
        assert!(
            command(&Localizer::new(Locale::En))
                .try_get_matches_from(["zuse", "daemon", "install", "--lan"])
                .is_err()
        );
    }

    /// The desktop app's shape: a link to open, the shared tree, and its own
    /// commands beside it.
    #[test]
    fn the_desktop_tree_keeps_links_and_its_own_commands() {
        #[derive(Parser, Debug)]
        #[command(name = "zuse", version)]
        struct Desktop {
            #[command(flatten)]
            global: Global,
            #[command(subcommand)]
            command: Option<DesktopCommand>,
            #[arg(value_name = "URL")]
            open_url: Option<String>,
            #[arg(long, value_name = "FILE")]
            paired_link_file: Option<PathBuf>,
        }
        #[derive(Subcommand, Debug)]
        enum DesktopCommand {
            #[command(flatten)]
            Shared(Command),
            Appshot,
        }
        let desktop = |values: &[&str]| {
            let matches = localize(Desktop::command(), &Localizer::new(Locale::En))
                .try_get_matches_from(values)
                .unwrap();
            Desktop::from_arg_matches(&matches).unwrap()
        };
        let headed = desktop(&["zuse"]);
        assert!(headed.command.is_none() && headed.open_url.is_none());
        let link = desktop(&["zuse", "zuse://chat/1"]);
        assert_eq!(link.open_url.as_deref(), Some("zuse://chat/1"));
        let paired = desktop(&["zuse", "--paired-link-file", "/tmp/link"]);
        assert!(paired.paired_link_file.is_some());
        assert!(matches!(
            desktop(&["zuse", "appshot"]).command,
            Some(DesktopCommand::Appshot)
        ));
        assert!(matches!(
            desktop(&["zuse", "serve", "--lan"]).command,
            Some(DesktopCommand::Shared(Command::Serve(serve))) if serve.lan
        ));
        let port = desktop(&["zuse", "--ipc-port", "27700"]);
        assert_eq!(port.global.ipc_port(), 27700);
        let mut command = localize(Desktop::command(), &Localizer::new(Locale::Ja));
        command.build();
        let help = command.render_long_help().to_string();
        assert!(help.contains(&m::appshot(&Localizer::new(Locale::Ja))));
        assert!(help.contains(&m::open_url(&Localizer::new(Locale::Ja))));
    }

    #[test]
    fn invalid_language_is_a_parser_error() {
        let error = command(&Localizer::new(Locale::ZhHans))
            .try_get_matches_from(["zuse", "--language", "unknown", "status"])
            .unwrap_err();
        assert_eq!(error.exit_code(), 2);
        let error = command(&Localizer::new(Locale::ZhHans))
            .try_get_matches_from(["zuse", "mcp", "--help"])
            .unwrap_err();
        assert_eq!(error.exit_code(), 0);
        assert!(
            error
                .to_string()
                .contains(&m::mcp(&Localizer::new(Locale::ZhHans)))
        );
    }
}
