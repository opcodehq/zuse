//! `zuse daemon …` — install/manage `zuse headless` (or, with `--serve`,
//! `zuse serve`) as a background service: a systemd **user** unit on Linux
//! (the VPS deployment target), a launchd LaunchAgent on macOS. The unit runs
//! the current executable with the `ZUSE_*` environment captured at install
//! time, so `ZUSE_EDGE_URL=… zuse daemon install` bakes that override in.
//!
//! Auth is decoupled: without a saved session the service remains up on the
//! local-only profile. `zuse login` and a service restart opt into sync.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

const LAUNCHD_LABEL: &str = "sh.zuse.app";
/// Same unit name the curl|sh installer (`edge/src/install.sh`) writes, so
/// `zuse daemon …` manages that installation rather than a competing copy.
const SYSTEMD_UNIT: &str = "zuse.service";

/// Environment captured into the unit file. `PATH` is always included (the
/// engine spawns harness CLIs like `claude`, which service managers' minimal
/// default PATH won't find); the `ZUSE_*`/logging vars only when set.
const CAPTURED_ENV: &[&str] = &[
    "PATH",
    "ZUSE_DATA_DIR",
    "ZUSE_API_URL",
    "ZUSE_CLOUD_API_URL",
    "ZUSE_EDGE_URL",
    "ZUSE_EDGE_TOKEN",
    "ZUSE_ORG_ID",
    "ZUSE_WORKOS_CLIENT_ID",
    "ZUSE_WORKOS_API_BASE",
    "ZUSE_IPC_PORT",
    "ZUSE_CALLBACK_PORT",
    "ZUSE_HARNESS",
    "ZUSE_DEVICE_NAME",
    "ZUSE_LANGUAGE",
    "ZUSE_WEB_ASSETS",
    "ZUSE_PUBLIC_ORIGIN",
    "ZUSE_TLS_CERT",
    "ZUSE_TLS_KEY",
    "RUST_LOG",
];

/// What the service runs: `headless`, or `serve` and its flags.
pub struct Service {
    pub arguments: Vec<String>,
}
impl Service {
    fn description(&self) -> &'static str {
        if self.arguments.iter().any(|argument| argument == "serve") {
            "Zuse server"
        } else {
            "Zuse headless engine"
        }
    }
}

pub fn install(data_dir: &Path, service: &Service) -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("resolving the zuse executable path")?;
    let env = captured_env();
    if cfg!(target_os = "macos") {
        let plist = launchd_plist_path()?;
        std::fs::create_dir_all(plist.parent().expect("LaunchAgents parent"))?;
        std::fs::create_dir_all(data_dir)?;
        // Reinstall-friendly: unload any previous incarnation before rewriting.
        let _ = run_quiet("launchctl", &["bootout", &launchd_service_target()?]);
        std::fs::write(
            &plist,
            render_launchd_plist(&exe, service, &env, &data_dir.join("daemon.log")),
        )?;
        run(
            "launchctl",
            &["bootstrap", &launchd_domain()?, &plist.to_string_lossy()],
        )?;
        println!(
            "Installed and started {LAUNCHD_LABEL} ({}).",
            plist.display()
        );
    } else if cfg!(target_os = "linux") {
        let unit = systemd_unit_path()?;
        std::fs::create_dir_all(unit.parent().expect("systemd user dir"))?;
        std::fs::write(&unit, render_systemd_unit(&exe, service, &env))?;
        run("systemctl", &["--user", "daemon-reload"])?;
        run("systemctl", &["--user", "enable", "--now", SYSTEMD_UNIT])?;
        println!("Installed and started {SYSTEMD_UNIT} ({}).", unit.display());
        println!(
            "For start-at-boot without an active login session (VPS): loginctl enable-linger $USER"
        );
    } else {
        bail!("zuse daemon is only supported on macOS (launchd) and Linux (systemd)");
    }
    println!(
        "Without a saved account the engine stays local-only; sign-in and restart are optional for sync."
    );
    println!(
        "Logs: {}",
        if cfg!(target_os = "macos") {
            format!("{}", data_dir.join("daemon.log").display())
        } else {
            format!("journalctl --user -u {SYSTEMD_UNIT}")
        }
    );
    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    if cfg!(target_os = "macos") {
        let _ = run_quiet("launchctl", &["bootout", &launchd_service_target()?]);
        let plist = launchd_plist_path()?;
        match std::fs::remove_file(&plist) {
            Ok(()) => println!("Removed {}.", plist.display()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                println!("Not installed.")
            }
            Err(err) => return Err(err.into()),
        }
    } else if cfg!(target_os = "linux") {
        let _ = run_quiet("systemctl", &["--user", "disable", "--now", SYSTEMD_UNIT]);
        let unit = systemd_unit_path()?;
        match std::fs::remove_file(&unit) {
            Ok(()) => {
                run("systemctl", &["--user", "daemon-reload"])?;
                println!("Removed {}.", unit.display());
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                println!("Not installed.")
            }
            Err(err) => return Err(err.into()),
        }
    } else {
        bail!("zuse daemon is only supported on macOS (launchd) and Linux (systemd)");
    }
    Ok(())
}

pub fn start() -> anyhow::Result<()> {
    if cfg!(target_os = "macos") {
        let plist = launchd_plist_path()?;
        if !plist.exists() {
            bail!("not installed — run `zuse daemon install` first");
        }
        // `stop` boots the job out of the domain, so start = bootstrap; already
        // loaded is fine, then kickstart guarantees a running process either way.
        let _ = run_quiet(
            "launchctl",
            &["bootstrap", &launchd_domain()?, &plist.to_string_lossy()],
        );
        run("launchctl", &["kickstart", &launchd_service_target()?])?;
    } else if cfg!(target_os = "linux") {
        run("systemctl", &["--user", "start", SYSTEMD_UNIT])?;
    } else {
        bail!("zuse daemon is only supported on macOS (launchd) and Linux (systemd)");
    }
    println!("Started.");
    Ok(())
}

pub fn stop() -> anyhow::Result<()> {
    if cfg!(target_os = "macos") {
        // bootout (not `kill`): with KeepAlive the job would otherwise respawn.
        run("launchctl", &["bootout", &launchd_service_target()?])?;
    } else if cfg!(target_os = "linux") {
        run("systemctl", &["--user", "stop", SYSTEMD_UNIT])?;
    } else {
        bail!("zuse daemon is only supported on macOS (launchd) and Linux (systemd)");
    }
    println!("Stopped.");
    Ok(())
}

pub fn restart() -> anyhow::Result<()> {
    if cfg!(target_os = "macos") {
        if run_quiet(
            "launchctl",
            &["kickstart", "-k", &launchd_service_target()?],
        )
        .is_err()
        {
            // Not loaded (e.g. after `stop`) — fall through to a plain start.
            return start();
        }
        println!("Restarted.");
        Ok(())
    } else if cfg!(target_os = "linux") {
        run("systemctl", &["--user", "restart", SYSTEMD_UNIT])?;
        println!("Restarted.");
        Ok(())
    } else {
        bail!("zuse daemon is only supported on macOS (launchd) and Linux (systemd)");
    }
}

pub fn status() -> anyhow::Result<()> {
    if cfg!(target_os = "macos") {
        let output = Command::new("launchctl")
            .args(["print", &launchd_service_target()?])
            .output()
            .context("running launchctl")?;
        if !output.status.success() {
            println!(
                "{LAUNCHD_LABEL}: not loaded{}",
                if launchd_plist_path()?.exists() {
                    " (installed — `zuse daemon start`)"
                } else {
                    " (not installed — `zuse daemon install`)"
                }
            );
            return Ok(());
        }
        // `launchctl print` is pages long; surface just the liveness lines.
        let text = String::from_utf8_lossy(&output.stdout);
        println!("{LAUNCHD_LABEL}: loaded");
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("state = ")
                || trimmed.starts_with("pid = ")
                || trimmed.starts_with("last exit code = ")
            {
                println!("  {trimmed}");
            }
        }
        Ok(())
    } else if cfg!(target_os = "linux") {
        // Passthrough; `status` exits nonzero for inactive units, which is not an
        // error for us to report — the output already says it.
        let _ = Command::new("systemctl")
            .args(["--user", "--no-pager", "status", SYSTEMD_UNIT])
            .status()
            .context("running systemctl")?;
        Ok(())
    } else {
        bail!("zuse daemon is only supported on macOS (launchd) and Linux (systemd)");
    }
}

// ---------------------------------------------------------------------------
// Unit rendering (pure — unit-tested below)
// ---------------------------------------------------------------------------

fn captured_env() -> Vec<(String, String)> {
    CAPTURED_ENV
        .iter()
        .filter_map(|key| std::env::var(key).ok().map(|v| (key.to_string(), v)))
        .collect()
}

fn render_systemd_unit(exe: &Path, service: &Service, env: &[(String, String)]) -> String {
    let mut unit = format!(
        "[Unit]\nDescription={}\nAfter=network-online.target\nStartLimitIntervalSec=60\nStartLimitBurst=5\n\n[Service]\n",
        service.description()
    );
    for (key, value) in env {
        // systemd unquotes the value; escape the characters it treats specially.
        let value = value.replace('\\', "\\\\").replace('"', "\\\"");
        unit.push_str(&format!("Environment=\"{key}={value}\"\n"));
    }
    let mut command = systemd_exec_path(exe);
    for argument in &service.arguments {
        command.push(' ');
        command.push_str(&systemd_quote(argument));
    }
    unit.push_str(&format!(
        "ExecStart={command}\nRestart=on-failure\nRestartSec=5\nEnvironmentFile=-%h/.zuse/env\n\n[Install]\nWantedBy=default.target\n",
    ));
    unit
}

/// One ExecStart word: specifiers and variables stay literal, and quotes keep
/// spaces inside the argument.
fn systemd_quote(argument: &str) -> String {
    let literal = argument.replace('%', "%%").replace('$', "$$");
    let plain = !literal.is_empty()
        && literal
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.,:/=@+%$".contains(c));
    if plain {
        return literal;
    }
    format!("\"{}\"", literal.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The ExecStart binary path. An exe under `~/.zuse/app/` came from the
/// curl|sh installer, whose upgrades relink `app/current` — point the unit at
/// the symlink (as the installer's own unit does) so it never pins one version.
/// (`current_exe` resolves symlinks, so the versioned dir is what we see here.)
fn systemd_exec_path(exe: &Path) -> String {
    exec_path_for(exe, std::env::var_os("HOME").map(PathBuf::from).as_deref())
}

fn exec_path_for(exe: &Path, home: Option<&Path>) -> String {
    let installed = home
        .map(|home| home.join(".zuse/app"))
        .is_some_and(|app_root| exe.starts_with(app_root));
    if installed {
        "%h/.zuse/app/current/zuse".to_string()
    } else {
        format!("{}", exe.display())
    }
}

fn render_launchd_plist(
    exe: &Path,
    service: &Service,
    env: &[(String, String)],
    log: &Path,
) -> String {
    let mut arguments = String::new();
    for argument in &service.arguments {
        arguments.push_str(&format!(
            "      <string>{}</string>\n",
            xml_escape(argument)
        ));
    }
    let mut env_dict = String::new();
    for (key, value) in env {
        env_dict.push_str(&format!(
            "      <key>{}</key><string>{}</string>\n",
            xml_escape(key),
            xml_escape(value)
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key><string>{label}</string>
    <key>ProgramArguments</key>
    <array>
      <string>{exe}</string>
{arguments}    </array>
    <key>EnvironmentVariables</key>
    <dict>
{env_dict}    </dict>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key>
    <dict>
      <key>SuccessfulExit</key><false/>
    </dict>
    <key>ThrottleInterval</key><integer>30</integer>
    <key>StandardOutPath</key><string>{log}</string>
    <key>StandardErrorPath</key><string>{log}</string>
  </dict>
</plist>
"#,
        label = LAUNCHD_LABEL,
        exe = xml_escape(&exe.to_string_lossy()),
        arguments = arguments,
        env_dict = env_dict,
        log = xml_escape(&log.to_string_lossy()),
    )
}

fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---------------------------------------------------------------------------
// Paths + process helpers
// ---------------------------------------------------------------------------

fn home_dir() -> anyhow::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME not set")
}

fn launchd_plist_path() -> anyhow::Result<PathBuf> {
    Ok(home_dir()?
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist")))
}

fn systemd_unit_path() -> anyhow::Result<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or(home_dir()?.join(".config"));
    Ok(config.join("systemd/user").join(SYSTEMD_UNIT))
}

fn launchd_domain() -> anyhow::Result<String> {
    let output = Command::new("id").arg("-u").output().context("id -u")?;
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uid.is_empty() {
        bail!("could not determine the current uid");
    }
    Ok(format!("gui/{uid}"))
}

fn launchd_service_target() -> anyhow::Result<String> {
    Ok(format!("{}/{LAUNCHD_LABEL}", launchd_domain()?))
}

/// Run a command echoing it first; error (with stderr) on nonzero exit.
fn run(program: &str, args: &[&str]) -> anyhow::Result<()> {
    println!("$ {program} {}", args.join(" "));
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("running {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Run without echoing; used where failure is an expected branch.
fn run_quiet(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("running {program}"))?;
    if !output.status.success() {
        bail!("{program} failed ({})", output.status);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headless() -> Service {
        Service {
            arguments: vec!["headless".into()],
        }
    }

    #[test]
    fn systemd_unit_shape() {
        let unit = render_systemd_unit(
            Path::new("/usr/local/bin/zuse"),
            &headless(),
            &[
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("ZUSE_EDGE_URL".into(), "https://edge.example".into()),
                ("RUST_LOG".into(), "info,zuse=\"debug\"".into()),
            ],
        );
        assert!(unit.contains("ExecStart=/usr/local/bin/zuse headless\n"));
        assert!(unit.contains("Description=Zuse headless engine\n"));
        assert!(unit.contains("Environment=\"PATH=/usr/bin:/bin\"\n"));
        assert!(unit.contains("Environment=\"ZUSE_EDGE_URL=https://edge.example\"\n"));
        // Inner quotes escaped so systemd re-parses the value verbatim.
        assert!(unit.contains("Environment=\"RUST_LOG=info,zuse=\\\"debug\\\"\"\n"));
        assert!(unit.contains("StartLimitIntervalSec=60\n"));
        assert!(unit.contains("StartLimitBurst=5\n"));
        assert!(unit.contains("Restart=on-failure"));
        assert!(!unit.contains("session.json"));
        assert!(!unit.contains("ConditionPathExists"));
        assert!(unit.contains("EnvironmentFile=-%h/.zuse/env"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn serve_services_keep_each_argument_whole_and_literal() {
        let service = Service {
            arguments: [
                "--ipc-port",
                "27700",
                "serve",
                "--lan",
                "--data-dir",
                "/srv/zuse data",
                "--public-origin",
                "https://box.example/$HOME%h",
                "--token",
                r#"a"b\c"#,
            ]
            .map(String::from)
            .to_vec(),
        };
        let unit = render_systemd_unit(Path::new("/usr/bin/zuse"), &service, &[]);
        assert!(unit.contains("Description=Zuse server\n"));
        assert!(unit.contains(
            r#"ExecStart=/usr/bin/zuse --ipc-port 27700 serve --lan --data-dir "/srv/zuse data" --public-origin https://box.example/$$HOME%%h --token "a\"b\\c""#
        ));
        let plist = render_launchd_plist(
            Path::new("/Applications/Zuse.app/Contents/MacOS/zuse"),
            &service,
            &[],
            Path::new("/tmp/daemon.log"),
        );
        let arguments: Vec<_> = plist
            .lines()
            .skip_while(|line| !line.contains("ProgramArguments"))
            .take_while(|line| !line.contains("</array>"))
            .filter_map(|line| line.trim().strip_prefix("<string>"))
            .filter_map(|line| line.strip_suffix("</string>"))
            .collect();
        assert_eq!(arguments[0], "/Applications/Zuse.app/Contents/MacOS/zuse");
        assert_eq!(arguments[3..6], ["serve", "--lan", "--data-dir"]);
        assert_eq!(arguments[6], "/srv/zuse data");
    }

    #[test]
    fn installed_exe_uses_the_current_symlink() {
        // Installer-managed binary (current_exe resolves the `current` symlink to
        // the versioned dir): the unit must point back at the symlink.
        assert_eq!(
            exec_path_for(
                Path::new("/home/u/.zuse/app/0.3.0/zuse"),
                Some(Path::new("/home/u")),
            ),
            "%h/.zuse/app/current/zuse"
        );
        // Source build: literal path.
        assert_eq!(
            exec_path_for(
                Path::new("/src/target/debug/zuse"),
                Some(Path::new("/home/u"))
            ),
            "/src/target/debug/zuse"
        );
    }

    #[test]
    fn launchd_plist_shape() {
        let plist = render_launchd_plist(
            Path::new("/Users/x/zuse & co/zuse"),
            &headless(),
            &[("ZUSE_EDGE_URL".into(), "https://e?a=1&b=2".into())],
            Path::new("/Users/x/.zuse/daemon.log"),
        );
        assert!(plist.contains("<key>Label</key><string>sh.zuse.app</string>"));
        // XML-escaped exe path and env value.
        assert!(plist.contains("<string>/Users/x/zuse &amp; co/zuse</string>"));
        assert!(plist.contains("<string>https://e?a=1&amp;b=2</string>"));
        assert!(plist.contains("<string>headless</string>"));
        assert!(plist.contains("<key>SuccessfulExit</key><false/>"));
        assert!(
            plist.contains("<key>StandardOutPath</key><string>/Users/x/.zuse/daemon.log</string>")
        );
    }
    #[cfg(unix)]
    #[test]
    fn staging_api_origins_survive_daemon_capture_and_spawn() {
        const CHILD: &str = "ZUSE_TEST_DAEMON_ORIGIN_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::staging_api_origins_survive_daemon_capture_and_spawn",
                    "--nocapture",
                ])
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env(CHILD, "1")
                .env("ZUSE_API_URL", "https://general-staging.example")
                .env("ZUSE_CLOUD_API_URL", "https://cloud-staging.example")
                .env("ZUSE_WORKOS_CLIENT_ID", "client_staging_public")
                .env("ZUSE_EDGE_URL", "http://127.0.0.1:27655")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let env = captured_env();
        for (key, value) in [
            ("ZUSE_API_URL", "https://general-staging.example"),
            ("ZUSE_CLOUD_API_URL", "https://cloud-staging.example"),
            ("ZUSE_WORKOS_CLIENT_ID", "client_staging_public"),
            ("ZUSE_EDGE_URL", "http://127.0.0.1:27655"),
        ] {
            assert!(
                env.iter()
                    .any(|(name, actual)| name == key && actual == value),
                "missing captured {key}"
            );
            let unit = render_systemd_unit(Path::new("/usr/bin/zuse"), &headless(), &env);
            assert!(unit.contains(&format!("Environment=\"{key}={value}\"")));
            let plist = render_launchd_plist(
                Path::new("/Applications/Zuse"),
                &headless(),
                &env,
                Path::new("/tmp/daemon.log"),
            );
            assert!(plist.contains(&format!("<key>{key}</key><string>{value}</string>")));
        }
        let spawned = Command::new("/usr/bin/env")
            .env_clear()
            .envs(env)
            .output()
            .unwrap();
        assert!(spawned.status.success());
        let public = String::from_utf8(spawned.stdout).unwrap();
        assert!(
            public
                .lines()
                .any(|s| s == "ZUSE_API_URL=https://general-staging.example")
        );
        assert!(
            public
                .lines()
                .any(|s| s == "ZUSE_CLOUD_API_URL=https://cloud-staging.example")
        );
        assert!(
            public
                .lines()
                .any(|s| s == "ZUSE_EDGE_URL=http://127.0.0.1:27655")
        );
    }
}
