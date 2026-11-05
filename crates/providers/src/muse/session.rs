//! One Muse Code runtime: the host process, its session, and the event loop
//! multiplexing host events, the steering mailbox, turn stops, background
//! task stops, the interrupt token and consumer liveness.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use zuse_protocol::{
    AgentEvent, DoneStatus, HarnessId, RunRequest, UserInputAnswer, UserInputQuestion,
};

use super::{
    catalog,
    normalize::Items,
    rpc::{self, Host, Mode},
};
use crate::{HarnessError, RunControls, SteerMessage, TurnControl, jsonrpc::Incoming};

pub(super) struct Config {
    pub exe: PathBuf,
    pub init_timeout: Duration,
    pub interrupt_grace: Duration,
    pub kill_grace: Duration,
}

type Events = mpsc::Sender<Result<AgentEvent, HarnessError>>;
type RequestInput =
    Arc<dyn Fn(Vec<UserInputQuestion>) -> oneshot::Receiver<Vec<UserInputAnswer>> + Send + Sync>;

/// The permission profile every Zuse session runs under: Muse asks, Zuse decides.
const ASK_ME: &str = ":ask-me";
/// How many events one `view/page` asks for (the protocol allows 1–1000).
const PAGE_LIMIT: u64 = 200;

pub(super) fn is_compact(prompt: &str) -> bool {
    prompt.trim() == "/compact"
}

/// Drive one runtime to completion. The stream always ends with a `Done`
/// for a turn in flight unless the consumer hung up.
pub(super) async fn run(config: Config, request: RunRequest, controls: RunControls, tx: Events) {
    let RunControls {
        realtime: _,
        execution_lease: _lease,
        request_input,
        mut steering,
        interrupt,
        turn,
    } = controls;
    let done =
        |error: Option<String>, status: DoneStatus, session: Option<String>| AgentEvent::Done {
            status,
            result: None,
            error,
            session_id: session,
        };
    let cwd = PathBuf::from(&request.cwd);
    let setup = async {
        let mut host = Host::open(
            &config.exe,
            &cwd,
            Mode::Session {
                read_only: request.plan,
            },
            config.init_timeout,
            config.kill_grace,
        )
        .await?;
        match open_session(&host, &request).await {
            Ok(opened) => Ok((host, opened)),
            Err(error) => {
                host.child.shutdown(config.kill_grace).await;
                Err(error)
            }
        }
    };
    let (host, opened) = tokio::select! {
        result = setup => match result {
            Ok(ready) => ready,
            Err(error) => {
                let _ = tx.send(Ok(done(Some(error.to_string()), DoneStatus::Errored, None))).await;
                return;
            }
        },
        _ = interrupt.cancelled() => {
            let _ = tx.send(Ok(done(None, DoneStatus::Interrupted, None))).await;
            return;
        }
    };
    let Host {
        mut child,
        client,
        mut incoming,
        tail,
        ..
    } = host;
    let mut runner = Runner {
        client,
        session: opened.session.clone(),
        root: workspace_root(&cwd),
        live: request.clone(),
        tx: tx.clone(),
        assistant: uuid::Uuid::new_v4().to_string(),
        items: Items::default(),
        active: None,
        running: false,
        stopping: false,
        interrupted: false,
        queued: VecDeque::new(),
        accepted: VecDeque::new(),
        approvals: HashMap::new(),
        decided: HashSet::new(),
        questions: HashSet::new(),
        served: HashSet::new(),
        completed: VecDeque::new(),
        request_input: Arc::from(request_input),
        turn: turn.clone(),
    };
    let started = runner
        .emit(AgentEvent::SessionStarted {
            harness: HarnessId::Muse,
            model: opened.model.clone(),
            tools: vec![],
            cwd: request.cwd.clone(),
            session_id: opened.session.clone(),
            assistant_message_id: runner.assistant.clone(),
        })
        .await;
    let mut result = started;
    if result.is_ok()
        && let Some(message) = opened.notice
    {
        result = runner.emit(AgentEvent::Error { message }).await;
    }
    if result.is_ok() {
        result = runner
            .start_turn(&request.prompt, &request.attachments)
            .await;
    }
    if let Err(error) = result {
        if !tx.is_closed() {
            let _ = tx
                .send(Ok(done(
                    Some(error.to_string()),
                    DoneStatus::Errored,
                    Some(runner.session.clone()),
                )))
                .await;
        }
        child.shutdown(config.kill_grace).await;
        return;
    }

    let mut steering_open = true;
    let outcome: Result<(), HarnessError> = loop {
        if !steering_open && !runner.running && runner.queued.is_empty() {
            break Ok(());
        }
        let step = tokio::select! {
            incoming = incoming.recv() => match incoming {
                Some(Incoming::Notification { method, params }) => runner.notification(&method, params).await,
                Some(Incoming::Request { id, method, params: _ }) => {
                    // The notification twins (`approval/requested`,
                    // `userInput/requested`) carry the decision flow.
                    if matches!(method.as_str(), "approval/request" | "userInput/request") {
                        runner.client.respond(&id, json!({}));
                    } else {
                        runner.client.respond_error(&id, -32601, &format!("unsupported method: {method}"));
                    }
                    Ok(())
                }
                Some(Incoming::Eof) | None => break Err(HarnessError::Protocol("Muse Code exited".into())),
            },
            steer = crate::receive_steer(&mut steering, &turn), if steering_open => match steer {
                Some(message) => runner.steer(message).await,
                None => {
                    steering_open = false;
                    Ok(())
                }
            },
            _ = turn.stop_requested() => runner.stop_turn().await,
            task = turn.background_stop_requested() => {
                let stop = rpc::command(&runner.client, "task/stop", json!({"sessionId": runner.session, "taskId": task}));
                tokio::spawn(async move {
                    if let Err(error) = stop.await {
                        tracing::warn!(target: "zuse_providers::muse", %error, "task/stop failed");
                    }
                });
                Ok(())
            },
            _ = interrupt.cancelled() => {
                runner.interrupted = true;
                runner.queued.clear();
                if runner.running {
                    runner.send_interrupt();
                    // Let the turn settle on its own terminal within the grace.
                    let settle = async {
                        while runner.running {
                            match incoming.recv().await {
                                Some(Incoming::Notification { method, params }) => runner.notification(&method, params).await?,
                                Some(Incoming::Request { id, .. }) => runner.client.respond(&id, json!({})),
                                Some(Incoming::Eof) | None => break,
                            }
                        }
                        Ok::<_, HarnessError>(())
                    };
                    let _ = tokio::time::timeout(config.interrupt_grace, settle).await;
                }
                if runner.running {
                    runner.running = false;
                    let _ = runner.emit(done(None, DoneStatus::Interrupted, Some(runner.session.clone()))).await;
                }
                break Ok(());
            },
            _ = tx.closed() => break Ok(()),
        };
        if let Err(error) = step {
            break Err(error);
        }
    };
    if let Err(error) = outcome
        && runner.running
        && !tx.is_closed()
    {
        let status = tokio::time::timeout(Duration::from_millis(300), child.wait())
            .await
            .ok()
            .and_then(Result::ok);
        let message = if status.is_some() {
            tail.wait_closed().await;
            crate::crash_message("Muse Code", status, &tail)
        } else {
            error.to_string()
        };
        let _ = tx
            .send(Ok(done(
                Some(message),
                DoneStatus::Errored,
                Some(runner.session.clone()),
            )))
            .await;
    }
    turn.set_background_tasks(Vec::new());
    child.shutdown(config.kill_grace).await;
}

struct Opened {
    session: String,
    model: String,
    /// Shown once when a saved conversation could not be reopened.
    notice: Option<String>,
}

/// The host's canonical spelling of the workspace (Muse wants absolute,
/// existing, canonical roots).
fn workspace_root(cwd: &Path) -> String {
    cwd.canonicalize()
        .unwrap_or_else(|_| cwd.to_path_buf())
        .display()
        .to_string()
}

/// Zuse's own MCP server as a session-scoped stdio server.
fn session_config(request: &RunRequest) -> Option<Value> {
    let mcp = request.mcp.as_ref()?;
    Some(json!({
        "mcpServers": {
            mcp.name.clone(): {
                "transport": "stdio",
                "command": mcp.command,
                "args": mcp.args,
                "env": mcp.env,
                "mode": "optional",
            }
        }
    }))
}

fn picked_model(request: &RunRequest) -> Option<&str> {
    request
        .model
        .as_deref()
        .filter(|m| !m.is_empty() && !matches!(*m, "default" | "auto"))
}

async fn open_session(host: &Host, request: &RunRequest) -> Result<Opened, HarnessError> {
    let client = &host.client;
    let config = session_config(request);
    if config.is_some() && !host.granted("sessionMcp") {
        return Err(HarnessError::Protocol(
            "Update Muse Code to enable Zuse's session tools (session MCP is unavailable)".into(),
        ));
    }
    let root = workspace_root(Path::new(&request.cwd));
    let mut notice = None;
    let mut resumed = None;
    if let Some(id) = &request.resume {
        let mut params = json!({ "sessionId": id, "excludeItems": true });
        if let Some(config) = &config {
            params["config"] = config.clone();
        }
        match rpc::command(client, "session/resume", params).await {
            Ok(result) => resumed = Some(result),
            Err(error) if !is_compact(&request.prompt) => {
                // Like every other harness, a conversation that can no longer
                // be reopened starts fresh with a visible notice; failing would
                // strand the chat resuming the same id forever.
                notice = Some(format!(
                    "Muse Code could not restore session {id}; starting a new session without the previous context: {error}"
                ));
            }
            Err(error) => return Err(error),
        }
    }
    let result = match resumed {
        Some(result) => result,
        None => {
            let mut params = json!({
                "workspaceRoot": root,
                "providerId": catalog::PROVIDER,
                "approvalMode": "promptUnmatched",
                "permissionProfile": ASK_ME,
            });
            if let Some(model) = picked_model(request) {
                params["modelId"] = model.into();
            }
            if let Some(config) = &config {
                params["config"] = config.clone();
            }
            rpc::command(client, "session/start", params).await?
        }
    };
    let session = result["session"]["sessionId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| HarnessError::Protocol("Muse Code did not open a session".into()))?
        .to_owned();
    if notice.is_none()
        && let Some(id) = &request.resume
        && id != &session
    {
        return Err(HarnessError::Protocol(
            "Muse Code resumed a different session".into(),
        ));
    }
    let mut profile = result["session"]["permissionState"]["profileId"].clone();
    if profile.is_null() {
        let read = rpc::command(client, "session/read", json!({ "sessionId": session })).await?;
        profile = read["session"]["permissionState"]["profileId"].clone();
    }
    if profile != ASK_ME {
        return Err(HarnessError::Protocol(
            "Muse Code did not apply the ask-me permission profile; update Muse Code before continuing"
                .into(),
        ));
    }
    let resumed = request.resume.is_some() && notice.is_none();
    if resumed {
        rpc::command(
            client,
            "session/setApprovalMode",
            json!({ "sessionId": session, "mode": "promptUnmatched" }),
        )
        .await?;
        if let Some(model) = picked_model(request) {
            set_model(client, &session, model).await?;
        }
    }
    let model = picked_model(request)
        .map(str::to_owned)
        .or_else(|| result["session"]["modelId"].as_str().map(str::to_owned))
        .unwrap_or_default();
    Ok(Opened {
        session,
        model,
        notice,
    })
}

async fn set_model(
    client: &crate::jsonrpc::RpcClient,
    session: &str,
    model: &str,
) -> Result<(), HarnessError> {
    rpc::command(
        client,
        "session/setModel",
        json!({
            "sessionId": session,
            "model": { "modelId": model, "providerId": catalog::PROVIDER },
        }),
    )
    .await
    .map(drop)
}

struct Runner {
    client: crate::jsonrpc::RpcClient,
    session: String,
    root: String,
    /// The configuration the runtime is running with now.
    live: RunRequest,
    tx: Events,
    assistant: String,
    items: Items,
    /// The turn in flight (from its acknowledgement or `turn/started`).
    active: Option<String>,
    /// A turn is in flight and owes a `Done`.
    running: bool,
    /// A turn stop is in flight: its end reads as Interrupted.
    stopping: bool,
    interrupted: bool,
    /// Steers waiting for the running turn to end.
    queued: VecDeque<SteerMessage>,
    /// `turn/steer` command ids accepted into the running turn, confirmed
    /// when their user message joins the transcript.
    accepted: VecDeque<String>,
    /// Pending approvals as last requested, by approval id.
    approvals: HashMap<String, Value>,
    /// Approval stages already decided (`approvalId` + requirement).
    decided: HashSet<String>,
    /// Prompts already put to the user.
    questions: HashSet<String>,
    /// Cursors a gap fill applied at or after its target: their live twins
    /// may still arrive and must not apply twice.
    served: HashSet<String>,
    /// Recently settled turns: a late `turn/started` never revives one.
    completed: VecDeque<String>,
    request_input: RequestInput,
    turn: TurnControl,
}

impl Runner {
    async fn emit(&self, event: AgentEvent) -> Result<(), HarnessError> {
        self.tx
            .send(Ok(event))
            .await
            .map_err(|_| HarnessError::Protocol("Muse Code event consumer closed".into()))
    }

    fn rotate(&mut self) -> AgentEvent {
        let previous = std::mem::replace(&mut self.assistant, uuid::Uuid::new_v4().to_string());
        AgentEvent::Steered {
            assistant_message_id: Some(previous),
            next_assistant_message_id: Some(self.assistant.clone()),
        }
    }

    async fn input(&self, text: &str, attachments: &[String]) -> Value {
        let mut input = vec![json!({ "type": "text", "text": text })];
        input.extend(load_images(attachments).await);
        Value::Array(input)
    }

    /// Start a turn (or a compaction) for `text`. The runtime must be idle.
    async fn start_turn(&mut self, text: &str, attachments: &[String]) -> Result<(), HarnessError> {
        self.stopping = false;
        if is_compact(text) {
            let id = rpc::command_id();
            let ack = rpc::command_with_id(
                &self.client,
                "session/compact",
                json!({ "sessionId": self.session }),
                id.clone(),
            )
            .await?;
            if ack["status"] == "noop" {
                let reason = ack["reason"]
                    .as_str()
                    .map(|r| format!(" ({})", r.replace('_', " ")))
                    .unwrap_or_default();
                self.emit(AgentEvent::TextDelta {
                    text: format!("Nothing to compact{reason}."),
                })
                .await?;
                return self
                    .emit(AgentEvent::Done {
                        status: DoneStatus::Completed,
                        result: None,
                        error: None,
                        session_id: Some(self.session.clone()),
                    })
                    .await;
            }
            if ack["status"] != "accepted" {
                return Err(HarnessError::Protocol(
                    "Muse Code rejected context compaction".into(),
                ));
            }
            // A compaction runs as a turn whose id is its command id.
            self.active = Some(id);
            self.running = true;
            return Ok(());
        }
        let mut params = json!({
            "sessionId": self.session,
            "input": self.input(text, attachments).await,
            "displayText": text,
            "workspaceRoots": [self.root],
        });
        if let Some(effort) = catalog::effort(self.live.reasoning) {
            params["reasoningEffort"] = effort.into();
        }
        let ack = rpc::command(&self.client, "turn/start", params).await?;
        let id = ack["turnId"].as_str().unwrap_or_default().to_owned();
        self.active = (!id.is_empty()).then_some(id);
        self.running = true;
        Ok(())
    }

    /// Adopt a new configuration in place before the prompt it came with.
    async fn reconfigure(&mut self, next: RunRequest) -> Result<(), HarnessError> {
        if let Some(model) = picked_model(&next)
            && picked_model(&self.live) != Some(model)
        {
            set_model(&self.client, &self.session, model).await?;
        }
        self.live = next;
        Ok(())
    }

    /// Deliver a steer between turns: a fresh turn on the same session.
    /// A turn Muse refuses ends errored; the runtime stays for the next one.
    async fn deliver(&mut self, message: SteerMessage) -> Result<(), HarnessError> {
        let steered = self.rotate();
        self.emit(steered).await?;
        let started = match message.config {
            Some(next) => self.reconfigure(*next).await,
            None => Ok(()),
        };
        let started = match started {
            Ok(()) => self.start_turn(&message.prompt, &message.attachments).await,
            Err(error) => Err(error),
        };
        if let Err(error) = started {
            if self.client.is_closed() {
                return Err(error);
            }
            self.running = false;
            self.active = None;
            self.emit(AgentEvent::Done {
                status: DoneStatus::Errored,
                result: None,
                error: Some(error.to_string()),
                session_id: Some(self.session.clone()),
            })
            .await?;
        }
        Ok(())
    }

    async fn steer(&mut self, message: SteerMessage) -> Result<(), HarnessError> {
        if !self.running {
            return self.deliver(message).await;
        }
        // Commands and configuration changes apply at a turn boundary; later
        // messages stay behind queued ones (Steered retires FIFO).
        let Some(expected) = self.active.clone().filter(|_| {
            self.queued.is_empty() && message.config.is_none() && !is_compact(&message.prompt)
        }) else {
            self.queued.push_back(message);
            return Ok(());
        };
        let id = rpc::command_id();
        let params = json!({
            "sessionId": self.session,
            "expectedTurnId": expected,
            "input": self.input(&message.prompt, &message.attachments).await,
        });
        match rpc::command_with_id(&self.client, "turn/steer", params, id.clone()).await {
            Ok(_) => self.accepted.push_back(id),
            // Most often the turn ended between the send and the steer: the
            // text becomes the next turn instead.
            Err(error) => {
                tracing::debug!(target: "zuse_providers::muse", %error, "turn/steer rejected; queued as the next turn");
                if self.running {
                    self.queued.push_back(message);
                } else {
                    return self.deliver(message).await;
                }
            }
        }
        Ok(())
    }

    fn send_interrupt(&self) {
        let Some(turn) = self.active.clone() else {
            return;
        };
        let interrupt = rpc::command(
            &self.client,
            "turn/interrupt",
            json!({ "sessionId": self.session, "turnId": turn }),
        );
        tokio::spawn(async move {
            if let Err(error) = interrupt.await {
                tracing::warn!(target: "zuse_providers::muse", %error, "turn/interrupt failed");
            }
        });
    }

    /// End only the turn in flight; the session stays for the next prompt.
    async fn stop_turn(&mut self) -> Result<(), HarnessError> {
        self.queued.clear();
        if self.running && self.active.is_some() {
            self.stopping = true;
            self.send_interrupt();
            return Ok(());
        }
        self.running = false;
        self.emit(AgentEvent::Done {
            status: DoneStatus::Interrupted,
            result: None,
            error: None,
            session_id: Some(self.session.clone()),
        })
        .await
    }

    /// The turn in flight reached its terminal.
    async fn finish_turn(
        &mut self,
        status: DoneStatus,
        error: Option<String>,
        usage: &Value,
    ) -> Result<(), HarnessError> {
        // Accepted steers whose message never surfaced still joined this turn.
        while self.accepted.pop_front().is_some() {
            let steered = self.rotate();
            self.emit(steered).await?;
        }
        if usage.is_object() {
            self.emit(AgentEvent::Usage {
                input_tokens: usage["inputTokens"].as_u64().unwrap_or(0),
                output_tokens: usage["outputTokens"].as_u64().unwrap_or(0),
            })
            .await?;
        }
        let status = if self.stopping || self.interrupted {
            DoneStatus::Interrupted
        } else {
            status
        };
        if let Some(id) = self.active.take() {
            self.completed.push_back(id);
            while self.completed.len() > 32 {
                self.completed.pop_front();
            }
        }
        self.running = false;
        self.stopping = false;
        self.items.prune();
        self.emit(AgentEvent::Done {
            status,
            result: None,
            error: (status == DoneStatus::Errored).then_some(error).flatten(),
            session_id: Some(self.session.clone()),
        })
        .await?;
        if !self.interrupted
            && let Some(next) = self.queued.pop_front()
        {
            self.deliver(next).await?;
        }
        Ok(())
    }

    async fn notification(&mut self, method: &str, params: Value) -> Result<(), HarnessError> {
        if params["sessionId"]
            .as_str()
            .is_some_and(|s| s != self.session)
        {
            return Ok(());
        }
        if let Some(cursor) = params["viewCursor"].as_str()
            && self.served.remove(cursor)
        {
            return Ok(());
        }
        if method == "view/gap" {
            let after = params["after"].as_str().unwrap_or_default().to_owned();
            let next = params["next"].as_str().unwrap_or_default().to_owned();
            if let Err(error) = self.fill_gap(after, next).await {
                // The live tail keeps folding; the hole is reported, not hidden.
                self.emit(AgentEvent::Error {
                    message: format!(
                        "Muse Code dropped session events and they could not be recovered; reopen the chat to reload it ({error})"
                    ),
                })
                .await?;
            }
            return Ok(());
        }
        self.apply(method, params).await
    }

    /// Recover a delivery hole: page forward from `after` until the page
    /// that carries `next` (or the end of the view).
    async fn fill_gap(&mut self, after: String, next: String) -> Result<(), HarnessError> {
        let mut cursor = after;
        loop {
            let page = self
                .client
                .request(
                    "view/page",
                    json!({ "sessionId": self.session, "cursor": cursor, "limit": PAGE_LIMIT }),
                )
                .await?;
            let events = page["events"].as_array().cloned().unwrap_or_default();
            let mut reached = false;
            for event in &events {
                let method = event["method"].as_str().unwrap_or_default();
                let params = event["params"].clone();
                if params["sessionId"]
                    .as_str()
                    .is_some_and(|s| s != self.session)
                {
                    return Err(HarnessError::Protocol(
                        "view/page served another session's events".into(),
                    ));
                }
                let at = params["viewCursor"].as_str().map(str::to_owned);
                if at.as_deref() == Some(next.as_str()) {
                    reached = true;
                }
                if reached && let Some(at) = at {
                    self.served.insert(at);
                }
                self.apply(method, params).await?;
            }
            match page["nextCursor"].as_str() {
                None => return Ok(()),
                Some(_) if reached => return Ok(()),
                Some(following) if events.is_empty() || following == cursor => {
                    return Err(HarnessError::Protocol("view/page made no progress".into()));
                }
                Some(following) => cursor = following.to_owned(),
            }
        }
    }

    async fn apply(&mut self, method: &str, params: Value) -> Result<(), HarnessError> {
        match method {
            "item/started" | "item/updated" | "item/completed" => {
                let item = &params["item"];
                if item["kind"] == "userMessage"
                    && let Some(at) = item["commandId"]
                        .as_str()
                        .and_then(|id| self.accepted.iter().position(|s| s == id))
                {
                    for _ in 0..=at {
                        self.accepted.pop_front();
                        let steered = self.rotate();
                        self.emit(steered).await?;
                    }
                }
                for event in self.items.item(item) {
                    self.emit(event).await?;
                }
                self.turn.set_background_tasks(self.items.background());
            }
            "item/delta" => {
                for event in self.items.delta(&params) {
                    self.emit(event).await?;
                }
            }
            "turn/started" => {
                let id = params["turnId"].as_str().unwrap_or_default();
                // A turn Muse began on its own (a background wake): publish
                // the boundary before its output reaches the parked chat.
                if !self.running && !id.is_empty() && !self.completed.iter().any(|c| c == id) {
                    self.active = Some(id.to_owned());
                    self.running = true;
                    let steered = self.rotate();
                    self.emit(steered).await?;
                }
            }
            "turn/completed" | "turn/unqueued" => {
                let id = params["turnId"].as_str().unwrap_or_default();
                let ours = self.running && self.active.as_deref().is_none_or(|a| a == id);
                if !ours {
                    return Ok(());
                }
                let (status, error) = if method == "turn/unqueued" {
                    (DoneStatus::Interrupted, None)
                } else {
                    match params["terminal"].as_str() {
                        Some("completed") => (DoneStatus::Completed, None),
                        Some("cancelled" | "interrupted") => (DoneStatus::Interrupted, None),
                        _ => (
                            DoneStatus::Errored,
                            Some(
                                params["error"]["message"]
                                    .as_str()
                                    .or_else(|| params["reason"].as_str())
                                    .unwrap_or("Muse Code could not complete the turn")
                                    .to_owned(),
                            ),
                        ),
                    }
                };
                self.finish_turn(status, error, &params["usage"]).await?;
            }
            "session/contextUsage" => {
                self.emit(AgentEvent::ContextUsage {
                    tokens: params["usedTokens"].as_u64(),
                    window: params["windowTokens"].as_u64(),
                })
                .await?;
            }
            "approval/requested" => self.approval(params),
            "approval/updated" => {
                if params["change"]["kind"] == "alreadyTerminal" {
                    return Ok(());
                }
                let id = params["approvalId"].as_str().unwrap_or_default();
                if let Some(mut merged) = self.approvals.get(id).cloned() {
                    for key in [
                        "availableChoices",
                        "currentRequirementId",
                        "subject",
                        "viewCursor",
                    ] {
                        if !params[key].is_null() {
                            merged[key] = params[key].clone();
                        }
                    }
                    self.approval(merged);
                }
            }
            "approval/resolved" => {
                if let Some(id) = params["approvalId"].as_str() {
                    // `decided` keeps the stage: a redelivered request for a
                    // settled approval must never prompt again.
                    self.approvals.remove(id);
                }
            }
            "userInput/requested" => self.question(params),
            "userInput/settled" => {
                if let Some(id) = params["userInputId"].as_str() {
                    self.questions.remove(id);
                }
            }
            // Model, effort, todo, branch and name changes: nothing to show.
            _ => {}
        }
        Ok(())
    }

    /// Settle one approval stage through Zuse's access policy, choosing only
    /// among the choices Muse offered.
    fn approval(&mut self, params: Value) {
        let Some(id) = params["approvalId"].as_str().map(str::to_owned) else {
            return;
        };
        let stage = format!("{id} {}", params["currentRequirementId"]);
        if !self.decided.insert(stage) {
            return;
        }
        self.approvals.insert(id, params.clone());
        let verdict = verdict(&self.live, &params);
        let client = self.client.clone();
        let tx = self.tx.clone();
        let session = self.session.clone();
        let request_input = self.request_input.clone();
        tokio::spawn(async move {
            let allow = match verdict {
                Verdict::Allow => true,
                Verdict::Deny => false,
                Verdict::Ask => {
                    let question = approval_question(&params);
                    // A dropped bridge degrades to a denial, never an allowance.
                    let answers = request_input(vec![question.clone()])
                        .await
                        .unwrap_or_default();
                    answers.iter().any(|a| {
                        a.question_id == question.id
                            && a.labels.iter().any(|l| l.eq_ignore_ascii_case("yes"))
                    })
                }
            };
            let Some(choice) = choice(&params, allow) else {
                let _ = tx
                    .send(Ok(AgentEvent::Error {
                        message: "Muse Code did not offer a one-time approval choice; the request was left for Muse Code to settle".into(),
                    }))
                    .await;
                return;
            };
            let decide = rpc::command(
                &client,
                "approval/decide",
                json!({
                    "sessionId": session,
                    "approvalId": params["approvalId"],
                    "choiceId": choice,
                    "requirementId": params["currentRequirementId"],
                }),
            );
            if let Err(error) = decide.await {
                tracing::warn!(target: "zuse_providers::muse", %error, "approval/decide failed");
            }
        });
    }

    /// Put a `userInput/requested` prompt to the user and answer it.
    fn question(&mut self, params: Value) {
        let Some(id) = params["userInputId"].as_str().map(str::to_owned) else {
            return;
        };
        if !self.questions.insert(id.clone()) {
            return;
        }
        let wire: Vec<Value> = params["questions"].as_array().cloned().unwrap_or_default();
        let asked: Vec<UserInputQuestion> = wire
            .iter()
            .map(|q| UserInputQuestion {
                approval_prompt: None,
                approval: None,
                id: uuid::Uuid::new_v4().to_string(),
                header: q["header"].as_str().unwrap_or("Muse Code").to_owned(),
                question: q["question"].as_str().unwrap_or_default().to_owned(),
                options: q["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|o| o["label"].as_str().map(str::to_owned))
                    .collect(),
                multi_select: q["selection"]["mode"] == "multiple",
                prefill: None,
                multiline: false,
            })
            .collect();
        let client = self.client.clone();
        let session = self.session.clone();
        let request_input = self.request_input.clone();
        tokio::spawn(async move {
            let answers = request_input(asked.clone()).await.unwrap_or_default();
            let mut wire_answers = vec![];
            for (question, asked) in wire.iter().zip(&asked) {
                let Some(answer) = answers.iter().find(|a| a.question_id == asked.id) else {
                    break;
                };
                if answer.labels.is_empty() {
                    break;
                }
                let mut entry = json!({ "questionId": question["id"] });
                let known = |label: &String| asked.options.contains(label);
                if asked.multi_select && answer.labels.iter().all(known) {
                    entry["selectedLabels"] = json!(answer.labels);
                } else if !asked.multi_select && known(&answer.labels[0]) {
                    entry["selectedLabel"] = json!(answer.labels[0]);
                } else {
                    entry["freeText"] = json!(answer.labels.join(", "));
                }
                wire_answers.push(entry);
            }
            let reply = if wire_answers.len() == wire.len() && !wire.is_empty() {
                rpc::command(
                    &client,
                    "userInput/answer",
                    json!({ "sessionId": session, "userInputId": id, "answers": wire_answers }),
                )
            } else {
                rpc::command(
                    &client,
                    "userInput/cancel",
                    json!({ "sessionId": session, "userInputId": id }),
                )
            };
            if let Err(error) = reply.await {
                tracing::warn!(target: "zuse_providers::muse", %error, "user input reply failed");
            }
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Allow,
    Deny,
    Ask,
}

/// Zuse's access policy for one Muse approval: plan-mode sessions only ever
/// consider reads, full access with auto-approve allows everything but
/// protected writes, and everything else asks the user.
fn verdict(request: &RunRequest, approval: &Value) -> Verdict {
    let subject = &approval["subject"];
    let read = subject["kind"] == "fileAccess"
        && subject["access"]
            .as_str()
            .is_some_and(|a| a.eq_ignore_ascii_case("read"));
    if request.plan {
        return if read { Verdict::Ask } else { Verdict::Deny };
    }
    if request.auto_approves_tools() && approval["protectedWrite"] != true {
        return Verdict::Allow;
    }
    Verdict::Ask
}

/// The offered once-only choice for the decision; never a broader scope.
fn choice(approval: &Value, allow: bool) -> Option<String> {
    let wanted = if allow { "approved" } else { "denied" };
    approval["availableChoices"]
        .as_array()?
        .iter()
        .find(|c| c["scope"] == "once" && c["decision"] == wanted)
        .and_then(|c| c["choiceId"].as_str())
        .map(str::to_owned)
}

fn approval_question(approval: &Value) -> UserInputQuestion {
    let subject = &approval["subject"];
    let tool = approval["toolName"]
        .as_str()
        .or_else(|| subject["toolName"].as_str())
        .unwrap_or("a tool");
    let (header, question) = match subject["kind"].as_str() {
        Some("shell") => (
            "Approve command",
            match subject["command"].as_str().filter(|c| !c.is_empty()) {
                Some(command) => format!("Muse Code wants to run `{command}`. Allow it?"),
                None => "Muse Code wants to run a command. Allow it?".into(),
            },
        ),
        Some("fileAccess") => (
            "Approve file access",
            format!(
                "Muse Code wants to {} {}. Allow it?",
                subject["access"].as_str().unwrap_or("access"),
                subject["path"].as_str().unwrap_or("a file")
            ),
        ),
        Some("network") => (
            "Approve network access",
            format!(
                "Muse Code wants to connect to {}. Allow it?",
                subject["host"]
                    .as_str()
                    .or_else(|| subject["target"].as_str())
                    .unwrap_or("the network")
            ),
        ),
        _ => {
            let args = approval["rawArgs"].as_str().unwrap_or_default();
            let args: String = args.chars().take(300).collect();
            (
                "Approve tool",
                if args.is_empty() || args == "{}" {
                    format!("Muse Code wants to use {tool}. Allow it?")
                } else {
                    format!("Muse Code wants to use {tool} with {args}. Allow it?")
                },
            )
        }
    };
    let question = if approval["protectedWrite"] == true {
        format!("{question} This writes to a protected path.")
    } else {
        question
    };
    UserInputQuestion {
        approval_prompt: None,
        approval: None,
        id: uuid::Uuid::new_v4().to_string(),
        header: header.into(),
        question,
        options: vec!["Yes".into(), "No".into()],
        prefill: None,
        multiline: false,
        multi_select: false,
    }
}

/// Inline attachments as image parts, best-effort: the path refs already
/// ride the prompt text, so an unreadable, oversized or non-image file must
/// not fail the turn.
async fn load_images(paths: &[String]) -> Vec<Value> {
    use base64::Engine;
    let mut images = vec![];
    for path in paths {
        match tokio::fs::metadata(path).await {
            Ok(meta) if meta.len() <= 20 * 1024 * 1024 => {}
            _ => continue,
        }
        let Ok(bytes) = tokio::fs::read(path).await else {
            continue;
        };
        let mime = if bytes.starts_with(b"\x89PNG") {
            "image/png"
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            "image/jpeg"
        } else if bytes.starts_with(b"GIF8") {
            "image/gif"
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            "image/webp"
        } else {
            tracing::debug!(target: "zuse_providers::muse", %path, "attachment is not an inlinable image; path ref only");
            continue;
        };
        images.push(json!({
            "type": "image",
            "base64Data": base64::engine::general_purpose::STANDARD.encode(bytes),
            "mediaType": mime,
        }));
    }
    images
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(sandbox: &str, auto: bool) -> RunRequest {
        let plan = sandbox == "plan";
        let sandbox = if plan { "danger-full-access" } else { sandbox };
        serde_json::from_value(
            json!({"prompt":"x","cwd":"/tmp","sandbox":sandbox,"autoApprove":auto,"plan":plan}),
        )
        .unwrap()
    }

    fn approval(kind: &str, protected: bool) -> Value {
        json!({
            "approvalId": "a",
            "currentRequirementId": {"approvalId": "a", "sourceIndex": 0},
            "protectedWrite": protected,
            "subject": {"kind": kind, "access": "read", "command": "ls"},
            "availableChoices": [
                {"choiceId": "always", "decision": "approved", "scope": "localPersistent", "label": "Always"},
                {"choiceId": "yes", "decision": "approved", "scope": "once", "label": "Allow"},
                {"choiceId": "no", "decision": "denied", "scope": "once", "label": "Deny"},
            ],
        })
    }

    #[test]
    fn policy_follows_the_access_level() {
        let shell = approval("shell", false);
        assert_eq!(verdict(&request("plan", true), &shell), Verdict::Deny);
        assert_eq!(
            verdict(&request("plan", false), &approval("fileAccess", false)),
            Verdict::Ask
        );
        assert_eq!(
            verdict(&request("workspace-write", true), &shell),
            Verdict::Ask
        );
        assert_eq!(
            verdict(&request("danger-full-access", true), &shell),
            Verdict::Allow
        );
        assert_eq!(
            verdict(
                &request("danger-full-access", true),
                &approval("shell", true)
            ),
            Verdict::Ask
        );
    }

    #[test]
    fn only_offered_once_choices_are_selected() {
        let offered = approval("shell", false);
        assert_eq!(choice(&offered, true).as_deref(), Some("yes"));
        assert_eq!(choice(&offered, false).as_deref(), Some("no"));
        let broad_only = json!({"availableChoices": [
            {"choiceId": "always", "decision": "approved", "scope": "localPersistent"}
        ]});
        assert_eq!(choice(&broad_only, true), None);
        assert_eq!(choice(&broad_only, false), None);
    }

    #[test]
    fn approval_questions_name_the_command_and_protected_writes() {
        let question = approval_question(&approval("shell", true));
        assert!(question.question.contains("`ls`"));
        assert!(question.question.contains("protected"));
        assert_eq!(question.options, ["Yes", "No"]);
    }

    #[test]
    fn zuse_mcp_rides_session_config_as_an_optional_stdio_server() {
        let mut request = request("workspace-write", false);
        assert!(session_config(&request).is_none());
        request.mcp = Some(zuse_protocol::McpServer {
            name: "zuse".into(),
            command: "/bin/zuse".into(),
            args: vec!["mcp".into()],
            env: [("ZUSE_CHAT".to_owned(), "c".to_owned())].into(),
        });
        let config = session_config(&request).unwrap();
        assert_eq!(
            config["mcpServers"]["zuse"],
            json!({"transport":"stdio","command":"/bin/zuse","args":["mcp"],"env":{"ZUSE_CHAT":"c"},"mode":"optional"})
        );
    }
}
