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
            _state: observe,
        }
    }
    fn request(&mut self, event: Request, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        let Some(panel) = self.panel.clone() else {
            return;
        };
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
        panel.update(cx, |panel, cx| {
            panel.colors = colors;
            panel.set_target(target, cx);
        });
        div().child(panel)
    }
}
