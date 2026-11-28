use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::{env, sync::Arc};
use zuse_cloud::{
    auth::WorkosIdentity,
    config::Config,
    db::{
        Budget, ProcessDb, Role,
        wake::{Cadence, Reason, Work, sleep_or_notify},
    },
    routes::{AppState, router},
    runtime::{
        RuntimeConfig, RuntimeControl,
        due::{WorkerDue, should_run},
    },
    store::Store,
};

/// Hourly maintenance: cleanup sweeps and condition-based retirement.
const MAINTENANCE_MS: i64 = 3_600_000;

#[derive(Parser)]
#[command(about = "Zuse hosted cloud control plane")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Discover actual selected-account Codex models; staging authority operator only.
    DiscoverCodexModels {
        #[arg(long)]
        owner: String,
        #[arg(long)]
        connection: String,
        #[arg(long, env = "ZUSE_AUTH_CODEX_BINARY")]
        binary: std::path::PathBuf,
        #[arg(
            long,
            env = "ZUSE_CLOUD_AUTH_DRIVER",
            default_value = "/usr/local/lib/zuse/auth-driver.py"
        )]
        driver: std::path::PathBuf,
    },
    /// Discover the selected Grok account through isolated external authentication.
    DiscoverGrokModels {
        #[arg(long)]
        owner: String,
        #[arg(long)]
        connection: String,
        #[arg(long, env = "ZUSE_AUTH_GROK_BINARY")]
        binary: std::path::PathBuf,
        /// Exact managed guest executable mounted under its auth-helper name.
        #[arg(long)]
        helper: std::path::PathBuf,
        #[arg(
            long,
            env = "ZUSE_CLOUD_AUTH_DRIVER",
            default_value = "/usr/local/lib/zuse/auth-driver.py"
        )]
        driver: std::path::PathBuf,
    },
    /// Request a genuine selected-connection OAuth refresh; staging authority only, no model call.
    RenewCodexConnection {
        #[arg(long)]
        owner: String,
        #[arg(long)]
        connection: String,
        #[arg(
            long,
            env = "ZUSE_CLOUD_AUTH_DRIVER",
            default_value = "/usr/local/lib/zuse/auth-driver.py"
        )]
        driver: std::path::PathBuf,
    },
    /// Verify staging object storage with an isolated immutable write/read/delete.
    ObjectStoreCheck,
    /// Certify a staged managed-base candidate on its provider: one computer is
    /// created, checked end to end and deleted. Writes the report, and the
    /// registrable release only when every check passed.
    CertifyManagedBase {
        #[arg(long)]
        provider: String,
        #[arg(long)]
        release_id: String,
        #[arg(long, default_value = ".")]
        output: std::path::PathBuf,
        /// Seconds between sleep and resume (660 certifies Boat's cold restore).
        #[arg(long, default_value_t = 0)]
        sleep_wait_secs: u64,
    },
    /// Print the curated Claude Code catalogue every managed base carries
    /// (`infra/cloud/managed-base-claude-catalog.json`).
    PrintClaudeBaseCatalog,
    /// Register an already certified immutable platform base using an operator database role.
    RegisterManagedBase {
        #[arg(long)]
        release: std::path::PathBuf,
    },
    /// Register a signed guest runtime release using an operator database role.
    /// The signature is checked against the release key before anything is written.
    RegisterRuntimeRelease {
        #[arg(long)]
        release: std::path::PathBuf,
        /// Channel state to register (or move) the release into.
        #[arg(long, default_value = "candidate", value_parser = ["candidate", "staging", "stable", "retired"])]
        state: String,
    },
    /// Move a registered runtime release to another channel state.
    SetRuntimeReleaseState {
        #[arg(long)]
        sha256: String,
        #[arg(long, value_parser = ["candidate", "staging", "stable", "retired"])]
        state: String,
    },
    /// Register a signed agent CLI harness bundle using an operator database
    /// role. The signature is checked against the release key before anything
    /// is written.
    RegisterHarnessBundle {
        #[arg(long)]
        bundle: std::path::PathBuf,
        /// Channel state to register (or move) the bundle into.
        #[arg(long, default_value = "candidate", value_parser = ["candidate", "staging", "canary", "stable", "retired"])]
        state: String,
    },
    /// Move a registered harness bundle to another channel state. Retiring
    /// the bundle computers run rolls them back to the newest good bundle.
    SetHarnessBundleState {
        #[arg(long)]
        sha256: String,
        #[arg(long, value_parser = ["candidate", "staging", "canary", "stable", "retired"])]
        state: String,
    },
    /// Stage an uncertified artifact for one explicitly pinned staging owner.
    StageManagedBaseCandidate {
        #[arg(long)]
        candidate: std::path::PathBuf,
        #[arg(long)]
        owner: String,
        /// Exact initiating actor for an organization-only staging candidate.
        #[arg(long)]
        actor: Option<String>,
        #[arg(long,default_value_t=7200,value_parser=clap::value_parser!(i64).range(60..=86400))]
        ttl_seconds: i64,
        #[arg(long, default_value_t = false)]
        allow_staging_compute: bool,
    },
    /// Serve the cloud API; runtime control writes require explicit configuration.
    Serve,
    /// Serve only the workspace WebSocket relay and health endpoints.
    Gateway,
    /// Inspect existing schema/data counts without changing ownership or data.
    Inspect,
    /// Apply the plugin metadata and pinned Executor SDK migrations explicitly.
    PluginMigrate,
    /// Admit initial allocation for an already Rust-owned workspace and approved prepared image.
    Allocate {
        #[arg(long)]
        workspace_id: String,
        #[arg(long)]
        snapshot: String,
    },
    /// Reconcile explicitly owned account image preparation and verified snapshots.
    ImageWorker {
        #[arg(long, default_value_t = false)]
        once: bool,
    },
    /// Verify agent connections and run cancellable device login operations.
    AuthWorker {
        #[arg(long, default_value_t = false)]
        once: bool,
        /// Validate the installed driver and schema without claiming any operation.
        #[arg(long, default_value_t = false, conflicts_with = "once")]
        check: bool,
        #[arg(
            long,
            env = "ZUSE_CLOUD_AUTH_DRIVER",
            default_value = "/usr/local/lib/zuse/auth-driver.py"
        )]
        driver: std::path::PathBuf,
    },
    /// Drain billing/telemetry outboxes for accounts with explicit Rust export ownership.
    BillingWorker {
        #[arg(long, default_value_t = false)]
        once: bool,
    },
    /// Publish reviewed embedded billing prices; never runs on API startup.
    BillingCatalog {
        #[arg(long)]
        catalog: std::path::PathBuf,
        #[arg(long)]
        expected_revision: i64,
    },
    /// Enroll one owned account; adoption keeps customer/subscription IDs and invoice credit.
    BillingEnroll {
        #[arg(long)]
        owner: String,
        #[arg(long, default_value_t = false)]
        adopt_existing: bool,
    },
    /// Inspect or explicitly retry a durable account-scoped billing operation.
    BillingOperation {
        #[arg(long)]
        owner: String,
        #[arg(long)]
        operation: String,
        #[arg(long, default_value_t = false)]
        retry: bool,
    },
    /// Run only durable project preparation; one-shot mode awaits its actual result.
    ProjectPreparationWorker {
        #[arg(long, default_value_t = false)]
        once: bool,
        /// Validate configuration/schema without claiming preparation or contacting providers.
        #[arg(long, default_value_t = false, conflicts_with = "once")]
        check: bool,
        #[arg(long, default_value_t = 1000,
            value_parser = clap::value_parser!(u64).range(250..=60_000))]
        poll_interval_ms: u64,
    },
    /// Reconcile allocations and retained-machine lifecycle independently of API viewers.
    Worker {
        #[arg(long, default_value_t = false)]
        once: bool,
        /// Validate configuration and schema without claiming jobs or contacting providers.
        #[arg(long, default_value_t = false, conflicts_with = "once")]
        check: bool,
        /// Shortest wait while work stays due. Idle waits end on NOTIFY, the next
        /// due time, or ZUSE_CLOUD_WORKER_IDLE_POLL_MS.
        #[arg(long, env = "ZUSE_CLOUD_WORKER_POLL_MS", default_value_t = 1000,
            value_parser = clap::value_parser!(u64).range(250..=60_000))]
        poll_interval_ms: u64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(
            if std::env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").as_deref() == Ok("staging")
                && std::env::var("ZUSE_CLOUD_STARTUP_TRACING").as_deref() == Ok("1")
            {
                tracing_subscriber::fmt::format::FmtSpan::CLOSE
            } else {
                tracing_subscriber::fmt::format::FmtSpan::NONE
            },
        )
        .init();
    let args = Args::parse();
    match args.command.unwrap_or(Command::Serve) {
        command @ (Command::BillingCatalog { .. }
        | Command::BillingEnroll { .. }
        | Command::BillingOperation { .. }) => {
            let url = env::var("ZUSE_CLOUD_DATABASE_URL")
                .context("ZUSE_CLOUD_DATABASE_URL is required")?;
            let accounts =
                zuse_cloud::access_policy::Accounts::from_env("ZUSE_CLOUD_BILLING_WRITE_ACCOUNTS")?;
            let db = process_db(&url, Role::BillingWorker).await?;
            let mut control = zuse_cloud::billing::BillingControl::from_pools(
                db.pool.clone(),
                db.receipts_pool(),
                accounts,
            )
            .connect_native_from_env(&url)
            .await?;
            if matches!(&command, Command::BillingEnroll { .. }) {
                if let Some(providers) = zuse_cloud::billing::PaymentProviders::from_env()? {
                    control = control.with_payment_providers(providers);
                }
            }
            let result = match command {
                Command::BillingCatalog {
                    catalog,
                    expected_revision,
                } => {
                    let catalog = serde_json::from_slice(&std::fs::read(catalog)?)?;
                    control
                        .publish_native_catalog(catalog, expected_revision)
                        .await
                }
                Command::BillingEnroll {
                    owner,
                    adopt_existing,
                } => control
                    .enroll_native(&owner, adopt_existing, now_ms())
                    .await
                    .map(|_| serde_json::json!({"enrolled":true})),
                Command::BillingOperation {
                    owner,
                    operation,
                    retry,
                } => control.native_operation(&owner, &operation, retry).await,
                _ => unreachable!(),
            };
            let closed = control.close_native_billing().await;
            println!("{}", result.map_err(|e| anyhow::anyhow!("{}", e.code))?);
            closed.map_err(|e| anyhow::anyhow!("{}", e.code))?;
        }
        command @ (Command::DiscoverCodexModels { .. } | Command::DiscoverGrokModels { .. }) => {
            let (agent, owner, connection, binary, helper, driver) = match command {
                Command::DiscoverCodexModels {
                    owner,
                    connection,
                    binary,
                    driver,
                } => ("codex", owner, connection, binary, None, driver),
                Command::DiscoverGrokModels {
                    owner,
                    connection,
                    binary,
                    helper,
                    driver,
                } => ("grok", owner, connection, binary, Some(helper), driver),
                _ => unreachable!(),
            };
            anyhow::ensure!(
                env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").as_deref() == Ok("staging"),
                "Model discovery requires staging"
            );
            let accounts = env::var("ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            anyhow::ensure!(
                accounts == [owner.clone()],
                "Model discovery requires exact staging owner"
            );
            let url =
                env::var("ZUSE_CLOUD_DATABASE_URL").context("Authority database unavailable")?;
            let encryption =
                env::var("CLOUD_DATA_ENCRYPTION_KEY").context("Authority key unavailable")?;
            let master = zuse_cloud::crypto::MasterKey::parse(&encryption)
                .map_err(|_| anyhow::anyhow!("Invalid authority key"))?;
            let control = Arc::new(zuse_cloud::connections::ConnectionControl::from_pool(
                process_db(&url, Role::Operator).await?.pool,
                master,
                accounts,
            ));
            let driver = Arc::new(zuse_cloud::connections::worker::CommandDriver::new(
                &driver,
            )?);
            let catalog = if let Some(helper) = helper {
                control
                    .discover_grok_catalog(&owner, &connection, &binary, &helper, driver)
                    .await
            } else {
                control
                    .discover_codex_catalog(&owner, &connection, &binary, driver)
                    .await
            }
            .map_err(|e| anyhow::anyhow!("{}", e.code))?;
            // This is public catalog data only; credential bytes never leave authority memory.
            println!(
                "{}",
                serde_json::to_string(
                    &serde_json::json!({"agent":agent,"connectionId":connection,"source":"live-account-model-list","catalog":catalog})
                )?
            );
        }
        Command::RenewCodexConnection {
            owner,
            connection,
            driver,
        } => {
            anyhow::ensure!(
                env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").as_deref() == Ok("staging"),
                "Credential renewal requires staging"
            );
            let accounts = env::var("ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            anyhow::ensure!(
                accounts == [owner.clone()],
                "Credential renewal requires exact staging owner"
            );
            let binary = std::path::PathBuf::from(
                env::var_os("ZUSE_AUTH_CODEX_BINARY")
                    .context("Pinned Codex authority binary unavailable")?,
            );
            anyhow::ensure!(
                binary.is_absolute() && binary.is_file(),
                "Pinned Codex authority binary unavailable"
            );
            let url =
                env::var("ZUSE_CLOUD_DATABASE_URL").context("Authority database unavailable")?;
            let encryption =
                env::var("CLOUD_DATA_ENCRYPTION_KEY").context("Authority key unavailable")?;
            let master = zuse_cloud::crypto::MasterKey::parse(&encryption)
                .map_err(|_| anyhow::anyhow!("Invalid authority key"))?;
            let control = zuse_cloud::connections::ConnectionControl::from_pool(
                process_db(&url, Role::Operator).await?.pool,
                master,
                accounts,
            );
            let driver = zuse_cloud::connections::worker::CommandDriver::new(&driver)?;
            let receipt = control
                .renew_codex_connection(&owner, &connection, &driver)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.code))?;
            println!("{}", serde_json::to_string(&receipt)?);
        }
        Command::PrintClaudeBaseCatalog => {
            print!(
                "{}",
                zuse_cloud::model_catalog::claude_code_base_catalog_json()
            );
        }
        Command::ObjectStoreCheck => {
            let report = zuse_cloud::objects::probe::staging().await?;
            println!("{}", serde_json::to_string(&report)?);
            anyhow::ensure!(
                report.status == "passed",
                "Staging object store verification failed"
            );
        }
        Command::CertifyManagedBase {
            provider,
            release_id,
            output,
            sleep_wait_secs,
        } => {
            let url = env::var("ZUSE_CLOUD_DATABASE_URL")
                .context("Certification reads the candidate through ZUSE_CLOUD_DATABASE_URL")?;
            let candidate = zuse_cloud::certification::candidate(&url, &provider, &release_id)
                .await
                .map_err(|e| anyhow::anyhow!(e))?;
            let target = zuse_cloud::certification::Target::from_candidate(&candidate)
                .map_err(|e| anyhow::anyhow!(e))?;
            let registry = zuse_cloud::providers::registry::Registry::from_environment();
            let adapter = registry
                .get(target.provider)
                .with_context(|| format!("{provider} is not configured on this host"))?;
            let options = zuse_cloud::certification::Options {
                sleep_wait: std::time::Duration::from_secs(sleep_wait_secs),
                ..Default::default()
            };
            let report =
                zuse_cloud::certification::certify(adapter.as_ref(), &target, &options).await;
            std::fs::create_dir_all(&output)?;
            // Release ids may contain '/' or '.'; keep report files inside --output.
            let stem: String = release_id
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let path = output.join(format!("{stem}-certification.json"));
            std::fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
            for check in &report.checks {
                println!(
                    "{} {:<26} {}",
                    if check.passed { "pass" } else { "FAIL" },
                    check.name,
                    check.detail
                );
            }
            println!("traits {}", report.traits);
            println!("report {} sha256 {}", path.display(), report.sha256());
            if !report.passed {
                anyhow::bail!("Certification failed; the base is not registrable");
            }
            let release = zuse_cloud::certification::release(&candidate, &report)
                .map_err(|e| anyhow::anyhow!(e))?;
            let release_path = output.join(format!("{stem}-release.json"));
            std::fs::write(&release_path, serde_json::to_vec_pretty(&release)?)?;
            println!(
                "Certified. Register with: register-managed-base --release {}",
                release_path.display()
            );
        }
        Command::RegisterManagedBase { release } => {
            let bytes = std::fs::read(release).context("Managed base release file unavailable")?;
            anyhow::ensure!(
                bytes.len() <= 256 * 1024,
                "Managed base release file too large"
            );
            let release: zuse_cloud::managed_bases::ManagedBaseRelease =
                serde_json::from_slice(&bytes)
                    .map_err(|_| anyhow::anyhow!("Invalid managed base release manifest"))?;
            release
                .validate()
                .map_err(|e| anyhow::anyhow!("{}", e.code))?;
            let url = env::var("ZUSE_CLOUD_RELEASE_DATABASE_URL")
                .context("Registration requires explicit ZUSE_CLOUD_RELEASE_DATABASE_URL with operator permissions")?;
            release
                .register_url(&url, now_ms())
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.code))?;
            println!(
                "Managed base registered: {}/{}",
                release.provider.as_str(),
                release.release_id
            );
        }
        Command::RegisterRuntimeRelease { release, state } => {
            let bytes = std::fs::read(release).context("Runtime release file unavailable")?;
            anyhow::ensure!(bytes.len() <= 64 * 1024, "Runtime release file too large");
            let release: zuse_cloud::runtime_releases::ReleaseFile = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("Invalid runtime release file"))?;
            let state = zuse_cloud::runtime_releases::ReleaseState::parse(&state)
                .context("Invalid runtime release state")?;
            let key = zuse_cloud::runtime_releases::contract_verifying_key();
            // Verify before reading any database credential.
            release
                .verify(&key)
                .map_err(|e| anyhow::anyhow!("{}", e.code))?;
            let url = env::var("ZUSE_CLOUD_RELEASE_DATABASE_URL")
                .context("Registration requires explicit ZUSE_CLOUD_RELEASE_DATABASE_URL with operator permissions")?;
            release
                .register_url(&url, &key, state, now_ms())
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.code))?;
            println!(
                "Runtime release registered: {} {} {} ({})",
                release.release.architecture,
                release.release.version,
                release.release.sha256,
                state.as_str()
            );
        }
        Command::SetRuntimeReleaseState { sha256, state } => {
            let state = zuse_cloud::runtime_releases::ReleaseState::parse(&state)
                .context("Invalid runtime release state")?;
            let url = env::var("ZUSE_CLOUD_RELEASE_DATABASE_URL")
                .context("Changing a release requires explicit ZUSE_CLOUD_RELEASE_DATABASE_URL with operator permissions")?;
            zuse_cloud::runtime_releases::set_state_url(&url, &sha256, state, now_ms())
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.code))?;
            println!("Runtime release {sha256} is now {}", state.as_str());
        }
        Command::RegisterHarnessBundle { bundle, state } => {
            let bytes = std::fs::read(bundle).context("Harness bundle file unavailable")?;
            anyhow::ensure!(bytes.len() <= 256 * 1024, "Harness bundle file too large");
            let bundle: zuse_cloud::harness_bundles::BundleFile = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("Invalid harness bundle file"))?;
            let state = zuse_cloud::harness_bundles::BundleState::parse(&state)
                .context("Invalid harness bundle state")?;
            let key = zuse_cloud::runtime_releases::contract_verifying_key();
            // Verify before reading any database credential.
            bundle
                .verify(&key)
                .map_err(|e| anyhow::anyhow!("{}", e.code))?;
            let url = env::var("ZUSE_CLOUD_RELEASE_DATABASE_URL")
                .context("Registration requires explicit ZUSE_CLOUD_RELEASE_DATABASE_URL with operator permissions")?;
            bundle
                .register_url(&url, &key, state, now_ms())
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.code))?;
            println!(
                "Harness bundle registered: {} {} {} {} ({})",
                bundle.bundle.agent,
                bundle.bundle.architecture,
                bundle.bundle.version,
                bundle.bundle.sha256,
                state.as_str()
            );
        }
        Command::SetHarnessBundleState { sha256, state } => {
            let state = zuse_cloud::harness_bundles::BundleState::parse(&state)
                .context("Invalid harness bundle state")?;
            let url = env::var("ZUSE_CLOUD_RELEASE_DATABASE_URL")
                .context("Changing a bundle requires explicit ZUSE_CLOUD_RELEASE_DATABASE_URL with operator permissions")?;
            zuse_cloud::harness_bundles::set_state_url(&url, &sha256, state, now_ms())
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.code))?;
            println!("Harness bundle {sha256} is now {}", state.as_str());
        }
        Command::StageManagedBaseCandidate {
            candidate,
            owner,
            actor,
            ttl_seconds,
            allow_staging_compute,
        } => {
            let bytes = std::fs::read(candidate).context("Candidate file unavailable")?;
            anyhow::ensure!(bytes.len() <= 256 * 1024, "Candidate file too large");
            let candidate: zuse_cloud::managed_bases::candidate::ManagedBaseCandidate =
                serde_json::from_slice(&bytes).context("Invalid candidate manifest")?;
            let url = env::var("ZUSE_CLOUD_RELEASE_DATABASE_URL")
                .context("Candidate staging requires operator database URL")?;
            let pin = if let Some(actor) = actor.as_deref() {
                candidate
                    .stage_for_actor_url(
                        &url,
                        &owner,
                        actor,
                        ttl_seconds,
                        allow_staging_compute,
                        now_ms(),
                    )
                    .await
            } else {
                candidate
                    .stage_url(&url, &owner, ttl_seconds, allow_staging_compute, now_ms())
                    .await
            }
            .map_err(|e| anyhow::anyhow!("{}", e.code))?;
            println!("{}", serde_json::to_string(&pin)?);
        }
        Command::Gateway => serve_gateway().await?,
        Command::AuthWorker {
            once,
            check,
            driver,
        } => {
            let accounts =
                zuse_cloud::access_policy::Accounts::from_env("ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS")?;
            anyhow::ensure!(
                !accounts.is_empty(),
                "Credential authority requires explicit account ownership"
            );
            let url = env::var("ZUSE_CLOUD_DATABASE_URL")
                .context("Credential authority requires its database URL")?;
            let encryption = env::var("CLOUD_DATA_ENCRYPTION_KEY")
                .context("Credential authority requires its encryption key")?;
            let master = zuse_cloud::crypto::MasterKey::parse(&encryption)
                .map_err(|_| anyhow::anyhow!("Invalid credential authority encryption key"))?;
            let db = process_db(&url, Role::AuthWorker).await?;
            let control = zuse_cloud::connections::ConnectionControl::from_pool(
                db.pool.clone(),
                master,
                accounts,
            );
            zuse_cloud::connections::worker::CommandDriver::deployment_ready(&driver)?;
            let driver = zuse_cloud::connections::worker::CommandDriver::new(&driver)?;
            control
                .ready()
                .await
                .map_err(|_| anyhow::anyhow!("Credential authority schema is not ready"))?;
            if check {
                return Ok(());
            }
            if once {
                let result = control.work_once(&driver).await;
                if matches!(result, Ok(true)) {
                    zuse_cloud::events::hint::send();
                }
                println!("{}", result.map_err(|e| anyhow::anyhow!("{}", e.code))?);
                return Ok(());
            }
            // Sleeps until NOTIFY (a sign-in or proposal became claimable, a
            // refresh started or went stale), the next known deadline, or the
            // idle safety net. NOTIFY only shortens waits; without `LISTEN`
            // the short `ZUSE_CLOUD_AUTH_WORKER_POLL_MS` fallback applies.
            let wakeups = db.wakeups().await;
            let poll = std::time::Duration::from_millis(env_ms(
                "ZUSE_CLOUD_AUTH_WORKER_POLL_MS",
                2_000,
                250..=60_000,
            )?);
            // Before migration 0045 no sign-in ever notifies: keep polling.
            let notified = wakeups.enabled()
                && control
                    .auth_wakeups_installed()
                    .await
                    .map_err(|e| anyhow::anyhow!("{}", e.code))?;
            if wakeups.enabled() && !notified {
                tracing::warn!(
                    code = "auth_wakeups_missing",
                    "Credential worker wakeups are not installed; polling instead"
                );
            }
            let cadence = Cadence {
                idle: if notified {
                    std::time::Duration::from_secs(300)
                } else {
                    poll
                },
                busy: std::time::Duration::from_secs(1).min(poll),
            };
            tracing::info!(
                idle_ms = cadence.idle.as_millis() as u64,
                busy_ms = cadence.busy.as_millis() as u64,
                listen = notified,
                "Credential worker schedule"
            );
            let mut notifications = wakeups.subscribe();
            let interest = [Work::Auth];
            let mut failures = 0u32;
            // Startup runs a full round: anything may have changed meanwhile.
            let mut run = true;
            loop {
                let outcome = async {
                    let mut next = None;
                    if !run {
                        // Idle safety pass: one statement decides whether a
                        // round is needed at all.
                        let now = now_ms();
                        next = control.auth_due(now).await?.next(now);
                        run = next.is_some_and(|at| at <= now);
                    }
                    if run {
                        while control.work_once(&driver).await? {
                            zuse_cloud::events::hint::send();
                        }
                        let now = now_ms();
                        next = control.auth_due(now).await?.next(now);
                    }
                    Ok::<_, zuse_cloud::error::ApiError>(next)
                }
                .await;
                let (wait, timer) = match outcome {
                    Ok(next) => {
                        failures = 0;
                        zuse_cloud::db::wake::next_wait(now_ms(), next, cadence)
                    }
                    Err(error) => {
                        failures = failures.saturating_add(1);
                        let wait = zuse_cloud::db::failure_backoff(
                            failures,
                            poll,
                            std::time::Duration::from_secs(30),
                        );
                        tracing::warn!(
                            code = error.code,
                            retry_ms = wait.as_millis() as u64,
                            "Credential authority operation deferred"
                        );
                        (wait, Reason::Due)
                    }
                };
                let reason = tokio::select! {
                    _ = tokio::signal::ctrl_c() => break,
                    reason = sleep_or_notify(&mut notifications, &interest, wait, timer) => reason,
                };
                // An idle wake first asks whether anything is due.
                run = reason != Reason::Idle;
                tracing::debug!(
                    reason = reason.as_str(),
                    waited_ms = wait.as_millis() as u64,
                    "Credential worker woke"
                );
            }
        }
        Command::PluginMigrate => {
            let url = env::var("ZUSE_CLOUD_DATABASE_URL")
                .context("ZUSE_CLOUD_DATABASE_URL is required")?;
            let settings = zuse_cloud::plugins::PluginConfig::from_env()?
                .context("Plugin encryption key and origins are required")?;
            zuse_cloud::plugins::PluginControl::migrate_url(&url, &settings.key)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.code))?;
            println!("Plugin and Executor schemas ready");
        }
        Command::Inspect => {
            let url = env::var("ZUSE_CLOUD_DATABASE_URL")
                .context("ZUSE_CLOUD_DATABASE_URL is required")?;
            let store = Store::connect_read_only(&url).await?;
            println!(
                "{}",
                serde_pretty(
                    &store
                        .migration_report()
                        .await
                        .map_err(|error| anyhow::anyhow!("{}", error.code))?
                )?
            );
        }
        Command::Allocate {
            workspace_id,
            snapshot,
        } => {
            let (runtime, _db) = writable_runtime(Role::Operator).await?;
            let operation = runtime
                .queue_allocation(&workspace_id, &snapshot, now_ms())
                .await
                .map_err(|error| anyhow::anyhow!("{}", error.code))?;
            println!("{operation}");
        }
        Command::ImageWorker { once } => {
            let Some(settings) = zuse_cloud::images::ImageConfig::from_env()? else {
                println!("Image worker disabled");
                return Ok(());
            };
            let accounts =
                zuse_cloud::access_policy::Accounts::from_env("ZUSE_CLOUD_IMAGE_WRITE_ACCOUNTS")?;
            let url = env::var("ZUSE_CLOUD_DATABASE_URL")
                .context("Image worker requires ZUSE_CLOUD_DATABASE_URL")?;
            let control = zuse_cloud::images::ImageControl::from_pool(
                process_db(&url, Role::ImageWorker).await?.pool,
                accounts,
                settings,
            );
            let mut provider = zuse_cloud::providers::boxd::Boxd::new(
                env::var("BOXD_API_KEY").context("Image worker requires BOXD_API_KEY")?,
                env::var("BOXD_ORG").ok(),
            )
            .map_err(|_| anyhow::anyhow!("Invalid Boxd image configuration"))?;
            if let Ok(base) = env::var("BOXD_BASE_URL") {
                provider = provider
                    .with_base_url(&base)
                    .map_err(|_| anyhow::anyhow!("Invalid Boxd image endpoint"))?;
            }
            let issuer = zuse_cloud::images::github::GithubIssuer::from_env()?;
            loop {
                let result = control
                    .work_once(
                        &provider,
                        issuer
                            .as_ref()
                            .map(|v| v as &dyn zuse_cloud::images::github::GrantIssuer),
                    )
                    .await;
                if once {
                    println!("{}", result.map_err(|e| anyhow::anyhow!("{}", e.code))?);
                    break;
                }
                if let Err(error) = result {
                    tracing::warn!(code = error.code, "Image build deferred");
                }
                tokio::select! {_=tokio::signal::ctrl_c()=>break,_=tokio::time::sleep(std::time::Duration::from_secs(2))=>{}}
            }
        }
        Command::BillingWorker { once } => {
            let settings = zuse_cloud::billing::MeterExportConfig::from_env()?;
            let settlement = zuse_cloud::billing::BoxdSettlementConfig::from_env()?;
            let storage = zuse_cloud::billing::SnapshotSettlementConfig::from_env()?;
            if !settings.enabled()
                && settlement.is_none()
                && storage.is_none()
                && env::var("ZUSE_BROKEBOY_ACCOUNTS")
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
            {
                println!(
                    "{}",
                    serde_json::to_string(&zuse_cloud::billing::ExportReport::default())?
                );
                return Ok(());
            }
            let accounts =
                zuse_cloud::access_policy::Accounts::from_env("ZUSE_CLOUD_BILLING_WRITE_ACCOUNTS")?;
            anyhow::ensure!(
                !accounts.is_empty(),
                "Billing worker requires explicit billing account ownership configuration"
            );
            let providers = if settings.enabled() {
                let providers = zuse_cloud::billing::PaymentProviders::from_env()?
                    .context("Billing export requires configured payment providers")?;
                anyhow::ensure!(
                    !settings.financial
                        || settings
                            .meters
                            .contains_key(providers.default_provider_id()),
                    "Financial export requires the default billing provider's overage meter"
                );
                Some(providers)
            } else {
                None
            };
            let provider = if settlement.is_some() {
                let mut provider = zuse_cloud::providers::boxd::Boxd::new(
                    env::var("BOXD_API_KEY").context("Boxd settlement requires BOXD_API_KEY")?,
                    env::var("BOXD_ORG").ok(),
                )
                .map_err(|_| anyhow::anyhow!("Invalid Boxd settlement configuration"))?;
                if let Ok(base) = env::var("BOXD_BASE_URL") {
                    provider = provider
                        .with_base_url(&base)
                        .map_err(|_| anyhow::anyhow!("Invalid Boxd base URL"))?;
                }
                Some(provider)
            } else {
                None
            };
            let url = env::var("ZUSE_CLOUD_DATABASE_URL")
                .context("ZUSE_CLOUD_DATABASE_URL is required")?;
            let db = process_db(&url, Role::BillingWorker).await?;
            let mut control = zuse_cloud::billing::BillingControl::from_pools(
                db.pool.clone(),
                db.receipts_pool(),
                accounts,
            )
            .with_meter_exports(settings);
            if let Some(providers) = providers {
                control = control.with_payment_providers(providers);
            }
            control = control.connect_native_from_env(&url).await?;
            // Settlement and export run hourly, at the instant a closed day
            // becomes settleable, and at the earliest known due time of
            // outbox retries, engine work and reservation or grant expiries.
            // Between those instants the worker runs no statements. Spending
            // holds are recomputed by every event that changes them (see
            // infra/cloud/DATABASE.md, "Billing schedule and spending caps").
            use zuse_cloud::billing::schedule;
            let mut pass: u64 = 0;
            let mut retry: Option<u32> = None;
            let mut next_settlement = i64::MIN; // settle at startup
            let mut next_reconcile: Option<i64> = None;
            loop {
                let started = now_ms();
                if started >= next_settlement {
                    let mut latest_pending = false;
                    if let Some(config) = storage {
                        match control.settle_snapshot_storage_once(config, now_ms()).await {
                            Ok(report) => println!("{}", serde_json::to_string(&report)?),
                            Err(error) if once => anyhow::bail!("{}", error.code),
                            Err(error) => {
                                tracing::warn!(
                                    code = error.code,
                                    "Snapshot storage settlement failed"
                                )
                            }
                        }
                    }
                    if let (Some(provider), Some(config)) = (&provider, settlement) {
                        match control
                            .settle_boxd_once(provider, config, now_ms(), pass)
                            .await
                        {
                            Ok(report) => {
                                latest_pending = report.latest_pending;
                                println!("{}", serde_json::to_string(&report)?)
                            }
                            Err(error) if once => anyhow::bail!("{}", error.code),
                            Err(error) => {
                                latest_pending = true;
                                tracing::warn!(
                                    code = error.code,
                                    "Provider settlement maintenance failed"
                                )
                            }
                        }
                    }
                    pass = pass.wrapping_add(1);
                    retry = latest_pending.then(|| retry.map_or(0, |n| n.saturating_add(1)));
                    next_settlement = schedule::next_settlement(now_ms(), retry);
                }
                // Right after settlement: debit new compute charges from the
                // shared wallet and recompute the spending hold.
                let mut more = false;
                match control.native_maintenance().await {
                    Ok(report) => {
                        more = report["reports"]
                            .as_array()
                            .is_some_and(|r| r.iter().any(|r| r["more"] == true));
                        println!("{}", report)
                    }
                    Err(error) if once => anyhow::bail!("{}", error.code),
                    Err(error) => {
                        tracing::warn!(code = error.code, "Embedded billing maintenance deferred")
                    }
                }
                next_reconcile = next_reconcile.filter(|at| *at > started);
                match control.export_once(now_ms(), 25).await {
                    Ok(report) => {
                        next_reconcile = zuse_cloud::db::wake::earliest([
                            next_reconcile,
                            schedule::next_reconciliation(now_ms(), report.financial),
                        ]);
                        println!("{}", serde_json::to_string(&report)?)
                    }
                    Err(error) if once => anyhow::bail!("{}", error.code),
                    Err(error) => {
                        tracing::warn!(code = error.code, "Billing export maintenance failed")
                    }
                }
                if once {
                    break;
                }
                let native_due = control
                    .native_next_due(started)
                    .await
                    .unwrap_or_else(|error| {
                        tracing::warn!(code = error.code, "Embedded billing due time unavailable");
                        None
                    });
                let export_due = control.export_next_due().await.unwrap_or_else(|error| {
                    tracing::warn!(code = error.code, "Billing export due time unavailable");
                    None
                });
                let due = zuse_cloud::db::wake::earliest([
                    Some(next_settlement),
                    native_due,
                    export_due,
                    next_reconcile,
                    more.then_some(started),
                ]);
                let wait = schedule::wait_until(now_ms(), due);
                tracing::debug!(wait_ms = wait.as_millis() as u64, "Billing worker sleeping");
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => break,
                    _ = tokio::time::sleep(wait) => {}
                }
            }
            control
                .close_native_billing()
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.code))?;
        }
        Command::ProjectPreparationWorker {
            once,
            check,
            poll_interval_ms,
        } => {
            let (runtime, _db) = writable_runtime(Role::ProjectPreparationWorker).await?;
            runtime
                .configure_public_project_preparation()
                .await
                .map_err(|error| anyhow::anyhow!("{}", error.code))?;
            if check {
                println!(
                    "{}",
                    serde_json::json!({"status":"configuration-valid", "effects":false})
                );
            } else {
                loop {
                    let worked = runtime
                        .work_project_preparation_once(now_ms())
                        .await
                        .map_err(|error| anyhow::anyhow!("{}", error.code))?;
                    if worked {
                        zuse_cloud::events::hint::send();
                    }
                    if once {
                        println!("{}", serde_json::json!({"worked":worked}));
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(poll_interval_ms)).await;
                }
            }
        }
        Command::Worker {
            once,
            check,
            poll_interval_ms,
        } => {
            let (runtime, db) = writable_runtime(Role::Worker).await?;
            let registry = Arc::new(zuse_cloud::providers::registry::Registry::from_environment());
            if let Err(error) = runtime.configure_public_project_preparation().await {
                tracing::warn!(
                    code = error.code,
                    "Project preparation configuration unavailable"
                );
            }
            match runtime.configure_user_images_from_env().await {
                Ok(true) => tracing::info!("User image builder enabled"),
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(code = error.code, "User image builder unavailable")
                }
            }
            match runtime.configure_user_image_repositories() {
                Ok(true) => tracing::info!("User image repositories enabled"),
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(code = error.code, "User image repositories unavailable")
                }
            }

            for (provider, code) in registry.configuration_errors() {
                tracing::warn!(
                    provider = provider.as_str(),
                    code,
                    "Provider configuration unavailable"
                );
            }
            anyhow::ensure!(
                !registry.configured_ids().is_empty(),
                "Worker requires at least one configured managed provider"
            );
            let preflight = zuse_cloud::providers::registry::preflight_errors();
            for error in &preflight {
                tracing::error!(error, "Provider configuration preflight failed");
            }
            if check {
                anyhow::ensure!(
                    preflight.is_empty(),
                    "Worker configuration invalid: {}",
                    preflight.join("; ")
                );
                println!(
                    "Worker configuration and allocation/lifecycle schema verified; no jobs claimed"
                );
                return Ok(());
            }
            // Idle loops sleep until NOTIFY, the next known due time, or the idle
            // fallback; NOTIFY only shortens waits, it is never required.
            let wakeups = if once {
                zuse_cloud::db::wake::Wakeups::disabled()
            } else {
                db.wakeups().await
            };
            let cadence = Cadence::from_env(wakeups.enabled(), poll_interval_ms)?;
            tracing::info!(
                idle_ms = cadence.idle.as_millis() as u64,
                busy_ms = cadence.busy.as_millis() as u64,
                listen = wakeups.enabled(),
                "Worker schedule"
            );
            let recovery_kick = Arc::new(tokio::sync::Notify::new());
            let recovery_task = if once {
                None
            } else {
                let recovery_runtime = runtime.clone();
                let recovery_registry = registry.clone();
                let kick = recovery_kick.clone();
                Some(tokio::spawn(async move {
                    let mut failures = 0u32;
                    loop {
                        let mut failed = false;
                        if let Err(error) = recovery_runtime
                            .recovery_registry_once(&recovery_registry, now_ms())
                            .await
                        {
                            failed = true;
                            tracing::warn!(code = error.code, "Recovery stage deferred");
                        }
                        if let Err(error) = recovery_runtime
                            .restore_registry_once(&recovery_registry, now_ms())
                            .await
                        {
                            failed = true;
                            tracing::warn!(code = error.code, "Restore stage deferred");
                        }
                        // Kicked by the worker loop when recovery/restore jobs are
                        // due (including expired leases of crashed attempts) or
                        // an archived computer's deletion is due
                        // (`machine_delete_due_at`); the idle timer is only a
                        // safety net.
                        let wait = if failed {
                            failures = failures.saturating_add(1);
                            zuse_cloud::db::failure_backoff(
                                failures,
                                cadence.busy,
                                cadence.failure_cap(),
                            )
                        } else {
                            failures = 0;
                            cadence.idle
                        };
                        tokio::select! {
                            _ = kick.notified() => {},
                            _ = tokio::time::sleep(wait) => {},
                        }
                    }
                }))
            };
            // Compare what providers say is running with what the database
            // expects (and check daily compute cost). Kicked by the worker loop
            // when the schedule row is due; it never polls the provider.
            let reconciliation = zuse_cloud::runtime::provider_reconciliation::Config::from_env()?;
            tracing::info!(
                mode = reconciliation.mode.as_str(),
                interval_ms = reconciliation.interval_ms,
                "Provider reconciliation"
            );
            let reconciliation_kick = Arc::new(tokio::sync::Notify::new());
            let reconciliation_task = if once
                || reconciliation.mode == zuse_cloud::runtime::provider_reconciliation::Mode::Off
            {
                None
            } else {
                runtime.enable_provider_reconciliation(true);
                let runtime = runtime.clone();
                let registry = registry.clone();
                let kick = reconciliation_kick.clone();
                Some(tokio::spawn(async move {
                    loop {
                        if let Err(error) = runtime
                            .provider_reconciliation_once(&registry, &reconciliation, now_ms())
                            .await
                        {
                            tracing::warn!(code = error.code, "Provider reconciliation deferred");
                        }
                        if let Err(error) = runtime
                            .cost_anomalies_once(&registry, &reconciliation, now_ms())
                            .await
                        {
                            tracing::warn!(code = error.code, "Compute cost check deferred");
                        }
                        // The worker's due-time query kicks this when the next
                        // pass is due; the idle timer is only a safety net.
                        tokio::select! {
                            _ = kick.notified() => {},
                            _ = tokio::time::sleep(cadence.idle) => {},
                        }
                    }
                }))
            };
            // Provider running limits (E2B: one hour) must not pause a busy computer.
            let keep_running_task = if once {
                None
            } else {
                let runtime = runtime.clone();
                let registry = registry.clone();
                Some(tokio::spawn(async move {
                    loop {
                        if let Err(error) = runtime
                            .keep_running_registry_once(&registry, now_ms())
                            .await
                        {
                            tracing::warn!(code = error.code, "Provider running limits deferred");
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(
                            zuse_cloud::runtime::keep_running::INTERVAL_SECS,
                        ))
                        .await;
                    }
                }))
            };
            // Remote object cleanup is independent of the latency-sensitive
            // lifecycle hint loop. Unknown PUT receipts survive cancellation.
            let object_cleanup_task = if once {
                None
            } else {
                let runtime = runtime.clone();
                Some(tokio::spawn(async move {
                    loop {
                        // Hourly maintenance; the same throughput bound as the
                        // former four reconciles a minute.
                        for _ in 0..240 {
                            match runtime.reconcile_object_write_once(now_ms()).await {
                                Ok(true) => {}
                                Ok(false) => break,
                                Err(error) => {
                                    tracing::warn!(
                                        code = error.code,
                                        "Uncertain object write cleanup deferred"
                                    );
                                    break;
                                }
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(MAINTENANCE_MS as u64))
                            .await;
                    }
                }))
            };
            let mut lifecycle_signal =
                match zuse_cloud::runtime::lifecycle::wakeup::Listener::from_env() {
                    Ok(listener) => listener,
                    Err(_) => {
                        tracing::warn!(
                            code = "lifecycle_wake_hint_unavailable",
                            "Using durable lifecycle reconciliation"
                        );
                        zuse_cloud::runtime::lifecycle::wakeup::Listener::disabled()
                    }
                };
            let mut grant_cleanup_at = 0;
            let mut runtime_update_expiry_at = 0;
            let mut stale_lifecycle_at = 0;
            let allocation_cancel = tokio_util::sync::CancellationToken::new();
            let allocation_kick = Arc::new(tokio::sync::Notify::new());
            let allocation_task = if once {
                None
            } else {
                let concurrency = std::env::var("ZUSE_CLOUD_ALLOCATION_CONCURRENCY")
                    .unwrap_or_else(|_| "4".into())
                    .parse::<usize>()?;
                anyhow::ensure!(
                    (1..=16).contains(&concurrency),
                    "invalid_allocation_concurrency"
                );
                Some(tokio::spawn(zuse_cloud::runtime::allocation::workers::run(
                    runtime.clone(),
                    registry.clone(),
                    concurrency,
                    allocation_kick.clone(),
                    cadence.idle,
                    allocation_cancel.clone(),
                )))
            };
            let mut notifications = wakeups.subscribe();
            let interest = [
                Work::Jobs,
                Work::TaskDrain,
                Work::Recreation,
                Work::MachineImages,
                Work::ProjectCache,
                Work::IdleSleep,
                Work::ArchiveRetention,
            ];
            let mut asset_cleanup_at = 0;
            let mut idle_sleep_at = 0;
            // Condition-based retirement sweeps have no due time; they run as
            // hourly maintenance (and after a missed-notification wake).
            let mut idle_stages_at = 0;
            let mut due_failures = 0u32;
            let mut lifecycle_priority = false;
            loop {
                if allocation_task
                    .as_ref()
                    .is_some_and(|task| task.is_finished())
                {
                    anyhow::bail!("allocation_dispatcher_stopped");
                }
                let hinted_cycle = lifecycle_priority;
                lifecycle_priority = false;
                if hinted_cycle {
                    // Coalesce before reading the durable queue. Never discard
                    // a new guest ACK after the final empty-queue observation.
                    if lifecycle_signal.drain_ready().is_err() {
                        tracing::warn!(
                            code = "lifecycle_wake_hint_disconnected",
                            "Using durable lifecycle reconciliation"
                        );
                        lifecycle_signal =
                            zuse_cloud::runtime::lifecycle::wakeup::Listener::disabled();
                    }
                    let burst = zuse_cloud::runtime::lifecycle::scheduling::ready_burst(|| {
                        runtime.lifecycle_registry_once(&registry, now_ms())
                    })
                    .await;
                    if burst.claimed > 0 {
                        zuse_cloud::events::hint::send();
                    }
                    tracing::debug!(
                        claimed = burst.claimed,
                        limit_reached = burst.limit_reached,
                        elapsed_ms = burst.elapsed.as_millis() as u64,
                        "Committed lifecycle hint reconciled"
                    );
                    lifecycle_priority = burst.limit_reached;
                    if let Some(error) = burst.error {
                        tracing::warn!(code = error.code, "Retained lifecycle stage deferred");
                    }
                }
                // One statement says which stages have due work. If it fails,
                // every stage runs as it did before this schedule existed.
                let now = now_ms();
                let due = if once {
                    None
                } else {
                    match runtime.worker_due().await {
                        Ok(due) => {
                            due_failures = 0;
                            Some(due)
                        }
                        Err(error) => {
                            due_failures = due_failures.saturating_add(1);
                            tracing::warn!(code = error.code, "Worker due-time query deferred");
                            None
                        }
                    }
                };
                let run = |at: fn(&WorkerDue) -> Option<i64>| should_run(due.as_ref(), at, now);
                let idle_round = now >= idle_stages_at;
                if idle_round {
                    idle_stages_at = now + MAINTENANCE_MS;
                }
                let mut worked = false;
                if now_ms() >= grant_cleanup_at {
                    // Bounded batches until caught up (former cadence: one
                    // batch a minute).
                    for _ in 0..60 {
                        match runtime.cleanup_grant_receipts(now_ms()).await {
                            Ok(true) => {}
                            Ok(false) => break,
                            Err(error) => {
                                tracing::warn!(
                                    code = error.code,
                                    "Credential receipt cleanup deferred"
                                );
                                break;
                            }
                        }
                    }
                    grant_cleanup_at = now_ms() + MAINTENANCE_MS;
                }
                if now_ms() >= runtime_update_expiry_at {
                    // Deadline for in-flight runtime updates: never
                    // "Downloading" forever on an online computer.
                    match runtime.expire_runtime_updates(now_ms()).await {
                        Ok(0) => {}
                        Ok(expired) => tracing::warn!(expired, "Runtime updates timed out"),
                        Err(error) => {
                            tracing::warn!(code = error.code, "Runtime update expiry deferred")
                        }
                    }
                    runtime_update_expiry_at =
                        now_ms() + zuse_cloud::runtime::runtime_updates::EXPIRY_SWEEP_MS;
                }
                if now_ms() >= stale_lifecycle_at {
                    // Deadline for lifecycle operations a paused computer was
                    // left with: they must never block Archive or Resume.
                    match runtime.settle_stale_lifecycle(now_ms()).await {
                        Ok(0) => {}
                        Ok(settled) => {
                            lifecycle_priority = true;
                            tracing::warn!(settled, "Unfinished lifecycle operations settled");
                        }
                        Err(error) => {
                            tracing::warn!(code = error.code, "Lifecycle settlement sweep deferred")
                        }
                    }
                    stale_lifecycle_at = now_ms() + zuse_cloud::runtime::lifecycle::stale::SWEEP_MS;
                }
                if now_ms() >= idle_sleep_at {
                    match runtime.idle_sleep_once(now_ms()).await {
                        Ok(true) => {
                            idle_sleep_at = 0;
                            lifecycle_priority = true;
                            zuse_cloud::events::hint::send();
                        }
                        // Sleep until the earliest computer's idle deadline. A
                        // shared-credit hold wakes the loop with NOTIFY; a newly
                        // idle-eligible computer's deadline is at least the idle
                        // timeout away, which the bounded sweep never misses.
                        Ok(false) => {
                            let now = now_ms();
                            idle_sleep_at = match runtime.idle_sleep_due(now).await {
                                Ok(due) => zuse_cloud::runtime::lifecycle::idle::next_check(
                                    now,
                                    due,
                                    runtime.idle_sweep_ms(),
                                ),
                                Err(error) => {
                                    tracing::warn!(
                                        code = error.code,
                                        "Idle sleep deadline deferred"
                                    );
                                    now + cadence.failure_cap().as_millis() as i64
                                }
                            };
                        }
                        Err(error) => {
                            idle_sleep_at = now_ms() + cadence.failure_cap().as_millis() as i64;
                            tracing::warn!(code = error.code, "Idle sleep admission deferred");
                        }
                    }
                }
                let allocation = if once {
                    runtime.allocate_registry_once(&registry, now_ms()).await
                } else {
                    if run(|d| d.allocation) {
                        allocation_kick.notify_one();
                    }
                    Ok(false)
                };
                if matches!(allocation, Ok(true)) {
                    zuse_cloud::events::hint::send();
                }
                if let Err(error) = allocation {
                    tracing::warn!(code = error.code, "Allocation stage deferred");
                    if once {
                        anyhow::bail!("{}", error.code);
                    }
                }
                // Archived computers whose retention ended are admitted by the
                // recovery task; the idle timer there is only a safety net.
                if run(|d| d.recovery) || run(|d| d.restore) || run(|d| d.archive_retention) {
                    recovery_kick.notify_one();
                }
                if reconciliation_task.is_some() && run(|d| d.provider_reconciliation) {
                    reconciliation_kick.notify_one();
                }
                if run(|d| d.repair) {
                    let repair = runtime
                        .runtime_repair_registry_once(&registry, now_ms())
                        .await;
                    if matches!(repair, Ok(true)) {
                        worked = true;
                        zuse_cloud::events::hint::send();
                    }
                    if let Err(error) = repair {
                        tracing::warn!(code = error.code, "Runtime repair stage deferred");
                        if once {
                            anyhow::bail!("{}", error.code);
                        }
                    }
                }
                // One durable due task drain per ordinary round; never inside
                // the priority lifecycle burst. Missing optional schema is cached.
                // Provider preparation cannot hold up lifecycle/control dispatch.
                if idle_round || run(|d| d.project_cache) {
                    runtime.dispatch_project_preparation(now_ms());
                }
                if idle_round || run(|d| d.user_images) {
                    runtime.dispatch_user_images(now_ms());
                }
                if run(|d| d.task_drain) {
                    match runtime.work_task_drain_once(now_ms()).await {
                        Ok(true) => {
                            worked = true;
                            zuse_cloud::events::hint::send()
                        }
                        Err(error) => tracing::warn!(code = error.code, "Task drain deferred"),
                        _ => {}
                    }
                }
                if run(|d| d.recreation) {
                    match runtime.recreation_once(now_ms()).await {
                        Ok(true) => {
                            worked = true;
                            zuse_cloud::events::hint::send()
                        }
                        Err(error) => {
                            tracing::warn!(code = error.code, "Computer recreation stage deferred")
                        }
                        _ => {}
                    }
                }
                if !hinted_cycle && run(|d| d.lifecycle) {
                    let lifecycle = runtime.lifecycle_registry_once(&registry, now_ms()).await;
                    if matches!(lifecycle, Ok(true)) {
                        worked = true;
                        zuse_cloud::events::hint::send();
                    }
                    if let Err(error) = lifecycle {
                        tracing::warn!(code = error.code, "Retained lifecycle stage deferred");
                        if once {
                            anyhow::bail!("{}", error.code);
                        }
                    }
                }
                if run(|d| d.deletion) {
                    let deletion = runtime.delete_registry_once(&registry, now_ms()).await;
                    if matches!(deletion, Ok(true)) {
                        worked = true;
                        zuse_cloud::events::hint::send();
                    }
                    if let Err(error) = deletion {
                        tracing::warn!(code = error.code, "Deletion stage deferred");
                        if once {
                            anyhow::bail!("{}", error.code);
                        }
                    }
                }
                // Expired uploads are already unauthorized. Their physical
                // purge is bounded maintenance, not part of every task wake.
                if now_ms() >= asset_cleanup_at {
                    // Bounded batches until caught up (former cadence: one
                    // pass a minute).
                    for _ in 0..60 {
                        match runtime.sweep_launch_assets_once(now_ms()).await {
                            Ok(true) => {}
                            Ok(false) => break,
                            Err(error) => {
                                tracing::warn!(
                                    code = error.code,
                                    "Abandoned launch upload cleanup deferred"
                                );
                                break;
                            }
                        }
                    }
                    // Failed remote deletes stay `releasing`; never retry them in a burst.
                    for _ in 0..10 {
                        match runtime.sweep_asset_releases_once(now_ms()).await {
                            Ok(true) => {}
                            Ok(false) => break,
                            Err(error) => {
                                tracing::warn!(
                                    code = error.code,
                                    "Attachment release sweep deferred"
                                );
                                break;
                            }
                        }
                    }
                    asset_cleanup_at = now_ms() + MAINTENANCE_MS;
                }
                if once {
                    runtime
                        .reconcile_object_write_once(now_ms())
                        .await
                        .map_err(|error| anyhow::anyhow!("{}", error.code))?;
                    runtime
                        .recovery_registry_once(&registry, now_ms())
                        .await
                        .map_err(|e| anyhow::anyhow!("{}", e.code))?;
                    runtime
                        .restore_registry_once(&registry, now_ms())
                        .await
                        .map_err(|e| anyhow::anyhow!("{}", e.code))?;
                    break;
                }
                // Always give allocation/repair/deletion a round before more
                // ready lifecycle work. Four real claims never become a spin.
                if lifecycle_priority {
                    continue;
                }
                let (wait, timer) = if due.is_none() {
                    // The due query failed (for example an exhausted pool):
                    // back off instead of retrying in a tight loop.
                    let wait = zuse_cloud::db::failure_backoff(
                        due_failures,
                        cadence.busy,
                        cadence.failure_cap(),
                    );
                    (wait, Reason::Due)
                } else if worked {
                    // More may be ready; look again after the busy floor.
                    (cadence.busy, Reason::Due)
                } else {
                    let next = zuse_cloud::db::wake::earliest([
                        due.as_ref().and_then(WorkerDue::earliest),
                        Some(idle_stages_at),
                        Some(grant_cleanup_at),
                        Some(runtime_update_expiry_at),
                        Some(stale_lifecycle_at),
                        Some(idle_sleep_at),
                        Some(asset_cleanup_at),
                    ]);
                    zuse_cloud::db::wake::next_wait(now_ms(), next, cadence)
                };
                let reason = tokio::select! {
                    _ = tokio::signal::ctrl_c() => break,
                    result = lifecycle_signal.receive() => {
                        if result.is_ok() {lifecycle_priority=true;}
                        if result.is_err() {
                            tracing::warn!(code="lifecycle_wake_hint_disconnected","Using durable lifecycle reconciliation");
                            lifecycle_signal=zuse_cloud::runtime::lifecycle::wakeup::Listener::disabled();
                        }
                        Reason::Hint
                    },
                    reason = sleep_or_notify(&mut notifications, &interest, wait, timer) => reason,
                };
                if reason == Reason::Notify(Work::All) {
                    // Notifications may have been missed: rerun the idle stages too.
                    idle_stages_at = 0;
                    idle_sleep_at = 0;
                }
                if reason == Reason::Notify(Work::IdleSleep) {
                    idle_sleep_at = 0;
                }
                tracing::debug!(
                    reason = reason.as_str(),
                    waited_ms = wait.as_millis() as u64,
                    "Worker woke"
                );
            }
            allocation_cancel.cancel();
            if let Some(task) = allocation_task {
                task.await??;
            }
            if let Some(task) = object_cleanup_task {
                task.abort();
            }
            if let Some(task) = keep_running_task {
                task.abort();
            }
            if let Some(task) = reconciliation_task {
                task.abort();
            }
            if let Some(task) = recovery_task {
                task.abort();
            }
        }
        Command::Serve => {
            let config = Config::from_env()?;
            // Every subsystem below shares these pools; none opens its own.
            let db = process_db(&config.database_url, Role::Serve).await?;
            let runtime = match RuntimeConfig::from_env()? {
                Some(settings) => Some(RuntimeControl::open(db.pool.clone(), settings).await?),
                None => None,
            };
            let runtime_enabled = runtime.is_some();
            let wakeups = db.wakeups().await;
            if let Some(runtime) = &runtime {
                runtime.attach_wakeups(wakeups.clone());
            }
            let mut store = Store::from_pool(db.read_only_pool())
                .configure_transcripts()?
                .with_staging_base_pin(runtime.as_ref().and_then(|r| r.staging_base_pin()));
            let auth_accounts =
                zuse_cloud::access_policy::Accounts::from_env("ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS")?;
            if let Some(authority) = runtime
                .as_ref()
                .and_then(|runtime| runtime.credential_authority())
            {
                store = store.with_shared_connections(authority);
            } else if !auth_accounts.is_empty() {
                let key = env::var("CLOUD_DATA_ENCRYPTION_KEY")
                    .context("Credential authority requires CLOUD_DATA_ENCRYPTION_KEY")?;
                let master = zuse_cloud::crypto::MasterKey::parse(&key)
                    .map_err(|_| anyhow::anyhow!("Invalid credential authority encryption key"))?;
                store =
                    store.with_connections(zuse_cloud::connections::ConnectionControl::from_pool(
                        db.pool.clone(),
                        master,
                        auth_accounts,
                    ));
            }
            store.devices = zuse_cloud::devices::Authority::from_env(&db).await?;
            store =
                store.with_project_templates(zuse_cloud::projects::template_versions_from_env()?);
            if let Ok(origin) = env::var("ZUSE_CLOUD_LEGACY_GITHUB_URL") {
                store =
                    store.with_github_authority(zuse_cloud::github::LegacyGithub::new(&origin)?);
            }
            store = store
                .with_native_github_from_env(runtime.as_ref().map(|runtime| runtime.shared_pool()))?
                .with_github_organization_authority(&config)?;
            let project_accounts =
                zuse_cloud::access_policy::Accounts::from_env("ZUSE_CLOUD_PROJECT_WRITE_ACCOUNTS")?;
            if !project_accounts.is_empty() {
                store = store
                    .with_computer_control(zuse_cloud::computers::ComputerControl::from_pool(
                        db.pool.clone(),
                        project_accounts.clone(),
                    ))
                    .with_project_control(zuse_cloud::projects::ProjectControl::from_pool(
                        db.pool.clone(),
                        project_accounts,
                    ));
            }
            if let Some(settings) = zuse_cloud::images::ImageConfig::from_env()? {
                let accounts = zuse_cloud::access_policy::Accounts::from_env(
                    "ZUSE_CLOUD_IMAGE_WRITE_ACCOUNTS",
                )?;
                store = store.with_image_control(zuse_cloud::images::ImageControl::from_pool(
                    db.pool.clone(),
                    accounts,
                    settings,
                ));
            }
            let billing_accounts =
                zuse_cloud::access_policy::Accounts::from_env("ZUSE_CLOUD_BILLING_WRITE_ACCOUNTS")?;
            if !billing_accounts.is_empty() {
                // Checkout takes a receipt connection while its control
                // transaction is open: keep receipts on their own pool.
                let mut billing = zuse_cloud::billing::BillingControl::from_pools(
                    db.pool.clone(),
                    db.receipts_pool(),
                    billing_accounts,
                );
                if let Some(providers) = zuse_cloud::billing::PaymentProviders::from_env()? {
                    billing = billing.with_payment_providers(providers);
                }
                // Payment notifications for embedded billing accounts are
                // projected in this process right after they commit.
                billing = billing
                    .connect_native_from_env(&config.database_url)
                    .await?
                    .with_native_follow_ups();
                store = store.with_billing_control(billing);
            }
            let identity = WorkosIdentity::new(&config)?;
            if let Some(runtime) = &runtime {
                runtime
                    .configure_scope_authority(
                        identity.clone(),
                        store.native_github(),
                        if config.organizations_enabled {
                            config.organization_ids.clone()
                        } else {
                            vec![]
                        },
                    )
                    .map_err(|error| anyhow::anyhow!("{}", error.code))?;
            }
            if let Some(joining) = store.github_organization_authority() {
                tokio::spawn(async move {
                    let mut seeded = false;
                    loop {
                        if !seeded {
                            match joining.seed(now_ms()).await {
                                Ok(()) => seeded = true,
                                Err(error) => tracing::warn!(
                                    code = error.code,
                                    "GitHub organization startup reconciliation deferred"
                                ),
                            }
                        }
                        for _ in 0..16 {
                            match joining.run_once(now_ms()).await {
                                Ok(true) => {}
                                Ok(false) => break,
                                Err(error) => {
                                    tracing::warn!(
                                        code = error.code,
                                        "GitHub organization reconciliation deferred"
                                    );
                                    break;
                                }
                            }
                        }
                        joining.wait_for_dispatch(now_ms()).await;
                    }
                });
            }
            store = store
                .with_account_events_from_env(runtime.as_ref(), wakeups.clone())
                .await?;
            #[cfg(feature = "hosted-browser")]
            let hosted_browser = zuse_cloud::hosted_browser::from_env(&config)?;
            if let Some(settings) = zuse_cloud::plugins::PluginConfig::from_env()? {
                let plugins =
                    zuse_cloud::plugins::PluginControl::from_pool(db.pool.clone(), settings)?;
                plugins.attach_wakeups(wakeups.clone()).await;
                let maintenance = plugins.clone();
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                    loop {
                        interval.tick().await;
                        if let Err(error) = maintenance.sweep().await {
                            tracing::warn!(code = error.code, "Plugin maintenance failed");
                        }
                    }
                });
                store = store.with_plugin_control(plugins);
            }
            let state = AppState {
                store,
                identity,
                organizations_enabled: config.organizations_enabled,
                organization_ids: Arc::new(config.organization_ids),
                runtime,
                gateway: Arc::new(
                    zuse_cloud::gateway::Hub::with_capacity(
                        config.gateway_max_sockets,
                        config.gateway_queued_bytes,
                    )
                    .map_err(|error| anyhow::anyhow!("{}", error.code))?,
                ),
            };
            let billing_shutdown = state.store.clone();
            let origins = config
                .allowed_origins
                .iter()
                .map(|origin| origin.parse())
                .collect::<Result<Vec<_>, _>>()?;
            let listener = tokio::net::TcpListener::bind(config.listen).await?;
            tracing::info!(address = %listener.local_addr()?,runtime_control=runtime_enabled, "Zuse cloud API listening");
            let app = router(state.clone(), origins);
            let state_for_pull_requests = state.clone();
            #[cfg(feature = "hosted-browser")]
            let app = if let Some(extension) = hosted_browser.as_ref() {
                app.merge(extension.router())
            } else {
                app
            };
            let mut app = app;
            let slack_worker = if let Some(slack_config) = zuse_cloud::slack::Config::from_env()? {
                // The Slack worker queries the pool while its advisory-lock
                // transaction holds one shared connection.
                anyhow::ensure!(
                    db.budget.shared_connections() >= 2,
                    "Slack requires at least two shared database connections; raise ZUSE_CLOUD_DB_MAX_CONNECTIONS"
                );
                let slack =
                    zuse_cloud::slack::Slack::connect(state, slack_config, db.pool.clone()).await?;
                let worker = slack.start_worker();
                app = app.merge(slack.router());
                Some(worker)
            } else {
                None
            };
            // Merge-conflict checks and deliveries to chats on cloud computers.
            let pull_request_worker =
                zuse_cloud::github::native::PullRequestWorker::new(&state_for_pull_requests)
                    .map(|worker| worker.start());
            axum_serve(listener, app).await?;
            if let Some(worker) = pull_request_worker {
                worker.abort();
            }
            if let Some(worker) = slack_worker {
                worker.abort();
            }
            billing_shutdown
                .close_native_billing()
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.code))?;
        }
    }
    Ok(())
}

fn env_ms(name: &str, default: u64, range: std::ops::RangeInclusive<u64>) -> Result<u64> {
    match env::var(name) {
        Err(_) => Ok(default),
        Ok(raw) => raw
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|ms| range.contains(ms))
            .with_context(|| format!("Invalid {name}")),
    }
}

/// The single capped pool set of this process (see `zuse_cloud::db`).
async fn process_db(url: &str, role: Role) -> Result<ProcessDb> {
    ProcessDb::connect(url, Budget::from_env(role)?).await
}

fn serde_pretty(value: &serde_json::Value) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)?)
}

async fn serve_gateway() -> Result<()> {
    let config = Config::from_env()?;
    let settings = RuntimeConfig::from_env()?
        .context("Gateway requires ZUSE_CLOUD_RUNTIME_WRITES=true for lease fencing")?;
    let db = process_db(&config.database_url, Role::Gateway).await?;
    let runtime = RuntimeControl::open(db.pool.clone(), settings).await?;
    let state = AppState {
        store: Store::from_pool(db.read_only_pool()),
        identity: WorkosIdentity::new(&config)?,
        organizations_enabled: config.organizations_enabled,
        organization_ids: Arc::new(config.organization_ids),
        runtime: Some(runtime),
        gateway: Arc::new(
            zuse_cloud::gateway::Hub::with_capacity(
                config.gateway_max_sockets,
                config.gateway_queued_bytes,
            )
            .map_err(|error| anyhow::anyhow!("{}", error.code))?,
        ),
    };
    let origins = config
        .allowed_origins
        .iter()
        .map(|v| v.parse())
        .collect::<Result<Vec<_>, _>>()?;
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    tracing::info!(address = %listener.local_addr()?, "Zuse WebSocket gateway listening");
    axum_serve(listener, zuse_cloud::routes::gateway_router(state, origins)).await
}
async fn axum_serve(listener: tokio::net::TcpListener, app: axum::Router) -> Result<()> {
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            #[cfg(unix)]
            {
                let mut terminate =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("install SIGTERM handler");
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
        })
        .await?;
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
async fn writable_runtime(role: Role) -> Result<(Arc<RuntimeControl>, ProcessDb)> {
    let settings = RuntimeConfig::from_env()?
        .context("This command requires ZUSE_CLOUD_RUNTIME_WRITES=true")?;
    let url = env::var("ZUSE_CLOUD_DATABASE_URL").context("ZUSE_CLOUD_DATABASE_URL is required")?;
    let db = process_db(&url, role).await?;
    let runtime = RuntimeControl::open(db.pool.clone(), settings).await?;
    runtime.allocation_ready().await.map_err(|error| {
        anyhow::anyhow!(
            "{}; apply migration 0004_allocations.sql before running the allocation worker",
            error.code
        )
    })?;
    runtime
        .lifecycle_ready()
        .await
        .map_err(|error| anyhow::anyhow!("{}; apply migration 0005_lifecycle.sql", error.code))?;
    Ok((runtime, db))
}
