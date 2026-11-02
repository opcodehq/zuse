//! Settings → General → Thread naming: which agent and model title new
//! threads, chosen with the composer's own model picker (title-bound mode).

use crate::localization::message as tr;
use gpui::{AnyElement, Context, Entity, Subscription, Task, div, prelude::*, px};
use zuse_engine::registry::TitleSettings;
use zuse_i18n::messages as msg;
use zuse_rpc::methods;

use crate::pickers::{Pickers, TitleModelPicked};
use crate::popover::Loadable;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

pub struct ThreadNamingCard {
    state: Entity<AppState>,
    settings: Loadable<TitleSettings>,
    picker: Entity<Pickers>,
    error: Option<String>,
    task: Option<Task<()>>,
    owner: (Option<String>, bool),
    epoch: u64,
    confirmed: bool,
    attempt: Option<TitleSettings>,
    _picked: Subscription,
    _state: Subscription,
}

impl ThreadNamingCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let picker =
            cx.new(|cx| Pickers::new_for_titles(state.clone(), TitleSettings::default(), cx));
        cx.observe(&picker, |_, _, cx| cx.notify()).detach();
        let picked = cx.subscribe(
            &picker,
            |this: &mut Self, _, event: &TitleModelPicked, cx| {
                this.call(Some(event.0.clone()), cx);
            },
        );
        let owner = (
            state.read(cx).local_device_id.clone(),
            state.read(cx).engine().is_some(),
        );
        let state_sub = cx.observe(&state, |this: &mut Self, state, cx| {
            let next = (
                state.read(cx).local_device_id.clone(),
                state.read(cx).engine().is_some(),
            );
            if this.owner != next {
                this.owner = next;
                this.epoch += 1;
                this.task = None;
                this.confirmed = false;
                this.settings = Loadable::Idle;
                this.error = None;
                this.attempt = None;
            }
            if matches!(this.settings, Loadable::Idle) && this.task.is_none() {
                this.call(None, cx);
            }
        });
        let mut card = Self {
            state,
            settings: Loadable::Idle,
            picker,
            error: None,
            task: None,
            owner,
            epoch: 0,
            confirmed: false,
            attempt: None,
            _picked: picked,
            _state: state_sub,
        };
        card.call(None, cx);
        card
    }
    /// Read after an uncertain write; a lost response never repeats the write.
    fn call(&mut self, save: Option<TitleSettings>, cx: &mut Context<Self>) {
        if self.task.is_some() || (save.is_some() && !self.confirmed) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let writing = save.is_some();
        if let Some(settings) = &save {
            self.attempt = Some(settings.clone());
            self.error = None;
        }
        let (method, params) = match save {
            Some(settings) => (
                methods::SET_TITLE_SETTINGS,
                serde_json::to_value(settings).unwrap(),
            ),
            None => (methods::GET_TITLE_SETTINGS, serde_json::json!({})),
        };
        self.epoch += 1;
        let epoch = self.epoch;
        if matches!(self.settings, Loadable::Idle) {
            self.settings = Loadable::Loading;
        }
        self.picker.update(cx, |picker, cx| {
            picker.set_settings_pending(Some(if writing { "Saving…" } else { "Loading…" }), cx)
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|e| e.to_string())
                .and_then(|value| {
                    serde_json::from_value::<TitleSettings>(value).map_err(|e| e.to_string())
                });
            let _ = this.update(cx, |card, cx| {
                if card.epoch != epoch {
                    return;
                }
                card.task = None;
                match result {
                    Ok(settings) => {
                        let attempt = card.attempt.take();
                        let reconciled = attempt.as_ref().is_some_and(|attempt| {
                            attempt.harness == settings.harness && attempt.model == settings.model
                        });
                        card.confirmed = true;
                        card.picker.update(cx, |picker, cx| {
                            picker.set_title_settings(settings.clone(), cx);
                            picker.set_settings_pending(None, cx);
                        });
                        card.settings = Loadable::Ready(settings);
                        if writing || attempt.is_none() || reconciled {
                            card.error = None;
                        }
                    }
                    Err(error) => {
                        card.confirmed = false;
                        card.error = Some(error.clone());
                        if let Some(saved) = card.settings.ready().cloned() {
                            card.picker
                                .update(cx, |picker, cx| picker.set_title_settings(saved, cx));
                        } else {
                            card.settings = Loadable::Error(error);
                        }
                        card.picker.update(cx, |picker, cx| {
                            picker.set_settings_pending(Some("Unavailable"), cx)
                        });
                        if writing {
                            card.call(None, cx);
                        }
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Render for ThreadNamingCard {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let follows_session = self
            .settings
            .ready()
            .is_some_and(|settings| settings.harness.is_none());
        let control: AnyElement = match &self.settings {
            Loadable::Ready(_) => div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .when(!follows_session, |row| {
                    row.child(
                        widgets::text_action(
                            &theme,
                            widgets::ActionTone::Quiet,
                            tr(cx, msg::chat::chat_composer_reset),
                        )
                        .id("thread-naming-reset")
                        .tab_index(0)
                        .role(gpui::Role::Button)
                        .aria_label(tr(cx, msg::native::name_threads_with_the_session_agent))
                        .focus_visible(|s| s.border_2().border_color(theme.accent))
                        .on_click(cx.listener(|card, _, _, cx| {
                            card.call(Some(TitleSettings::default()), cx)
                        })),
                    )
                })
                .child(self.picker.clone())
                .into_any_element(),
            Loadable::Error(_) => widgets::text_action(
                &theme,
                widgets::ActionTone::Quiet,
                tr(cx, msg::settings::sharing_retry),
            )
            .id("thread-naming-retry")
            .role(gpui::Role::Button)
            .aria_label(tr(cx, msg::native::retry_thread_naming_settings))
            .on_click(cx.listener(|this, _, _, cx| this.call(None, cx)))
            .into_any_element(),
            Loadable::Idle | Loadable::Loading => div()
                .text_color(theme.text_muted)
                .child(tr(cx, msg::common::loading))
                .into_any_element(),
        };
        let description = if follows_session {
            "Each thread is named by its own agent."
        } else {
            "A small model keeps titles fast and cheap."
        };
        widgets::group(&theme, tr(cx, msg::native::thread_names), "").child(
            widgets::section_card(&theme)
                .mt_0()
                .child(
                    widgets::card_row(&theme, true)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(widgets::row_title(
                                    &theme,
                                    tr(cx, msg::native::thread_naming),
                                ))
                                .child(widgets::meta_line(
                                    &theme,
                                    vec![div().child(description).into_any_element()],
                                )),
                        )
                        .child(control),
                )
                .when_some(self.error.clone(), |view, error| {
                    view.child(
                        widgets::error_strip(&theme, error).child(
                            widgets::text_action(
                                &theme,
                                widgets::ActionTone::Quiet,
                                tr(cx, msg::settings::organizations_refresh),
                            )
                            .id("thread-naming-refresh")
                            .role(gpui::Role::Button)
                            .aria_label(tr(cx, msg::native::refresh_saved_naming_choice))
                            .on_click(cx.listener(|this, _, _, cx| this.call(None, cx))),
                        ),
                    )
                }),
        )
    }
}
