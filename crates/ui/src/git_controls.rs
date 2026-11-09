//! Native transport for the same Git controls used by the browser.
use crate::{
    state::{AppState, EngineHandle},
    theme::Theme,
};
use gpui::{prelude::*, *};
use zuse_viewport::{
    git_controls::{GitControls, Request, Target},
    plugin_setup::SetupColors,
};
pub struct GitPanel {
    state: Entity<AppState>,
    engine: Option<EngineHandle>,
    panel: Option<Entity<GitControls>>,
    events: Option<Subscription>,
    action: Option<Task<()>>,
    watch: Option<Task<()>>,
    active: bool,
    #[cfg(feature = "browser-fixture")]
    fixture: Option<(Target, Option<serde_json::Value>)>,
    _state: Subscription,
}
impl GitPanel {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            engine: None,
            panel: None,
            events: None,
            action: None,
            watch: None,
            active: true,
            #[cfg(feature = "browser-fixture")]
            fixture: None,
            _state: observe,
        }
    }
    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if !active {
            self.watch = None;
        } else if let Some(panel) = &self.panel {
            panel.update(cx, |panel, cx| panel.watch_current(cx));
        }
    }
    /// Show a canned status frame without an engine, for visual review.
    #[cfg(feature = "browser-fixture")]
    pub(crate) fn fixture_seed(&mut self, target: Target, frame: serde_json::Value) {
        self.fixture = Some((target, Some(frame)));
    }
    fn request(&mut self, event: Request, cx: &mut Context<Self>) {
        if event.method == zuse_rpc::methods::WATCH_GIT_WORKSPACE && !self.active {
            return;
        }
        let Some(engine) = self.engine.clone() else {
            return;
        };
        let Some(panel) = self.panel.clone() else {
            return;
        };
        if event.method == zuse_rpc::methods::WATCH_GIT_WORKSPACE {
            self.watch = None;
            if event.params["chatId"].is_null() {
                return;
            }
            self.watch = Some(cx.spawn(async move |_, cx| {
                loop {
                    match engine
                        .client()
                        .subscribe_checked(event.method, event.params.clone())
                        .await
                    {
                        Ok(mut stream) => {
                            while let Some(value) = stream.recv().await {
                                if let Ok(frame) = serde_json::from_value(value) {
                                    panel.update(cx, |panel, cx| panel.observe(frame, cx));
                                }
                            }
                        }
                        Err(error) => {
                            let terminal = !matches!(
                                error,
                                zuse_rpc::RpcError::Transport(_) | zuse_rpc::RpcError::Closed
                            );
                            let message = match error {
                                zuse_rpc::RpcError::UnknownMethod(_) => {
                                    "Update Zuse on this device to receive Git status.".into()
                                }
                                zuse_rpc::RpcError::Failed(message)
                                | zuse_rpc::RpcError::BadParams(message) => message,
                                zuse_rpc::RpcError::Capability(_) => {
                                    "Git status is unavailable for this checkout.".into()
                                }
                                _ => "Git status disconnected. Reconnecting…".into(),
                            };
                            let _ =
                                panel.update(cx, |panel, cx| panel.observation_error(message, cx));
                            if terminal {
                                return;
                            }
                        }
                    }
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(2))
                        .await;
                }
            }));
            return;
        }
        self.action = Some(cx.spawn(async move |_, cx| {
            let result = engine
                .client()
                .call(event.method, event.params)
                .await
                .map_err(|e| e.to_string());
            let _ = panel.update(cx, |panel, cx| panel.respond(event.id, result, cx));
        }));
    }
}
impl Render for GitPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let colors = SetupColors {
            text: theme.text,
            muted: theme.text_muted,
            background: theme.panel_bg(),
            border: theme.border,
            danger: theme.danger,
        };
        if self.panel.is_none() {
            let panel = cx.new(|cx| GitControls::new(colors, window, cx));
            self.events = Some(cx.subscribe(&panel, |this, _, event: &Request, cx| {
                this.request(event.clone(), cx)
            }));
            self.panel = Some(panel);
        }
        let engine = self.state.read(cx).engine().cloned();
        let same = match (&self.engine, &engine) {
            (Some(a), Some(b)) => a.same_connection(b),
            (None, None) => true,
            _ => false,
        };
        let panel = self.panel.as_ref().unwrap().clone();
        if !same {
            self.action = None;
            self.watch = None;
            self.engine = engine;
            panel.update(cx, |panel, cx| {
                panel.disconnected(cx);
                panel.set_target(None, cx);
            });
        }
        let target = self
            .engine
            .as_ref()
            .and_then(|_| self.state.read(cx).selected_chat_row())
            .and_then(|chat| {
                chat.cwd.as_ref().map(|cwd| Target {
                    chat_id: chat.id.clone(),
                    cwd: cwd.clone(),
                    device_id: chat.device_id.clone(),
                    archived: chat.archived,
                })
            });
        #[cfg(feature = "browser-fixture")]
        let target = self
            .fixture
            .as_ref()
            .map(|(target, _)| target.clone())
            .or(target);
        panel.update(cx, |panel, cx| {
            panel.colors = colors;
            panel.set_target(target, cx);
        });
        #[cfg(feature = "browser-fixture")]
        if let Some(frame) = self
            .fixture
            .as_mut()
            .and_then(|(_, frame)| frame.take())
            .and_then(|frame| serde_json::from_value(frame).ok())
        {
            panel.update(cx, |panel, cx| panel.observe(frame, cx));
        }
        div().child(panel)
    }
}
