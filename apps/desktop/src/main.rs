//! zuse — headed by default; every subcommand comes from `zuse-commands`, the
//! same tree the standalone binary runs. Only the window, opening links and
//! Appshot live here.

#![cfg_attr(windows, windows_subsystem = "windows")]

use clap::{Parser, Subcommand};
use zuse_commands::{Command, Global, Logging};

#[derive(Parser)]
#[command(name = "zuse", bin_name = "zuse", version)]
struct Cli {
    #[command(flatten)]
    global: Global,
    #[command(subcommand)]
    command: Option<DesktopCommand>,
    #[arg(value_name = "URL")]
    open_url: Option<String>,
    #[arg(long, value_name = "FILE")]
    paired_link_file: Option<std::path::PathBuf>,
    #[cfg(windows)]
    #[arg(long, hide = true)]
    wait_for_exit: Option<u32>,
}

#[derive(Subcommand)]
enum DesktopCommand {
    #[command(flatten)]
    Shared(Command),
    #[cfg(target_os = "linux")]
    /// Desktop shortcut fallback.
    Appshot,
}

/// mimalloc, macOS only: libmalloc never returns the streaming churn's
/// high-water pages, so transient allocation became permanent RSS
/// (docs/memory-plan.md §1). Pinned to mimalloc v2 in the workspace manifest —
/// the crate's default v3 has the same pathology (churn retained as permanent
/// RSS, ~6x glibc's growth on identical workloads, no idle recovery). Linux
/// measured flat on glibc, so it keeps the system allocator.
#[cfg(target_os = "macos")]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> anyhow::Result<()> {
    if zuse_engine::command_host::requested() {
        zuse_engine::command_host::run_blocking()?;
        return Ok(());
    }
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--noop-browser")) {
        return Ok(());
    }
    #[cfg(windows)]
    attach_parent_console();
    // A stray ZUSE_LANGUAGE must not keep the app from opening; --language stays strict.
    if std::env::var("ZUSE_LANGUAGE").is_ok_and(|value| !zuse_commands::is_language(&value)) {
        // SAFETY: still single-threaded; nothing has read the environment concurrently.
        unsafe { std::env::remove_var("ZUSE_LANGUAGE") };
    }
    let arguments = std::env::args_os().collect::<Vec<_>>();
    let l10n = zuse_commands::localizer(&arguments);
    let cli: Cli = zuse_commands::parse(arguments, &l10n);
    #[cfg(windows)]
    if let Some(pid) = cli.wait_for_exit {
        zuse_update::windows::wait_for_exit(pid)?;
    } else if matches!(
        &cli.command,
        None | Some(DesktopCommand::Shared(Command::Headless))
    ) {
        zuse_update::windows::cleanup_previous_image();
    }
    zuse_commands::init_logging(match &cli.command {
        None => Logging::Headed,
        Some(DesktopCommand::Shared(command)) => command.logging(),
        #[cfg(target_os = "linux")]
        Some(DesktopCommand::Appshot) => Logging::Brief,
    });
    anyhow::ensure!(
        cli.paired_link_file.is_none() || cli.command.is_none(),
        "--paired-link-file is available for the desktop workspace"
    );
    match cli.command {
        Some(DesktopCommand::Shared(command)) => zuse_commands::run(cli.global, command, &l10n),
        #[cfg(target_os = "linux")]
        Some(DesktopCommand::Appshot) => {
            zuse_ui::appshots::request_running_appshot(&zuse_commands::paths::data_dir())
                .map_err(anyhow::Error::msg)
        }
        None => {
            // Headed: the UI probes the IPC port and connects to a running
            // daemon, or embeds the engine in-process (ARCHITECTURE §1).
            let paired_target = cli
                .paired_link_file
                .as_deref()
                .map(read_paired_target)
                .transpose()?;
            zuse_ui::run_app(zuse_ui::UiConfig {
                paired_target,
                data_dir: zuse_commands::paths::data_dir(),
                ipc_port: cli.global.ipc_port(),
                edge_url: zuse_commands::config::edge_url_from_env(),
                workos_client_id: zuse_commands::config::workos_client_id_from_env(),
                edge_token: std::env::var("ZUSE_EDGE_TOKEN").ok(),
                org_id: std::env::var("ZUSE_ORG_ID").ok(),
                default_harness: zuse_ui::HarnessId::ClaudeCode,
                initial_url: cli.open_url,
            });
            Ok(())
        }
    }
}

#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE, SetStdHandle,
    };

    // The GUI subsystem prevents Explorer from creating a console at startup.
    // Reuse an existing parent's console for CLI output and cargo run, without
    // allocating one. Attach before Clap so help and argument errors work too.
    // Preserve redirected pipes/files: attaching may replace standard handles.
    unsafe {
        let saved = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .map(|id| (id, GetStdHandle(id)));
        if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            for (id, handle) in saved {
                if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                    SetStdHandle(id, handle);
                }
            }
        }
    }
}

/// Import without printing capabilities in diagnostics.
fn read_paired_target(path: &std::path::Path) -> anyhow::Result<zuse_remote::PairedTarget> {
    zuse_remote::PairedTarget::read_link_file(path)
        .map_err(|_| anyhow::anyhow!("Could not read a valid private pairing link file"))
}
