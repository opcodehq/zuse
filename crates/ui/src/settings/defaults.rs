//! Defaults edit the shared UI store; the current chat is never mutated here.
use super::{SavePolicy, widgets};
use crate::{
    composer::{ComposerInput, ComposerInputEvent},
    icons, pickers,
    state::AppState,
    theme::Theme,
};
use futures::{StreamExt, stream::FuturesUnordered};
use gpui::{prelude::*, *};
use serde_json::json;
use std::collections::HashMap;
use zuse_engine::registry::HarnessDescriptor;
use zuse_protocol::{
    HarnessId, Model,
    presentation::{DefaultModelSelection, PermissionMode},
};

pub struct DefaultsPage {
    state: Entity<AppState>,
    search: Entity<ComposerInput>,
    open: bool,
    scope: Option<HarnessId>,
    active: Option<(HarnessId, String)>,
    descriptors: Vec<HarnessDescriptor>,
    models: HashMap<HarnessId, Result<Vec<Model>, String>>,
    loading: bool,
    error: Option<String>,
    load: Option<Task<()>>,
    epoch: u64,
    device: Option<String>,
    engine_present: bool,
    permission: widgets::SelectState,
    scroll: widgets::PageScroll,
    _state: Subscription,
    _search: Subscription,
    _settings: Subscription,
    _catalog: Subscription,
}
impl DefaultsPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            ComposerInput::with_context("Search models…", "PaletteSearch", cx)
                .with_single_line()
                .with_text_metrics(crate::typography::TEXT_XS, 16.)
        });
        let input_events = cx.subscribe(
            &search,
            |this: &mut Self, _, event: &ComposerInputEvent, cx| {
                match event {
                    ComposerInputEvent::Edited => this.active = None,
                    ComposerInputEvent::Submitted | ComposerInputEvent::ModifiedSubmitted
                        if this.open =>
                    {
                        this.choose_active(cx)
                    }
                    _ => {}
                }
                cx.notify();
            },
        );
        let observe = cx.observe(&state, |this: &mut Self, state, cx| {
            let state = state.read(cx);
            let device = state.effective_device_id();
            let present = state.engine().is_some();
            if device != this.device || present != this.engine_present {
                this.device = device;
                this.engine_present = present;
                this.invalidate();
                if this.open {
                    this.load(cx);
                }
                cx.notify();
            }
        });
        let settings = cx.observe_global::<super::SettingsStore>(|_: &mut Self, cx| cx.notify());
        let catalog = cx.observe_global::<pickers::HarnessCatalogChanged>(|this: &mut Self, cx| {
            this.invalidate();
            if this.open {
                this.load(cx);
            }
            cx.notify();
        });
        let device = state.read(cx).effective_device_id();
        let engine_present = state.read(cx).engine().is_some();
        Self {
            state,
            search,
            open: false,
            scope: None,
            active: None,
            descriptors: vec![],
            models: HashMap::new(),
            loading: false,
            error: None,
            load: None,
            epoch: 0,
            device,
            engine_present,
            permission: Default::default(),
            scroll: Default::default(),
            _state: observe,
            _search: input_events,
            _settings: settings,
            _catalog: catalog,
        }
    }
    fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.load = None;
        self.loading = false;
        self.descriptors.clear();
        self.models.clear();
        self.error = None;
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.error = Some("Connect to an execution host to load its models.".into());
            return;
        };
        self.epoch = self.epoch.wrapping_add(1);
        let epoch = self.epoch;
        let target = self
            .device
            .clone()
            .filter(|device| self.state.read(cx).local_device_id.as_ref() != Some(device));
        self.loading = true;
        self.error = None;
        self.models.clear();
        self.descriptors.clear();
        self.load = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    zuse_rpc::methods::LIST_HARNESSES,
                    json!({"targetDeviceId":target}),
                )
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    serde_json::from_value::<Vec<HarnessDescriptor>>(value)
                        .map_err(|error| error.to_string())
                });
            let descriptors = match result {
                Ok(descriptors) => pickers::offered_harnesses(&descriptors),
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.epoch == epoch {
                            this.loading = false;
                            this.error = Some(error);
                            cx.notify();
                        }
                    });
                    return;
                }
            };
            if this
                .update(cx, |this, cx| {
                    if this.epoch == epoch {
                        this.descriptors = descriptors.clone();
                        cx.notify();
                    }
                })
                .is_err()
            {
                return;
            }
            let mut pending = FuturesUnordered::new();
            for descriptor in descriptors {
                let engine = engine.clone();
                let target = target.clone();
                pending.push(async move {
                    let id = descriptor.id;
                    let models = engine
                        .client()
                        .call(
                            zuse_rpc::methods::LIST_MODELS,
                            json!({"harness":id,"targetDeviceId":target}),
                        )
                        .await
                        .map_err(|error| error.to_string())
                        .and_then(|value| {
                            serde_json::from_value::<Vec<Model>>(value)
                                .map_err(|error| error.to_string())
                        });
                    (id, models)
                });
            }
            while let Some((provider, models)) = pending.next().await {
                if this
                    .update(cx, |this, cx| {
                        if this.epoch == epoch {
                            this.models.insert(provider, models);
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                if this.epoch == epoch {
                    this.loading = false;
                    cx.notify();
                }
            });
        }));
    }
    fn choose(&mut self, provider: HarnessId, model: Model, cx: &mut Context<Self>) {
        if !self
            .descriptors
            .iter()
            .any(|descriptor| descriptor.id == provider)
        {
            return;
        }
        super::update(SavePolicy::Immediate, cx, |settings| {
            settings.default_model = Some(DefaultModelSelection {
                provider,
                model: model.id,
                label: model.label,
            })
        });
        self.open = false;
        cx.refresh_windows();
        cx.notify();
    }
    pub fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.open {
            self.open = false;
            cx.notify();
            true
        } else if self.permission.is_open() {
            widgets::close_select(self, |this| &mut this.permission, cx);
            true
        } else {
            false
        }
    }
    fn candidates(&self, cx: &App) -> Vec<(HarnessId, Model)> {
        let query = self.search.read(cx).text().to_ascii_lowercase();
        let mut candidates = Vec::new();
        for descriptor in &self.descriptors {
            if self.scope.is_some_and(|scope| scope != descriptor.id) {
                continue;
            }
            if let Some(Ok(models)) = self.models.get(&descriptor.id) {
                candidates.extend(
                    models
                        .iter()
                        .filter(|model| {
                            query.is_empty()
                                || [&model.label, &model.id, &descriptor.name]
                                    .iter()
                                    .any(|label| label.to_ascii_lowercase().contains(&query))
                        })
                        .cloned()
                        .map(|model| (descriptor.id, model)),
                );
            }
        }
        candidates
    }
    fn choose_active(&mut self, cx: &mut Context<Self>) {
        let candidates = self.candidates(cx);
        let selected = self
            .active
            .as_ref()
            .and_then(|(provider, id)| {
                candidates
                    .iter()
                    .find(|(candidate, model)| candidate == provider && &model.id == id)
            })
            .or_else(|| candidates.first())
            .cloned();
        if let Some((provider, model)) = selected {
            self.choose(provider, model, cx);
        }
    }
    fn on_picker_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        let key = event.keystroke.key.as_str();
        match key {
            "escape" => self.open = false,
            "enter" if !event.is_held => self.choose_active(cx),
            "up" | "down" | "home" | "end" => {
                let candidates = self.candidates(cx);
                if !candidates.is_empty() {
                    let selected = self.active.as_ref().and_then(|(provider, id)| {
                        candidates
                            .iter()
                            .position(|(candidate, model)| candidate == provider && &model.id == id)
                    });
                    let next = match key {
                        "home" => 0,
                        "end" => candidates.len() - 1,
                        "up" => selected
                            .map(|index| (index + candidates.len() - 1) % candidates.len())
                            .unwrap_or(candidates.len() - 1),
                        _ => selected
                            .map(|index| (index + 1) % candidates.len())
                            .unwrap_or(0),
                    };
                    self.active = Some((candidates[next].0, candidates[next].1.id.clone()));
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
    fn picker(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let selected = super::default_model(cx);
        let query = self.search.read(cx).text().to_ascii_lowercase();
        let rail = div()
            .w(px(44.))
            .flex_none()
            .p(px(6.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .bg(theme.settings_frame_bg().opacity(0.2))
            .border_r_1()
            .border_color(theme.border.opacity(0.5))
            .child(
                div()
                    .id("defaults-all-providers")
                    .size(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.))
                    .text_size(crate::typography::text_xs())
                    .when(self.scope.is_none(), |view| {
                        view.bg(theme.accent.opacity(0.12))
                    })
                    .cursor_pointer()
                    .child("All")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.scope = None;
                        this.active = None;
                        cx.notify();
                    })),
            )
            .children(
                self.descriptors
                    .iter()
                    .enumerate()
                    .map(|(index, descriptor)| {
                        let provider = descriptor.id;
                        let (path, tint) = pickers::harness_brand_icon(provider);
                        div()
                            .id(("defaults-provider", index))
                            .role(Role::Tab)
                            .aria_label(descriptor.name.clone())
                            .aria_selected(self.scope == Some(provider))
                            .size(px(32.))
                            .rounded(px(7.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .when(self.scope == Some(provider), |view| {
                                view.bg(theme.accent.opacity(0.12))
                            })
                            .child(
                                icons::icon(path)
                                    .size(px(14.))
                                    .text_color(tint.unwrap_or(theme.text_muted)),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.scope = Some(provider);
                                this.active = None;
                                cx.notify();
                            }))
                    }),
            );
        let mut results = Vec::<AnyElement>::new();
        for descriptor in &self.descriptors {
            if self.scope.is_some_and(|scope| scope != descriptor.id) {
                continue;
            }
            let Some(models) = self.models.get(&descriptor.id) else {
                continue;
            };
            let models = match models {
                Ok(models) => models,
                Err(error) => {
                    results.push(widgets::error_strip(theme, error.clone()).into_any_element());
                    continue;
                }
            };
            let models: Vec<_> = models
                .iter()
                .filter(|model| {
                    query.is_empty()
                        || [&model.label, &model.id, &descriptor.name]
                            .iter()
                            .any(|label| label.to_ascii_lowercase().contains(&query))
                })
                .collect();
            if models.is_empty() {
                continue;
            }
            results.push(
                div()
                    .px(px(8.))
                    .pt(px(10.))
                    .pb(px(4.))
                    .text_size(crate::typography::text_xs())
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_strong)
                    .child(descriptor.name.clone())
                    .into_any_element(),
            );
            for (index, model) in models.into_iter().enumerate() {
                let provider = descriptor.id;
                let model = model.clone();
                let active = self
                    .active
                    .as_ref()
                    .is_some_and(|(candidate, id)| *candidate == provider && *id == model.id);
                let chosen = selected.is_some_and(|selection| {
                    selection.provider == provider && selection.model == model.id
                });
                results.push(
                    div()
                        .id(SharedString::from(format!(
                            "default-model-{provider:?}-{index}"
                        )))
                        .role(Role::Button)
                        .aria_label(format!("Choose {}", model.label))
                        .cursor_pointer()
                        .min_h(px(32.))
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(px(7.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .when(chosen, |view| view.bg(theme.accent.opacity(0.12)))
                        .when(active, |view| view.bg(theme.settings_frame_bg()))
                        .hover(|view| view.bg(theme.settings_frame_bg().opacity(0.6)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(crate::typography::text_xs())
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.text_strong)
                                        .child(model.label.clone()),
                                )
                                .when_some(model.description.clone(), |view, description| {
                                    view.child(
                                        div()
                                            .text_size(crate::typography::text_xs())
                                            .line_height(px(16.))
                                            .text_color(theme.text_muted)
                                            .child(description),
                                    )
                                }),
                        )
                        .when(chosen, |view| {
                            view.child(
                                icons::icon(icons::CHECK)
                                    .size(px(14.))
                                    .text_color(theme.accent),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.choose(provider, model.clone(), cx)
                        }))
                        .into_any_element(),
                );
            }
        }
        let empty = results.is_empty();
        let body = div()
            .id("defaults-model-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(10.))
            .pb(px(10.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(results)
            .when(self.loading, |view| {
                view.child(widgets::meta_line(
                    theme,
                    vec![div().child("Loading models…").into_any_element()],
                ))
            })
            .when(!self.loading && empty && self.error.is_none(), |view| {
                view.child(
                    div()
                        .px(px(12.))
                        .py(px(24.))
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .child("No models found."),
                )
            })
            .when_some(self.error.clone(), |view, error| {
                view.child(widgets::error_strip(theme, error))
            });
        div()
            .w(px(380.))
            .h(px(480.))
            .flex()
            .rounded(px(16.))
            .overflow_hidden()
            .map(|el| crate::popover::glass_surface(theme, el))
            .child(rail)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .p(px(10.))
                            .border_b_1()
                            .border_color(theme.border.opacity(0.5))
                            .child(
                                div()
                                    .id("default-model-search")
                                    .on_key_down(cx.listener(Self::on_picker_key))
                                    .h(px(32.))
                                    .px(px(10.))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .rounded(px(7.))
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(theme.bg)
                                    .child(
                                        icons::icon(icons::MAGNIFER)
                                            .size(px(14.))
                                            .text_color(theme.text_muted),
                                    )
                                    .child(div().flex_1().min_w_0().child(self.search.clone())),
                            ),
                    )
                    .child(body),
            )
    }
}
impl Render for DefaultsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let preferences = super::current(cx);
        let label = preferences
            .default_model
            .as_ref()
            .map(|model| model.label.clone())
            .unwrap_or_else(|| "Agent default".into());
        let selected = PermissionMode::ALL
            .iter()
            .position(|mode| *mode == preferences.default_permission_mode)
            .unwrap_or(0);
        let permission = widgets::select(
            "default-permission",
            "Default permission mode",
            &theme,
            |this: &mut Self| &mut this.permission,
        )
        .options(
            PermissionMode::ALL
                .into_iter()
                .map(|mode| widgets::SelectOption::new(mode.label())),
            selected,
        )
        .width(256_f32.min(f32::from(window.viewport_size().width) * 0.4))
        .on_select(|this, index, _, cx| {
            if let Some(mode) = PermissionMode::ALL.get(index) {
                this.open = false;
                super::update(SavePolicy::Immediate, cx, |settings| {
                    settings.default_permission_mode = *mode
                });
                cx.refresh_windows();
                cx.notify();
            }
        })
        .render(&self.permission, cx);
        let trigger = div()
            .relative()
            .child(
                widgets::action_button(&theme, widgets::ActionTone::Outlined)
                    .id("settings-default-model")
                    .w(px(256.))
                    .max_w(window.viewport_size().width * 0.4)
                    .h(px(28.))
                    .min_h(px(28.))
                    .px(px(10.))
                    .py(px(0.))
                    .text_size(px(12.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .role(Role::Button)
                    .aria_label("Default model")
                    .aria_expanded(self.open)
                    .tab_index(0)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .flex_1()
                            .min_w_0()
                            .when_some(preferences.default_model.as_ref(), |view, selection| {
                                let (path, tint) = pickers::harness_brand_icon(selection.provider);
                                view.child(
                                    icons::icon(path)
                                        .size(px(14.))
                                        .text_color(tint.unwrap_or(theme.text_muted)),
                                )
                            })
                            .child(div().min_w_0().truncate().child(label)),
                    )
                    .child(
                        icons::icon(icons::ALT_ARROW_DOWN)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open = !this.open;
                        if this.open {
                            widgets::close_select(this, |this| &mut this.permission, cx);
                            this.scope = None;
                            this.search.update(cx, |search, cx| search.set_text("", cx));
                            this.load(cx);
                            window.focus(&this.search.read(cx).focus_handle(cx), cx);
                        }
                        cx.notify();
                    })),
            )
            .when(self.open, |view| {
                view.child(widgets::dropdown(
                    "default-model-picker",
                    self.picker(&theme, cx),
                    None,
                    28.,
                ))
            });
        let rows=widgets::section_card(&theme).mt_0()
            .child(widgets::card_row(&theme,true).items_start().child(details(&theme,"Default model",&preferences.default_model.as_ref().map(|selection|format!("Model for new chats · {}",super::accounts::provider_name(selection.provider))).unwrap_or_else(||"Model for new chats.".into()))).child(trigger))
            .child(widgets::card_row(&theme,false).items_start().child(details(&theme,"Default permission mode","How new chats handle tool calls. Each chat can override this from the composer.")).child(permission));
        let content = widgets::page_column()
            .child(widgets::section_title(&theme,"Default models","Choose how new chats start."))
            .child(widgets::section_stack().child(widgets::group(&theme,"Chat defaults","These choices apply when you start a new chat. You can still change either one from the composer.").child(rows)));
        let area = div()
            .id("default-models-page")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll.scroll)
            .child(content);
        widgets::scroll_faded("defaults-settings-scroll", area)
    }
}
fn details(theme: &Theme, title: &str, description: &str) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(widgets::row_title(
            theme,
            SharedString::from(title.to_owned()),
        ))
        .child(
            div()
                .text_size(px(11.))
                .line_height(px(16.))
                .text_color(theme.text_muted)
                .child(SharedString::from(description.to_owned())),
        )
}
