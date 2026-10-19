//! Alternate presentation of the same model and option mutations.
use super::*;

pub(super) const COMPACT_WIDTH: f32 = 380.0;
pub(super) const COMPACT_ROW_HEIGHT: f32 = 32.0;
const COMPACT_LIST_ROWS: usize = 7;
const THUMB_INSET: f32 = 12.0;

pub(super) struct CompactEffort {
    pub labels: Vec<SharedString>,
    pub selected: usize,
    stops: EffortStops,
}

enum EffortStops {
    Reasoning(Vec<ReasoningLevel>),
    Option {
        id: String,
        choices: Vec<String>,
        default: String,
    },
}

#[derive(Default)]
pub(super) struct CompactMotion {
    drag_fraction: Option<f32>,
    pub frame_height: f32,
}

pub(super) struct ScalarTransition {
    from: f32,
    to: f32,
    started: std::time::Instant,
}
impl ScalarTransition {
    pub(super) fn value(&self, now: std::time::Instant) -> f32 {
        let progress = motion::RESIZE.progress(
            now.duration_since(self.started).as_secs_f32()
                / motion::RESIZE
                    .total()
                    .mul_f32(motion::speed_scale())
                    .as_secs_f32(),
        );
        motion::lerp(self.from, self.to, progress)
    }
    pub(super) fn sample(
        state: &mut Option<Self>,
        target: f32,
        now: std::time::Instant,
        reduced: bool,
    ) -> (f32, bool) {
        let Some(transition) = state else {
            *state = Some(Self {
                from: target,
                to: target,
                started: now,
            });
            return (target, false);
        };
        if reduced {
            transition.from = target;
            transition.to = target;
            return (target, false);
        }
        if transition.to != target {
            transition.from = transition.value(now);
            transition.to = target;
            transition.started = now;
        }
        let value = transition.value(now);
        (value, (value - target).abs() > 0.001)
    }
}

/// One entry of the compact model list. Labels and provider headers share
/// the rows' height, so the list stays a virtualized uniform list; model
/// rows keep their row index (keyboard cursor, ⌘N slots, activation).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum CompactItem {
    /// The list's uppercase section label.
    Label(SharedString),
    /// A group heading: the provider (or the starred models) and its count.
    Header {
        harness: Option<HarnessId>,
        name: SharedString,
        count: usize,
        current: bool,
    },
    Row(usize),
}

impl Pickers {
    pub(super) fn render_compact_model_row(
        &mut self,
        ix: usize,
        row: &ModelRowData,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let selected = self.effective_harness(cx) == Some(row.harness)
            && self
                .selected_model(cx)
                .is_some_and(|model| model.id == row.model.id);
        let favorite = self.defaults.is_favorite(row.harness, &row.model.id);
        let (icon, tint) = harness_brand_icon(row.harness);
        let harness = row.harness;
        let model = row.model.id.clone();
        let label: SharedString = row.model.label.clone().into();
        // The brand mark names the provider; a description appears only to
        // tell identically named rows apart.
        let attribution: Option<SharedString> = row
            .model
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| row.ambiguous && !d.is_empty())
            .map(|d| SharedString::from(d.to_owned()));
        let hovered = self.active == ix;
        let details: SharedString = match row.model.description.as_deref() {
            Some(description) => format!(
                "{} · {}\n{}",
                row.model.label, row.harness_name, description
            )
            .into(),
            None => format!("{} · {}", row.model.label, row.harness_name).into(),
        };
        let accessible_name = details.clone();
        let favorite_hint: SharedString = format!(
            "{} {} {}",
            if favorite { "Remove" } else { "Add" },
            row.model.label,
            if favorite {
                "from favorites"
            } else {
                "to favorites"
            }
        )
        .into();
        let favorite_tooltip: SharedString =
            format!("{}\n⌘⇧F on the highlighted model", favorite_hint).into();
        let item = div()
            .id(("model-row", ix))
            .role(gpui::Role::ListBoxOption)
            .aria_label(accessible_name)
            .aria_selected(selected)
            .tooltip(crate::settings::widgets::text_tooltip(details))
            .tooltip_show_delay(std::time::Duration::from_millis(500))
            .h(px(COMPACT_ROW_HEIGHT))
            .pl(px(8.0))
            .pr(px(4.0))
            .rounded(px(MODEL_ROW_RADIUS))
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .text_color(theme.text)
            .when(selected, |el| model_selected(el, &theme))
            .when(!selected && self.active == ix, |el| {
                el.bg(theme.muted_fill().opacity(0.6))
            })
            .when(self.compact_keyboard && self.active == ix, |el| {
                el.aria_active_descendant()
            })
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && (this.active != ix || this.compact_keyboard) {
                    this.compact_keyboard = false;
                    this.active = ix;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.activate_model_index(ix, cx)))
            .child(
                crate::icons::icon(icon)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(tint.unwrap_or(theme.text_muted)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(px(6.0))
                    .text_size(crate::typography::text_xs())
                    .child(
                        div()
                            .flex_none()
                            .max_w_full()
                            .truncate()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(label),
                    )
                    .when_some(attribution, |el, attribution| {
                        el.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(crate::typography::text_xs())
                                .text_color(theme.text_muted)
                                .child(attribution),
                        )
                    }),
            )
            .child(
                div()
                    .id(("model-star", ix))
                    .role(gpui::Role::Button)
                    .aria_label(favorite_hint)
                    .aria_toggled(if favorite {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .aria_keyshortcuts("Meta+Shift+F")
                    .tooltip(crate::settings::widgets::text_tooltip(favorite_tooltip))
                    .size(px(24.0))
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    // Quiet until the row is hovered or the model is starred.
                    .when(!favorite && !hovered, |el| el.opacity(0.0))
                    .hover(|s| s.bg(crate::theme::ink(0.08)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_model_favorite(harness, &model, cx);
                    }))
                    .child(
                        crate::icons::icon(if favorite {
                            crate::icons::STAR_BOLD
                        } else {
                            crate::icons::STAR
                        })
                        .size(px(13.0))
                        .text_color(if favorite {
                            theme.accent
                        } else {
                            theme.text_muted
                        }),
                    ),
            )
            .when(selected, |el| el.child(model_check(&theme)))
            // ⌘1…⌘9 pick the first nine rows (see `jump_model_slot`).
            .when(ix < 9, |el| el.child(shortcut_chip(&theme, ix + 1)))
            .when(selected, |el| el.relative().child(selected_bar(&theme)));
        div()
            .pb(px(popover::MENU_GAP))
            .child(item)
            .into_any_element()
    }

    /// The compact list's entries: a section label, then — browsing every
    /// provider unsearched — each group under its heading; a search or a
    /// single provider lists its rows flat under a count.
    pub(super) fn compact_items(&self, cx: &App) -> Vec<CompactItem> {
        let rows = self.model_rows(cx);
        let searching = !self.search.read(cx).text().trim().is_empty();
        let mut items = Vec::with_capacity(rows.len() + 4);
        if self.model_rail != ModelRail::All || searching {
            let count = rows.len();
            let noun = match (searching, count == 1) {
                (true, true) => "match",
                (true, false) => "matches",
                (false, true) => "model",
                (false, false) => "models",
            };
            items.push(CompactItem::Label(format!("{count} {noun}").into()));
            items.extend((0..count).map(CompactItem::Row));
            return items;
        }
        items.push(CompactItem::Label("Models".into()));
        let effective = self.effective_harness(cx);
        let groups = self.compact_groups(cx);
        for (gx, (harness, start)) in groups.iter().enumerate() {
            let end = groups.get(gx + 1).map_or(rows.len(), |(_, next)| *next);
            let name: SharedString = match harness {
                Some(_) => rows[*start].harness_name.clone(),
                None => "Starred".into(),
            };
            items.push(CompactItem::Header {
                harness: *harness,
                name,
                count: end - start,
                current: harness.is_some() && *harness == effective,
            });
            items.extend((*start..end).map(CompactItem::Row));
        }
        items
    }

    /// Where model row `row` sits in the rendered list: the compact list
    /// interleaves labels and headers; the standard list is rows only.
    pub(super) fn model_item_index(&self, row: usize, cx: &App) -> usize {
        if !self.compact_model_picker(cx) {
            return row;
        }
        self.compact_items(cx)
            .iter()
            .position(|item| *item == CompactItem::Row(row))
            .unwrap_or(row)
    }

    /// Scroll model row `row` into view by its rendered position.
    pub(super) fn scroll_to_model_row(&self, row: usize, strategy: gpui::ScrollStrategy, cx: &App) {
        self.model_scroll
            .scroll_to_item(self.model_item_index(row, cx), strategy);
    }

    /// A compact list entry that is not a model row.
    pub(super) fn render_compact_item(
        &self,
        ix: usize,
        item: &CompactItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let el = div()
            .id(("model-list-item", ix))
            .w_full()
            .h(px(COMPACT_ROW_HEIGHT + popover::MENU_GAP))
            .px(px(8.0))
            .flex()
            .gap(px(8.0));
        match item {
            CompactItem::Label(label) => el
                .items_end()
                .pb(px(6.0))
                .text_size(crate::typography::text_xs())
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text_muted)
                .child(SharedString::from(popover::tracked_upper(label)))
                .into_any_element(),
            CompactItem::Header {
                harness,
                name,
                count,
                current,
            } => {
                let icon = match harness {
                    Some(harness) => {
                        let (icon, tint) = harness_brand_icon(*harness);
                        crate::icons::icon(icon)
                            .size(px(14.0))
                            .flex_none()
                            .text_color(tint.unwrap_or(theme.text))
                    }
                    None => crate::icons::icon(crate::icons::STAR_BOLD)
                        .size(px(13.0))
                        .flex_none()
                        .text_color(theme.accent),
                };
                el.items_center()
                    .text_size(crate::typography::text_xs())
                    .child(icon)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text_strong)
                            .child(name.clone()),
                    )
                    .when(*current, |el| el.child(lime_badge(&theme, "Current")))
                    .child(
                        div()
                            .ml_auto()
                            .pr(px(4.0))
                            .text_size(crate::typography::text_xs())
                            .text_color(theme.text_muted)
                            .child(SharedString::from(count.to_string())),
                    )
                    .into_any_element()
            }
            CompactItem::Row(_) => el.into_any_element(),
        }
    }

    /// "Search N models": the count of everything the list can show.
    pub(super) fn compact_search_placeholder(&self, cx: &App) -> SharedString {
        let descriptors = self.rail_descriptors(cx);
        let harnesses: Vec<HarnessId> = match self.model_rail {
            ModelRail::Harness => self.effective_harness(cx).into_iter().collect(),
            _ => descriptors.iter().map(|d| d.id).collect(),
        };
        let count: usize = harnesses
            .iter()
            .filter_map(|h| self.models.get(h).and_then(Loadable::ready))
            .map(Vec::len)
            .sum();
        match count {
            0 => "Search models…".into(),
            1 => "Search 1 model".into(),
            n => format!("Search {n} models").into(),
        }
    }

    /// Keep the search's model count current as catalogs land while open.
    pub(super) fn refresh_compact_placeholder(&mut self, cx: &mut Context<Self>) {
        if !self.compact_model_picker(cx) || self.open_kind() != Some(PickerKind::HarnessModel) {
            return;
        }
        let placeholder = self.compact_search_placeholder(cx);
        self.search
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
    }

    pub(super) fn compact_model_picker(&self, cx: &App) -> bool {
        self.title.is_none() && crate::settings::compact_model_picker(cx)
    }

    pub(super) fn show_compact_models(&mut self, cx: &mut Context<Self>) {
        // Browse every offered provider, just as the standard picker's rail
        // allows. rail_descriptors still limits existing chats to their provider.
        // A foreign-provider row switches the provider before picking its model.
        let rail = if self.harness_locked(cx) {
            ModelRail::Harness
        } else {
            ModelRail::All
        };
        self.show_compact_list(rail, cx);
    }

    fn show_compact_list(&mut self, rail: ModelRail, cx: &mut Context<Self>) {
        self.setting_menu = None;
        self.compact_model_list = true;
        self.model_rail = rail;
        let placeholder = self.compact_search_placeholder(cx);
        self.reset_compact_search(placeholder, cx);
        // Open on the current model: its provider's group at the top when
        // the model sits near the start of it, else centred on the model.
        let selected = self.selected_model_index(cx);
        self.active = selected;
        let start = self
            .compact_groups(cx)
            .into_iter()
            .rev()
            .find(|(_, start)| *start <= selected)
            .map_or(0, |(_, start)| start);
        if selected - start < COMPACT_LIST_ROWS - 2 {
            // The group's heading (the section label too, for the first
            // group) tops the list; a later group keeps one row of the
            // previous group above, under the edge fade, so the list shows
            // there is more above.
            let heading = self.model_item_index(start, cx).saturating_sub(1);
            if heading <= 1 {
                self.model_scroll
                    .scroll_to_item_strict(0, gpui::ScrollStrategy::Top);
            } else {
                self.model_scroll.scroll_to_item_strict_with_offset(
                    heading,
                    gpui::ScrollStrategy::Top,
                    1,
                );
            }
        } else {
            self.model_scroll.scroll_to_item_strict(
                self.model_item_index(selected, cx),
                gpui::ScrollStrategy::Center,
            );
        }
        self.focus_on_mount = true;
        cx.notify();
    }

    /// Back from the model list to the panel.
    pub(super) fn show_compact_panel(&mut self, cx: &mut Context<Self>) {
        self.compact_model_list = false;
        self.focus_on_mount = true;
        cx.notify();
    }

    /// Clear the shared filter for a new page without letting the clear's
    /// Edited event reset the highlight anchored right after.
    fn reset_compact_search(&mut self, placeholder: SharedString, cx: &mut Context<Self>) {
        self.search_reset_muted = !self.search.read(cx).text().is_empty();
        self.search.update(cx, |input, cx| {
            input.set_placeholder(placeholder, cx);
            if !input.text().is_empty() {
                input.set_text("", cx);
            }
        });
    }

    /// Picking a provider runs its last-used model at that model's last
    /// settings (the remembered defaults take over once the harness moves).
    pub(super) fn pick_compact_provider(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        self.model_rail = ModelRail::Harness;
        self.pick_harness(harness, cx);
        self.show_compact_panel(cx);
    }

    /// Where each provider's group starts in the unsearched list: starred
    /// models first, then one run per provider.
    pub(super) fn compact_groups(&self, cx: &App) -> Vec<(Option<HarnessId>, usize)> {
        let rows = self.model_rows(cx);
        let mut groups: Vec<(Option<HarnessId>, usize)> = Vec::new();
        for (ix, row) in rows.iter().enumerate() {
            let group =
                (!self.defaults.is_favorite(row.harness, &row.model.id)).then_some(row.harness);
            if groups.last().is_none_or(|(last, _)| *last != group) {
                groups.push((group, ix));
            }
        }
        groups
    }

    pub(super) fn compact_fast_choice(&self, cx: &App) -> Option<(String, String, bool, bool)> {
        let (option, on, off) = self
            .selected_model(cx)?
            .options
            .iter()
            .find_map(|o| fast_mode_values(o).map(|(on, off)| (o, on, off)))?;
        let options = self.explicit_options(cx);
        let fast = options
            .get(&option.id)
            .and_then(|v| v.as_str())
            .unwrap_or(&option.default_choice)
            == on;
        let next = if fast { off } else { on };
        Some((
            option.id.clone(),
            next.to_owned(),
            next == option.default_choice,
            fast,
        ))
    }

    /// The slider's stops: the model's reasoning ladder, else an option
    /// shaped like one (Cursor's `effort`/`reasoning` choices), so every
    /// model with an effort gets the same slider.
    pub(super) fn compact_effort(&self, cx: &App) -> Option<CompactEffort> {
        let levels = self.trait_ladder(cx);
        if !levels.is_empty() {
            let current = self.effective_reasoning(cx);
            return Some(CompactEffort {
                labels: levels.iter().map(|l| reasoning_label(*l).into()).collect(),
                selected: levels.iter().position(|l| Some(*l) == current).unwrap_or(0),
                stops: EffortStops::Reasoning(levels),
            });
        }
        let option = zuse_protocol::presentation::model_effort_option(self.selected_model(cx)?)?;
        let options = self.explicit_options(cx);
        let current = options
            .get(&option.id)
            .and_then(|v| v.as_str())
            .unwrap_or(&option.default_choice);
        Some(CompactEffort {
            labels: option
                .choices
                .iter()
                .map(|c| SharedString::from(c.label.clone()))
                .collect(),
            selected: option
                .choices
                .iter()
                .position(|c| c.id == current)
                .or_else(|| {
                    option
                        .choices
                        .iter()
                        .position(|choice| choice.id == option.default_choice)
                })
                .unwrap_or(0),
            stops: EffortStops::Option {
                id: option.id.clone(),
                choices: option.choices.iter().map(|c| c.id.clone()).collect(),
                default: option.default_choice.clone(),
            },
        })
    }

    /// Option ids the compact card draws as its own controls, not rows.
    pub(super) fn compact_hidden_options(&self, cx: &App) -> Vec<String> {
        let effort = match self.compact_effort(cx).map(|e| e.stops) {
            Some(EffortStops::Option { id, .. }) => Some(id),
            _ => None,
        };
        // Fast mode is the Advanced footer's lightning toggle.
        let fast = self.compact_fast_choice(cx).map(|(option, _, _, _)| option);
        effort.into_iter().chain(fast).collect()
    }

    pub(super) fn pick_effort_at(&mut self, x: gpui::Pixels, cx: &mut Context<Self>) {
        let Some(bounds) = self.effort_bounds else {
            return;
        };
        let Some(effort) = self.compact_effort(cx) else {
            return;
        };
        let count = effort.labels.len();
        let Some(index) = effort_index(
            f32::from(x - bounds.left()),
            f32::from(bounds.size.width),
            count,
        ) else {
            return;
        };
        if self.effort_dragging {
            let fraction =
                effort_fraction(f32::from(x - bounds.left()), f32::from(bounds.size.width));
            self.compact_motion.drag_fraction = Some(if count > 1 { fraction } else { 0.0 });
            cx.notify();
        }
        self.pick_compact_effort(effort, index, cx);
    }

    /// Effort and fast mode have their own controls in the compact card.
    pub(super) fn compact_option_visible(id: &ModelSetting, hidden: &[String]) -> bool {
        !matches!(id, ModelSetting::Reasoning)
            && !matches!(id, ModelSetting::Option(id) if hidden.contains(id))
    }

    pub(super) fn pick_compact_effort(
        &mut self,
        effort: CompactEffort,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        if index == effort.selected {
            return;
        }
        match effort.stops {
            EffortStops::Reasoning(levels) => {
                let Some(level) = levels.get(index).copied() else {
                    return;
                };
                self.pick_reasoning(level, cx);
            }
            EffortStops::Option {
                id,
                choices,
                default,
            } => {
                let Some(choice) = choices.get(index).cloned() else {
                    return;
                };
                let is_default = choice == default;
                self.pick_option(id, choice, is_default, cx);
            }
        }
        crate::haptics::selection_step();
    }

    pub(super) fn render_compact_menu(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.compact_model_list = true;
        self.compact_motion.frame_height = self.menu_geometry().height.min(self.open_model_height);
        let content = self.render_harness_model_popover(cx);
        self.popover_frame_flush(COMPACT_WIDTH, content, cx)
    }

    /// The "Advanced" footer: the fast-mode toggle (when the selected model
    /// offers fast mode) beside the label, the effort slider below it.
    pub(super) fn render_effort_slider(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let effort = self.compact_effort(cx);
        let fast = self.compact_fast_choice(cx);
        if effort.is_none() && fast.is_none() {
            return None;
        }
        let theme = Theme::of(cx).for_popup();
        let footer = div()
            .flex_none()
            .px(px(10.))
            .pt(px(6.))
            .pb(px(if effort.is_some() { 4. } else { 6. }))
            .border_t_1()
            .border_color(theme.border.opacity(0.5))
            .bg(theme.muted_fill().opacity(0.15))
            .child(
                div()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(crate::typography::text_xs())
                    .text_color(theme.text_muted)
                    .child("Advanced")
                    .children(fast.map(|(option, choice, default, on)| {
                        fast_toggle(&theme, on).on_click(cx.listener(move |this, _, _, cx| {
                            this.pick_option(option.clone(), choice.clone(), default, cx);
                            cx.notify();
                        }))
                    })),
            );
        let Some(effort) = effort else {
            return Some(footer.into_any_element());
        };
        let selected = effort.selected;
        let count = effort.labels.len();
        let fraction = selected as f32 / count.saturating_sub(1).max(1) as f32;
        let entity = cx.entity().downgrade();
        let effort_label = effort
            .labels
            .get(selected)
            .cloned()
            .unwrap_or_else(|| "Default".into());
        Some(
            footer
                .child(
                    div()
                        .id("model-effort-slider")
                        .role(gpui::Role::Slider)
                        .tab_index(0)
                        .aria_label(SharedString::from(format!(
                            "Reasoning effort: {effort_label}"
                        )))
                        .tooltip(crate::settings::widgets::text_tooltip(effort_label.clone()))
                        .aria_numeric_value(selected as f64)
                        .aria_min_numeric_value(0.)
                        .aria_max_numeric_value(count.saturating_sub(1) as f64)
                        .h(px(30.))
                        .relative()
                        .cursor_pointer()
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                                this.effort_dragging = true;
                                this.pick_effort_at(event.position.x, cx);
                                cx.stop_propagation();
                            }),
                        )
                        .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                            if this.effort_dragging
                                && event.pressed_button == Some(gpui::MouseButton::Left)
                            {
                                this.pick_effort_at(event.position.x, cx);
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
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                            let Some(effort) = this.compact_effort(cx) else {
                                return;
                            };
                            let index = match event.keystroke.key.as_str() {
                                "left" | "down" => effort.selected.saturating_sub(1),
                                "right" | "up" => {
                                    (effort.selected + 1).min(effort.labels.len().saturating_sub(1))
                                }
                                "home" => 0,
                                "end" => effort.labels.len().saturating_sub(1),
                                _ => return,
                            };
                            this.pick_compact_effort(effort, index, cx);
                            cx.notify();
                            cx.stop_propagation();
                        }))
                        .child(
                            gpui::canvas(
                                move |bounds, _, cx| {
                                    let _ = entity
                                        .update(cx, |this, _| this.effort_bounds = Some(bounds));
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
                                .right_0()
                                .top(px(5.))
                                .h(px(20.))
                                .rounded_full()
                                .bg(theme.surface_raised),
                        )
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top(px(5.))
                                .h(px(20.))
                                .w(gpui::relative(fraction))
                                .rounded_full()
                                .bg(theme.accent),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(12.))
                                .right(px(12.))
                                .top(px(5.))
                                .h(px(20.))
                                .flex()
                                .items_center()
                                .justify_between()
                                .children((0..count).map(|_| {
                                    div()
                                        .size(px(4.))
                                        .rounded_full()
                                        .bg(theme.text_muted.opacity(0.5))
                                })),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(12.))
                                .right(px(12.))
                                .top(px(3.))
                                .h(px(24.))
                                .child(
                                    div()
                                        .absolute()
                                        .left(gpui::relative(fraction))
                                        .ml(px(-12.))
                                        .size(px(24.))
                                        .rounded_full()
                                        .bg(gpui::white())
                                        .border_1()
                                        .border_color(theme.accent.opacity(0.6)),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The panel's shortcuts, live only while it is the picker's page: Up
    /// and Down open the model list on the model beside the selected one,
    /// Tab cycles providers, Left/Right (Home/End) set the effort, F toggles
    /// fast mode.
    pub(super) fn compact_panel_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        self.compact_keyboard = true;
        match event.keystroke.key.as_str() {
            "escape" => self.animate_close(cx),
            "f" if !event.keystroke.modifiers.modified() => {
                let Some((option, choice, default, _)) = self.compact_fast_choice(cx) else {
                    return;
                };
                self.pick_option(option, choice, default, cx);
            }
            "up" | "down" => {
                self.show_compact_models(cx);
                let delta = if event.keystroke.key == "up" { -1 } else { 1 };
                if let Some(next) =
                    popover::menu_step(Some(self.active), self.model_rows_len(cx), delta)
                {
                    self.active = next;
                    self.scroll_to_model_row(next, gpui::ScrollStrategy::Nearest, cx);
                }
            }
            "enter" | "space" => self.show_compact_models(cx),
            "tab" => {
                let delta = if event.keystroke.modifiers.shift {
                    -1
                } else {
                    1
                };
                self.cycle_compact_provider(delta, cx);
            }
            "left" | "right" | "home" | "end" => {
                if let Some(effort) = self.compact_effort(cx) {
                    let last = effort.labels.len().saturating_sub(1);
                    let next = match event.keystroke.key.as_str() {
                        "left" => effort.selected.saturating_sub(1),
                        "right" => (effort.selected + 1).min(last),
                        "home" => 0,
                        _ => last,
                    };
                    self.pick_compact_effort(effort, next, cx);
                }
            }
            _ => return,
        }
        cx.notify();
        cx.stop_propagation();
    }

    /// Tab on the panel: the next provider on offer, wrapping, at its
    /// last-used model. Pending first sends keep their snapshot stable.
    fn cycle_compact_provider(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.harness_locked(cx) {
            return;
        }
        let providers = self.rail_descriptors(cx);
        let current = self
            .effective_harness(cx)
            .and_then(|harness| providers.iter().position(|d| d.id == harness));
        if let Some(next) = popover::menu_step(current, providers.len(), delta) {
            self.pick_compact_provider(providers[next].id, cx);
        }
    }
}

/// The ⌘N slot chip at a model row's trailing edge.
fn shortcut_chip(theme: &Theme, slot: usize) -> gpui::Div {
    div()
        .flex_none()
        .ml(px(2.0))
        .h(px(18.0))
        .min_w(px(18.0))
        .px(px(4.0))
        .rounded(px(4.0))
        .bg(theme.muted_fill().opacity(0.7))
        .flex()
        .items_center()
        .justify_center()
        .text_size(crate::typography::text_xs())
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text_muted)
        .child(SharedString::from(slot.to_string()))
}

/// The 2px lime bar on the selected row's leading edge.
fn selected_bar(theme: &Theme) -> gpui::Div {
    div()
        .absolute()
        .left_0()
        .top(px((COMPACT_ROW_HEIGHT - 16.0) / 2.0))
        .w(px(2.0))
        .h(px(16.0))
        .rounded_full()
        .bg(theme.accent)
}

/// Small uppercase lime tag ("CURRENT").
fn lime_badge(theme: &Theme, label: &str) -> gpui::Div {
    let dark = theme.appearance.is_dark();
    div()
        .flex_none()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(4.0))
        .bg(theme.accent.opacity(if dark { 0.15 } else { 0.35 }))
        .text_size(crate::typography::text_xs())
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(if dark { theme.accent } else { theme.on_accent })
        .child(SharedString::from(popover::tracked_upper(label)))
}

/// The Advanced footer's fast-mode button: an amber-tinted square while on.
fn fast_toggle(theme: &Theme, on: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id("model-fast-mode")
        .role(gpui::Role::Button)
        .aria_label(if on {
            "Turn off fast mode"
        } else {
            "Turn on fast mode"
        })
        .aria_toggled(if on {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .aria_keyshortcuts("F")
        .tooltip(crate::settings::widgets::text_tooltip_above(
            "1.5× speed\nMore usage",
        ))
        .size(px(24.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(on, |el| {
            el.bg(theme.warning.opacity(0.2))
                .hover(|s| s.bg(theme.warning.opacity(0.3)))
        })
        .when(!on, |el| {
            el.hover(|s| s.bg(theme.muted_fill().opacity(0.6)))
        })
        .child(
            crate::icons::icon(crate::icons::FAST_TIER_BOLD)
                .size(px(14.0))
                .text_color(if on { theme.warning } else { theme.text_muted }),
        )
}

fn effort_fraction(x: f32, width: f32) -> f32 {
    ((x - THUMB_INSET) / (width - THUMB_INSET * 2.0).max(1.0)).clamp(0.0, 1.0)
}

/// Slider stops include both endpoints and only advertised reasoning levels.
fn effort_index(x: f32, width: f32, count: usize) -> Option<usize> {
    (count > 0)
        .then(|| (effort_fraction(x, width) * count.saturating_sub(1) as f32).round() as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_transitions_start_settled_and_retarget_without_jumping() {
        let start = std::time::Instant::now();
        let mut state = None;
        assert_eq!(
            ScalarTransition::sample(&mut state, 0.5, start, false),
            (0.5, false)
        );
        assert_eq!(
            ScalarTransition::sample(&mut state, 1.0, start, false).0,
            0.5
        );
        let middle = start + motion::RESIZE.total().mul_f32(motion::speed_scale() * 0.5);
        let before = state.as_ref().unwrap().value(middle);
        let retargeted = ScalarTransition::sample(&mut state, 0.0, middle, false).0;
        assert!((before - retargeted).abs() < 0.0001);
        assert_eq!(
            ScalarTransition::sample(&mut state, 0.0, middle, true),
            (0.0, false)
        );
    }

    #[test]
    fn continuous_drag_and_stops_share_inset_geometry() {
        let width = 228.0;
        for count in 2..=8 {
            for index in 0..count {
                let fraction = index as f32 / (count - 1) as f32;
                let x = THUMB_INSET + fraction * (width - THUMB_INSET * 2.0);
                assert!((effort_fraction(x, width) - fraction).abs() < 0.00001);
                assert_eq!(effort_index(x, width, count), Some(index));
                assert!(x - 2.0 >= 0.0 && x + 2.0 <= width);
            }
        }
        assert!(
            (effort_fraction(THUMB_INSET + (width - 2.0 * THUMB_INSET) * 0.25, width) - 0.25).abs()
                < 0.00001
        );
        assert_eq!(effort_fraction(-100.0, width), 0.0);
        assert_eq!(effort_fraction(500.0, width), 1.0);
    }

    #[test]
    fn effort_stops_clamp_and_round_to_advertised_levels() {
        assert_eq!(effort_index(100.0, 200.0, 0), None);
        assert_eq!(effort_index(100.0, 200.0, 1), Some(0));
        assert_eq!(effort_index(-20.0, 200.0, 5), Some(0));
        assert_eq!(effort_index(100.0, 200.0, 5), Some(2));
        assert_eq!(effort_index(250.0, 200.0, 5), Some(4));
    }
}
