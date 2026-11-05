//! Always-on Linux workspace guest, using the existing Rust engine and tools.
mod secret_logs;
use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;
use zuse_cloud_runtime::{Authority, DataLock, Identity, IdentityBinding, Storage, supervisor};
use zuse_engine::{EngineCore, EngineProfile, InstanceLock, default_registry};
use zuse_protocol::{
    HarnessId, SandboxLevel,
    cloud::{CloudRuntimeBootstrap, CloudRuntimeMode},
    cloud_computers::{PreparationStage, PreparationStepState},
};
use zuse_rpc::{RpcService, methods};

mod active;
mod early_ingress;
mod early_start;

#[derive(Parser)]
#[command(
    name = "zuse-cloud-runtime",
    version,
    about = "Run an enrolled Rust cloud workspace"
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(long, env = "ZUSE_CLOUD_WORKSPACE_ID")]
    workspace_id: Option<String>,
    #[arg(long, env = "ZUSE_API_URL")]
    api_url: Option<String>,
    #[arg(long, env = "ZUSE_USER_DATA")]
    data_dir: Option<PathBuf>,
    #[arg(long, env = "ZUSE_CLOUD_WORKSPACE_ROOT")]
    workspace_root: Option<PathBuf>,
    #[arg(long, env = "ZUSE_STORAGE_INCARNATION_ID")]
    storage_incarnation_id: Option<String>,
    #[arg(long, env = "ZUSE_RUNTIME_GENERATION")]
    generation: Option<i64>,
    #[arg(long, env = "ZUSE_RUNTIME_BOOT_TOKEN_FILE")]
    boot_token_file: Option<PathBuf>,
    #[arg(long, default_value_t = 27654)]
    ipc_port: u16,
    #[arg(long, default_value_t = false)]
    test_harness: bool,
}
#[derive(Subcommand)]
enum Command {
    /// Wait for this computer's checkout, branch and required setup; no API polling.
    WorkspaceWait {
        #[arg(long, default_value_t = 900, value_parser = clap::value_parser!(u64).range(1..=3600))]
        timeout_seconds: u64,
    },
    /// Report implementation protocols and this binary hash without enrolling.
    RuntimeCapabilities,
    /// Report project-cache protocol without enrolling or preparing anything.
    ProjectCacheCapabilities,
    /// Exact identity-fenced native network transport recovery; no engine boot.
    #[cfg(target_os = "linux")]
    NativeSockets {
        #[arg(value_parser=["capture","reset"])]
        operation: String,
    },
    /// Notify an existing parked runtime; exit3 means a cold retained start is needed.
    #[cfg(unix)]
    NotifyResume {
        #[arg(long)]
        operation_id: String,
    },
    /// Prepare a credential-free repository image in an isolated build machine.
    PrepareImage {
        #[arg(long)]
        plan: PathBuf,
    },
    /// Prepare a token-free project bundle without enrolling or starting Core.
    PrepareProjectCache {
        #[arg(long)]
        plan: PathBuf,
    },
    /// Fence a credential-free prepared image before its original snapshot.
    CaptureProjectCache {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        plan_sha256: String,
        #[arg(long)]
        status: bool,
    },
    Mcp,
    /// Capture or restore a fenced workspace using its scoped archive capability.
    Recover,
}
fn main() -> Result<()> {
    if std::env::args_os().count() == 2
        && std::env::args_os().nth(1).as_deref()
            == Some(std::ffi::OsStr::new("runtime-capabilities"))
    {
        println!(
            "{}",
            serde_json::to_string(&zuse_cloud_runtime::capabilities::current()?)?
        );
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        if args.len() == 2 && args[0] == "native-sockets" {
            if let Some(mode) = args[1]
                .to_str()
                .filter(|v| matches!(*v, "capture" | "reset"))
            {
                return zuse_cloud_runtime::native_sockets::run_cli(mode);
            }
        }
    }

    #[cfg(unix)]
    if std::env::args_os()
        .next()
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_name())
        == Some(std::ffi::OsStr::new("zuse-grok-auth"))
    {
        use std::io::Write;
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                let socket = std::env::var_os("ZUSE_GROK_AUTH_SOCKET")
                    .ok_or_else(|| anyhow::anyhow!("grok_auth_unavailable"))?;
                zuse_cloud_runtime::grok_auth::helper(
                    socket.into(),
                    std::env::var("GROK_AUTH_EXPIRED").as_deref() == Ok("1"),
                )
                .await
            })
            .map_err(|_| anyhow::anyhow!("grok_auth_unavailable"))?;
        std::io::stdout().lock().write_all(&result)?;
        return Ok(());
    }
    #[cfg(unix)]
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("github-from-profile")) {
        let args: Vec<_> = std::env::args_os().skip(2).collect();
        std::process::exit(zuse_cloud_runtime::github::run_profile(&args)?);
    }
    #[cfg(unix)]
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("github-cli")) {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        ensure!(
            args.len() >= 6
                && args[1] == "--socket"
                && args[3] == "--repository"
                && args[5] == "--",
            "github_cli_arguments_invalid"
        );
        let repository = args[4]
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("github_repository_invalid"))?;
        let code = zuse_cloud_runtime::github::run_cli(
            std::path::Path::new(&args[2]),
            repository,
            &args[6..],
        )?;
        std::process::exit(code);
    }
    #[cfg(unix)]
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("github-credential")) {
        let arguments: Vec<_> = std::env::args_os().skip(1).collect();
        ensure!(
            arguments.len() == 4 && arguments[1] == "--socket",
            "github_credential_unavailable"
        );
        let operation = arguments[3]
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("github_credential_unavailable"))?;
        return zuse_cloud_runtime::github::run_helper_cli(
            std::path::Path::new(&arguments[2]),
            operation,
        );
    }
    if zuse_engine::command_host::requested() {
        zuse_engine::command_host::run_blocking()?;
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}
async fn run() -> Result<()> {
    let secret_filter = Arc::new(zuse_providers::credentials::redaction::Secrets::default());
    tracing_subscriber::fmt()
        .with_writer(secret_logs::Writer::new(secret_filter.clone()))
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    "warn,zuse_cloud_runtime=info,zuse_providers::codex=info",
                )
            }),
        )
        .init();
    let args = Args::parse();
    if matches!(args.command, Some(Command::RuntimeCapabilities)) {
        println!(
            "{}",
            serde_json::to_string(&zuse_cloud_runtime::capabilities::current()?)?
        );
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    if let Some(Command::NativeSockets { operation }) = &args.command {
        return zuse_cloud_runtime::native_sockets::run_cli(operation);
    }
    if let Some(Command::PrepareImage { plan }) = &args.command {
        let plan: zuse_cloud_runtime::image::ImagePlan =
            serde_json::from_slice(&std::fs::read(plan)?)?;
        let result = zuse_cloud_runtime::image::prepare(&plan)
            .map_err(|_| anyhow::anyhow!("image_preparation_failed"))?;
        println!("{}", serde_json::to_string(&result)?);
        return Ok(());
    }
    if matches!(args.command, Some(Command::ProjectCacheCapabilities)) {
        println!(
            "{}",
            serde_json::to_string(&zuse_cloud_runtime::project_cache::capabilities()?)?
        );
        return Ok(());
    }
    if let Some(Command::PrepareProjectCache { plan }) = &args.command {
        let receipt = zuse_cloud_runtime::project_cache::prepare_cli(plan)
            .await
            .map_err(|_| anyhow::anyhow!("project_cache_preparation_failed"))?;
        println!("{}", serde_json::to_string(&receipt)?);
        return Ok(());
    }
    if let Some(Command::CaptureProjectCache {
        plan,
        plan_sha256,
        status,
    }) = &args.command
    {
        let receipt = zuse_cloud_runtime::project_cache::capture::cli(plan, plan_sha256, *status)
            .map_err(|_| anyhow::anyhow!("project_cache_capture_failed"))?;
        println!("{}", serde_json::to_string(&receipt)?);
        return Ok(());
    }
    if matches!(args.command, Some(Command::Mcp)) {
        return zuse_mcp::run(zuse_mcp::McpConfig::from_env()).await;
    }
    if matches!(args.command, Some(Command::Recover)) {
        let data = args
            .data_dir
            .ok_or_else(|| anyhow::anyhow!("runtime_data_directory_required"))?;
        let config = std::env::var("ZUSE_ARCHIVE_CONFIG")
            .map_err(|_| anyhow::anyhow!("recovery_config_required"))?;
        let outcome = zuse_cloud_runtime::recovery::run(data, &config)
            .await
            .map_err(|_| anyhow::anyhow!("workspace_recovery_failed"))?;
        println!("{}", outcome.phase());
        return Ok(());
    }
    #[cfg(unix)]
    if let Some(Command::NotifyResume { operation_id }) = &args.command {
        let wake = zuse_cloud_runtime::parking::Wake {
            workspace_id: args
                .workspace_id
                .clone()
                .ok_or_else(|| anyhow::anyhow!("workspace_id_required"))?,
            generation: args
                .generation
                .ok_or_else(|| anyhow::anyhow!("runtime_generation_required"))?,
            storage_incarnation_id: args
                .storage_incarnation_id
                .clone()
                .ok_or_else(|| anyhow::anyhow!("storage_incarnation_required"))?,
            operation_id: operation_id.clone(),
        };
        let data = args
            .data_dir
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("runtime_data_directory_required"))?;
        if zuse_cloud_runtime::parking::notify(data, &wake).await? {
            println!("runtime_wake_delivered");
            return Ok(());
        }
        std::process::exit(3);
    }
    if let Some(Command::WorkspaceWait { timeout_seconds }) = &args.command {
        zuse_cloud_runtime::workspace_wait::wait(
            args.data_dir
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("runtime_data_directory_required"))?,
            args.workspace_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("workspace_id_required"))?,
            args.generation
                .ok_or_else(|| anyhow::anyhow!("runtime_generation_required"))?,
            args.storage_incarnation_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("storage_incarnation_required"))?,
            std::time::Duration::from_secs(*timeout_seconds),
            &CancellationToken::new(),
        )
        .await?;
        println!("Workspace files, branch and required setup are ready.");
        return Ok(());
    }
    let workspace = args
        .workspace_id
        .ok_or_else(|| anyhow::anyhow!("workspace_id_required"))?;
    let authority = Authority::new(
        &args
            .api_url
            .ok_or_else(|| anyhow::anyhow!("api_url_required"))?,
    )?;
    let directory = args
        .data_dir
        .ok_or_else(|| anyhow::anyhow!("runtime_data_directory_required"))?;
    let inherited = std::env::var("ZUSE_WORKSPACE_RUNTIME_LOCK_FD")
        .ok()
        .map(|s| s.parse::<i32>())
        .transpose()?;
    let _data_lock = DataLock::acquire(&directory, inherited)?;
    let directory = std::fs::canonicalize(directory)?;
    let binding = IdentityBinding {
        workspace,
        api_url: authority.origin(),
        incarnation: args
            .storage_incarnation_id
            .ok_or_else(|| anyhow::anyhow!("storage_incarnation_required"))?,
        generation: args
            .generation
            .ok_or_else(|| anyhow::anyhow!("runtime_generation_required"))?,
    };
    let token = match args.boot_token_file {
        Some(path) => Some(std::fs::read_to_string(path)?.trim().to_owned()),
        None => std::env::var("ZUSE_RUNTIME_BOOT_TOKEN").ok(),
    };
    let mut phase = zuse_cloud_runtime::startup::Phase::new(&binding.workspace, "guest-identity");
    let mut identity = Identity::open(&directory, &binding, token.as_deref())?;
    phase.complete();
    drop(phase);
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            if let Ok(mut term) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
                signal.cancel();
                return;
            }
        }
        let _ = tokio::signal::ctrl_c().await;
        signal.cancel();
    });
    let mut phase = zuse_cloud_runtime::startup::Phase::new(&binding.workspace, "guest-enrollment");
    let mut bootstrap = loop {
        match authority
            .enroll(&mut identity, &binding, token.as_deref())
            .await
        {
            Ok(boot) => break boot,
            Err(error) if supervisor::retryable(&error) => {
                supervisor::wait(&cancel, 3).await;
                if cancel.is_cancelled() {
                    return Ok(());
                }
            }
            Err(error) => return Err(error),
        }
    };
    phase.complete();
    drop(phase);
    let checkout = std::fs::canonicalize(
        args.workspace_root
            .ok_or_else(|| anyhow::anyhow!("workspace_root_required"))?,
    )?;
    ensure!(checkout.is_dir(), "workspace_checkout_missing");
    let profile = EngineProfile::synced(
        &directory,
        &bootstrap.workspace_id,
        &bootstrap.zuse_account_id,
    )
    .with_durable_command_commits()
    .with_managed_recovery()
    .with_preparation_pending();
    let enrollment = match Storage::enrollment(&directory)? {
        Some(id) => id,
        None => authority.prepare_storage(&bootstrap).await?,
    };
    let mut storage = Storage::prepare(
        &directory,
        &profile.store_root().join("docs.sqlite3"),
        &bootstrap,
        &binding.incarnation,
        &enrollment,
    )?;
    let command_key = {
        let key = identity.open_transcript_key(&bootstrap.sealed_transcript_key)?;
        zuse_cloud_runtime::mailbox::CommandKey::parse(&key)?
    };
    zuse_cloud_runtime::mailbox::cold_recovery::quarantine_missing_ack(
        storage.database_path(),
        &bootstrap,
        &command_key,
    )?;
    let (credentials, updates) = tokio::sync::watch::channel(bootstrap.clone());
    let retained = storage.initialized();
    let interrupted_preparation = early_ingress::pending(&directory, &bootstrap, &enrollment)?;
    let repository_import_allowed = !retained || interrupted_preparation;
    if retained && interrupted_preparation && bootstrap.launch_intent.is_some() {
        // Fence delayed stage reports from the retired preparation process.
        // Its command/turn identity stays unchanged; only its bearer rotates.
        authority.renew(&mut identity).await?;
        bootstrap = identity
            .bootstrap()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?;
        credentials.send_replace(bootstrap.clone());
    }
    if bootstrap.extra.get("rustRepositoryCheckout").is_some()
        && !directory.join("rust-repository-ready.json").is_file()
    {
        ensure!(
            repository_import_allowed,
            "runtime_repository_import_required"
        );
    }
    early_ingress::begin(&directory, &bootstrap, &enrollment)?;
    let track_preparation =
        bootstrap.extra.get("rustFastStartVersion") == Some(&serde_json::json!(1));
    if track_preparation {
        zuse_cloud_runtime::workspace_wait::publish(&directory, &bootstrap, "pending")?;
    }
    let lock = InstanceLock::acquire(&directory)?;
    let ipc = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, args.ipc_port)).await?;
    let selected_agent = bootstrap
        .launch_intent
        .as_ref()
        .map(|intent| intent.agent.as_str())
        .or_else(|| {
            bootstrap
                .extra
                .get("agent")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or_else(|| {
            if bootstrap.extra.get("codexAuthMode") == Some(&serde_json::json!("broker-v1"))
                && bootstrap.extra.get("providerAuthMode") != Some(&serde_json::json!("broker-v1"))
            {
                "codex"
            } else {
                "claude"
            }
        });
    let default_harness = if args.test_harness {
        HarnessId::Mock
    } else if selected_agent == "codex" {
        HarnessId::Codex
    } else if selected_agent == "grok" {
        HarnessId::Grok
    } else {
        HarnessId::ClaudeCode
    };
    let mut phase =
        zuse_cloud_runtime::startup::Phase::new(&binding.workspace, "guest-engine-assembly");
    let core = Arc::new(EngineCore::assemble_with_profile_locked(
        profile,
        Arc::new(zuse_engine::HarnessRegistry::new()),
        default_harness,
        None,
        lock,
    )?);
    phase.complete();
    drop(phase);
    if track_preparation {
        let data = directory.clone();
        let boot = bootstrap.clone();
        core.sessions
            .set_provider_context(Arc::new(move |request| {
                let context = (|| -> anyhow::Result<Option<String>> {
                    if zuse_cloud_runtime::workspace_wait::needs_wait(&data, &boot)? {
                        Ok(Some(zuse_cloud_runtime::workspace_wait::instruction(
                            &data, &boot,
                        )?))
                    } else {
                        Ok(None)
                    }
                })()
                .map_err(|error| zuse_engine::EngineError::Other(error.to_string()))?;
                if let Some(context) = context {
                    request.prompt.push_str(&context);
                }
                Ok(())
            }))?;
    }
    core.sessions.set_ipc_port(ipc.local_addr()?.port());
    core.registry.set_secret_filter(secret_filter.clone());
    core.sessions.set_secret_filter(secret_filter.clone());
    core.doc_host.set_secret_filter(secret_filter.clone());
    core.doc_host.disable_native_provider_import();
    core.require_github_launcher();
    let canonical_task = bootstrap.extra.get("commandDeliveryMode")
        == Some(&serde_json::json!(zuse_protocol::cloud_task_commands::MODE))
        && bootstrap.extra.get("commandAdmissionVersion")
            == Some(&serde_json::json!(
                zuse_protocol::cloud_task_commands::VERSION
            ));
    if canonical_task {
        core.doc_host.require_canonical_cloud_commands();
    }
    let task_progress = if canonical_task {
        Some(zuse_cloud_runtime::task_progress::Publisher::open(
            storage.database_path(),
            &bootstrap,
            &core,
        )?)
    } else {
        None
    };
    let publisher = if bootstrap.extra.get("nativeTranscriptProjectionEnabled")
        == Some(&serde_json::Value::Bool(true))
    {
        let key = identity.open_transcript_key(&bootstrap.sealed_transcript_key)?;
        Some(Arc::new(zuse_cloud_runtime::projection::Publisher::open(
            storage.database_path(),
            &key,
        )?))
    } else {
        None
    };
    let result = async {
        storage.commit()?;
        authority.commit_storage(&bootstrap, &enrollment).await?;
        let rpc = if canonical_task {
            zuse_cloud_runtime::engine::CanonicalTaskRpc::new(core.rpc_service())
        } else {
            core.rpc_service()
        };
        let rpc = zuse_cloud_runtime::engine::PreparationRpc::new(rpc, core.sessions.clone(), directory.clone());
        let ipc_task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
            zuse_rpc::serve_ws_listener(ipc, rpc.clone()),
        ));
        // Only install conversation identity. No initial command, assets or
        // model handshake is admitted until project preparation completes.
        adopt(&core, &bootstrap, &checkout, args.test_harness, &command_key, &authority, true).await?;
        let ingress = if bootstrap.extra.get("preparationIngressVersion") == Some(&serde_json::json!(1)) {
            serde_json::json!({"diagnosticsReady":true,"agentReady":false,"credentialPreparationVersion":1})
        } else { authority.preparation_ingress(&bootstrap).await? };
        let credential_reports = zuse_cloud_runtime::preparation::credential_reports_supported(&ingress);
        let selected_broker = if !args.test_harness && (
            bootstrap.extra.get("providerAuthMode") == Some(&serde_json::json!("broker-v1"))
            || bootstrap.extra.get("codexAuthMode") == Some(&serde_json::json!("broker-v1"))
        ) {
            let provider = match selected_agent {
                "claude" | "claude-code" => zuse_protocol::cloud_auth::CloudAuthProvider::Claude,
                "codex" => zuse_protocol::cloud_auth::CloudAuthProvider::Codex,
                "grok" => zuse_protocol::cloud_auth::CloudAuthProvider::Grok,
                _ => anyhow::bail!("runtime_agent_unsupported"),
            };
            let broker = zuse_cloud_runtime::grants::ProviderBroker::new(
                identity.grant_recipient(&bootstrap)?, provider, authority.clone(), updates.clone(),
            );
            broker.set_secret_filter(secret_filter.clone())?;
            Some((provider, broker))
        } else { None };
        let git_recipient = identity.git_recipient(&bootstrap)?;
        let workload_recipient = identity.workload_recipient(&bootstrap)?;
        let fast_start = canonical_task && bootstrap.launch_intent.is_some()
            && bootstrap.extra.get("rustFastStartVersion") == Some(&serde_json::json!(1))
            && selected_broker.is_some() && !interrupted_preparation;
        let early_workload = if fast_start {
            let workload = early_start::workload(&identity, &bootstrap, &authority, &updates)?;
            workload.set_secret_filter(secret_filter.clone())?;
            Some(workload)
        } else { None };
        let early_ready = Arc::new(tokio::sync::Notify::new());
        let mut early_brokers = Vec::new();
        if fast_start {
            for provider in [zuse_protocol::cloud_auth::CloudAuthProvider::Claude,
                zuse_protocol::cloud_auth::CloudAuthProvider::Codex,
                zuse_protocol::cloud_auth::CloudAuthProvider::Grok] {
                let broker = zuse_cloud_runtime::preparation::reuse_selected(selected_broker.as_ref(), provider, || {
                    let broker = zuse_cloud_runtime::grants::ProviderBroker::new(
                        identity.grant_recipient(&bootstrap)?, provider, authority.clone(), updates.clone());
                    broker.set_secret_filter(secret_filter.clone())?;
                    Ok(broker)
                })?;
                early_brokers.push((provider, broker));
            }
        }
        let binding_wait_cancel = cancel.child_token();
        let preparation = async {
    let git_auth = if bootstrap
        .extra
        .get("gitAuthMode")
        .and_then(serde_json::Value::as_str)
        == Some("broker-v1")
    {
        let url = bootstrap
            .extra
            .get("rustRepositoryCheckout")
            .and_then(|v| v.get("repositoryUrl"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("github_repository_missing"))?;
        let repository = zuse_cloud_runtime::github::repository_from_url(url)?;
        let auth = zuse_cloud_runtime::github::GithubAuth::start(
            git_recipient,
            authority.clone(),
            updates.clone(),
            &repository,
            &std::env::current_exe()?,
        )?;
        auth.set_secret_filter(secret_filter.clone())?;
        if bootstrap.extra.get("rustGithubCliVersion")==Some(&serde_json::json!(1)) {
            let launcher=auth.install_cli(&std::env::current_exe()?)?;
            core.install_github_launcher(launcher)?;
        }
        Some(auth)
    } else {
        None
    };
    // Bootstrap already prepared storage, which is all the Git grant requires.
    // Fetch it while the gateway commits storage; the repository step reuses
    // the cached grant. A failed prefetch is ignored and retried there.
    let storage_bound = zuse_cloud_runtime::preparation::wait_startup_binding(&bootstrap, updates.clone(), &binding_wait_cancel);
    match &git_auth {
        Some(auth) => {
            let prefetch = async {
                tokio::select! {
                    _ = binding_wait_cancel.cancelled() => {}
                    _ = auth.verify() => {}
                }
            };
            let (bound, ()) = tokio::join!(storage_bound, prefetch);
            bound?;
        }
        None => storage_bound.await?,
    }
    if let (Some(auth), Some(workload)) = (&git_auth, &early_workload) {
        if bootstrap.extra.get("rustGithubCliVersion") == Some(&serde_json::json!(1)) {
            workload.set_command_path(auth.cli_directory())?;
        }
    }
    let repository = async {
    if let Some(plan) = bootstrap.extra.get("rustRepositoryCheckout") {
        ensure!(
            repository_import_allowed || directory.join("rust-repository-ready.json").is_file(),
            "runtime_repository_import_required"
        );
        let plan: zuse_cloud_runtime::repository::CheckoutPlan =
            serde_json::from_value(plan.clone())?;
        let repository_progress = report_preparation(
            &authority,
            &updates,
            &cancel,
            &directory,
            PreparationStage::Repository,
            PreparationStepState::Running,
            None,
        );
        if let Some(auth) = &git_auth {
            let access = zuse_cloud_runtime::preparation::access_with_progress(
                repository_progress, auth.verify(),
            ).await?;
            if let Err(error) = access {
                report_preparation(
                    &authority,
                    &updates,
                    &cancel,
                    &directory,
                    PreparationStage::Repository,
                    PreparationStepState::Failed,
                    None,
                )
                .await?;
                return Err(error);
            }
        } else {
            repository_progress.await?;
        }
        let prepared = {
            let directory = directory.clone();
            let checkout = checkout.clone();
            let workspace = binding.workspace.clone();
            let incarnation = binding.incarnation.clone();
            let helper = git_auth.as_ref().map(|auth| auth.helper_command().to_owned());
            let repository_cancel = cancel.clone();
            tokio::task::spawn_blocking(move || {
                zuse_cloud_runtime::repository::prepare_cancellable(
                    &directory, &checkout, &workspace, &incarnation, &plan,
                    helper.as_deref(), &repository_cancel,
                )
            }).await.map_err(|_| anyhow::anyhow!("repository_preparation_worker_failed"))?
        };
        match prepared {
            Ok(()) => {
                let source = zuse_cloud_runtime::repository::prepared_source(
                    &directory,
                    &binding.workspace,
                    &binding.incarnation,
                )?;
                report_preparation(
                    &authority,
                    &updates,
                    &cancel,
                    &directory,
                    PreparationStage::Repository,
                    PreparationStepState::Completed,
                    Some(source),
                )
                .await?;
            }
            Err(error) => {
                report_preparation(
                    &authority,
                    &updates,
                    &cancel,
                    &directory,
                    PreparationStage::Repository,
                    PreparationStepState::Failed,
                    None,
                )
                .await?;
                return Err(error);
            }
        }
    }
    if let Some(auth) = &git_auth {
        auth.configure_checkout(&checkout).await?;
    }
        Ok(())
    };
    let credentials = async {
        // Retained resumes keep their lazy grants/processes; only fresh task
        // preparation performs this selected-agent access prefetch.
        if bootstrap.launch_intent.is_none() { return Ok(()); }
        let Some((_, broker)) = &selected_broker else { return Ok(()); };
        let access = async {
            tokio::select! {
                _ = cancel.cancelled() => anyhow::bail!("runtime_preparation_cancelled"),
                outcome = broker.credential() => outcome.map(|_| ()),
            }
        };
        let outcome = if credential_reports {
            zuse_cloud_runtime::preparation::access_with_progress(
                report_preparation(&authority, &updates, &cancel, &directory,
                    PreparationStage::Credentials, PreparationStepState::Running, None),
                access,
            ).await?
        } else {
            access.await
        };
        if credential_reports {
            report_preparation(&authority, &updates, &cancel, &directory,
                PreparationStage::Credentials,
                if outcome.is_ok() { PreparationStepState::Completed } else { PreparationStepState::Failed },
                None).await?;
        }
        // Never expose authority/vendor error bodies or credential material.
        outcome.map_err(|_| anyhow::anyhow!("credential_preparation_failed"))?;
        if let Some(workload) = &early_workload {
            for (provider, broker) in &early_brokers {
                if selected_broker.as_ref().is_some_and(|(selected, _)| selected == provider)
                    || bootstrap.extra.get("managedAgentCatalogs").and_then(|catalogs| catalogs.get(provider.id())).is_some() {
                    early_start::install(&core, &bootstrap, &directory, *provider, broker.clone(), workload.clone())?;
                }
            }
            core.sessions.complete_preparation(core.sessions.preparation_epoch())?;
            let intent = bootstrap.launch_intent.as_ref().expect("fast start requires initial intent");
            let current = updates.borrow().clone();
            authority.early_agent_ready(&current, &intent.command_id).await?;
            early_ready.notify_one();
            core.doc_host.kick_drains();
        }
        Ok(())
    };
    zuse_cloud_runtime::preparation::join_repository_and_credentials(repository, credentials).await?;
    ensure!(!cancel.is_cancelled(), "runtime_preparation_cancelled");
    let configured_environment = bootstrap
        .extra
        .get("environmentRecipe")
        .map(|recipe| {
            serde_json::from_value::<zuse_protocol::cloud_computers::EnvironmentRecipe>(
                recipe.clone(),
            )
        })
        .transpose()?;
    let mut workload_bindings = bootstrap
        .extra
        .get("workloadSecretBindings")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if let Some(recipe) = &configured_environment {
        workload_bindings.extend(
            zuse_cloud_runtime::environment::selected_recipe(&checkout, recipe)?.secret_bindings,
        );
    }
    workload_bindings.sort();
    workload_bindings.dedup();
    if fast_start {
        // An already-running native process cannot receive new environment
        // variables. Reject undeclared repository bindings before tools proceed.
        let mut declared = bootstrap.extra.get("workloadSecretBindings").and_then(serde_json::Value::as_array)
            .into_iter().flatten().filter_map(serde_json::Value::as_str).map(str::to_owned).collect::<Vec<_>>();
        if let Some(recipe) = &configured_environment { declared.extend(recipe.secret_bindings.clone()); }
        if !workload_bindings.iter().all(|binding| declared.contains(binding)) {
            report_preparation(&authority, &updates, &cancel, &directory,
                PreparationStage::Environment, PreparationStepState::Failed, None).await?;
            anyhow::bail!("early_agent_requires_declared_secret_bindings");
        }
    }
    let workload = zuse_cloud_runtime::workload_secrets::Broker::from_recipient(
        workload_recipient,
        &bootstrap,
        authority.clone(),
        updates.clone(),
        workload_bindings,
    )?;
    workload.set_secret_filter(secret_filter.clone())?;
    if bootstrap.extra.get("rustGithubCliVersion")==Some(&serde_json::json!(1)) {
        if let Some(auth)=&git_auth {workload.set_command_path(auth.cli_directory())?;}
    }
    let workload_values = workload.values(false).await?;
    let _environment_services = if let Some(recipe) = bootstrap.extra.get("environmentRecipe") {
        report_preparation(
            &authority,
            &updates,
            &cancel,
            &directory,
            PreparationStage::Environment,
            PreparationStepState::Running,
            None,
        )
        .await?;
        let result: Result<_> = async {
            let recipe: zuse_protocol::cloud_computers::EnvironmentRecipe =
                serde_json::from_value(recipe.clone())?;
            if let Some(cache)=bootstrap.extra.get("rustProjectCache") {
                zuse_cloud_runtime::project_cache::consume(
                    std::path::Path::new(zuse_cloud_runtime::project_cache::ROOT),
                    &directory,&checkout,&binding.workspace,&binding.incarnation,cache,&recipe,
                )?;
            }
            let prepared = zuse_cloud_runtime::environment::prepare_with_protection(
                &directory,
                &checkout,
                &binding.workspace,
                &binding.incarnation,
                &recipe,
                &workload_values,
                zuse_cloud_runtime::environment::OutputProtection::new(secret_filter.clone()),
            )
            .await?;
            #[cfg(target_os = "linux")]
            {
                Ok(Some(
                    zuse_cloud_runtime::environment::start_services_with_protection(
                        &directory,
                        &prepared,
                        workload_values.clone(),
                        zuse_cloud_runtime::environment::OutputProtection::new(
                            secret_filter.clone(),
                        ),
                    )
                    .await?,
                ))
            }
            #[cfg(not(target_os = "linux"))]
            {
                ensure!(
                    prepared.recipe.startup.is_empty(),
                    "environment_startup_requires_linux"
                );
                Ok(Some(()))
            }
        }
        .await;
        match result {
            Ok(services) => {
                report_preparation(
                    &authority,
                    &updates,
                    &cancel,
                    &directory,
                    PreparationStage::Environment,
                    PreparationStepState::Completed,
                    None,
                )
                .await?;
                services
            }
            Err(error) => {
                report_preparation(
                    &authority,
                    &updates,
                    &cancel,
                    &directory,
                    PreparationStage::Environment,
                    PreparationStepState::Failed,
                    None,
                )
                .await?;
                return Err(error);
            }
        }
    } else {
        None
    };
            Ok::<_, anyhow::Error>((git_auth, workload, _environment_services))
        };
        tokio::pin!(preparation);
        let cycle = cancel.child_token();
        let stop_task = if fast_start {
            let (_, mut events) = core.sessions.subscribe(&bootstrap.initial_session_id, 0)?;
            let (cancel, cycle, data, boot) = (cancel.clone(), cycle.clone(), directory.clone(), bootstrap.clone());
            Some(tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
                let mut first_output = true;
                loop {
                    tokio::select! {
                        _ = cycle.cancelled() => break,
                        event = events.recv() => match event {
                            Ok(event) if matches!(event.event, zuse_protocol::AgentEvent::Done { status: zuse_protocol::DoneStatus::Interrupted, .. }) => {
                                let _ = zuse_cloud_runtime::workspace_wait::publish(&data, &boot, "cancelled");
                                cancel.cancel(); break;
                            }
                            Ok(event) if first_output && matches!(event.event, zuse_protocol::AgentEvent::TextDelta { .. } | zuse_protocol::AgentEvent::ReasoningDelta { .. }) => {
                                first_output = false;
                                tracing::info!(workspace=%boot.workspace_id, phase="first-model-output", observed_at=zuse_cloud_runtime::now_ms(), "Cloud startup milestone");
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            _ => {},
                        }
                    }
                }
            })))
        } else { None };
        let preparing_mailbox = if fast_start {
            active::mailbox(&core, &authority, &identity, &bootstrap, storage.database_path(),
                &checkout, args.test_harness, &updates, task_progress.as_ref())?
        } else { None };
        let mailbox_task = preparing_mailbox.map(|mailbox| {
            let authority = authority.clone(); let updates = updates.clone(); let cancel = cycle.clone();
            let ready = early_ready.clone(); let fatal = cancel.clone();
            tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
                tokio::select! { _ = ready.notified() => {}, _ = cancel.cancelled() => return Ok(()) }
                let result = mailbox.run(authority, updates, cancel).await;
                if result.is_err() { fatal.cancel(); }
                result
            }))
        });
        let progress_task = if fast_start {
            task_progress.as_ref().map(|progress| {
                let progress = progress.clone(); let authority = authority.clone(); let updates = updates.clone(); let cancel = cycle.clone();
                tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
                    let boot = updates.borrow().clone();
                    let binding = zuse_cloud_runtime::preparation::wait_startup_binding(&boot, updates.clone(), &cancel).await;
                    if cancel.is_cancelled() { return Ok(()); }
                    binding?;
                    progress.run(authority, updates, cancel).await
                }))
            })
        } else { None };
        let projection_task = if fast_start {
            publisher.as_ref().map(|publisher| {
                let publisher = publisher.clone(); let core = core.clone();
                let authority = authority.clone(); let updates = updates.clone(); let cancel = cycle.clone();
                tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
                    let boot = updates.borrow().clone();
                    if zuse_cloud_runtime::preparation::wait_startup_binding(&boot, updates.clone(), &cancel).await.is_err() { return; }
                    publisher.run(core, authority, updates, cancel).await
                }))
            })
        } else { None };
        let prepared = {
            let gateway = supervisor::serve(&authority, &mut identity, rpc.clone(), cycle.clone(), credentials.clone());
            tokio::pin!(gateway);
            tokio::select! {
                result = &mut preparation => {
                    match result {
                        Ok(value) => {
                            if track_preparation { zuse_cloud_runtime::workspace_wait::publish(&directory, &bootstrap, if cancel.is_cancelled() { "cancelled" } else { "ready" })?; }
                            cycle.cancel();
                            gateway.await?;
                            value
                        }
                        Err(error) => {
                            if track_preparation { zuse_cloud_runtime::workspace_wait::publish(&directory, &bootstrap, if cancel.is_cancelled() { "cancelled" } else { "failed" })?; }
                            // Retain the same engine and diagnostic terminals.
                            // The failed stage remains visible; no retry can
                            // resend its durable first message implicitly.
                            tracing::warn!("Cloud preparation requires attention; diagnostic gateway retained");
                            let _ = gateway.await;
                            return Err(error);
                        }
                    }
                }
                result = &mut gateway => {
                    binding_wait_cancel.cancel();
                    cycle.cancel();
                    // Blocking checkout work may still own its child process.
                    // Join preparation before acknowledging quiescence so no
                    // Git/setup writer can survive a parked receipt.
                    let _ = (&mut preparation).await;
                    result?;
                    anyhow::bail!("runtime_preparation_gateway_stopped");
                }
            }
        };
        cycle.cancel();
        if let Some(task) = mailbox_task { task.await??; }
        if let Some(task) = progress_task { task.await??; }
        if let Some(task) = projection_task { let _ = task.await; }
        if let Some(task) = stop_task { task.await?; }
        let (_git_auth, workload, _environment_services) = prepared;
    let mut resume_brokers = None;
    let registry = if args.test_harness {
        ensure!(
            bootstrap
                .launch_intent
                .as_ref()
                .is_none_or(|i| i.agent == "mock"),
            "runtime_test_harness_mismatch"
        );
        let registry = zuse_engine::HarnessRegistry::new();
        registry.register(Arc::new(zuse_providers::mock::MockHarness {
            script: vec![
                zuse_protocol::AgentEvent::TextDelta {
                    text: "Cloud runtime fixture reply".into(),
                },
                zuse_protocol::AgentEvent::Done {
                    status: zuse_protocol::DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                },
            ],
        }));
        registry
    } else if bootstrap.extra.get("providerAuthMode") == Some(&serde_json::json!("broker-v1"))
        || bootstrap.extra.get("codexAuthMode") == Some(&serde_json::json!("broker-v1"))
    {
        let registry = zuse_engine::HarnessRegistry::new();
        ensure!(matches!(selected_agent, "claude" | "claude-code" | "codex" | "grok"), "runtime_agent_unsupported");
        let managed_catalogs: std::collections::BTreeMap<String, zuse_protocol::cloud_catalog::CloudAgentModels> = bootstrap.extra.get("managedAgentCatalogs")
            .map(|value| serde_json::from_value(value.clone())).transpose()?.unwrap_or_default();
        let mut brokers = Vec::new();
        for provider in [
            zuse_protocol::cloud_auth::CloudAuthProvider::Claude,
            zuse_protocol::cloud_auth::CloudAuthProvider::Codex,
            zuse_protocol::cloud_auth::CloudAuthProvider::Grok,
        ] {
        let broker = zuse_cloud_runtime::preparation::reuse_selected(
            early_brokers.iter().find(|(id, _)| *id == provider).or(selected_broker.as_ref()), provider, || {
                let broker = zuse_cloud_runtime::grants::ProviderBroker::new(
                    identity.grant_recipient(&bootstrap)?, provider, authority.clone(), updates.clone(),
                );
                broker.set_secret_filter(secret_filter.clone())?;
                Ok(broker)
            },
        )?;
        brokers.push(broker.clone());
        if !managed_catalogs.is_empty() && !managed_catalogs.contains_key(provider.id()) {
            continue;
        }
        if managed_catalogs.is_empty() && provider.id() != if selected_agent == "claude-code" { "claude" } else { selected_agent } {
            continue;
        }

        match provider {
            zuse_protocol::cloud_auth::CloudAuthProvider::Claude => registry.register(Arc::new(
                zuse_providers::ClaudeHarness::new().with_credentials(
                    zuse_cloud_runtime::workload_secrets::Credentials::wrap(
                        broker,
                        workload.clone(),
                    ),
                ),
            )),
            zuse_protocol::cloud_auth::CloudAuthProvider::Codex => registry.register(Arc::new(
                zuse_providers::CodexHarness::new()
                    .with_managed_startup()
                    .with_managed_memory_sessions(
                        (bootstrap.extra.get("internalCodexMemoryTransportReset")
                            == Some(&serde_json::json!(true)))
                        .then(|| broker.native_memory_sessions()),
                    )
                    .with_internal_tool_observations(
                        bootstrap.extra.get("internalCodexToolObservations")
                            == Some(&serde_json::json!(true)),
                    )
                    .with_credentials(
                        zuse_cloud_runtime::workload_secrets::CodexCredentials::wrap(
                            broker,
                            workload.clone(),
                        ),
                    ),
            )),
            #[cfg(unix)]
            zuse_protocol::cloud_auth::CloudAuthProvider::Grok => {
                let helper = std::env::var_os("ZUSE_GROK_AUTH_HELPER")
                    .map(PathBuf::from)
                    .unwrap_or(std::env::current_exe()?.with_file_name("zuse-grok-auth"));
                let sessions = directory.join("grok-managed").join("sessions");
                let credentials = zuse_cloud_runtime::grok_auth::LazyGrokAuth::new(
                    broker,
                    helper,
                    directory.join("grok-managed"),
                );
                registry.register(Arc::new(
                    zuse_providers::AcpHarness::grok()
                        .with_sessions_root(sessions)
                        .with_credentials(zuse_cloud_runtime::workload_secrets::Credentials::wrap(
                            credentials,
                            workload.clone(),
                        )),
                ));
            }
            _ => anyhow::bail!("runtime_agent_unsupported"),
        }
        }
        resume_brokers = Some(zuse_cloud_runtime::grants::ProviderBrokers::new(brokers));
        registry
    } else {
        default_registry()
    };
    for descriptor in registry.descriptors() {
        if let Ok(harness) = registry.resolve(descriptor.id) {
            core.registry.register(harness);
        }
    }
        if let Some(catalogs) = bootstrap.extra.get("managedAgentCatalogs") {
            let catalogs: std::collections::BTreeMap<String, zuse_protocol::cloud_catalog::CloudAgentModels> = serde_json::from_value(catalogs.clone())?;
            for (_, catalog) in catalogs {
                core.registry.pin_models(catalog.id, catalog.models);
            }
        }
        workload.attach_terminals(core.terminals.clone()).await?;
        let resume = if bootstrap.launch_intent.is_none() {
            let directive = authority
                .lifecycle(
                    &bootstrap,
                    zuse_protocol::cloud_lifecycle::RuntimeRequest::Query,
                )
                .await?;
            if let Some(d) = &directive {
                zuse_cloud_runtime::lifecycle::activate(&core, &bootstrap, d)?;
            }
            directive
        } else {
            None
        };
        let prepared_bootstrap = updates.borrow().clone();
        adopt(
            &core,
            &prepared_bootstrap,
            &checkout,
            args.test_harness,
            &command_key,
            &authority,
            false,
        )
        .await?;
        if !core.sessions.agent_preparation_ready() { core.sessions.complete_preparation(core.sessions.preparation_epoch())?; }
        core.doc_host.kick_drains();
        early_ingress::complete(&directory, &bootstrap, &enrollment)?;
        if let Some(intent) = &bootstrap.launch_intent {
            let head = core
                .sessions
                .subscribe(&bootstrap.initial_session_id, 0)?
                .0
                .last()
                .map(|e| e.seq as i64)
                .unwrap_or(0);
            loop {
                match authority
                    .launch_ready(
                        identity
                            .bootstrap()
                            .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?,
                        &intent.command_id,
                        head,
                    )
                    .await
                {
                    Ok(()) => {
                        identity.complete_launch(&intent.command_id)?;
                        break;
                    }
                    Err(error) if supervisor::retryable(&error) => {
                        supervisor::wait(&cancel, 3).await;
                        if cancel.is_cancelled() {
                            return Ok(());
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        if let Some(d) = resume {
            authority
                .lifecycle(
                    &bootstrap,
                    zuse_protocol::cloud_lifecycle::RuntimeRequest::Resumed {
                        operation_id: d.operation_id,
                    },
                )
                .await?;
        }
        tracing::info!("Cloud runtime enrolled; serving workspace gateway");
        credentials.send_replace(
            identity
                .bootstrap()
                .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?
                .clone(),
        );
        let result = active::run(
            &core,
            &authority,
            &mut identity,
            rpc,
            publisher.as_ref(),
            &directory,
            storage.database_path(),
            &checkout,
            args.test_harness,
            &cancel,
            &credentials,
            &updates,
            resume_brokers.as_ref(),
            task_progress.as_ref(),
        )
        .await;
        drop(ipc_task);
        result
    }
    .await;
    cancel.cancel();
    let current = identity.bootstrap().cloned().unwrap_or(bootstrap);
    let shutdown = match authority
        .lifecycle(
            &current,
            zuse_protocol::cloud_lifecycle::RuntimeRequest::Query,
        )
        .await
    {
        Ok(Some(d))
            if matches!(
                d.action,
                zuse_protocol::cloud_lifecycle::Action::Pause
                    | zuse_protocol::cloud_lifecycle::Action::Archive
            ) && d.phase == "quiescing" =>
        {
            Some(zuse_cloud_runtime::lifecycle::hold(&core, &current, d))
        }
        _ => None,
    };
    core.shutdown().await;
    if let Some(progress) = &task_progress {
        tokio::time::timeout(
            std::time::Duration::from_secs(8),
            progress.flush(&authority, &current),
        )
        .await
        .map_err(|_| anyhow::anyhow!("task_progress_flush_timeout"))??;
    }
    if let Some(shutdown) = shutdown {
        shutdown?
            .acknowledge(&core, &authority, &current, publisher.as_deref())
            .await?;
        return Ok(());
    }
    result
}
/// Stage reports are durable and idempotent, and cannot admit a turn. Retained
/// resume skips initial preparation reporting and keeps the original source pin.
async fn report_preparation(
    authority: &Authority,
    updates: &tokio::sync::watch::Receiver<CloudRuntimeBootstrap>,
    cancel: &CancellationToken,
    data: &std::path::Path,
    stage: PreparationStage,
    state: PreparationStepState,
    source_commit: Option<String>,
) -> Result<()> {
    let boot = updates.borrow().clone();
    if boot.launch_intent.is_none()
        || boot.extra.get("rustPreparationVersion") != Some(&serde_json::json!(1))
    {
        return Ok(());
    }
    let operation_id = boot
        .extra
        .get("rustAllocationOperationId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("runtime_preparation_operation_missing"))?
        .to_owned();
    let error_code = if state == PreparationStepState::Failed {
        Some(
            match stage {
                PreparationStage::Repository => "repository_preparation_failed",
                PreparationStage::Environment => "environment_setup_failed",
                PreparationStage::Credentials => "credential_preparation_failed",
                _ => anyhow::bail!("runtime_preparation_stage_invalid"),
            }
            .to_owned(),
        )
    } else {
        None
    };
    let update = zuse_protocol::cloud_computers::RuntimePreparationUpdate {
        operation_id,
        stage,
        state,
        source_commit,
        error_code,
        log: if stage == PreparationStage::Environment && state != PreparationStepState::Running {
            // Optional diagnostic publication must not change a stage outcome.
            zuse_cloud_runtime::environment::preparation_log(
                data,
                &[&boot.runtime_credential, &boot.sealed_transcript_key],
            )
            .ok()
            .flatten()
        } else {
            None
        },
    };
    loop {
        ensure!(!cancel.is_cancelled(), "runtime_preparation_cancelled");
        let current = updates.borrow().clone();
        match authority.preparation(&current, &update).await {
            Ok(()) => return Ok(()),
            Err(error)
                if supervisor::retryable(&error)
                    && zuse_cloud_runtime::now_ms() < current.runtime_credential_expires_at =>
            {
                tokio::select! { _=cancel.cancelled()=>anyhow::bail!("runtime_preparation_cancelled"), _=tokio::time::sleep(std::time::Duration::from_millis(500))=>{} }
            }
            Err(error) => return Err(error),
        }
    }
}

async fn adopt(
    core: &EngineCore,
    bootstrap: &CloudRuntimeBootstrap,
    checkout: &std::path::Path,
    allow_test: bool,
    command_key: &zuse_cloud_runtime::mailbox::CommandKey,
    authority: &zuse_cloud_runtime::Authority,
    identity_only: bool,
) -> Result<()> {
    let Some(intent) = &bootstrap.launch_intent else {
        ensure!(
            core.workspace.chat(&bootstrap.chat_id)?.is_some()
                && core
                    .workspace
                    .chat(&bootstrap.initial_session_id)?
                    .is_some(),
            "runtime_retained_history_missing"
        );
        for chat in core.workspace.read_chats()? {
            if !chat.archived
                && chat.device_id == core.device_id
                && (chat.id == bootstrap.chat_id
                    || chat.thread_id.as_deref() == Some(&bootstrap.chat_id))
            {
                core.doc_host.open_local(&chat.id)?;
            }
        }
        return Ok(());
    };
    ensure!(intent.fork_source.is_none(), "runtime_fork_import_required");
    ensure!(
        intent.permissions.is_empty(),
        "runtime_permission_import_required"
    );
    let harness = HarnessId::try_from(if intent.agent == "claude" {
        "claude-code".into()
    } else {
        intent.agent.clone()
    })
    .map_err(|_| anyhow::anyhow!("runtime_agent_unsupported"))?;
    ensure!(
        harness != HarnessId::Mock || allow_test,
        "runtime_test_harness_disabled"
    );
    let (sandbox, auto) = match intent.runtime_mode.as_ref() {
        Some(CloudRuntimeMode::FullAccess) => (SandboxLevel::DangerFullAccess, true),
        Some(CloudRuntimeMode::Auto) => (SandboxLevel::Auto, false),
        Some(CloudRuntimeMode::AutoAcceptEdits | CloudRuntimeMode::AutoAcceptEditsAndBash) => {
            anyhow::bail!("runtime_permission_mode_requires_port")
        }
        _ => (SandboxLevel::WorkspaceWrite, false),
    };
    if sandbox == SandboxLevel::Auto {
        SandboxLevel::automatic_review_support(&harness)
            .map_err(|_| anyhow::anyhow!("runtime_permission_mode_requires_port"))?;
    }
    let service = core.rpc_service();
    service.handle(methods::MUTATE,serde_json::json!({"op":"createChat","chatId":bootstrap.chat_id,"deviceId":core.device_id,"cwd":checkout,"config":{"harness":harness,"model":intent.model,"reasoning":intent.reasoning,"modelOptions":intent.model_options,"sandbox":sandbox}})).await?;
    if bootstrap.initial_session_id != bootstrap.chat_id {
        service.handle(methods::CREATE_THREAD_SESSION,serde_json::json!({"chatId":bootstrap.initial_session_id,"threadId":bootstrap.chat_id})).await?;
    }
    zuse_cloud_runtime::engine::seed_adoption_titles(
        core,
        &bootstrap.chat_id,
        &bootstrap.initial_session_id,
        &intent.title,
    )?;
    if identity_only {
        return Ok(());
    }
    if bootstrap.extra.get("commandDeliveryMode")
        == Some(&serde_json::json!(zuse_protocol::cloud_task_commands::MODE))
        && bootstrap.extra.get("commandAdmissionVersion")
            == Some(&serde_json::json!(
                zuse_protocol::cloud_task_commands::VERSION
            ))
    {
        // The atomic API admission owns the sole first command and its assets.
        // Initial adoption only installs conversation identity; no local copy
        // of firstMessage or duplicate asset download may dispatch here.
        ensure!(
            intent.first_message.is_none(),
            "cloud_canonical_initial_delivery_conflict"
        );
        return Ok(());
    }
    let fingerprint = command_key.fingerprint(&serde_json::to_vec(intent)?);
    if core.doc_host.mailbox_command_admitted(
        &bootstrap.initial_session_id,
        &intent.command_id,
        &fingerprint,
    )? {
        return Ok(());
    }
    let mut paths = Vec::new();
    for asset in &intent.attachments {
        let (metadata, bytes) = authority.asset(bootstrap, asset).await?;
        paths.push(core.uploads.import_cloud_asset(&metadata, &bytes)?);
    }
    if let Some(prompt) = &intent.first_message
        && !core.doc_host.legacy_launch_admitted(
            &bootstrap.initial_session_id,
            &intent.command_id,
            &intent.turn_id,
            prompt,
        )?
    {
        let prompt = zuse_protocol::message_attachments::with_attachments(prompt, &paths);
        let entry: zuse_doc::SessionCommandEntry = serde_json::from_value(
            serde_json::json!({"id":intent.command_id,"payload":{"kind":"run","messageId":intent.turn_id,"request":{"prompt":prompt,"attachments":paths,"harness":harness,"model":intent.model,"reasoning":intent.reasoning,"modelOptions":intent.model_options,"cwd":checkout,"sandbox":sandbox,"autoApprove":auto}},"issuedBy":format!("cloud-mailbox:{fingerprint}"),"issuedAt":zuse_cloud_runtime::now_ms(),"expiresAt":9_007_199_254_740_991i64,"status":"pending"}),
        )?;
        core.doc_host
            .admit_mailbox_command(&bootstrap.initial_session_id, entry)
            .await?;
    }
    Ok(())
}
