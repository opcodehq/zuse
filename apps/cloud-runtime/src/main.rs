//! Always-on Linux workspace guest, using the existing Rust engine and tools.
use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;
use zuse_cloud_runtime::{Authority, DataLock, Identity, IdentityBinding, Storage, supervisor};
use zuse_engine::{EngineCore, EngineProfile, InstanceLock, default_registry};
use zuse_protocol::{
    HarnessId, SandboxLevel,
    cloud::{CloudRuntimeBootstrap, CloudRuntimeMode},
};
use zuse_rpc::{RpcService, methods};

#[derive(Parser)]
#[command(
    name = "zuse-cloud-runtime",
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
    /// Prepare a credential-free repository image in an isolated build machine.
    PrepareImage {
        #[arg(long)]
        plan: PathBuf,
    },
    Mcp,
    /// Capture or restore a fenced workspace using its scoped archive capability.
    Recover,
}
fn main() -> Result<()> {
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
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args = Args::parse();
    if let Some(Command::PrepareImage { plan }) = &args.command {
        let plan: zuse_cloud_runtime::image::ImagePlan =
            serde_json::from_slice(&std::fs::read(plan)?)?;
        let result = zuse_cloud_runtime::image::prepare(&plan)
            .map_err(|_| anyhow::anyhow!("image_preparation_failed"))?;
        println!("{}", serde_json::to_string(&result)?);
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
    let mut identity = Identity::open(&directory, &binding, token.as_deref())?;
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
    let bootstrap = loop {
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
    ensure!(
        !bootstrap.zuse_account_id.starts_with("organization:"),
        "runtime_organization_access_unavailable"
    );
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
    .with_durable_command_commits();
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
    if let Some(plan) = bootstrap.extra.get("rustRepositoryCheckout") {
        ensure!(
            !storage.initialized() || directory.join("rust-repository-ready.json").is_file(),
            "runtime_repository_import_required"
        );
        let plan: zuse_cloud_runtime::repository::CheckoutPlan =
            serde_json::from_value(plan.clone())?;
        zuse_cloud_runtime::repository::prepare(
            &directory,
            &checkout,
            &binding.workspace,
            &binding.incarnation,
            &plan,
        )?;
    }
    let retained = storage.initialized();
    if retained {
        authority.commit_storage(&bootstrap, &enrollment).await?;
    }
    let lock = InstanceLock::acquire(&directory)?;
    let ipc = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, args.ipc_port)).await?;
    let (credentials, updates) = tokio::sync::watch::channel(bootstrap.clone());
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
    } else if bootstrap.extra.get("providerAuthMode") == Some(&serde_json::json!("broker-v1")) {
        let registry = zuse_engine::HarnessRegistry::new();
        let broker = zuse_cloud_runtime::grants::ProviderBroker::new(
            identity.grant_recipient(&bootstrap)?,
            zuse_protocol::cloud_auth::CloudAuthProvider::Claude,
            authority.clone(),
            updates.clone(),
        );
        registry.register(Arc::new(
            zuse_providers::ClaudeHarness::new().with_credentials(broker),
        ));
        registry
    } else {
        default_registry()
    };
    let core = Arc::new(EngineCore::assemble_with_profile_locked(
        profile,
        Arc::new(registry),
        HarnessId::ClaudeCode,
        None,
        lock,
    )?);
    core.sessions.set_ipc_port(ipc.local_addr()?.port());
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
        if !retained {
            authority.commit_storage(&bootstrap, &enrollment).await?;
        }
        let rpc = core.rpc_service();
        let ipc_task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
            zuse_rpc::serve_ws_listener(ipc, rpc.clone()),
        ));
        let command_key = {
            let key = identity.open_transcript_key(&bootstrap.sealed_transcript_key)?;
            zuse_cloud_runtime::mailbox::CommandKey::parse(&key)?
        };
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
        adopt(
            &core,
            &bootstrap,
            &checkout,
            args.test_harness,
            &command_key,
            &authority,
        )
        .await?;
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
        let mailbox = if bootstrap.extra.get("commandMailboxEnabled")
            == Some(&serde_json::Value::Bool(true))
        {
            Some(zuse_cloud_runtime::mailbox::Mailbox::new(
                storage.database_path(),
                &bootstrap,
                command_key,
                Arc::new(zuse_cloud_runtime::engine::EngineExecutor {
                    core: core.clone(),
                    thread: bootstrap.chat_id.clone(),
                    checkout: checkout.clone(),
                    allow_test: args.test_harness,
                }),
            )?)
        } else {
            None
        };
        let projection = publisher.as_ref().map(|publisher| {
            let publisher = publisher.clone();
            let core = core.clone();
            let authority = authority.clone();
            let updates = updates.clone();
            let cancel = cancel.clone();
            tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
                publisher.run(core, authority, updates, cancel).await
            }))
        });
        let supervisor =
            supervisor::serve(&authority, &mut identity, rpc, cancel.clone(), credentials);
        tokio::pin!(supervisor);
        let result = if let Some(mailbox) = mailbox {
            tokio::select! {
                result=&mut supervisor=>result,
                result=mailbox.run(authority.clone(),updates,cancel.clone())=>result,
            }
        } else {
            supervisor.await
        };
        cancel.cancel();
        drop(ipc_task);
        if let Some(mut projection) = projection {
            let _ = (&mut projection).await;
        }
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
    if let Some(shutdown) = shutdown {
        shutdown?
            .acknowledge(&core, &authority, &current, publisher.as_deref())
            .await?;
        return Ok(());
    }
    result
}
async fn adopt(
    core: &EngineCore,
    bootstrap: &CloudRuntimeBootstrap,
    checkout: &std::path::Path,
    allow_test: bool,
    command_key: &zuse_cloud_runtime::mailbox::CommandKey,
    authority: &zuse_cloud_runtime::Authority,
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
    if core
        .workspace
        .chat(&bootstrap.chat_id)?
        .is_some_and(|c| c.title.is_none())
    {
        core.workspace
            .rename_chat(&bootstrap.chat_id, &intent.title)?;
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
