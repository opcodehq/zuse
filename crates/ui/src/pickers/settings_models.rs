//! Settings reuse the model menu while keeping conversation choices separate.
use super::*;

pub(super) struct State {
    pub defaults: bool,
    pub pending: Option<&'static str>,
    pub window: Option<gpui::AnyWindowHandle>,
    pub trigger: gpui::FocusHandle,
}
impl State {
    pub fn new(defaults: bool, cx: &mut Context<Pickers>) -> Self {
        Self {
            defaults,
            pending: None,
            window: None,
            trigger: cx.focus_handle().tab_stop(true),
        }
    }
}
impl Pickers {
    pub fn new_for_defaults(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self::build(state, None, true, cx)
    }
    pub(super) fn is_settings_model(&self) -> bool {
        self.title.is_some() || self.settings_model.defaults
    }
    pub fn set_settings_pending(&mut self, pending: Option<&'static str>, cx: &mut Context<Self>) {
        self.settings_model.pending = pending;
        if pending.is_some() {
            self.dismiss(cx);
        }
        cx.notify();
    }
    pub fn dismiss_settings_menu(&mut self, return_focus: bool, cx: &mut Context<Self>) -> bool {
        let open = self.is_open();
        if open {
            if return_focus {
                self.close(cx);
            } else {
                self.dismiss(cx);
            }
        }
        open
    }
    pub(super) fn return_settings_focus(&self, cx: &mut Context<Self>) {
        if let Some(window) = self.settings_model.window {
            let trigger = self.settings_model.trigger.clone();
            cx.defer(move |cx| {
                let _ = cx.update_window(window, |_, window, cx| window.focus(&trigger, cx));
            });
        }
    }
    pub(super) fn choose_default_model(&mut self, model: String, cx: &mut Context<Self>) {
        let Some(provider) = self.effective_harness(cx) else {
            return;
        };
        let label = self
            .models
            .get(&provider)
            .and_then(Loadable::ready)
            .and_then(|models| models.iter().find(|row| row.id == model))
            .map(|row| row.label.clone())
            .unwrap_or_else(|| model.clone());
        crate::settings::update(crate::settings::SavePolicy::Immediate, cx, |settings| {
            settings.default_model = Some(zuse_protocol::presentation::DefaultModelSelection {
                provider,
                model,
                label,
            });
        });
        self.config.harness = None;
        self.config.model = None;
        self.catalog_rev += 1;
        self.close(cx);
        cx.refresh_windows();
    }
}
