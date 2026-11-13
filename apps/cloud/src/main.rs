use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::{env, sync::Arc};
use zuse_cloud::{
    auth::WorkosIdentity,
    config::Config,
    routes::{AppState, router},
    runtime::{RuntimeConfig, RuntimeControl},
    store::Store,
};

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
        /// Bound idle database polling while preserving durable job reconciliation.
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
            let accounts = env::var("ZUSE_CLOUD_BILLING_WRITE_ACCOUNTS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .collect();
            let mut control = zuse_cloud::billing::BillingControl::connect(&url, accounts)
                .await?
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
            let control = Arc::new(
                zuse_cloud::connections::ConnectionControl::connect(&url, master, accounts).await?,
            );
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
            let control =
                zuse_cloud::connections::ConnectionControl::connect(&url, master, accounts).await?;
            let driver = zuse_cloud::connections::worker::CommandDriver::new(&driver)?;
            let receipt = control
                .renew_codex_connection(&owner, &connection, &driver)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.code))?;
            println!("{}", serde_json::to_string(&receipt)?);
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
            let accounts = env::var("ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
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
            let control =
                zuse_cloud::connections::ConnectionControl::connect(&url, master, accounts).await?;
            zuse_cloud::connections::worker::CommandDriver::deployment_ready(&driver)?;
            let driver = zuse_cloud::connections::worker::CommandDriver::new(&driver)?;
            control
                .ready()
                .await
                .map_err(|_| anyhow::anyhow!("Credential authority schema is not ready"))?;
            if check {
                return Ok(());
            }
            loop {
                let result = control.work_once(&driver).await;
                if matches!(result, Ok(true)) {
                    zuse_cloud::events::hint::send();
                }
                if once {
                    println!("{}", result.map_err(|e| anyhow::anyhow!("{}", e.code))?);
                    break;
                }
                let idle = match result {
                    Ok(worked) => !worked,
                    Err(error) => {
                        tracing::warn!(
                            code = error.code,
                            "Credential authority operation deferred"
                        );
                        true
                    }
                };
                if idle {
                    tokio::select! {_=tokio::signal::ctrl_c()=>break,_=tokio::time::sleep(std::time::Duration::from_secs(1))=>{}}
                }
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
            let runtime = writable_runtime().await?;
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
            let accounts = env::var("ZUSE_CLOUD_IMAGE_WRITE_ACCOUNTS")?
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .collect();
            let url = env::var("ZUSE_CLOUD_DATABASE_URL")
                .context("Image worker requires ZUSE_CLOUD_DATABASE_URL")?;
            let control =
                zuse_cloud::images::ImageControl::connect(&url, accounts, settings).await?;
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
            let accounts = env::var("ZUSE_CLOUD_BILLING_WRITE_ACCOUNTS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
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
            let mut control = zuse_cloud::billing::BillingControl::connect(&url, accounts)
                .await?
                .with_meter_exports(settings);
            if let Some(providers) = providers {
                control = control.with_payment_providers(providers);
            }
            control = control.connect_native_from_env(&url).await?;
            loop {
                match control.native_maintenance().await {
                    Ok(report) => println!("{}", report),
                    Err(error) if once => anyhow::bail!("{}", error.code),
                    Err(error) => {
                        tracing::warn!(code = error.code, "Embedded billing maintenance deferred")
                    }
                }
                if let Some(config) = storage {
                    match control.settle_snapshot_storage_once(config, now_ms()).await {
                        Ok(report) => println!("{}", serde_json::to_string(&report)?),
                        Err(error) if once => anyhow::bail!("{}", error.code),
                        Err(error) => {
                            tracing::warn!(code = error.code, "Snapshot storage settlement failed")
                        }
                    }
                }

                if let (Some(provider), Some(config)) = (&provider, settlement) {
                    match control.settle_boxd_once(provider, config, now_ms()).await {
                        Ok(report) => println!("{}", serde_json::to_string(&report)?),
                        Err(error) if once => anyhow::bail!("{}", error.code),
                        Err(error) => tracing::warn!(
                            code = error.code,
                            "Provider settlement maintenance failed"
                        ),
                    }
                }
                match control.export_once(now_ms(), 25).await {
                    Ok(report) => println!("{}", serde_json::to_string(&report)?),
                    Err(error) if once => anyhow::bail!("{}", error.code),
                    Err(error) => {
                        tracing::warn!(code = error.code, "Billing export maintenance failed")
                    }
                }
                if once {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
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
            let runtime = writable_runtime().await?;
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
            let runtime = writable_runtime().await?;
            let registry = Arc::new(zuse_cloud::providers::registry::Registry::from_environment());
            if let Err(error) = runtime.configure_public_project_preparation().await {
                tracing::warn!(
                    code = error.code,
                    "Project preparation configuration unavailable"
                );
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
            if check {
                println!(
                    "Worker configuration and allocation/lifecycle schema verified; no jobs claimed"
                );
                return Ok(());
            }
            let recovery_task = if once {
                None
            } else {
                let recovery_runtime = runtime.clone();
                let recovery_registry = registry.clone();
                Some(tokio::spawn(async move {
                    loop {
                        if let Err(error) = recovery_runtime
                            .recovery_registry_once(&recovery_registry, now_ms())
                            .await
                        {
                            tracing::warn!(code = error.code, "Recovery stage deferred");
                        }
                        if let Err(error) = recovery_runtime
                            .restore_registry_once(&recovery_registry, now_ms())
                            .await
                        {
                            tracing::warn!(code = error.code, "Restore stage deferred");
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(poll_interval_ms))
                            .await;
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
                        for _ in 0..4 {
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
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
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
            let allocation_cancel = tokio_util::sync::CancellationToken::new();
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
                    std::time::Duration::from_millis(poll_interval_ms),
                    allocation_cancel.clone(),
                )))
            };
            let mut asset_cleanup_at = 0;
            let mut idle_sleep_at = 0;
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
                if now_ms() >= grant_cleanup_at {
                    if let Err(error) = runtime.cleanup_grant_receipts(now_ms()).await {
                        tracing::warn!(code = error.code, "Credential receipt cleanup deferred");
                    }
                    grant_cleanup_at = now_ms() + 60_000;
                }
                if now_ms() >= idle_sleep_at {
                    match runtime.idle_sleep_once(now_ms()).await {
                        Ok(true) => {
                            idle_sleep_at = 0;
                            lifecycle_priority = true;
                            zuse_cloud::events::hint::send();
                        }
                        Ok(false) => idle_sleep_at = now_ms() + 10_000,
                        Err(error) => {
                            idle_sleep_at = now_ms() + 10_000;
                            tracing::warn!(code = error.code, "Idle sleep admission deferred");
                        }
                    }
                }
                let allocation = if once {
                    runtime.allocate_registry_once(&registry, now_ms()).await
                } else {
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
                let repair = runtime
                    .runtime_repair_registry_once(&registry, now_ms())
                    .await;
                if matches!(repair, Ok(true)) {
                    zuse_cloud::events::hint::send();
                }
                if let Err(error) = repair {
                    tracing::warn!(code = error.code, "Runtime repair stage deferred");
                    if once {
                        anyhow::bail!("{}", error.code);
                    }
                }
                // One durable due task drain per ordinary round; never inside
                // the priority lifecycle burst. Missing optional schema is cached.
                // Provider preparation cannot hold up lifecycle/control dispatch.
                runtime.dispatch_project_preparation(now_ms());
                match runtime.work_task_drain_once(now_ms()).await {
                    Ok(true) => zuse_cloud::events::hint::send(),
                    Err(error) => tracing::warn!(code = error.code, "Task drain deferred"),
                    _ => {}
                }
                match runtime.recreation_once(now_ms()).await {
                    Ok(true) => zuse_cloud::events::hint::send(),
                    Err(error) => {
                        tracing::warn!(code = error.code, "Computer recreation stage deferred")
                    }
                    _ => {}
                }
                if !hinted_cycle {
                    let lifecycle = runtime.lifecycle_registry_once(&registry, now_ms()).await;
                    if matches!(lifecycle, Ok(true)) {
                        zuse_cloud::events::hint::send();
                    }
                    if let Err(error) = lifecycle {
                        tracing::warn!(code = error.code, "Retained lifecycle stage deferred");
                        if once {
                            anyhow::bail!("{}", error.code);
                        }
                    }
                }
                let deletion = runtime.delete_registry_once(&registry, now_ms()).await;
                if matches!(deletion, Ok(true)) {
                    zuse_cloud::events::hint::send();
                }
                if let Err(error) = deletion {
                    tracing::warn!(code = error.code, "Deletion stage deferred");
                    if once {
                        anyhow::bail!("{}", error.code);
                    }
                }
                // Expired uploads are already unauthorized. Their physical
                // purge is bounded maintenance, not part of every task wake.
                if now_ms() >= asset_cleanup_at {
                    if let Err(error) = runtime.sweep_launch_assets_once(now_ms()).await {
                        tracing::warn!(
                            code = error.code,
                            "Abandoned launch upload cleanup deferred"
                        );
                    }
                    if let Err(error) = runtime.sweep_asset_releases_once(now_ms()).await {
                        tracing::warn!(code = error.code, "Attachment release sweep deferred");
                    }
                    asset_cleanup_at = now_ms() + 60_000;
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
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => break,
                    result = lifecycle_signal.receive() => {
                        if result.is_ok() {lifecycle_priority=true;}
                        if result.is_err() {
                            tracing::warn!(code="lifecycle_wake_hint_disconnected","Using durable lifecycle reconciliation");
                            lifecycle_signal=zuse_cloud::runtime::lifecycle::wakeup::Listener::disabled();
                        }
                    },
                    _ = tokio::time::sleep(std::time::Duration::from_millis(poll_interval_ms)) => {},
                }
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
            if let Some(task) = recovery_task {
                task.abort();
            }
        }
        Command::Serve => {
            let config = Config::from_env()?;
            let runtime = match RuntimeConfig::from_env()? {
                Some(settings) => {
                    Some(RuntimeControl::connect(&config.database_url, settings).await?)
                }
                None => None,
            };
            let runtime_enabled = runtime.is_some();
            let mut store = Store::connect_read_only(&config.database_url)
                .await?
                .configure_transcripts()?
                .with_staging_base_pin(runtime.as_ref().and_then(|r| r.staging_base_pin()));
            let auth_accounts = env::var("ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
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
                store = store.with_connections(
                    zuse_cloud::connections::ConnectionControl::connect(
                        &config.database_url,
                        master,
                        auth_accounts,
                    )
                    .await?,
                );
            }
            store.devices = zuse_cloud::devices::Authority::from_env().await?;
            store =
                store.with_project_templates(zuse_cloud::projects::template_versions_from_env()?);
            if let Ok(origin) = env::var("ZUSE_CLOUD_LEGACY_GITHUB_URL") {
                store =
                    store.with_github_authority(zuse_cloud::github::LegacyGithub::new(&origin)?);
            }
            store = store
                .with_native_github_from_env(runtime.as_ref().map(|runtime| runtime.shared_pool()))?
                .with_github_organization_authority(&config)?;
            let project_accounts = env::var("ZUSE_CLOUD_PROJECT_WRITE_ACCOUNTS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if !project_accounts.is_empty() {
                if let Some(runtime) = &runtime {
                    store = store
                        .with_computer_control(zuse_cloud::computers::ComputerControl::from_pool(
                            runtime.shared_pool(),
                            project_accounts.clone(),
                        ))
                        .with_project_control(zuse_cloud::projects::ProjectControl::from_pool(
                            runtime.shared_pool(),
                            project_accounts,
                        ));
                } else {
                    store = store.with_computer_control(
                        zuse_cloud::computers::ComputerControl::connect(
                            &config.database_url,
                            project_accounts.clone(),
                        )
                        .await?,
                    );
                    store = store.with_project_control(
                        zuse_cloud::projects::ProjectControl::connect(
                            &config.database_url,
                            project_accounts,
                        )
                        .await?,
                    );
                }
            }
            if let Some(settings) = zuse_cloud::images::ImageConfig::from_env()? {
                let accounts = env::var("ZUSE_CLOUD_IMAGE_WRITE_ACCOUNTS")?
                    .split(',')
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned)
                    .collect();
                if let Some(runtime) = &runtime {
                    store = store.with_image_control(zuse_cloud::images::ImageControl::from_pool(
                        runtime.shared_pool(),
                        accounts,
                        settings,
                    ));
                } else {
                    store = store.with_image_control(
                        zuse_cloud::images::ImageControl::connect(
                            &config.database_url,
                            accounts,
                            settings,
                        )
                        .await?,
                    );
                }
            }
            let billing_accounts = env::var("ZUSE_CLOUD_BILLING_WRITE_ACCOUNTS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if !billing_accounts.is_empty() {
                let mut billing = if let Some(runtime) = &runtime {
                    zuse_cloud::billing::BillingControl::from_pool(
                        runtime.shared_pool(),
                        billing_accounts,
                    )
                } else {
                    zuse_cloud::billing::BillingControl::connect(
                        &config.database_url,
                        billing_accounts,
                    )
                    .await?
                };
                if let Some(providers) = zuse_cloud::billing::PaymentProviders::from_env()? {
                    billing = billing.with_payment_providers(providers);
                }
                billing = billing
                    .connect_native_from_env(&config.database_url)
                    .await?;
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
                        joining.wait_for_dispatch().await;
                    }
                });
            }
            store = store.with_account_events_from_env(runtime.as_ref()).await?;
            #[cfg(feature = "hosted-browser")]
            let hosted_browser = zuse_cloud::hosted_browser::from_env(&config)?;
            if let Some(settings) = zuse_cloud::plugins::PluginConfig::from_env()? {
                let plugins =
                    zuse_cloud::plugins::PluginControl::connect(&config.database_url, settings)
                        .await?;
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
            #[cfg(feature = "hosted-browser")]
            let app = if let Some(extension) = hosted_browser.as_ref() {
                app.merge(extension.router())
            } else {
                app
            };
            let mut app = app;
            let slack_worker = if let Some(slack_config) = zuse_cloud::slack::Config::from_env()? {
                let slack =
                    zuse_cloud::slack::Slack::connect(state, slack_config, &config.database_url)
                        .await?;
                let worker = slack.start_worker();
                app = app.merge(slack.router());
                Some(worker)
            } else {
                None
            };
            axum_serve(listener, app).await?;
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

fn serde_pretty(value: &serde_json::Value) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)?)
}

async fn serve_gateway() -> Result<()> {
    let config = Config::from_env()?;
    let settings = RuntimeConfig::from_env()?
        .context("Gateway requires ZUSE_CLOUD_RUNTIME_WRITES=true for lease fencing")?;
    let runtime = RuntimeControl::connect(&config.database_url, settings).await?;
    let state = AppState {
        store: Store::connect_read_only(&config.database_url).await?,
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
async fn writable_runtime() -> Result<Arc<RuntimeControl>> {
    let settings = RuntimeConfig::from_env()?
        .context("This command requires ZUSE_CLOUD_RUNTIME_WRITES=true")?;
    let url = env::var("ZUSE_CLOUD_DATABASE_URL").context("ZUSE_CLOUD_DATABASE_URL is required")?;
    let runtime = RuntimeControl::connect(&url, settings).await?;
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
    Ok(runtime)
}
