//! The standalone `zuse` command for machines without the desktop app: the
//! same commands, without the window. Invoked as `zuse-server`, it keeps that
//! command's old interface.
use clap::{CommandFactory, Parser};
use zuse_commands::{Command, Global};

/// Usage reads `zuse` whatever cargo or a package named the file.
#[derive(Parser)]
#[command(name = "zuse", bin_name = "zuse", version)]
struct Cli {
    #[command(flatten)]
    global: Global,
    #[command(subcommand)]
    command: Option<Command>,
}

fn main() -> anyhow::Result<()> {
    if zuse_engine::command_host::requested() {
        zuse_engine::command_host::run_blocking()?;
        return Ok(());
    }
    let arguments = std::env::args_os().collect::<Vec<_>>();
    let l10n = zuse_commands::localizer(&arguments);
    if let Some(arguments) = zuse_commands::legacy::invoked(&arguments) {
        return zuse_commands::legacy::main(arguments, &l10n);
    }
    let cli: Cli = zuse_commands::parse(arguments, &l10n);
    let Some(command) = cli.command else {
        zuse_commands::desktop_missing(zuse_commands::localize(Cli::command(), &l10n), &l10n)
    };
    zuse_commands::init_logging(command.logging());
    zuse_commands::run(cli.global, command, &l10n)
}
