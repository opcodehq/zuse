//! One owner for native configuration watches, coalesced writes and uncertainty.
use crate::state::AppState;
use gpui::{Context, Task};
use std::{collections::HashMap, time::Duration};
use zuse_protocol::{Chat, ChatConfig, ChatConfigPatch};
use zuse_rpc::{ChatConfigTarget, RpcClient, RpcError, methods};

struct Pending {
    id: uuid::Uuid,
    target: ChatConfigTarget,
    queued: ChatConfigPatch,
    fallback: ChatConfig,
    projection: ChatConfig,
}

#[derive(Default)]
pub(crate) struct ChatConfiguration {
    target: Option<ChatConfigTarget>,
    confirmed: Option<Option<ChatConfig>>,
    clear_notice_on_confirm: bool,
    generation: uuid::Uuid,
    watch: Option<Task<()>>,
    pending: HashMap<String, Pending>,
    tasks: HashMap<String, Task<()>>,
    uncertain: HashMap<String, String>,
    notice: Option<(String, String)>,
    // A failed read is not a failed Send or an uncertain configuration write.
    // A later authoritative snapshot can clear it without replaying a mutation.
    watch_notice: Option<(String, String)>,
}

fn configuration_watch_notice(error: Option<&RpcError>) -> Option<String> {
    match error {
        // The stable client reopens reads when its transport returns. Keep
        // choices gated meanwhile; the conversation owns reconnect progress.
        None | Some(RpcError::Closed | RpcError::Transport(_)) => None,
        Some(error) => Some(format!(
            "Could not read saved chat choices: {error}. Refresh access before sending."
        )),
    }
}

struct Outcome {
    snapshot: Option<Option<ChatConfig>>,
    error: Option<String>,
}

async fn save(
    client: &RpcClient,
    target: &ChatConfigTarget,
    patch: ChatConfigPatch,
    fallback: ChatConfig,
) -> Outcome {
    let before = match client.read_chat_config(target).await {
        Ok(config) => config,
        Err(error) => {
            return Outcome {
                snapshot: None,
                error: Some(error.to_string()),
            };
        }
    };
    if patch.is_empty() {
        return Outcome {
            snapshot: Some(before),
            error: None,
        };
    }
    let desired = patch.apply_to(before.as_ref().unwrap_or(&fallback));
    if before.as_ref() == Some(&desired) {
        return Outcome {
            snapshot: Some(before),
            error: None,
        };
    }
    let error = client
        .compare_set_chat_config(target, &before, &desired)
        .await
        .err()
        .map(|error| error.to_string());
    // Even a lost acknowledgement may follow a committed write. Read once,
    // never replay the write or the choices waiting behind it on an error.
    match client.read_chat_config(target).await {
        Ok(config) => Outcome {
            snapshot: Some(config),
            error,
        },
        Err(read) => Outcome {
            snapshot: None,
            error: Some(match error {
                Some(write) => format!("{write}. Could not read saved choices: {read}"),
                None => format!("Could not confirm saved choices: {read}"),
            }),
        },
    }
}

impl AppState {
    fn configuration_target(&self) -> Option<ChatConfigTarget> {
        if self.side_chat_unsaved() {
            return None;
        }
        let chat = self.selected_chat_row()?;
        Some(ChatConfigTarget {
            chat_id: chat.id.clone(),
            device_id: chat.device_id.clone(),
        })
    }

    /// Treat the selected chat's saved choices as already read back, as the
    /// configuration watch does once it hears from the host.
    #[cfg(test)]
    pub(crate) fn confirm_selected_chat_configuration_for_test(&mut self) {
        let target = self.configuration_target();
        let config = self
            .selected_chat_row()
            .and_then(|chat| chat.config.clone());
        self.chat_configuration.target = target;
        self.chat_configuration.confirmed = Some(config);
    }

    pub(crate) fn configuration_saving(&self) -> bool {
        self.selected_chat
            .as_ref()
            .is_some_and(|id| self.chat_configuration.pending.contains_key(id))
    }

    pub(crate) fn configuration_blocked(&self) -> bool {
        if self.selected_chat.is_none() || self.side_chat_unsaved() {
            return false;
        }
        let Some(target) = self.configuration_target() else {
            return true;
        };
        self.chat_configuration.target.as_ref() != Some(&target)
            || self.chat_configuration.confirmed.is_none()
            || self
                .chat_configuration
                .pending
                .contains_key(&target.chat_id)
            || self.chat_configuration.uncertain.get(&target.chat_id) == Some(&target.device_id)
    }

    pub(crate) fn configuration_notice(&self) -> Option<(String, String)> {
        self.chat_configuration
            .notice
            .as_ref()
            .filter(|(id, _)| self.selected_chat.as_ref() == Some(id))
            .or_else(|| {
                self.chat_configuration
                    .watch_notice
                    .as_ref()
                    .filter(|(id, _)| self.selected_chat.as_ref() == Some(id))
            })
            .cloned()
    }

    /// Replicated lists cannot replace a selected host's confirmed snapshot or
    /// a user's pending projection. Other conversations keep normal replication.
    pub(crate) fn overlay_chat_configuration(&self, chats: &mut [Chat]) {
        for chat in chats {
            if let Some(pending) = self.chat_configuration.pending.get(&chat.id)
                && pending.target.device_id == chat.device_id
            {
                chat.config = Some(pending.projection.clone());
            } else if self
                .chat_configuration
                .target
                .as_ref()
                .is_some_and(|target| {
                    target.chat_id == chat.id && target.device_id == chat.device_id
                })
                && let Some(config) = &self.chat_configuration.confirmed
            {
                chat.config = config.clone();
            }
        }
    }

    pub(crate) fn reconcile_configuration_watch(&mut self, cx: &mut Context<Self>) {
        let target = self.configuration_target();
        if target == self.chat_configuration.target && self.chat_configuration.watch.is_some() {
            return;
        }
        self.chat_configuration.watch = None;
        self.chat_configuration.confirmed = None;
        self.chat_configuration.watch_notice = None;
        self.chat_configuration.target = target.clone();
        self.chat_configuration.generation = uuid::Uuid::new_v4();
        let generation = self.chat_configuration.generation;
        let (Some(target), Some(engine)) = (target, self.engine().cloned()) else {
            return;
        };
        self.chat_configuration.watch = Some(cx.spawn(async move |this, cx| {
            loop {
                let mut watch = engine
                    .client()
                    .subscribe_checked(methods::WATCH_CHAT_CONFIG, target.params())
                    .await;
                let mut error = None;
                if let Ok(watch) = &mut watch {
                    while let Some(value) = watch.recv().await {
                        match target.decode(value) {
                            Ok(config) => {
                                let alive = this.update(cx, |state, cx| {
                                    if state.chat_configuration.generation != generation
                                        || state.chat_configuration.target.as_ref() != Some(&target)
                                    {
                                        return false;
                                    }
                                    state.chat_configuration.confirmed = Some(config.clone());
                                    state.chat_configuration.watch_notice = None;
                                    state.chat_configuration.uncertain.remove(&target.chat_id);
                                    if state.chat_configuration.clear_notice_on_confirm {
                                        if state
                                            .chat_configuration
                                            .notice
                                            .as_ref()
                                            .is_some_and(|(id, _)| id == &target.chat_id)
                                        {
                                            state.chat_configuration.notice = None;
                                        }
                                        state.chat_configuration.clear_notice_on_confirm = false;
                                    }
                                    let projection = state
                                        .chat_configuration
                                        .pending
                                        .get(&target.chat_id)
                                        .map(|pending| pending.projection.clone());
                                    state.stamp_configuration(
                                        &target,
                                        projection.map(Some).unwrap_or(config),
                                    );
                                    cx.notify();
                                    true
                                });
                                if !matches!(alive, Ok(true)) {
                                    return;
                                }
                            }
                            Err(err) => {
                                error = Some(err);
                                break;
                            }
                        }
                    }
                } else if let Err(err) = watch {
                    error = Some(err);
                }
                let alive = this.update(cx, |state, cx| {
                    if state.chat_configuration.generation != generation
                        || state.chat_configuration.target.as_ref() != Some(&target)
                    {
                        return false;
                    }
                    state.chat_configuration.confirmed = None;
                    if let Some(notice) = configuration_watch_notice(error.as_ref()) {
                        state.chat_configuration.watch_notice =
                            Some((target.chat_id.clone(), notice));
                    }
                    cx.notify();
                    true
                });
                if !matches!(alive, Ok(true)) {
                    return;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        }));
    }

    fn stamp_configuration(&mut self, target: &ChatConfigTarget, config: Option<ChatConfig>) {
        if let Some(chat) = self
            .chats
            .iter_mut()
            .find(|chat| chat.id == target.chat_id && chat.device_id == target.device_id)
        {
            chat.config = config;
        }
    }

    pub(crate) fn stage_chat_configuration(
        &mut self,
        patch: ChatConfigPatch,
        fallback: ChatConfig,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(chat_id) = self.selected_chat.clone() else {
            return false;
        };
        if self.side_chat_unsaved() {
            let config = patch.apply_to(
                self.selected_chat_row()
                    .and_then(|chat| chat.config.as_ref())
                    .unwrap_or(&fallback),
            );
            self.apply_chat_config(&chat_id, config);
            cx.notify();
            return true;
        }
        let Some(target) = self.configuration_target() else {
            return false;
        };
        if let Some(pending) = self.chat_configuration.pending.get_mut(&chat_id) {
            if pending.target != target {
                self.chat_configuration.notice = Some((chat_id, "Wait for the previous host’s configuration request to finish before choosing again.".into()));
                cx.notify();
                return false;
            }
            pending.projection = patch.apply_to(&pending.projection);
            pending.queued.merge(patch);
            let config = pending.projection.clone();
            self.apply_chat_config(&chat_id, config);
            cx.notify();
            return true;
        }
        if self.engine().is_none() || self.configuration_blocked() {
            self.chat_configuration.notice = Some((chat_id, "Connect to the execution host and refresh saved chat choices before changing them.".into()));
            cx.notify();
            return false;
        }
        let projection = patch.apply_to(
            self.selected_chat_row()
                .and_then(|chat| chat.config.as_ref())
                .unwrap_or(&fallback),
        );
        if self
            .chat_configuration
            .notice
            .as_ref()
            .is_some_and(|(id, _)| id == &chat_id)
        {
            self.chat_configuration.notice = None;
        }
        self.chat_configuration.pending.insert(
            chat_id.clone(),
            Pending {
                id: uuid::Uuid::new_v4(),
                target,
                queued: patch,
                fallback,
                projection: projection.clone(),
            },
        );
        self.apply_chat_config(&chat_id, projection);
        self.pump_configuration(chat_id, cx);
        cx.notify();
        true
    }

    fn pump_configuration(&mut self, chat_id: String, cx: &mut Context<Self>) {
        let Some(engine) = self.engine().cloned() else {
            return;
        };
        let pending = self.chat_configuration.pending.get_mut(&chat_id).unwrap();
        let id = pending.id;
        let target = pending.target.clone();
        let patch = std::mem::take(&mut pending.queued);
        let fallback = pending.fallback.clone();
        let key = chat_id.clone();
        let task = cx.spawn(async move |this, cx| {
            let operation = save(engine.client(), &target, patch, fallback);
            let timeout = cx.background_executor().timer(Duration::from_secs(15));
            let result = match futures::future::select(Box::pin(operation), Box::pin(timeout)).await {
                futures::future::Either::Left((outcome, _)) => outcome,
                futures::future::Either::Right(_) => Outcome { snapshot: None, error: Some("Timed out confirming the configuration. Refresh saved choices; no write will be replayed.".into()) },
            };
            let _ = this.update(cx, |state, cx| {
                if !state.chat_configuration.pending.get(&chat_id).is_some_and(|pending| pending.id == id) { return; }
                let mut pending = state.chat_configuration.pending.remove(&chat_id).unwrap();
                state.chat_configuration.tasks.remove(&chat_id);
                let same_host = state.chats.iter().any(|chat| chat.id == chat_id && chat.device_id == target.device_id);
                if let Some(config) = &result.snapshot {
                    if state.chat_configuration.target.as_ref() == Some(&target) {
                        state.chat_configuration.confirmed = Some(config.clone());
                    }
                    state.chat_configuration.uncertain.remove(&chat_id);
                    state.stamp_configuration(&target, config.clone());
                } else {
                    state.chat_configuration.uncertain.insert(chat_id.clone(), target.device_id.clone());
                }
                if let Some(error) = result.error {
                    state.chat_configuration.notice = Some((chat_id, format!("The chat choices were not confirmed: {error}. Review the saved settings before choosing again.")));
                } else if !same_host {
                    state.chat_configuration.notice = Some((chat_id, "The conversation moved or was removed. Review its current host before changing choices.".into()));
                } else if !pending.queued.is_empty() {
                    let canonical = result.snapshot.as_ref().and_then(|config| config.as_ref()).unwrap_or(&pending.fallback);
                    pending.projection = pending.queued.apply_to(canonical);
                    pending.fallback = canonical.clone();
                    state.apply_chat_config(&chat_id, pending.projection.clone());
                    state.chat_configuration.pending.insert(chat_id.clone(), pending);
                    state.pump_configuration(chat_id, cx);
                } else if state.chat_configuration.notice.as_ref().is_some_and(|(id, _)| id == &chat_id) {
                    state.chat_configuration.notice = None;
                }
                cx.notify();
            });
        });
        self.chat_configuration.tasks.insert(key, task);
    }

    pub(crate) fn reset_configuration_transport(&mut self) {
        self.chat_configuration.watch = None;
        self.chat_configuration.confirmed = None;
        self.chat_configuration.target = None;
        self.chat_configuration.generation = uuid::Uuid::new_v4();
        self.chat_configuration.clear_notice_on_confirm = false;
        self.chat_configuration.watch_notice = None;
        self.chat_configuration.tasks.clear();
        for (_, pending) in std::mem::take(&mut self.chat_configuration.pending) {
            let id = pending.target.chat_id;
            self.chat_configuration
                .uncertain
                .insert(id.clone(), pending.target.device_id);
            if self.chat_configuration.notice.is_none() || self.selected_chat.as_ref() == Some(&id)
            {
                self.chat_configuration.notice = Some((id, "The connection changed while chat choices were saving. Review the host's saved settings; pending choices will not be replayed.".into()));
            }
        }
    }

    pub(crate) fn refresh_selected_configuration(&mut self, cx: &mut Context<Self>) {
        // Reopening the read stream is safe even when a previous write's
        // acknowledgement was lost. The mutation itself is never retried.
        self.chat_configuration.watch = None;
        self.chat_configuration.clear_notice_on_confirm = true;
        self.reconcile_configuration_watch(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use gpui::AppContext;
    use serde_json::{Value, json};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use zuse_protocol::{HarnessId, SandboxLevel};
    use zuse_rpc::{RpcError, RpcReply, RpcService};

    struct Host {
        config: Mutex<ChatConfig>,
        snapshots: tokio::sync::watch::Sender<Value>,
        writes: AtomicUsize,
        gate: tokio::sync::Notify,
        bad_receipt: bool,
    }
    fn config(model: &str, sandbox: SandboxLevel) -> ChatConfig {
        ChatConfig {
            plan: false,
            harness: HarnessId::Codex,
            model: Some(model.into()),
            reasoning: None,
            model_options: Default::default(),
            sandbox,
        }
    }
    fn snapshot(config: &ChatConfig) -> Value {
        json!({"chatId":"chat","deviceId":"host","config":config})
    }
    fn row(config: &ChatConfig) -> Chat {
        serde_json::from_value(json!({"id":"chat","deviceId":"host","createdAt":"2026-01-01T00:00:00Z","archived":false,"config":config})).unwrap()
    }
    impl Host {
        fn new(bad_receipt: bool) -> Self {
            let config = config("original", SandboxLevel::WorkspaceWrite);
            let (snapshots, _) = tokio::sync::watch::channel(snapshot(&config));
            Self {
                config: Mutex::new(config),
                snapshots,
                writes: 0.into(),
                gate: Default::default(),
                bad_receipt,
            }
        }
    }
    #[async_trait::async_trait]
    impl RpcService for Host {
        async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
            if method == methods::WATCH_CHAT_CONFIG {
                assert_eq!(params["chatId"], "chat");
                assert_eq!(params["targetDeviceId"], "host");
                let rx = self.snapshots.subscribe();
                return Ok(RpcReply::Stream(
                    futures::stream::unfold((rx, false), |(mut rx, emitted)| async move {
                        if emitted {
                            rx.changed().await.ok()?;
                        }
                        let value = rx.borrow_and_update().clone();
                        Some((value, (rx, true)))
                    })
                    .boxed(),
                ));
            }
            if method != methods::MUTATE {
                return Err(RpcError::UnknownMethod(method.into()));
            }
            if params["op"] != "setChatConfig" {
                return Ok(RpcReply::Value(json!({"ok":true})));
            }
            assert_eq!(params["targetDeviceId"], "host");
            assert_eq!(params["expectedConfig"]["deviceId"], "host");
            let index = self.writes.fetch_add(1, Ordering::SeqCst);
            if index == 0 {
                self.gate.notified().await;
            }
            let mut current = self.config.lock().unwrap();
            assert_eq!(
                params["expectedConfig"]["config"],
                serde_json::to_value(&*current).unwrap()
            );
            *current = serde_json::from_value(params["config"].clone()).unwrap();
            self.snapshots.send_replace(snapshot(&current));
            Ok(RpcReply::Value(if self.bad_receipt {
                json!({})
            } else {
                json!({"ok":true})
            }))
        }
    }
    fn settle(
        cx: &gpui::TestAppContext,
        runtime: &tokio::runtime::Runtime,
        predicate: impl Fn() -> bool,
    ) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            // The deterministic GPUI scheduler must be woken on its test
            // thread. Drive the real RPC tasks cooperatively on that thread.
            runtime.block_on(tokio::task::yield_now());
            if predicate() {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "configuration did not settle"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn state(cx: &gpui::TestAppContext, host: Arc<Host>) -> gpui::Entity<AppState> {
        cx.update(|cx| {
            let state = cx.new(|_| AppState::new());
            state.update(cx, |state, cx| {
                state.chats = vec![row(&host.config.lock().unwrap())];
                state.selected_chat = Some("chat".into());
                state.set_test_engine(crate::state::EngineHandle::from_test_client(
                    zuse_rpc::memory_client(host),
                ));
                state.reconcile_configuration_watch(cx);
            });
            state
        })
    }
    #[gpui::test]
    fn native_config_coalesces_across_navigation_and_preserves_owner_snapshot(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _runtime = runtime.enter();
        let host = Arc::new(Host::new(false));
        let state = state(cx, host.clone());
        settle(cx, &runtime, || {
            cx.read(|cx| !state.read(cx).configuration_blocked())
        });
        let first = config("first", SandboxLevel::WorkspaceWrite);
        cx.update(|cx| {
            state.update(cx, |state, cx| {
                assert!(state.stage_chat_configuration(
                    ChatConfigPatch::model(first.clone()),
                    first.clone(),
                    cx
                ))
            })
        });
        settle(cx, &runtime, || host.writes.load(Ordering::SeqCst) == 1);
        cx.update(|cx| {
            state.update(cx, |state, cx| {
                assert!(state.configuration_blocked());
                assert!(state.stage_chat_configuration(
                    ChatConfigPatch::sandbox(SandboxLevel::Auto),
                    first.clone(),
                    cx
                ));
                assert!(state.stage_chat_configuration(
                    ChatConfigPatch::plan(true),
                    first.clone(),
                    cx
                ));
                let last = config("last", SandboxLevel::DangerFullAccess);
                assert!(state.stage_chat_configuration(
                    ChatConfigPatch::model(last.clone()),
                    last,
                    cx
                ));
                state.apply_chats(vec![row(&config("replica", SandboxLevel::WorkspaceWrite))]);
                let projected = state.selected_chat_row().unwrap().config.as_ref().unwrap();
                assert_eq!(projected.model.as_deref(), Some("last"));
                assert_eq!(projected.sandbox, SandboxLevel::Auto);
                assert!(projected.plan);
                state.select_chat(None, cx);
            })
        });
        host.gate.notify_one();
        settle(cx, &runtime, || {
            cx.read(|cx| state.read(cx).chat_configuration.pending.is_empty())
        });
        assert_eq!(host.writes.load(Ordering::SeqCst), 2);
        let mut planned = config("last", SandboxLevel::Auto);
        planned.plan = true;
        assert_eq!(*host.config.lock().unwrap(), planned);
        cx.update(|cx| state.update(cx, |state, cx| state.select_chat(Some("chat".into()), cx)));
        settle(cx, &runtime, || {
            cx.read(|cx| !state.read(cx).configuration_blocked())
        });
        cx.update(|cx| {
            state.update(cx, |state, _| {
                state.apply_chats(vec![row(&config("replica", SandboxLevel::WorkspaceWrite))]);
                assert_eq!(
                    state.selected_chat_row().unwrap().config.as_ref().unwrap(),
                    &planned
                );
            })
        });
    }
    #[gpui::test]
    fn native_config_bad_receipt_refreshes_without_replaying_queued_permission(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _runtime = runtime.enter();
        let host = Arc::new(Host::new(true));
        let state = state(cx, host.clone());
        settle(cx, &runtime, || {
            cx.read(|cx| !state.read(cx).configuration_blocked())
        });
        let desired = config("committed", SandboxLevel::WorkspaceWrite);
        cx.update(|cx| {
            state.update(cx, |state, cx| {
                assert!(state.stage_chat_configuration(
                    ChatConfigPatch::model(desired.clone()),
                    desired.clone(),
                    cx
                ))
            })
        });
        settle(cx, &runtime, || host.writes.load(Ordering::SeqCst) == 1);
        cx.update(|cx| {
            state.update(cx, |state, cx| {
                assert!(state.stage_chat_configuration(
                    ChatConfigPatch::sandbox(SandboxLevel::DangerFullAccess),
                    desired.clone(),
                    cx
                ))
            })
        });
        host.gate.notify_one();
        settle(cx, &runtime, || {
            cx.read(|cx| !state.read(cx).configuration_blocked())
        });
        assert_eq!(host.writes.load(Ordering::SeqCst), 1);
        assert_eq!(*host.config.lock().unwrap(), desired);
        cx.read(|cx| {
            let state = state.read(cx);
            assert!(state.configuration_notice().is_some());
            assert_eq!(
                state.selected_chat_row().unwrap().config.as_ref().unwrap(),
                &desired
            );
        });
        cx.update(|cx| state.update(cx, |state, cx| state.refresh_selected_configuration(cx)));
        settle(cx, &runtime, || {
            cx.read(|cx| {
                !state.read(cx).configuration_blocked()
                    && state.read(cx).configuration_notice().is_none()
            })
        });
        assert_eq!(host.writes.load(Ordering::SeqCst), 1);
    }
    #[gpui::test]
    fn native_config_transport_replacement_ignores_late_receipts_and_discards_unsent_choices(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _runtime = runtime.enter();
        let host = Arc::new(Host::new(false));
        let state = state(cx, host.clone());
        settle(cx, &runtime, || {
            cx.read(|cx| !state.read(cx).configuration_blocked())
        });
        let old_connection = cx.read(|cx| state.read(cx).engine().unwrap().clone());
        let desired = config("late-commit", SandboxLevel::WorkspaceWrite);
        cx.update(|cx| {
            state.update(cx, |state, cx| {
                assert!(state.stage_chat_configuration(
                    ChatConfigPatch::model(desired.clone()),
                    desired.clone(),
                    cx
                ))
            })
        });
        settle(cx, &runtime, || host.writes.load(Ordering::SeqCst) == 1);
        cx.update(|cx| {
            state.update(cx, |state, cx| {
                assert!(state.stage_chat_configuration(
                    ChatConfigPatch::sandbox(SandboxLevel::DangerFullAccess),
                    desired.clone(),
                    cx
                ));
                state.reset_configuration_transport();
                state.set_test_engine(crate::state::EngineHandle::from_test_client(
                    zuse_rpc::memory_client(host.clone()),
                ));
                state.reconcile_configuration_watch(cx);
            })
        });
        settle(cx, &runtime, || {
            cx.read(|cx| !state.read(cx).configuration_blocked())
        });
        host.gate.notify_one();
        settle(cx, &runtime, || {
            cx.read(|cx| {
                state.read(cx).selected_chat_row().unwrap().config.as_ref() == Some(&desired)
            })
        });
        assert_eq!(host.writes.load(Ordering::SeqCst), 1);
        assert_eq!(*host.config.lock().unwrap(), desired);
        assert!(!cx.read(|cx| state.read(cx).configuration_blocked()));
        drop(old_connection);
    }

    struct ReopeningHost {
        host: Host,
        first: &'static str,
        opens: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl RpcService for ReopeningHost {
        async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
            if method == methods::WATCH_CHAT_CONFIG
                && self.opens.fetch_add(1, Ordering::SeqCst) == 0
            {
                return match self.first {
                    "closed" => Ok(RpcReply::Stream(futures::stream::empty().boxed())),
                    "denied" => Err(RpcError::Capability("cloud_access_denied".into())),
                    "foreign" => Ok(RpcReply::Stream(
                        futures::stream::once(async {
                            json!({"chatId":"chat","deviceId":"another-host","config":null})
                        })
                        .boxed(),
                    )),
                    _ => unreachable!(),
                };
            }
            self.host.handle(method, params).await
        }
    }
    fn reopening_state(
        cx: &gpui::TestAppContext,
        first: &'static str,
    ) -> (gpui::Entity<AppState>, Arc<ReopeningHost>) {
        let host = Arc::new(ReopeningHost {
            host: Host::new(false),
            first,
            opens: 0.into(),
        });
        let state = cx.update(|cx| {
            let state = cx.new(|_| AppState::new());
            state.update(cx, |state, cx| {
                state.chats = vec![row(&host.host.config.lock().unwrap())];
                state.selected_chat = Some("chat".into());
                state.set_test_engine(crate::state::EngineHandle::from_test_client(
                    zuse_rpc::memory_client(host.clone()),
                ));
                state.reconcile_configuration_watch(cx);
            });
            state
        });
        (state, host)
    }

    #[test]
    fn native_config_watch_reconnects_are_not_send_failures_but_real_errors_are_actionable() {
        for error in [
            None,
            Some(RpcError::Closed),
            Some(RpcError::Transport("connection changed".into())),
        ] {
            assert!(configuration_watch_notice(error.as_ref()).is_none());
        }
        for error in [
            RpcError::Capability("cloud_access_denied".into()),
            RpcError::UnknownMethod("WatchChatConfig".into()),
            RpcError::Failed("The execution host changed".into()),
        ] {
            assert!(
                configuration_watch_notice(Some(&error))
                    .unwrap()
                    .contains("Refresh access")
            );
        }
    }

    #[gpui::test]
    fn native_config_closed_read_recovers_without_send_failure_or_any_mutation(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _runtime = runtime.enter();
        let (state, host) = reopening_state(cx, "closed");
        settle(cx, &runtime, || host.opens.load(Ordering::SeqCst) == 1);
        // Drive the real memory RPC's stream closure and the existing safe
        // read retry; no user action or configuration write is submitted.
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(tokio::task::yield_now());
            assert!(cx.read(|cx| state.read(cx).configuration_notice().is_none()));
            cx.executor().advance_clock(Duration::from_millis(250));
        }
        settle(cx, &runtime, || {
            cx.read(|cx| !state.read(cx).configuration_blocked())
        });
        assert_eq!(host.opens.load(Ordering::SeqCst), 2);
        assert_eq!(host.host.writes.load(Ordering::SeqCst), 0);
        cx.read(|cx| {
            let state = state.read(cx);
            assert_eq!(state.selected_chat.as_deref(), Some("chat"));
            assert!(state.configuration_notice().is_none());
            assert_eq!(
                state.selected_chat_row().unwrap().config.as_ref(),
                Some(&config("original", SandboxLevel::WorkspaceWrite))
            );
        });
    }

    #[gpui::test]
    fn native_config_recovered_snapshot_clears_read_error_without_manual_refresh(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _runtime = runtime.enter();
        let (state, host) = reopening_state(cx, "denied");
        settle(cx, &runtime, || {
            cx.read(|cx| state.read(cx).configuration_notice().is_some())
        });
        assert!(cx.read(|cx| state.read(cx).configuration_blocked()));
        // Recovery of this read must not clear an unrelated uncertain write.
        cx.update(|cx| {
            state.update(cx, |state, _| {
                state.chat_configuration.notice = Some((
                    "chat".into(),
                    "An earlier write is unconfirmed; review saved choices.".into(),
                ));
            })
        });
        cx.executor().advance_clock(Duration::from_secs(2));
        settle(cx, &runtime, || {
            cx.read(|cx| !state.read(cx).configuration_blocked())
        });
        cx.read(|cx| {
            let state = state.read(cx);
            assert!(state.chat_configuration.watch_notice.is_none());
            assert!(
                state
                    .configuration_notice()
                    .unwrap()
                    .1
                    .contains("earlier write is unconfirmed")
            );
        });
        assert_eq!(host.host.writes.load(Ordering::SeqCst), 0);
        assert_eq!(host.opens.load(Ordering::SeqCst), 2);
    }

    #[gpui::test]
    fn native_config_foreign_host_read_stays_blocked_and_actionable(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _runtime = runtime.enter();
        let (state, host) = reopening_state(cx, "foreign");
        settle(cx, &runtime, || {
            cx.read(|cx| state.read(cx).configuration_notice().is_some())
        });
        cx.read(|cx| {
            let state = state.read(cx);
            assert!(state.configuration_blocked());
            assert!(state.chat_configuration.confirmed.is_none());
            assert!(
                state
                    .configuration_notice()
                    .unwrap()
                    .1
                    .contains("host changed")
            );
            assert_eq!(state.selected_chat.as_deref(), Some("chat"));
            assert_eq!(state.selected_chat_row().unwrap().device_id, "host");
        });
        assert_eq!(host.host.writes.load(Ordering::SeqCst), 0);
    }
}
