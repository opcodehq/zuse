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
    Mcp,
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
    if matches!(args.command, Some(Command::Mcp)) {
        return zuse_mcp::run(zuse_mcp::McpConfig::from_env()).await;
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
    let retained = storage.initialized();
    if retained {
        authority.commit_storage(&bootstrap, &enrollment).await?;
    }
    let lock = InstanceLock::acquire(&directory)?;
    let ipc = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, args.ipc_port)).await?;
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
    let result = async {
        storage.commit()?;
        if !retained {
            authority.commit_storage(&bootstrap, &enrollment).await?;
        }
        let rpc = core.rpc_service();
        let ipc_task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
            zuse_rpc::serve_ws_listener(ipc, rpc.clone()),
        ));
        adopt(&core, &bootstrap, &checkout, args.test_harness).await?;
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
                    Ok(()) => break,
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
        tracing::info!("Cloud runtime enrolled; serving workspace gateway");
        let (credentials, updates) = tokio::sync::watch::channel(
            identity
                .bootstrap()
                .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?
                .clone(),
        );
        let mailbox = if bootstrap.extra.get("commandMailboxEnabled")
            == Some(&serde_json::Value::Bool(true))
        {
            let key = identity.open_transcript_key(&bootstrap.sealed_transcript_key)?;
            Some(zuse_cloud_runtime::mailbox::Mailbox::new(
                storage.database_path(),
                &bootstrap,
                zuse_cloud_runtime::mailbox::CommandKey::parse(&key)?,
                Arc::new(EngineMailbox {
                    core: core.clone(),
                    thread: bootstrap.chat_id.clone(),
                    checkout: checkout.clone(),
                }),
            )?)
        } else {
            None
        };
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
        result
    }
    .await;
    cancel.cancel();
    core.shutdown().await;
    result
}
async fn adopt(
    core: &EngineCore,
    bootstrap: &CloudRuntimeBootstrap,
    checkout: &std::path::Path,
    allow_test: bool,
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
    service.handle(methods::MUTATE,serde_json::json!({"op":"createChat","chatId":bootstrap.chat_id,"deviceId":core.device_id,"cwd":checkout,"config":{"harness":harness,"model":intent.model,"sandbox":sandbox}})).await?;
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
    if let Some(prompt) = &intent.first_message {
        let now = zuse_cloud_runtime::now_ms();
        let reply=service.handle(methods::RELAY_COMMAND,serde_json::json!({"chatId":bootstrap.initial_session_id,"entry":{"id":intent.command_id,"payload":{"kind":"run","messageId":intent.turn_id,"request":{"prompt":prompt,"harness":harness,"model":intent.model,"cwd":checkout,"sandbox":sandbox,"autoApprove":auto}},"issuedBy":"cloud-launch","issuedAt":now,"expiresAt":now+120000,"status":"pending"}})).await?;
        if let zuse_rpc::RpcReply::Value(value) = reply {
            ensure!(
                matches!(value["outcome"].as_str(), Some("executed" | "duplicate")),
                "runtime_launch_not_admitted"
            );
        } else {
            anyhow::bail!("runtime_launch_receipt_invalid");
        }
    }
    Ok(())
}

struct EngineMailbox {
    core: Arc<EngineCore>,
    thread: String,
    checkout: PathBuf,
}
#[async_trait::async_trait]
impl zuse_cloud_runtime::mailbox::Executor for EngineMailbox {
    async fn admitted(
        &self,
        payload: &zuse_cloud_runtime::mailbox::MessageSend,
        fingerprint: &str,
    ) -> Result<bool> {
        let Some(chat) = self.core.workspace.chat(&payload.session_id)? else {
            return Ok(false);
        };
        ensure!(
            chat.id == self.thread || chat.thread_id.as_deref() == Some(&self.thread),
            "mailbox_conversation_scope_rejected"
        );
        Ok(self.core.doc_host.mailbox_command_admitted(
            &payload.session_id,
            &payload.command_id,
            fingerprint,
        )?)
    }
    async fn admit(
        &self,
        payload: &zuse_cloud_runtime::mailbox::MessageSend,
        fingerprint: &str,
        created_at: i64,
    ) -> Result<()> {
        let chat = self
            .core
            .workspace
            .chat(&payload.session_id)?
            .ok_or_else(|| anyhow::anyhow!("mailbox_conversation_missing"))?;
        ensure!(
            chat.id == self.thread || chat.thread_id.as_deref() == Some(&self.thread),
            "mailbox_conversation_scope_rejected"
        );
        ensure!(
            !chat.archived && chat.device_id == self.core.device_id,
            "mailbox_conversation_unavailable"
        );
        let config = self
            .core
            .workspace
            .chat_config(&chat.id)
            .or_else(|| self.core.workspace.chat_config(&self.thread))
            .ok_or_else(|| anyhow::anyhow!("mailbox_conversation_settings_missing"))?;
        let cwd = chat
            .cwd
            .as_deref()
            .unwrap_or_else(|| self.checkout.to_str().unwrap_or(""));
        ensure!(!cwd.is_empty(), "mailbox_checkout_missing");
        let model_options = match &payload.model_options {
            Some(options) => serde_json::to_value(options)?,
            None => serde_json::to_value(&config.model_options)?,
        };
        let entry: zuse_doc::SessionCommandEntry = serde_json::from_value(
            serde_json::json!({"id":payload.command_id,"payload":{"kind":"run","messageId":payload.client_message_id,"request":{"prompt":payload.prompt(),"harness":config.harness,"model":config.model,"reasoning":config.reasoning,"modelOptions":model_options,"cwd":cwd,"sandbox":config.sandbox,"autoApprove":config.sandbox==SandboxLevel::DangerFullAccess}},"issuedBy":format!("cloud-mailbox:{fingerprint}"),"issuedAt":created_at,"status":"pending"}),
        )?;
        self.core
            .doc_host
            .admit_mailbox_command(&payload.session_id, entry)
            .await?;
        Ok(())
    }
}
