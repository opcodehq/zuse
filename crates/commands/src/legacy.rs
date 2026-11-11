//! `zuse-server`, the name `zuse serve` had before it joined the `zuse` tree.
//! Invoked under that name, the standalone binary keeps the old interface:
//! serve flags at the top level, and the remote controls beside them.
use crate::{cli::ServeArgs, remote};
use clap::{CommandFactory, Parser, Subcommand};
use std::ffi::OsString;
use zuse_i18n::{Localizer, messages::cli as m};

/// Hidden first argument for launchers that cannot rename the executable.
pub const MARKER: &str = "--as-zuse-server";

#[derive(Parser)]
#[command(name = "zuse-server", bin_name = "zuse-server")]
pub(crate) struct Args {
    #[arg(long, env = "ZUSE_LANGUAGE", global = true, default_value = "system", value_parser = ["system", "en", "fr", "de", "zh-Hans", "zh-Hant", "ja", "ko"])]
    language: String,
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(long, global = true, default_value_t = crate::DEFAULT_IPC_PORT)]
    ipc_port: u16,
    #[command(flatten)]
    serve: ServeArgs,
}

#[derive(Subcommand)]
enum Command {
    Mcp,
    Pair {
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        app: bool,
    },
    Login,
    Logout,
    Link {
        #[arg(long)]
        name: Option<String>,
    },
    Unlink,
    Status,
}

/// The arguments to parse the old way, when invoked as `zuse-server`.
pub fn invoked(arguments: &[OsString]) -> Option<Vec<OsString>> {
    // Either separator: Windows launchers may pass backslashed paths.
    let program = arguments.first()?.to_string_lossy().to_ascii_lowercase();
    let name = program.rsplit(['/', '\\']).next().unwrap_or_default();
    if name.strip_suffix(".exe").unwrap_or(name) == "zuse-server" {
        return Some(arguments.to_vec());
    }
    (arguments.get(1)? == MARKER).then(|| {
        let mut arguments = arguments.to_vec();
        arguments.remove(1);
        arguments[0] = "zuse-server".into();
        arguments
    })
}

fn about(path: &str, l10n: &Localizer) -> Option<String> {
    Some(match path {
        "zuse-server mcp" => m::mcp(l10n),
        "zuse-server pair" => m::pair(l10n),
        "zuse-server login" => m::login(l10n),
        "zuse-server logout" => m::logout(l10n),
        "zuse-server link" => m::link(l10n),
        "zuse-server unlink" => m::unlink(l10n),
        "zuse-server status" => m::status(l10n),
        _ => return None,
    })
}

pub(crate) fn localized_command(l10n: &Localizer) -> clap::Command {
    crate::cli::localize_frame(
        Args::command().about(m::about(l10n)),
        l10n,
        about,
        |id, l10n| {
            if id == "ipc_port" {
                Some(m::ipc_port(l10n))
            } else {
                crate::cli::argument_help(id, l10n)
            }
        },
    )
}

/// Run `zuse-server [flags] [command]`.
pub fn main(arguments: Vec<OsString>, l10n: &Localizer) -> anyhow::Result<()> {
    let args: Args = crate::cli::parse_with(localized_command(l10n), arguments, l10n);
    let action = match args.command {
        None => {
            crate::init_logging(crate::Logging::Serve);
            return crate::serve::main(args.serve, args.ipc_port, l10n);
        }
        Some(Command::Mcp) => {
            crate::init_logging(crate::Logging::Stdio);
            return crate::mcp();
        }
        Some(Command::Pair { label, app }) => remote::Action::Pair { label, app },
        Some(Command::Login) => remote::Action::SignIn,
        Some(Command::Logout) => remote::Action::SignOut,
        Some(Command::Link { name }) => remote::Action::Link { name },
        Some(Command::Unlink) => remote::Action::Unlink,
        Some(Command::Status) => remote::Action::Status,
    };
    crate::init_logging(crate::Logging::Brief);
    crate::block_on(remote::run(action, args.ipc_port, l10n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::FromArgMatches;
    use std::path::Path;
    use zuse_i18n::Locale;

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(Into::into).collect()
    }

    #[test]
    fn only_the_old_name_or_the_launcher_marker_selects_the_old_interface() {
        assert!(invoked(&arguments(&["/usr/bin/zuse-server", "pair"])).is_some());
        assert!(invoked(&arguments(&[r"C:\Zuse\libexec\zuse-server.exe"])).is_some());
        assert_eq!(
            invoked(&arguments(&["/opt/zuse/bin/zuse", MARKER, "--lan"])),
            Some(arguments(&["zuse-server", "--lan"]))
        );
        assert!(invoked(&arguments(&["/usr/bin/zuse", "pair"])).is_none());
        assert!(invoked(&arguments(&["/usr/bin/zuse", "serve", MARKER])).is_none());
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

    #[test]
    fn launchers_may_put_assets_before_a_command() {
        let matches = localized_command(&Localizer::new(Locale::En))
            .try_get_matches_from(["zuse-server", "--assets", "/opt/zuse/web", "status"])
            .unwrap();
        let args = Args::from_arg_matches(&matches).unwrap();
        assert!(matches!(args.command, Some(Command::Status)));
        assert_eq!(
            args.serve.assets.as_deref(),
            Some(Path::new("/opt/zuse/web"))
        );
    }
}
