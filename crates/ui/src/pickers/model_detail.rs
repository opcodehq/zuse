//! The model menu's detail card: what the hovered (or keyboard-highlighted)
//! model offers, and its reasoning, fast mode and other options.
//!
//! The card always describes one row. Its values are what picking that row
//! would run with; changing any of them applies to that model and selects it,
//! so a click on the row afterwards (or closing the menu) keeps them.
use super::*;
use crate::tooltip::TooltipExt as _;

pub(super) const CARD_WIDTH: f32 = 272.;
const PAD: f32 = 14.;
const SLIDER_HEIGHT: f32 = 28.;
const SLIDER_RADIUS: f32 = 8.;
const KNOB_WIDTH: f32 = 12.;
const KNOB_HEIGHT: f32 = 20.;

/// Everything the card shows for one row.
pub(super) struct Detail {
    pub row: ModelRowData,
    pub effort: Option<compact::CompactEffort>,
    /// Fast mode offered by the model, and whether it is on.
    pub fast: Option<bool>,
    /// The remaining options (context window, ...), excluding the ones the
    /// slider and the fast-mode switch own.
    pub options: Vec<SettingGroup>,
    pub favorite: bool,
    pub is_default: bool,
}

impl Pickers {
    /// The reasoning level the model trigger names after the model, for
    /// every model that has one (a reasoning ladder or an effort option).
    pub(super) fn chip_reasoning_label(&self, cx: &App) -> Option<SharedString> {
        if self.is_settings_model() {
            return None;
        }
        let effort = self.compact_effort(cx)?;
        effort.labels.get(effort.selected).cloned()
    }

    /// The row the card describes: the hovered or keyboard-highlighted row,
    /// which starts on the selected model when the menu opens.
    pub(super) fn detail_row(&self, cx: &App) -> Option<ModelRowData> {
        if self.is_settings_model() {
            return None;
        }
        self.model_rows(cx)
            .get(self.active)
            .filter(|row| !row.selected_only)
            .cloned()
    }

    fn row_ladder(&self, row: &ModelRowData, cx: &App) -> Vec<ReasoningLevel> {
        if !row.model.reasoning_levels.is_empty() {
            return row.model.reasoning_levels.clone();
        }
        self.harness_catalog(cx)
            .ready()
            .and_then(|list| list.iter().find(|d| d.id == row.harness))
            .map(|d| d.reasoning_levels.clone())
            .unwrap_or_default()
    }

    /// What picking `row` would run with — the same resolution
    /// [`Self::pick_harness`] and [`Self::pick_model`] apply: an existing
    /// chat keeps its level (clamped) on the same provider and takes the
    /// provider's remembered picks on another; a new chat takes the level
    /// and options last used with that model.
    fn predicted_settings(
        &self,
        row: &ModelRowData,
        levels: &[ReasoningLevel],
        cx: &App,
    ) -> (
        Option<ReasoningLevel>,
        serde_json::Map<String, serde_json::Value>,
    ) {
        let existing =
            self.state.read(cx).selected_chat.is_some() || self.cloud.task_binding.is_some();
        let same_provider = self.effective_harness(cx) == Some(row.harness);
        let (reasoning, options) = if existing && same_provider {
            (self.explicit_reasoning(cx), self.explicit_options(cx))
        } else if existing {
            let remembered = self.defaults.model_for(row.harness).map(|m| m.id.as_str());
            (
                self.defaults.reasoning_for(row.harness, remembered),
                remembered
                    .and_then(|model| self.defaults.model_options_for(row.harness, model))
                    .cloned()
                    .unwrap_or_default(),
            )
        } else {
            (
                self.defaults
                    .reasoning_for(row.harness, Some(&row.model.id))
                    .or(self.defaults.reasoning),
                self.defaults
                    .model_options_for(row.harness, &row.model.id)
                    .cloned()
                    .unwrap_or_default(),
            )
        };
        (
            clamp_reasoning(reasoning, levels),
            offered_options(&row.model, options),
        )
    }

    pub(super) fn model_detail(&self, cx: &App) -> Option<Detail> {
        let row = self.detail_row(cx)?;
        let selected = self.row_is_selected(&row, cx);
        let configurable = self.model_configuration_available(cx);
        let (effort, options) = if !configurable {
            (None, serde_json::Map::new())
        } else if selected {
            (self.compact_effort(cx), self.explicit_options(cx))
        } else {
            let levels = self.row_ladder(&row, cx);
            let (reasoning, options) = self.predicted_settings(&row, &levels, cx);
            (
                compact::effort_stops(levels, reasoning, Some(&row.model), &options),
                options,
            )
        };
        let fast_option = configurable
            .then(|| {
                row.model
                    .options
                    .iter()
                    .find_map(|o| fast_mode_values(o).map(|(on, _)| (o, on)))
            })
            .flatten();
        let fast = fast_option.map(|(option, on)| {
            options
                .get(&option.id)
                .and_then(|v| v.as_str())
                .unwrap_or(&option.default_choice)
                == on
        });
        let hidden: Vec<&str> = effort
            .as_ref()
            .and_then(|e| e.option_id())
            .into_iter()
            .chain(fast_option.map(|(o, _)| o.id.as_str()))
            .collect();
        let groups = if configurable {
            build_setting_groups(Vec::new(), None, Some(&row.model), &options)
                .into_iter()
                .filter(
                    |g| !matches!(&g.id, ModelSetting::Option(id) if hidden.contains(&id.as_str())),
                )
                .collect()
        } else {
            Vec::new()
        };
        Some(Detail {
            favorite: self.defaults.is_favorite(row.harness, &row.model.id),
            is_default: crate::settings::default_model(cx)
                .is_some_and(|m| m.provider == row.harness && m.model == row.model.id),
            effort,
            fast,
            options: groups,
            row,
        })
    }

    /// Keep the highlight on `harness`/`model` after the list re-sorts.
    fn follow_row(&mut self, harness: HarnessId, model: &str, cx: &App) {
        if let Some(ix) = self
            .model_rows(cx)
            .iter()
            .position(|row| row.harness == harness && row.model.id == model)
        {
            self.active = ix;
        }
    }

    /// Select the card's model (without closing the menu) so a setting
    /// changed in the card applies to it. False when it can't be selected
    /// (a chat locked to another provider).
    pub(super) fn select_detail_model(
        &mut self,
        harness: HarnessId,
        model: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let row = self
            .model_rows(cx)
            .iter()
            .find(|row| row.harness == harness && row.model.id == model)
            .cloned();
        let Some(row) = row else {
            return false;
        };
        if self.row_is_selected(&row, cx) {
            return true;
        }
        if self.effective_harness(cx) != Some(harness) {
            if self.harness_locked(cx) {
                return false;
            }
            self.pick_harness(harness, cx);
        }
        self.pick_model(model.to_owned(), cx);
        self.follow_row(harness, model, cx);
        self.row_is_selected(&row, cx)
    }

    /// Set the card model's effort stop `index` (selecting the model first).
    pub(super) fn set_detail_effort(
        &mut self,
        harness: HarnessId,
        model: &str,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        if !self.select_detail_model(harness, model, cx) {
            return;
        }
        if let Some(effort) = self.compact_effort(cx) {
            self.pick_compact_effort(effort, index, cx);
        }
        cx.notify();
    }

    /// Turn the card model's fast mode on or off (selecting the model first).
    pub(super) fn set_detail_fast(
        &mut self,
        harness: HarnessId,
        model: &str,
        on: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.select_detail_model(harness, model, cx) {
            return;
        }
        if let Some((option, next, default, current)) = self.compact_fast_choice(cx)
            && current != on
        {
            self.pick_option(option, next, default, cx);
        }
        cx.notify();
    }

    /// Choose one of the card model's option values (selecting it first).
    pub(super) fn set_detail_option(
        &mut self,
        harness: HarnessId,
        model: &str,
        option: String,
        choice: String,
        default: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.select_detail_model(harness, model, cx) {
            return;
        }
        self.pick_option(option, choice, default, cx);
        cx.notify();
    }

    /// Star or unstar the card's model; the highlight follows it as the
    /// list moves it into or out of Favorites.
    pub(super) fn toggle_detail_favorite(
        &mut self,
        harness: HarnessId,
        model: &str,
        cx: &mut Context<Self>,
    ) {
        self.reload_defaults();
        self.defaults.toggle_favorite(harness, model);
        self.persist_defaults();
        self.catalog_rev += 1;
        self.follow_row(harness, model, cx);
        self.scroll_to_model_row(self.active, gpui::ScrollStrategy::Nearest, cx);
        cx.notify();
    }

    /// Make the card's model the default for new chats (Settings → Default
    /// models).
    pub(super) fn set_detail_default(
        &mut self,
        harness: HarnessId,
        model: &str,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        let selection = zuse_protocol::presentation::DefaultModelSelection {
            provider: harness,
            model: model.to_owned(),
            label: label.to_owned(),
        };
        crate::settings::update(crate::settings::SavePolicy::Immediate, cx, |settings| {
            settings.default_model = Some(selection);
        });
        self.catalog_rev += 1;
        cx.refresh_windows();
        cx.notify();
    }

    pub(super) fn render_model_detail(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let detail = self.model_detail(cx)?;
        let theme = Theme::of(cx).for_popup();
        let ink = popover::dither_ink(&theme);
        let harness = detail.row.harness;
        let model_id: SharedString = detail.row.model.id.clone().into();
        let label: SharedString = detail.row.model.label.clone().into();
        let (path, tint) = harness_brand_icon(harness);
        let divider = || div().h(px(1.)).flex_none().bg(theme.border.opacity(0.5));
        let info = |name: &'static str, value: SharedString| {
            div()
                .h(px(20.))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .text_size(crate::typography::text_xs())
                .child(div().text_color(theme.text_muted).child(name))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text)
                        .child(value),
                )
        };
        let header = div()
            .relative()
            .px(px(PAD))
            .pt(px(12.))
            .pb(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .w(px(150.))
                    .h(px(44.))
                    .child(zuse_viewport::dither::corner(
                        ink.opacity(0.55),
                        popover::CARD_RADIUS,
                    )),
            )
            .child(
                div()
                    .id("model-detail-name")
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(crate::icons::brand_icon(
                        path,
                        16.,
                        tint.unwrap_or(theme.text),
                    ))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::text_sm())
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text_strong)
                            .child(label.clone()),
                    )
                    .when_some(detail.row.model.description.clone(), |el, description| {
                        el.tip(description)
                    }),
            )
            .child(info("Provider", detail.row.harness_name.clone()));

        let mut settings = div()
            .px(px(PAD))
            .pt(px(10.))
            .pb(px(10.))
            .flex()
            .flex_col()
            .gap(px(8.));
        let mut has_settings = false;
        if let Some(effort) = detail.effort.as_ref() {
            has_settings = true;
            let current = effort
                .labels
                .get(effort.selected)
                .cloned()
                .unwrap_or_default();
            settings = settings.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(crate::typography::text_xs())
                    .child(div().text_color(theme.text_muted).child("Reasoning"))
                    .child(div().text_color(theme.text).child(current)),
            );
            if effort.labels.len() > 1 {
                settings = settings.child(self.detail_slider(
                    effort,
                    harness,
                    model_id.clone(),
                    &theme,
                    cx,
                ));
            }
        }
        if let Some(on) = detail.fast {
            has_settings = true;
            let toggle_model = model_id.clone();
            settings = settings.child(
                div()
                    .id("model-detail-fast")
                    .role(gpui::Role::Switch)
                    .aria_label("Fast mode")
                    .aria_toggled(if on {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .tab_index(0)
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .text_size(crate::typography::text_xs())
                    .tip("Faster responses, more usage")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_detail_fast(harness, &toggle_model, !on, cx);
                    }))
                    .child(
                        crate::icons::icon(if on {
                            crate::icons::FAST_TIER_BOLD
                        } else {
                            crate::icons::FAST_TIER
                        })
                        .size(px(14.))
                        .text_color(if on {
                            theme.accent
                        } else {
                            theme.text_muted
                        }),
                    )
                    .child(div().flex_1().text_color(theme.text).child("Fast mode"))
                    .child(crate::settings::widgets::toggle_switch(
                        &theme,
                        on,
                        "model-detail-fast-switch",
                    )),
            );
        }
        for (gx, group) in detail.options.iter().enumerate() {
            has_settings = true;
            settings =
                settings.child(self.detail_option(gx, group, harness, &model_id, &theme, cx));
        }

        let favorite = detail.favorite;
        let favorite_model = model_id.clone();
        let default_model = model_id.clone();
        let default_label = label.clone();
        let is_default = detail.is_default;
        let button = |id: &'static str| {
            div()
                .id(id)
                .role(gpui::Role::Button)
                .tab_index(0)
                .flex_1()
                .h(px(30.))
                .rounded(px(8.))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .text_size(crate::typography::text_xs())
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text)
                .bg(theme.muted_fill().opacity(0.5))
        };
        let actions = div()
            .p(px(8.))
            .flex()
            .gap(px(6.))
            .child(
                button("model-detail-favorite")
                    .aria_label(if favorite {
                        "Remove from favorites"
                    } else {
                        "Add to favorites"
                    })
                    .aria_toggled(if favorite {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .tip(if favorite {
                        "Remove from favorites"
                    } else {
                        "Add to favorites"
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.muted_fill()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_detail_favorite(harness, &favorite_model, cx);
                    }))
                    .child(
                        crate::icons::icon(if favorite {
                            crate::icons::STAR_BOLD
                        } else {
                            crate::icons::STAR
                        })
                        .size(px(13.))
                        .text_color(if favorite {
                            theme.accent
                        } else {
                            theme.text_muted
                        }),
                    )
                    .child(if favorite {
                        "Favorite"
                    } else {
                        "Add to favorites"
                    }),
            )
            .child(
                button("model-detail-default")
                    .aria_label(if is_default {
                        "Default model"
                    } else {
                        "Set as default model"
                    })
                    .tip(if is_default {
                        "New chats start with this model"
                    } else {
                        "Start new chats with this model"
                    })
                    .when(is_default, |el| el.text_color(theme.text_muted))
                    .when(!is_default, |el| {
                        el.cursor_pointer()
                            .hover(|s| s.bg(theme.muted_fill()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.set_detail_default(
                                    harness,
                                    &default_model,
                                    &default_label,
                                    cx,
                                );
                            }))
                    })
                    .when(is_default, |el| {
                        el.child(
                            crate::icons::icon(crate::icons::CHECK)
                                .size(px(13.))
                                .text_color(theme.accent),
                        )
                    })
                    .child(if is_default {
                        "Default"
                    } else {
                        "Set as default"
                    }),
            );

        let entity = cx.entity().downgrade();
        Some(
            popover::popover_card_flush(&theme)
                .id("model-detail-card")
                .w(px(CARD_WIDTH))
                .flex()
                .flex_col()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(header)
                .when(has_settings, |card| card.child(divider()).child(settings))
                .child(divider())
                .child(actions)
                .child(
                    gpui::canvas(
                        move |bounds, _, cx| {
                            let _ = entity.update(cx, |this, _| {
                                this.model_menu.card_bounds = Some(bounds);
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .into_any_element(),
        )
    }

    /// The reasoning slider: a rounded track with a dithered fill up to a
    /// knob that rests on the model's reasoning levels.
    fn detail_slider(
        &self,
        effort: &compact::CompactEffort,
        harness: HarnessId,
        model: SharedString,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = effort.labels.len();
        let selected = effort.selected;
        let width = CARD_WIDTH - 2. * PAD;
        let inset = compact::THUMB_INSET;
        let stop_x = |index: usize| {
            inset + index as f32 / count.saturating_sub(1).max(1) as f32 * (width - 2. * inset)
        };
        let knob_x = stop_x(selected);
        let ink = popover::dither_ink(theme);
        let label = effort.labels.get(selected).cloned().unwrap_or_default();
        let entity = cx.entity().downgrade();
        let down_model = model.clone();
        let key_model = model.clone();
        div()
            .id("model-detail-effort")
            .role(gpui::Role::Slider)
            .tab_index(0)
            .aria_label(SharedString::from(format!("Reasoning: {label}")))
            .aria_numeric_value(selected as f64)
            .aria_min_numeric_value(0.)
            .aria_max_numeric_value(count.saturating_sub(1) as f64)
            .relative()
            .w(px(width))
            .h(px(SLIDER_HEIGHT))
            .rounded(px(SLIDER_RADIUS))
            .bg(theme.muted_fill().opacity(0.55))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if this.select_detail_model(harness, &down_model, cx) {
                        this.effort_dragging = true;
                        this.pick_effort_at(event.position.x, cx);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.effort_dragging && event.pressed_button == Some(gpui::MouseButton::Left) {
                    this.pick_effort_at(event.position.x, cx);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.effort_dragging = false;
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.effort_dragging = false;
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                let Some(detail) = this.model_detail(cx) else {
                    return;
                };
                let Some(effort) = detail.effort else {
                    return;
                };
                let last = effort.labels.len().saturating_sub(1);
                let index = match event.keystroke.key.as_str() {
                    "left" | "down" => effort.selected.saturating_sub(1),
                    "right" | "up" => (effort.selected + 1).min(last),
                    "home" => 0,
                    "end" => last,
                    _ => return,
                };
                cx.stop_propagation();
                this.set_detail_effort(harness, &key_model, index, cx);
            }))
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        let _ = entity.update(cx, |this, _| this.effort_bounds = Some(bounds));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(knob_x + KNOB_WIDTH / 2. + 3.))
                    .child(zuse_viewport::dither::ramp(ink, SLIDER_RADIUS)),
            )
            .children(((selected + 1)..count).map(|index| {
                div()
                    .absolute()
                    .left(px(stop_x(index) - 0.5))
                    .top(px((SLIDER_HEIGHT - 8.) / 2.))
                    .w(px(1.))
                    .h(px(8.))
                    .rounded_full()
                    .bg(theme.text_muted.opacity(0.45))
            }))
            .child(
                div()
                    .absolute()
                    .left(px(knob_x - KNOB_WIDTH / 2.))
                    .top(px((SLIDER_HEIGHT - KNOB_HEIGHT) / 2.))
                    .w(px(KNOB_WIDTH))
                    .h(px(KNOB_HEIGHT))
                    .rounded(px(5.))
                    .bg(gpui::white())
                    .border_1()
                    .border_color(ink)
                    .shadow_sm(),
            )
            .into_any_element()
    }

    /// One more option of the card's model: a switch for two-state options,
    /// otherwise a compact segmented choice.
    fn detail_option(
        &self,
        gx: usize,
        group: &SettingGroup,
        harness: HarnessId,
        model: &SharedString,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ModelSetting::Option(option) = group.id.clone() else {
            return gpui::Empty.into_any_element();
        };
        let label = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(theme.text)
            .child(SharedString::from(group.label.clone()));
        if is_toggle(group) {
            let on = toggle_group_is_on(group);
            let next = zuse_protocol::presentation::model_option_switch_values(
                &option,
                group.choices.iter().map(|c| c.value.as_str()),
            )
            .map(|(on_value, off_value)| if on { off_value } else { on_value }.to_owned());
            let next_default = next
                .as_ref()
                .is_some_and(|next| group.choices.iter().any(|c| c.default && c.value == *next));
            let model = model.clone();
            return div()
                .id(("model-detail-option", gx))
                .role(gpui::Role::Switch)
                .aria_label(SharedString::from(group.label.clone()))
                .aria_toggled(if on {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .tab_index(0)
                .h(px(26.))
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .text_size(crate::typography::text_xs())
                .when_some(next, |el, next| {
                    el.on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_detail_option(
                            harness,
                            &model,
                            option.clone(),
                            next.clone(),
                            next_default,
                            cx,
                        );
                    }))
                })
                .child(label)
                .child(crate::settings::widgets::toggle_switch(
                    theme,
                    on,
                    SharedString::from(format!("model-detail-option-switch-{gx}")),
                ))
                .into_any_element();
        }
        let segments = group.choices.iter().enumerate().map(|(cx_ix, choice)| {
            let model = model.clone();
            let option = option.clone();
            let value = choice.value.clone();
            let default = choice.default;
            div()
                .id(SharedString::from(format!(
                    "model-detail-option-{gx}-{cx_ix}"
                )))
                .role(gpui::Role::RadioButton)
                .aria_label(SharedString::from(choice.label.clone()))
                .aria_selected(choice.selected)
                .tab_index(0)
                .flex_1()
                .min_w_0()
                .h(px(22.))
                .px(px(6.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(if choice.selected {
                    theme.text
                } else {
                    theme.text_muted
                })
                .when(choice.selected, |el| model_selected(el, theme))
                .when(!choice.selected, |el| {
                    el.hover(|s| s.bg(theme.muted_fill().opacity(0.8)))
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.set_detail_option(
                        harness,
                        &model,
                        option.clone(),
                        value.clone(),
                        default,
                        cx,
                    );
                }))
                .child(
                    div()
                        .truncate()
                        .child(SharedString::from(choice.label.clone())),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .text_size(crate::typography::text_xs())
            .child(label)
            .child(
                div()
                    .p(px(2.))
                    .rounded(px(8.))
                    .bg(theme.muted_fill().opacity(0.45))
                    .flex()
                    .gap(px(2.))
                    .children(segments),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pickers::tests::{bare_model, descriptor};

    fn laddered(id: &str, label: &str, fast: bool) -> Model {
        let mut model = bare_model(id, label);
        model.reasoning_levels = vec![
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
        ];
        if fast {
            model.options = serde_json::from_value(serde_json::json!([
                {"id":"fastMode","label":"Fast mode","defaultChoice":"off",
                 "choices":[{"id":"off","label":"Off"},{"id":"on","label":"On"}]},
                {"id":"contextWindow","label":"Context window","defaultChoice":"200k",
                 "choices":[{"id":"200k","label":"200K"},{"id":"1m","label":"1M"}]}
            ]))
            .unwrap();
        }
        model
    }

    fn picker_window(
        cx: &mut gpui::TestAppContext,
    ) -> (tempfile::TempDir, gpui::WindowHandle<Pickers>) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            let mut settings = crate::settings::UiSettings::default();
            settings.compact_model_picker = true;
            crate::settings::init(settings, dir.path(), cx);
        });
        let handle = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            let mut picker = Pickers::new(state, cx);
            picker.harnesses = Loadable::Ready(vec![
                descriptor(HarnessId::Codex, "Codex"),
                descriptor(HarnessId::ClaudeCode, "Claude"),
            ]);
            picker.models.insert(
                HarnessId::Codex,
                Loadable::Ready(vec![
                    laddered("gpt-a", "GPT A", false),
                    bare_model("gpt-plain", "GPT Plain"),
                ]),
            );
            picker.models.insert(
                HarnessId::ClaudeCode,
                Loadable::Ready(vec![laddered("opus", "Opus", true)]),
            );
            picker.config.harness = Some(HarnessId::Codex);
            picker.config.model = Some("gpt-a".into());
            picker
        });
        (dir, handle)
    }

    fn row_index(picker: &Pickers, model: &str, cx: &App) -> usize {
        picker
            .model_rows(cx)
            .iter()
            .position(|row| row.model.id == model)
            .unwrap()
    }

    #[gpui::test]
    fn one_trigger_names_the_model_and_its_reasoning(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = picker_window(cx);
        handle
            .update(cx, |picker, _, cx| {
                // The default level of a laddered model, then the picked one.
                assert_eq!(picker.chip_reasoning_label(cx).as_deref(), Some("High"));
                picker.pick_reasoning(ReasoningLevel::Low, cx);
                assert_eq!(picker.chip_reasoning_label(cx).as_deref(), Some("Low"));
                // Every provider's laddered models name their level too.
                picker.pick_harness(HarnessId::ClaudeCode, cx);
                picker.pick_model("opus".into(), cx);
                assert!(picker.chip_reasoning_label(cx).is_some());
                // A model without reasoning names nothing after it.
                picker.pick_harness(HarnessId::Codex, cx);
                picker.pick_model("gpt-plain".into(), cx);
                assert_eq!(picker.chip_reasoning_label(cx), None);
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
    }

    #[gpui::test]
    fn fast_mode_and_options_show_only_for_models_that_offer_them(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = picker_window(cx);
        handle
            .update(cx, |picker, window, cx| {
                picker.open_model_menu(window, cx);
                let detail = picker.model_detail(cx).unwrap();
                assert_eq!(detail.row.model.id, "gpt-a", "starts on the selection");
                assert_eq!(detail.fast, None);
                assert!(detail.options.is_empty());
                assert_eq!(detail.effort.as_ref().unwrap().labels.len(), 3);

                picker.active = row_index(picker, "opus", cx);
                let detail = picker.model_detail(cx).unwrap();
                assert_eq!(detail.fast, Some(false));
                // Fast mode has its own switch; the context window remains.
                assert_eq!(detail.options.len(), 1);
                assert_eq!(
                    detail.options[0].id,
                    ModelSetting::Option("contextWindow".into())
                );

                picker.active = row_index(picker, "gpt-plain", cx);
                let detail = picker.model_detail(cx).unwrap();
                assert!(detail.effort.is_none() && detail.fast.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn hovering_a_row_shows_its_card_and_the_card_selects_with_its_settings(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_dir, handle) = picker_window(cx);
        handle
            .update(cx, |picker, window, cx| {
                picker.open_model_menu(window, cx);
                let opus = row_index(picker, "opus", cx);
                let gpt = row_index(picker, "gpt-a", cx);
                // The card sits to the right of the list.
                picker.model_menu.card_left = false;
                picker.model_menu.card_bounds = Some(gpui::Bounds::new(
                    gpui::point(px(400.), px(0.)),
                    gpui::size(px(272.), px(300.)),
                ));
                picker.hover_model_row(opus, gpui::point(px(100.), px(10.)), true, cx);
                assert_eq!(picker.active, opus);
                assert_eq!(picker.detail_row(cx).unwrap().model.id, "opus");

                // Travelling toward the card across another row keeps the
                // card on the hovered model.
                picker.hover_model_row(gpt, gpui::point(px(160.), px(30.)), true, cx);
                assert_eq!(picker.active, opus);
                assert_eq!(picker.detail_row(cx).unwrap().model.id, "opus");

                // Hovering alone selects nothing.
                assert_eq!(picker.resolved(cx).model.as_deref(), Some("gpt-a"));

                // A level set in the card selects its model with that level,
                // and the menu stays open on it.
                picker.set_detail_effort(HarnessId::ClaudeCode, "opus", 0, cx);
                assert!(picker.is_open());
                assert_eq!(picker.resolved(cx).harness, Some(HarnessId::ClaudeCode));
                assert_eq!(picker.resolved(cx).model.as_deref(), Some("opus"));
                assert_eq!(picker.effective_reasoning(cx), Some(ReasoningLevel::Low));
                assert_eq!(picker.detail_row(cx).unwrap().model.id, "opus");

                picker.set_detail_fast(HarnessId::ClaudeCode, "opus", true, cx);
                assert_eq!(picker.resolved(cx).model_options["fastMode"], "on");
                let detail = picker.model_detail(cx).unwrap();
                assert_eq!(detail.fast, Some(true));
                assert_eq!(detail.effort.unwrap().selected, 0);

                // Clicking the row keeps the card's settings and closes.
                let opus = row_index(picker, "opus", cx);
                picker.activate_model_index(opus, cx);
                assert!(!picker.is_open());
                assert_eq!(picker.effective_reasoning(cx), Some(ReasoningLevel::Low));
                assert_eq!(picker.resolved(cx).model_options["fastMode"], "on");
            })
            .unwrap();
    }

    #[gpui::test]
    fn card_favorites_lead_the_list_and_default_persists(cx: &mut gpui::TestAppContext) {
        let (dir, handle) = picker_window(cx);
        handle
            .update(cx, |picker, window, cx| {
                picker.open_model_menu(window, cx);
                picker.active = row_index(picker, "opus", cx);
                picker.toggle_detail_favorite(HarnessId::ClaudeCode, "opus", cx);
                assert_eq!(picker.model_rows(cx)[0].model.id, "opus");
                assert_eq!(picker.favorite_row_count(cx), 1);
                // The card stays on the starred model as the list re-sorts.
                assert_eq!(picker.active, 0);
                assert!(picker.model_detail(cx).unwrap().favorite);

                picker.set_detail_default(HarnessId::ClaudeCode, "opus", "Opus", cx);
                assert!(picker.model_detail(cx).unwrap().is_default);
                assert!(picker.is_open());
                // Picking a default does not change this chat's model.
                assert_eq!(picker.resolved(cx).model.as_deref(), Some("gpt-a"));
            })
            .unwrap();
        let saved = crate::settings::UiSettings::load(dir.path())
            .default_model
            .unwrap();
        assert_eq!(saved.provider, HarnessId::ClaudeCode);
        assert_eq!(saved.model, "opus");
        assert_eq!(saved.label, "Opus");
    }

    #[gpui::test]
    fn keyboard_moves_the_card_steps_reasoning_and_enter_picks(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = picker_window(cx);
        handle
            .update(cx, |picker, window, cx| picker.open_model_menu(window, cx))
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        let opus = handle
            .read_with(cx, |picker, cx| row_index(picker, "opus", cx))
            .unwrap();
        let start = handle.read_with(cx, |picker, _| picker.active).unwrap();
        for _ in start..opus {
            cx.simulate_keystrokes(handle.into(), "down");
        }
        handle
            .read_with(cx, |picker, cx| {
                assert_eq!(picker.detail_row(cx).unwrap().model.id, "opus");
                assert_eq!(picker.resolved(cx).model.as_deref(), Some("gpt-a"));
            })
            .unwrap();
        // Left steps the highlighted model's reasoning (selecting it).
        cx.simulate_keystrokes(handle.into(), "left");
        handle
            .read_with(cx, |picker, cx| {
                assert_eq!(picker.resolved(cx).model.as_deref(), Some("opus"));
                assert_eq!(picker.effective_reasoning(cx), Some(ReasoningLevel::Medium));
                assert!(picker.is_open());
            })
            .unwrap();
        // Typing searches; Enter picks the top match and closes.
        cx.simulate_keystrokes(handle.into(), "g p t");
        handle
            .read_with(cx, |picker, cx| {
                assert_eq!(picker.search.read(cx).text(), "gpt");
                assert!(
                    picker
                        .model_rows(cx)
                        .iter()
                        .all(|r| r.model.id.starts_with("gpt"))
                );
            })
            .unwrap();
        cx.simulate_keystrokes(handle.into(), "enter");
        handle
            .read_with(cx, |picker, cx| {
                assert!(!picker.is_open());
                assert_eq!(picker.resolved(cx).model.as_deref(), Some("gpt-a"));
            })
            .unwrap();
    }
}
