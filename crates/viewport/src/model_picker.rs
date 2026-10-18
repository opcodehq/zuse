//! Combined provider/model picker, following Zuse's provider rail and model list.
use super::*;
use crate::assets::icon;
use crate::shell::{
    BORDER, FAINT, MUTED, TEXT, column, control, icon_button, label, provider_icon, row,
};
use zuse_protocol::{Model, presentation::ModelSelection};
const ACCENT: u32 = 0x89ca21;
pub(super) type TriggerBounds = std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>;

impl WorkspaceView {
    pub(super) fn save_preferences(&self) {
        let _ = self
            .requests
            .unbounded_send(json!({"ui":"savePreferences","value":self.preferences}));
    }
    pub(super) fn apply_default_model(&mut self) {
        if self.selected.is_some() {
            return;
        }
        let preferred = self.preferences.starting_model();
        let available =
            |provider: &Value| provider["enabled"] != false && provider["installed"] != false;
        let chosen = preferred
            .as_ref()
            .and_then(|(provider, _)| {
                self.providers
                    .iter()
                    .position(|row| row["id"] == *provider && available(row))
            })
            .or_else(|| self.providers.iter().position(available));
        if let Some(index) = chosen {
            self.provider_index = index;
            self.model = preferred
                .filter(|(provider, _)| self.providers[index]["id"] == *provider)
                .and_then(|(_, model)| model);
            self.restore_model_choices();
        }
    }
    pub(super) fn selected_model(&self) -> Option<&Model> {
        let provider = self.providers.get(self.provider_index)?["id"].as_str()?;
        let models = &self.picker_catalog.providers.get(provider)?.models;
        let id = self.model.as_deref().unwrap_or("default");
        models.iter().find(|m| m.id == id)
    }
    pub(super) fn picker_label(&self) -> String {
        self.selected_model()
            .map(|m| m.label.clone())
            .or_else(|| self.model.clone())
            .unwrap_or_else(|| "Agent default".into())
    }
    pub(super) fn open_model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.context_menu = false;
        self.model_picker = !self.model_picker;
        self.model_picker_default = false;
        self.agent_picker = false;
        self.access_picker = false;
        self.add_menu = false;
        if self.model_picker {
            if self.selected.is_some() && !self.model_confirmed && self.model_watch.is_none() {
                self.refresh_model_config_watch();
            }
            self.picker_scope = None;
            self.model_search
                .update(cx, |input, cx| input.set_value(String::new(), cx));
            window.focus(&self.model_search.focus_handle(cx), cx);
            let this = cx.entity().downgrade();
            window.on_next_frame(move |window, cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.model_picker {
                        window.focus(&this.model_search.focus_handle(cx), cx);
                    }
                });
            });
            self.picker_active = None;
            self.configure_reasoning_slider(cx);
            self.load_picker_catalogs(false);
        }
        cx.notify();
    }
    fn return_model_picker_focus(
        &self,
        defaults: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(
            &if defaults {
                self.default_model_focus.clone()
            } else {
                self.input.focus_handle(cx)
            },
            cx,
        );
    }
    pub(super) fn load_picker_catalogs(&mut self, force: bool) {
        if !self.connected {
            return;
        }
        let providers = self
            .providers
            .iter()
            .filter(|p| p["installed"] != false && p["enabled"] != false)
            .filter_map(|p| p["id"].as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        for provider in providers {
            let existing = self.picker_catalog.providers.get(&provider);
            if existing
                .is_some_and(|c| c.request.is_some() || (!force && (c.loaded || c.error.is_some())))
            {
                continue;
            }
            let id = self.call(
                methods::LIST_MODELS,
                json!({"harness":provider,"force":force}),
            );
            self.picker_catalog.begin(&provider, id);
        }
    }
    pub(super) fn receive_picker_catalog(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.picker_catalog.contains_request(id) {
            return false;
        }
        let result = if let Some(error) = frame["err"].as_str() {
            Err(error.to_owned())
        } else if let Some(value) = frame.get("ok") {
            serde_json::from_value(value.clone())
                .map_err(|_| "The agent returned an invalid model catalog.".into())
        } else {
            return true;
        };
        self.picker_catalog.complete(id, result);
        for (provider, catalog) in &self.picker_catalog.providers {
            for model in &catalog.models {
                self.picker_anchors
                    .entry(format!("{provider}:{}", model.id))
                    .or_insert_with(|| ScrollAnchor::for_handle(self.picker_scroll.clone()));
            }
        }
        self.pending.remove(&id);
        if self.selected.is_none() {
            self.restore_model_choices();
        }
        self.configure_reasoning_slider(cx);
        cx.notify();
        true
    }
    fn choose_model(&mut self, provider: String, model: Option<String>, cx: &mut Context<Self>) {
        let Some(index) = self.providers.iter().position(|p| p["id"] == provider) else {
            return;
        };
        if self.model_picker_default {
            self.preferences.default_provider = Some(provider.clone());
            if let Some(model) = model {
                self.preferences
                    .set_default(ModelSelection { provider, model });
            } else {
                self.preferences.default_models.remove(&provider);
            }
            self.save_preferences();
            self.model_picker = false;
            self.model_picker_default = false;
            cx.notify();
            return;
        }
        if !self.model_provider_allowed(&provider) {
            self.error = Some("Start a new chat to use another provider.".into());
            cx.notify();
            return;
        }
        if !self.can_change_model(cx) {
            return;
        }
        if self.provider_index == index && self.model == model {
            self.model_picker = false;
            cx.notify();
            return;
        }
        self.provider_index = index;
        self.model = model.clone();
        self.restore_model_choices();
        if let Some(id) = model {
            self.preferences.record_model(ModelSelection {
                provider,
                model: id,
            });
            self.save_preferences();
        }
        self.model_picker = false;
        self.model_choices_changed(cx);
    }
    fn model_row(
        &self,
        provider: &str,
        model: &Model,
        ordinal: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let provider_id = provider.to_owned();
        let model_id = model.id.clone();
        let active_chat = self
            .providers
            .get(self.provider_index)
            .is_some_and(|p| p["id"] == provider)
            && (self.model.as_deref() == Some(&model.id)
                || (self.model.is_none() && model.id == "default"));
        let default = self.preferences.default_provider.as_deref() == Some(provider)
            && self.preferences.default_models.get(provider) == Some(&model.id);
        let active = if self.model_picker_default {
            default
        } else {
            active_chat
        };
        let selection = ModelSelection {
            provider: provider.to_owned(),
            model: model.id.clone(),
        };
        row()
            .id(SharedString::from(format!(
                "model-row-{provider}-{}-{ordinal}",
                model.id
            )))
            .role(Role::Button)
            .anchor_scroll(
                self.picker_anchors
                    .get(&format!("{provider}:{}", model.id))
                    .cloned(),
            )
            .aria_label(SharedString::from(format!("Choose {}", model.label)))
            .relative()
            .min_h(px(32.))
            .flex_shrink_0()
            .px_2()
            .py(px(6.))
            .gap(px(6.))
            .rounded(px(7.))
            .cursor_pointer()
            .when(
                self.picker_active
                    .as_ref()
                    .is_some_and(|m| m.provider == provider && m.model == model.id),
                |d| d.bg(crate::settings_style::frame_bg().opacity(0.6)),
            )
            .when(active, |d| d.bg(rgb(ACCENT).opacity(0.12)))
            .hover(|d| {
                d.bg(if active {
                    rgb(ACCENT).opacity(0.12).into()
                } else {
                    crate::settings_style::frame_bg().opacity(0.6)
                })
            })
            .when(active, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_1_2()
                        .mt(px(-8.))
                        .h(px(16.))
                        .w(px(2.))
                        .rounded_full()
                        .bg(rgb(ACCENT)),
                )
            })
            .child(
                icon(provider_icon(provider))
                    .size(px(16.))
                    .text_color(rgb(MUTED)),
            )
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(TEXT))
                            .child(model.label.clone()),
                    )
                    .when_some(model.description.clone(), |d, description| {
                        d.child(label(description).text_size(px(11.)).text_color(rgb(MUTED)))
                    }),
            )
            .child(
                icon_button(
                    SharedString::from(format!("default-{provider}-{}-{ordinal}", model.id)),
                    "star",
                    "Use this model by default",
                )
                .size(px(20.))
                .text_color(rgb(if default { ACCENT } else { FAINT }))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.preferences.set_default(selection.clone());
                    this.save_preferences();
                    cx.notify();
                })),
            )
            .when(active, |d| {
                d.child(icon("check").size(px(14.)).text_color(rgb(ACCENT)))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                let defaults = this.model_picker_default;
                this.choose_model(provider_id.clone(), Some(model_id.clone()), cx);
                if !this.model_picker {
                    this.return_model_picker_focus(defaults, window, cx);
                }
            }))
            .into_any_element()
    }
    pub(super) fn composer_picker(
        &self,
        cx: &mut Context<Self>,
        trigger: Option<TriggerBounds>,
    ) -> AnyElement {
        let query = self.model_search.read(cx).value(cx).to_lowercase();
        let current = if self.model_picker_default {
            self.preferences
                .default_provider
                .as_deref()
                .unwrap_or_default()
        } else {
            self.providers
                .get(self.provider_index)
                .and_then(|p| p["id"].as_str())
                .unwrap_or_default()
        };
        let total = self
            .picker_catalog
            .providers
            .iter()
            .filter(|(provider, _)| self.model_provider_allowed(provider))
            .map(|(_, catalog)| catalog.models.len())
            .sum::<usize>();
        let width = (self.content_width - 48.).clamp(272., 380.);
        let height = if self.content_height > 0. {
            (self.content_height - 220.).clamp(230., 480.)
        } else {
            480.
        };
        row()
            .id("composer-picker")
            .absolute()
            .when(!self.model_picker_default, |view| view.bottom_full().mb_2())
            .when(self.model_picker_default, |view| view.top_full().mt_2())
            .right_0()
            .w(px(width))
            .h(px(height))
            .items_stretch()
            .overflow_hidden()
            .rounded(px(16.))
            .border_1()
            .border_color(crate::settings_style::card_border())
            .bg(crate::palette::tone(
                hsla(132. / 360., 0.05, 0.105, 1.),
                hsla(0., 0., 1., 1.),
            ))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .capture_key_down(cx.listener(Self::on_picker_key))
            .when_some(trigger, |view, trigger| {
                view.on_mouse_down_out(cx.listener(
                    move |this, event: &MouseDownEvent, window, cx| {
                        if !trigger
                            .get()
                            .is_some_and(|bounds| bounds.contains(&event.position))
                        {
                            this.model_picker = false;
                            this.model_picker_default = false;
                            this.return_model_picker_focus(true, window, cx);
                            cx.notify();
                        }
                    },
                ))
            })
            .child(
                column()
                    .id("provider-rail")
                    .w(px(44.))
                    .flex_shrink_0()
                    .overflow_y_scroll()
                    .p(px(6.))
                    .gap_1()
                    .border_r_1()
                    .border_color(rgb(BORDER).opacity(0.5))
                    .bg(crate::settings_style::frame_bg().opacity(0.2))
                    .child(
                        control("models-all", "All")
                            .h(px(32.))
                            .flex_shrink_0()
                            .justify_center()
                            .px_1()
                            .text_size(px(10.))
                            .when(self.picker_scope.is_none(), |d| {
                                d.bg(rgb(ACCENT).opacity(0.12)).text_color(rgb(TEXT))
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.picker_scope = None;
                                cx.notify();
                            })),
                    )
                    .children(
                        self.providers
                            .iter()
                            .enumerate()
                            .filter(|(_, p)| {
                                self.model_provider_allowed(p["id"].as_str().unwrap_or_default())
                            })
                            .filter(|(_, p)| {
                                (p["installed"] != false && p["enabled"] != false)
                                    || (!self.model_picker_default
                                        && self.selected.is_some()
                                        && p["id"] == current)
                            })
                            .map(|(i, p)| {
                                let provider = p["id"].as_str().unwrap_or_default().to_owned();
                                let live = self
                                    .picker_catalog
                                    .providers
                                    .get(&provider)
                                    .is_some_and(|c| c.loaded && c.error.is_none());
                                icon_button(
                                    ("model-provider", i),
                                    provider_icon(&provider),
                                    "Filter models by agent",
                                )
                                .relative()
                                .size(px(32.))
                                .flex_shrink_0()
                                .when(self.picker_scope.as_deref() == Some(&provider), |d| {
                                    d.bg(rgb(ACCENT).opacity(0.12)).text_color(rgb(TEXT))
                                })
                                .when(live, |d| {
                                    d.child(
                                        div()
                                            .absolute()
                                            .right(px(3.))
                                            .bottom(px(3.))
                                            .size(px(4.))
                                            .rounded_full()
                                            .bg(rgb(ACCENT)),
                                    )
                                })
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.picker_scope = Some(provider.clone());
                                        cx.notify();
                                    },
                                ))
                            }),
                    ),
            )
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(
                        row()
                            .flex_shrink_0()
                            .p(px(10.))
                            .gap_1()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(
                                row()
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(32.))
                                    .px(px(10.))
                                    .gap_2()
                                    .rounded(px(7.))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .bg(rgb(crate::shell::BG))
                                    .child(icon("magnifer").size(px(14.)).text_color(rgb(FAINT)))
                                    .child(
                                        div().flex_1().min_w_0().child(self.model_search.clone()),
                                    ),
                            )
                            .child(
                                icon_button(
                                    "refresh-model-catalog",
                                    "refresh",
                                    "Refresh available models",
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.load_picker_catalogs(true);
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child(
                        column()
                            .id("model-results")
                            .track_scroll(&self.picker_scroll)
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px(px(10.))
                            .pb(px(10.))
                            .gap(px(2.))
                            .child(
                                label(format!("{} MODELS", total))
                                    .px_2()
                                    .py_2()
                                    .text_size(px(10.))
                                    .text_color(rgb(FAINT)),
                            )
                            .when(
                                query.is_empty()
                                    && self.picker_scope.is_none()
                                    && !self.preferences.recent_models.is_empty(),
                                |d| {
                                    let recent = self
                                        .preferences
                                        .recent_models
                                        .iter()
                                        .filter(|choice| {
                                            self.model_provider_allowed(&choice.provider)
                                        })
                                        .filter_map(|choice| {
                                            self.picker_catalog
                                                .providers
                                                .get(&choice.provider)
                                                .and_then(|c| {
                                                    c.models.iter().find(|m| m.id == choice.model)
                                                })
                                                .map(|m| (choice, m))
                                        })
                                        .take(3)
                                        .collect::<Vec<_>>();
                                    d.when(!recent.is_empty(), |d| {
                                        d.child(
                                            label("RECENT")
                                                .px_2()
                                                .py_1()
                                                .text_size(px(10.))
                                                .text_color(rgb(FAINT)),
                                        )
                                        .children(
                                            recent.into_iter().enumerate().map(
                                                |(i, (choice, model))| {
                                                    self.model_row(
                                                        &choice.provider,
                                                        model,
                                                        10000 + i,
                                                        cx,
                                                    )
                                                },
                                            ),
                                        )
                                    })
                                },
                            )
                            .children(
                                self.providers
                                    .iter()
                                    .filter(|p| {
                                        self.model_provider_allowed(
                                            p["id"].as_str().unwrap_or_default(),
                                        )
                                    })
                                    .filter(|p| {
                                        (p["installed"] != false && p["enabled"] != false)
                                            || (!self.model_picker_default
                                                && self.selected.is_some()
                                                && p["id"] == current)
                                    })
                                    .filter(|p| {
                                        self.picker_scope
                                            .as_deref()
                                            .is_none_or(|scope| p["id"] == scope)
                                    })
                                    .map(|p| {
                                        let provider = p["id"].as_str().unwrap_or_default();
                                        let name = p["name"].as_str().unwrap_or(provider);
                                        let catalog = self.picker_catalog.providers.get(provider);
                                        let matches = catalog
                                            .into_iter()
                                            .flat_map(|c| c.models.iter())
                                            .filter(|m| {
                                                query.is_empty()
                                                    || [
                                                        m.label.as_str(),
                                                        m.id.as_str(),
                                                        name,
                                                        m.description
                                                            .as_deref()
                                                            .unwrap_or_default(),
                                                    ]
                                                    .iter()
                                                    .any(|s| s.to_lowercase().contains(&query))
                                            })
                                            .collect::<Vec<_>>();
                                        let installed = p["installed"] != false;
                                        let mut group =
                                            column().flex_shrink_0().gap_1().pb_2().child(
                                                row()
                                                    .px_2()
                                                    .pt_2()
                                                    .pb_1()
                                                    .gap_2()
                                                    .child(
                                                        icon(provider_icon(provider)).size(px(14.)),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(11.))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .child(name.to_owned()),
                                                    )
                                                    .when(current == provider, |d| {
                                                        d.child(
                                                            label("CURRENT")
                                                                .text_size(px(8.))
                                                                .px_1()
                                                                .rounded_sm()
                                                                .bg(rgb(0x343d22))
                                                                .text_color(rgb(ACCENT)),
                                                        )
                                                    })
                                                    .child(div().flex_1())
                                                    .child(
                                                        label(matches.len().to_string())
                                                            .text_size(px(10.))
                                                            .text_color(rgb(FAINT)),
                                                    ),
                                            );
                                        if let Some(error) = catalog.and_then(|c| c.error.as_ref())
                                        {
                                            group = group.child(
                                                column().px_2().py_2().gap_1().child(
                                                    label(error.clone())
                                                        .text_size(px(10.))
                                                        .text_color(rgb(0xe9bb92)),
                                                ),
                                            );
                                        }
                                        if catalog.is_some_and(|c| c.request.is_some()) {
                                            group = group.child(
                                                label("Checking available models…")
                                                    .px_2()
                                                    .py_2()
                                                    .text_size(px(11.)),
                                            );
                                        }
                                        if installed
                                            && !catalog.is_some_and(|c| {
                                                c.models.iter().any(|m| m.id == "default")
                                            })
                                            && (query.is_empty()
                                                || "agent default".contains(&query))
                                        {
                                            let provider = provider.to_owned();
                                            group = group.child(
                                                control(
                                                    SharedString::from(format!(
                                                        "default-model-{provider}"
                                                    )),
                                                    "Agent default",
                                                )
                                                .h(px(32.))
                                                .flex_shrink_0()
                                                .text_color(rgb(TEXT))
                                                .when(
                                                    current == provider && self.model.is_none(),
                                                    |d| {
                                                        d.bg(rgb(0x2c351d))
                                                            .child(div().flex_1())
                                                            .child(
                                                                icon("check")
                                                                    .size(px(12.))
                                                                    .text_color(rgb(ACCENT)),
                                                            )
                                                    },
                                                )
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.choose_model(
                                                            provider.clone(),
                                                            None,
                                                            cx,
                                                        );
                                                        window.focus(
                                                            &this.input.focus_handle(cx),
                                                            cx,
                                                        );
                                                    },
                                                )),
                                            );
                                        }
                                        if !installed {
                                            group = group.child(
                                                label("Not installed on this device")
                                                    .px_2()
                                                    .py_2()
                                                    .text_size(px(10.)),
                                            );
                                        }
                                        if matches.is_empty() && !query.is_empty() {
                                            group = group.child(
                                                label("No matching models")
                                                    .px_2()
                                                    .py_1()
                                                    .text_size(px(10.)),
                                            );
                                        }
                                        group
                                            .children(
                                                matches.into_iter().enumerate().map(|(i, m)| {
                                                    self.model_row(provider, m, i, cx)
                                                }),
                                            )
                                            .into_any_element()
                                    }),
                            ),
                    )
                    .when(
                        !self.model_picker_default
                            && self.selected_model().is_some_and(|m| {
                                !m.reasoning_levels.is_empty() || !m.options.is_empty()
                            }),
                        |d| d.child(self.model_options_view(cx)),
                    ),
            )
            .into_any_element()
    }
    fn picker_matches(&self, cx: &App) -> Vec<ModelSelection> {
        let query = self.model_search.read(cx).value(cx).to_lowercase();
        let current = if self.model_picker_default {
            self.preferences
                .default_provider
                .as_deref()
                .unwrap_or_default()
        } else {
            self.providers
                .get(self.provider_index)
                .and_then(|p| p["id"].as_str())
                .unwrap_or_default()
        };
        let mut matches = Vec::new();
        for provider in &self.providers {
            let id = provider["id"].as_str().unwrap_or_default();
            if !self.model_provider_allowed(id) {
                continue;
            }
            if (provider["installed"] == false || provider["enabled"] == false)
                && (self.model_picker_default || self.selected.is_none() || id != current)
            {
                continue;
            }
            if self
                .picker_scope
                .as_deref()
                .is_some_and(|scope| scope != id)
            {
                continue;
            }
            if let Some(catalog) = self.picker_catalog.providers.get(id) {
                for model in &catalog.models {
                    if query.is_empty()
                        || [
                            model.id.as_str(),
                            model.label.as_str(),
                            provider["name"].as_str().unwrap_or_default(),
                            model.description.as_deref().unwrap_or_default(),
                        ]
                        .iter()
                        .any(|text| text.to_lowercase().contains(&query))
                    {
                        matches.push(ModelSelection {
                            provider: id.to_owned(),
                            model: model.id.clone(),
                        });
                    }
                }
            }
        }
        matches
    }
    fn on_picker_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.model_picker {
            return;
        }
        let key = event.keystroke.key.as_str();
        if key == "escape" {
            let defaults = self.model_picker_default;
            self.model_picker = false;
            self.model_picker_default = false;
            self.return_model_picker_focus(defaults, window, cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.reasoning_focus.is_focused(window) {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        let shortcut =
            (modifiers.control || modifiers.platform) && !modifiers.alt && !modifiers.shift;
        if !matches!(key, "up" | "down" | "enter")
            && !(shortcut && key.len() == 1 && key >= "1" && key <= "9")
        {
            return;
        }
        cx.stop_propagation();
        let matches = self.picker_matches(cx);
        if matches.is_empty() {
            return;
        }
        let current = self
            .picker_active
            .as_ref()
            .and_then(|active| matches.iter().position(|m| m == active));
        let index = match key {
            "up" => current.unwrap_or(0).saturating_sub(1),
            "down" => current.map_or(0, |i| (i + 1).min(matches.len() - 1)),
            "enter" => current.unwrap_or(0),
            _ => key.parse::<usize>().unwrap_or(1) - 1,
        };
        let Some(choice) = matches.get(index).cloned() else {
            return;
        };
        if key == "enter" || shortcut {
            let defaults = self.model_picker_default;
            self.choose_model(choice.provider, Some(choice.model), cx);
            if !self.model_picker {
                self.return_model_picker_focus(defaults, window, cx);
            }
        } else {
            if let Some(anchor) = self
                .picker_anchors
                .get(&format!("{}:{}", choice.provider, choice.model))
            {
                anchor.scroll_to(window, cx);
            }
            self.picker_active = Some(choice);
            cx.notify();
        }
    }
    fn effort_control(&self) -> Option<zuse_protocol::presentation::ModelEffort> {
        zuse_protocol::presentation::model_effort(
            self.selected_model()?,
            self.reasoning,
            &self.model_options,
        )
    }
    pub(super) fn picker_effort_label(&self) -> Option<String> {
        let control = self.effort_control()?;
        Some(control.stops.get(control.selected)?.label.clone())
    }
    pub(super) fn select_effort(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(control) = self.effort_control() {
            // Catalog/open synchronization sets the current stop too; it must
            // not silently rewrite a stored override without a user movement.
            if index != control.selected {
                if !self.can_change_model(cx) {
                    self.configure_reasoning_slider(cx);
                    return;
                }
                if control.select(index, &mut self.reasoning, &mut self.model_options) {
                    self.model_choices_changed(cx);
                }
            }
        }
    }
    pub(super) fn configure_reasoning_slider(&mut self, cx: &mut Context<Self>) {
        let Some(control) = self.effort_control() else {
            return;
        };
        let max = control.stops.len().saturating_sub(1).max(1) as f32;
        let index = control.selected as f32;
        let current = self.reasoning_slider.read(cx);
        if current.max_value() != max || current.value().end() != index {
            self.reasoning_slider.update(cx, |state, cx| {
                *state = gpui_base::slider::SliderState::new()
                    .min(0.)
                    .max(max)
                    .step(1.)
                    .default_value(index);
                cx.notify();
            });
        }
    }
    fn reasoning_slider_view(&self, cx: &mut Context<Self>) -> AnyElement {
        use gpui_base::slider::{Slider, SliderIndicator, SliderThumb, SliderTrack};
        let Some(control) = self.effort_control() else {
            return div().into_any_element();
        };
        let current_label = control
            .stops
            .get(control.selected)
            .map(|stop| stop.label.clone())
            .unwrap_or_default();
        let state = &self.reasoning_slider;
        let percentage = state.read(cx).percentage().end;
        let steps = state.read(cx).max_value() as usize;
        column()
            .gap_2()
            .child(
                row()
                    .child(
                        label(control.label.clone())
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM),
                    )
                    .child(div().flex_1())
                    .child(
                        label(current_label)
                            .text_size(px(11.))
                            .text_color(rgb(TEXT)),
                    ),
            )
            .child(
                div()
                    .id("reasoning-control")
                    .track_focus(&self.reasoning_focus)
                    .tab_stop(true)
                    .role(Role::Group)
                    .aria_label(control.label)
                    .relative()
                    .h(px(32.))
                    .px_2()
                    // The thumb consumes its bubble-phase pointer event, even
                    // when it stays at the current value and emits no change.
                    .capture_any_mouse_down(cx.listener(
                        |this, event: &MouseDownEvent, window, cx| {
                            if event.button == MouseButton::Left {
                                window.focus(&this.reasoning_focus, cx);
                            }
                        },
                    ))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        let current = this.reasoning_slider.read(cx).value().end();
                        let max = this.reasoning_slider.read(cx).max_value();
                        let value = match event.keystroke.key.as_str() {
                            "left" | "down" => (current - 1.).max(0.),
                            "right" | "up" => (current + 1.).min(max),
                            "home" => 0.,
                            "end" => max,
                            _ => return,
                        };
                        this.reasoning_slider
                            .update(cx, |state, cx| state.set_value(value, window, cx));
                        cx.stop_propagation();
                    }))
                    .child(
                        Slider::new(state).w_full().h(px(32.)).child(
                            SliderTrack::new(state)
                                .relative()
                                .w_full()
                                .h_full()
                                .child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .right_0()
                                        .top(px(6.))
                                        .h(px(20.))
                                        .rounded_full()
                                        .bg(rgb(0x32372f)),
                                )
                                .child(
                                    SliderIndicator::new(state)
                                        .absolute()
                                        .left_0()
                                        .right_0()
                                        .top(px(6.))
                                        .h(px(20.))
                                        .child(
                                            div()
                                                .absolute()
                                                .inset_0()
                                                .right(relative(1. - percentage))
                                                .rounded_full()
                                                .bg(rgb(0x89ca21)),
                                        ),
                                )
                                .children((1..steps).map(|step| {
                                    div()
                                        .absolute()
                                        .left(relative(step as f32 / steps.max(1) as f32))
                                        .top(px(13.))
                                        .ml(px(-3.))
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg(rgb(0x858e7c))
                                }))
                                .child(
                                    SliderThumb::new(state)
                                        .absolute()
                                        .left(relative(percentage))
                                        .top(px(4.))
                                        .ml(px(-12.))
                                        .size(px(24.))
                                        .rounded_full()
                                        .border_2()
                                        .border_color(rgb(0x89ca21))
                                        .bg(rgb(0xffffff)),
                                ),
                        ),
                    ),
            )
            .into_any_element()
    }
    fn model_options_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(model) = self.selected_model() else {
            return div().into_any_element();
        };
        let effort = self.effort_control();
        let hidden_option = effort.as_ref().and_then(|control| control.option_id());
        column()
            .flex_shrink_0()
            .p(px(10.))
            .gap_2()
            .border_t_1()
            .border_color(rgb(BORDER))
            .bg(crate::settings_style::frame_bg().opacity(0.15))
            .when(effort.is_some(), |d| {
                d.child(self.reasoning_slider_view(cx))
            })
            .children(
                model
                    .options
                    .iter()
                    .enumerate()
                    .filter(|(_, option)| hidden_option != Some(option.id.as_str()))
                    .map(|(i, option)| {
                        let id = option.id.clone();
                        let selected = self
                            .model_options
                            .get(&id)
                            .and_then(Value::as_str)
                            .unwrap_or(&option.default_choice);
                        if let Some((on, off)) =
                            zuse_protocol::presentation::model_option_switch_values(
                                &option.id,
                                option.choices.iter().map(|choice| choice.id.as_str()),
                            )
                        {
                            let checked = selected == on;
                            let value = if checked { off } else { on }.to_owned();
                            let current_label = option
                                .choices
                                .iter()
                                .find(|choice| choice.id == selected)
                                .map(|choice| choice.label.clone())
                                .unwrap_or_default();
                            return row()
                                .justify_between()
                                .gap_3()
                                .min_h(px(28.))
                                .child(label(option.label.clone()).text_size(px(11.)))
                                .child(
                                    row()
                                        .gap_2()
                                        .child(label(current_label).text_size(px(11.)))
                                        .child(
                                            crate::settings_style::toggle(
                                                SharedString::from(format!(
                                                    "model-option-switch-{i}"
                                                )),
                                                checked,
                                                &option.label,
                                            )
                                            .tab_index(0)
                                            .on_click(
                                                cx.listener({
                                                    let id = id.clone();
                                                    let value = value.clone();
                                                    move |this, _, _, cx| {
                                                        this.select_model_option(
                                                            id.clone(),
                                                            value.clone(),
                                                            cx,
                                                        );
                                                    }
                                                }),
                                            ),
                                        ),
                                )
                                .into_any_element();
                        }
                        column()
                            .gap_1()
                            .child(label(option.label.clone()).text_size(px(11.)))
                            .child(row().gap_1().flex_wrap().children(
                                option.choices.iter().enumerate().map(|(j, choice)| {
                                    let id = id.clone();
                                    let value = choice.id.clone();
                                    control(
                                        SharedString::from(format!("model-option-{i}-{j}")),
                                        SharedString::from(choice.label.clone()),
                                    )
                                    .h(px(26.))
                                    .text_size(px(10.))
                                    .when(selected == choice.id, |d| {
                                        d.bg(rgb(0x343d22)).text_color(rgb(ACCENT))
                                    })
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.select_model_option(id.clone(), value.clone(), cx);
                                        },
                                    ))
                                }),
                            ))
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }
}
