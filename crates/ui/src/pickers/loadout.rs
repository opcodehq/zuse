//! The model loadout: a short, user-curated list of models, each saved with
//! its effort and fast mode, that the composer's picker opens on. The full
//! catalog stays one click away ("All models"); Settings → Default models
//! edits the list.
use super::*;
#[cfg(feature = "browser-fixture")]
use crate::settings::composer::ModelOptions;
use crate::settings::composer::{LOADOUT_LIMIT, LoadoutEntry};
use crate::tooltip::TooltipExt as _;

const ROW_HEIGHT: f32 = 28.0;
const CONTROL_ROW_HEIGHT: f32 = 28.0;
const FOOTER_HEIGHT: f32 = 30.0;
const WIDTH: f32 = 256.0;
/// Gap between the frame's edge and its inner model card.
const INSET: f32 = 4.0;

/// Asks the composer's owner to open Settings → Default models.
pub struct EditLoadout;
impl gpui::EventEmitter<EditLoadout> for Pickers {}

impl Pickers {
    /// Whether the open model picker shows the loadout instead of the catalog.
    pub(super) fn loadout_view(&self, cx: &App) -> bool {
        self.compact_model_picker(cx)
            && !self.is_settings_model()
            && self.title.is_none()
            && !self.loadout_browse
            && !self.cloud_models_active(cx)
    }

    /// The current pick as a loadout entry: provider, model, effort and the
    /// explicit options it runs with.
    pub(super) fn current_loadout_entry(&self, cx: &App) -> Option<LoadoutEntry> {
        let harness = self.effective_harness(cx)?;
        let model = self.selected_model(cx)?;
        let effort = self.compact_effort(cx);
        let reasoning = match &effort {
            Some(effort) if matches!(effort.stops, compact::EffortStops::Reasoning(_)) => {
                self.effective_reasoning(cx)
            }
            _ => None,
        };
        Some(LoadoutEntry {
            harness,
            model: model.id.clone(),
            label: model.label.clone(),
            reasoning,
            options: offered_options(model, self.explicit_options(cx)),
            effort: effort.and_then(|e| e.labels.get(e.selected).map(|l| l.to_string())),
            fast: self.compact_fast_choice(cx).is_some_and(|(_, _, _, on)| on),
        })
    }

    /// The loadout entry that matches the current pick, if any.
    pub(super) fn current_loadout_index(&self, cx: &App) -> Option<usize> {
        let current = self.current_loadout_entry(cx);
        self.defaults
            .loadout
            .iter()
            .position(|e| Self::loadout_matches(e, current.as_ref()))
    }

    /// Whether `entry` describes what the composer would run right now.
    fn loadout_matches(entry: &LoadoutEntry, current: Option<&LoadoutEntry>) -> bool {
        current.is_some_and(|current| {
            entry.harness == current.harness
                && entry.model == current.model
                && entry.fast == current.fast
                && (entry.effort.is_none() || entry.effort == current.effort)
        })
    }

    /// Switch to loadout entry `index`: provider, model, effort and options
    /// in one configuration change.
    pub(super) fn apply_loadout_entry(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        if self.cloud_launch_frozen() {
            return false;
        }
        let Some(entry) = self.defaults.loadout.get(index).cloned() else {
            return false;
        };
        let switching = self.effective_harness(cx) != Some(entry.harness);
        if switching && self.harness_locked(cx) {
            return false;
        }
        if self.state.read(cx).selected_chat.is_some() || self.cloud.task_binding.is_some() {
            let LoadoutEntry {
                harness,
                model,
                reasoning,
                options,
                ..
            } = entry.clone();
            self.update_chat_config(cx, move |config| {
                config.harness = harness;
                config.model = Some(model);
                config.reasoning = reasoning;
                config.model_options = options;
            });
            if switching {
                self.config.model = None;
                self.config.reasoning = None;
            }
            self.config.harness = Some(entry.harness);
        } else {
            self.config.harness = Some(entry.harness);
            self.config.model = Some(entry.model.clone());
            self.config.reasoning = entry.reasoning;
            self.defaults
                .remember_model(entry.harness, entry.model.clone(), entry.label.clone());
            if let Some(level) = entry.reasoning {
                self.defaults
                    .remember_reasoning(entry.harness, Some(&entry.model), level);
            }
            *self.defaults.model_options_mut(entry.harness, &entry.model) = entry.options.clone();
            self.save_defaults();
        }
        self.ensure_models(entry.harness, false, cx);
        self.catalog_rev += 1;
        crate::haptics::selection_step();
        cx.notify();
        true
    }

    /// Append the current pick to the loadout.
    pub(super) fn add_current_to_loadout(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.current_loadout_entry(cx) else {
            return;
        };
        self.reload_defaults();
        if self.defaults.add_to_loadout(entry) {
            self.persist_defaults();
        }
        cx.notify();
    }

    /// Settings: add a catalog model at its remembered effort, or remove
    /// every entry for it when it is already in the loadout.
    pub(super) fn toggle_loadout_model(
        &mut self,
        harness: HarnessId,
        model: &Model,
        cx: &mut Context<Self>,
    ) {
        self.reload_defaults();
        if self
            .defaults
            .loadout
            .iter()
            .any(|e| e.harness == harness && e.model == model.id)
        {
            self.defaults
                .loadout
                .retain(|e| !(e.harness == harness && e.model == model.id));
        } else {
            let ladder = self.ladder_for(harness, model);
            let reasoning = (!ladder.is_empty()).then(|| {
                clamp_reasoning(
                    self.defaults.reasoning_for(harness, Some(&model.id)),
                    &ladder,
                )
            });
            let options = offered_options(
                model,
                self.defaults
                    .model_options_for(harness, &model.id)
                    .cloned()
                    .unwrap_or_default(),
            );
            let mut entry = LoadoutEntry {
                harness,
                model: model.id.clone(),
                label: model.label.clone(),
                reasoning: reasoning.flatten(),
                options,
                effort: None,
                fast: false,
            };
            Self::describe_entry(&mut entry, model, &ladder);
            self.defaults.add_to_loadout(entry);
        }
        self.persist_defaults();
        cx.notify();
    }

    pub(super) fn remove_loadout_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        self.reload_defaults();
        if self.defaults.remove_from_loadout(index).is_some() {
            self.persist_defaults();
        }
        cx.notify();
    }

    pub(super) fn move_loadout_entry(
        &mut self,
        index: usize,
        delta: isize,
        cx: &mut Context<Self>,
    ) {
        self.reload_defaults();
        if self.defaults.move_in_loadout(index, delta).is_some() {
            self.persist_defaults();
        }
        cx.notify();
    }

    /// Settings: step entry `index` to its model's next effort, wrapping.
    pub(super) fn cycle_loadout_effort(&mut self, index: usize, cx: &mut Context<Self>) {
        self.reload_defaults();
        let Some(entry) = self.defaults.loadout.get(index).cloned() else {
            return;
        };
        let Some(model) = self.catalog_model(entry.harness, &entry.model).cloned() else {
            return;
        };
        let ladder = self.ladder_for(entry.harness, &model);
        let mut next = entry.clone();
        if !ladder.is_empty() {
            let mut levels = ladder.clone();
            levels.sort();
            let current = clamp_reasoning(entry.reasoning, &levels);
            let at = levels.iter().position(|l| Some(*l) == current).unwrap_or(0);
            next.reasoning = levels.get((at + 1) % levels.len()).copied();
        } else if let Some(effort) =
            zuse_protocol::presentation::model_effort(&model, entry.reasoning, &entry.options)
        {
            let to = (effort.selected + 1) % effort.stops.len().max(1);
            effort.select(to, &mut next.reasoning, &mut next.options);
        } else {
            return;
        }
        Self::describe_entry(&mut next, &model, &ladder);
        if self
            .defaults
            .loadout
            .iter()
            .enumerate()
            .any(|(ix, e)| ix != index && e.same_run(&next))
        {
            return;
        }
        self.defaults.loadout[index] = next;
        self.persist_defaults();
        cx.notify();
    }

    /// Settings: flip fast mode on entry `index` when its model offers it.
    pub(super) fn toggle_loadout_fast(&mut self, index: usize, cx: &mut Context<Self>) {
        self.reload_defaults();
        let Some(entry) = self.defaults.loadout.get(index).cloned() else {
            return;
        };
        let Some(model) = self.catalog_model(entry.harness, &entry.model).cloned() else {
            return;
        };
        let Some((option, on, off)) = model
            .options
            .iter()
            .find_map(|o| fast_mode_values(o).map(|(on, off)| (o, on, off)))
        else {
            return;
        };
        let mut next = entry.clone();
        let choice = if entry.fast { off } else { on };
        if choice == option.default_choice {
            next.options.remove(&option.id);
        } else {
            next.options
                .insert(option.id.clone(), serde_json::Value::String(choice.into()));
        }
        let ladder = self.ladder_for(entry.harness, &model);
        Self::describe_entry(&mut next, &model, &ladder);
        self.defaults.loadout[index] = next;
        self.persist_defaults();
        cx.notify();
    }

    /// Refresh an entry's display fields from its model's catalog row.
    fn describe_entry(entry: &mut LoadoutEntry, model: &Model, ladder: &[ReasoningLevel]) {
        entry.label = model.label.clone();
        entry.effort = if !ladder.is_empty() {
            clamp_reasoning(entry.reasoning, ladder).map(|l| reasoning_label(l).to_string())
        } else {
            zuse_protocol::presentation::model_effort_option(model).map(|option| {
                let current = entry
                    .options
                    .get(&option.id)
                    .and_then(|v| v.as_str())
                    .unwrap_or(&option.default_choice);
                option
                    .choices
                    .iter()
                    .find(|c| c.id == current)
                    .map(|c| c.label.clone())
                    .unwrap_or_else(|| current.to_string())
            })
        };
        entry.fast = model.options.iter().any(|option| {
            fast_mode_values(option).is_some_and(|(on, _)| {
                entry
                    .options
                    .get(&option.id)
                    .and_then(|v| v.as_str())
                    .unwrap_or(&option.default_choice)
                    == on
            })
        });
    }

    fn catalog_model(&self, harness: HarnessId, id: &str) -> Option<&Model> {
        self.models
            .get(&harness)
            .and_then(Loadable::ready)
            .and_then(|models| models.iter().find(|m| m.id == id))
    }

    /// A model's reasoning ladder: its own levels, else its provider's.
    fn ladder_for(&self, harness: HarnessId, model: &Model) -> Vec<ReasoningLevel> {
        if !model.reasoning_levels.is_empty() {
            return model.reasoning_levels.clone();
        }
        self.harnesses
            .ready()
            .and_then(|list| list.iter().find(|d| d.id == harness))
            .map(|d| d.reasoning_levels.clone())
            .unwrap_or_default()
    }

    /// Leave the loadout for the full, searchable catalog.
    pub(super) fn browse_all_models(&mut self, cx: &mut Context<Self>) {
        self.loadout_browse = true;
        self.active = self.selected_model_index(cx);
        self.prefetch_models(false, cx);
        self.focus_on_mount = true;
        cx.notify();
    }

    fn edit_loadout(&mut self, cx: &mut Context<Self>) {
        self.close(cx);
        cx.emit(EditLoadout);
    }

    /// Keyboard on the loadout page. Returns whether the key was handled.
    pub(super) fn loadout_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let count = self.defaults.loadout.len();
        let modifiers = &event.keystroke.modifiers;
        if modifiers.platform
            && let Ok(n) = event.keystroke.key.parse::<usize>()
            && (1..=LOADOUT_LIMIT).contains(&n)
        {
            if self.apply_loadout_entry(n - 1, cx) {
                self.close(cx);
            }
            return true;
        }
        if modifiers.modified() && !modifiers.shift {
            return false;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.close(cx),
            "up" | "down" if count > 0 => {
                self.compact_keyboard = true;
                let delta = if event.keystroke.key == "up" { -1 } else { 1 };
                self.active =
                    popover::menu_step(Some(self.active.min(count - 1)), count, delta).unwrap_or(0);
            }
            "enter" | "space" if count > 0 => {
                if self.apply_loadout_entry(self.active.min(count - 1), cx) {
                    self.close(cx);
                }
            }
            "left" | "right" => {
                if let Some(effort) = self.compact_effort(cx) {
                    let last = effort.labels.len().saturating_sub(1);
                    let next = if event.keystroke.key == "left" {
                        effort.selected.saturating_sub(1)
                    } else {
                        (effort.selected + 1).min(last)
                    };
                    self.pick_compact_effort(effort, next, cx);
                }
            }
            "f" => {
                self.toggle_fast_mode(cx);
            }
            "a" => self.browse_all_models(cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    pub(super) fn render_loadout_menu(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let entries = self.defaults.loadout.clone();
        let current = self.current_loadout_entry(cx);
        let effort = self.compact_effort(cx);
        let fast = self.compact_fast_choice(cx);
        let locked = self.harness_locked(cx);
        let effective = self.effective_harness(cx);
        let active = self.active.min(entries.len().saturating_sub(1));
        let hover_bg = theme.muted_fill().opacity(0.6);
        // The models sit on the darker inner card; effort, fast and the
        // footer sit on the lighter frame around it.
        let rows = entries.iter().enumerate().map(|(index, entry)| {
            let selected = Self::loadout_matches(entry, current.as_ref());
            let unavailable = locked && Some(entry.harness) != effective;
            let (icon, tint) = harness_brand_icon(entry.harness);
            let keyboard = self.compact_keyboard && index == active;
            div()
                .id(("loadout-row", index))
                .role(gpui::Role::ListBoxOption)
                .aria_selected(selected)
                .aria_label(SharedString::from(entry.label.clone()))
                .h(px(ROW_HEIGHT))
                .px(px(8.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(px(MODEL_ROW_RADIUS))
                .text_size(crate::typography::text_xs())
                .when(keyboard, |row| row.bg(hover_bg).aria_active_descendant())
                .when(unavailable, |row| row.opacity(0.4))
                .when(!unavailable, |row| {
                    row.cursor_pointer()
                        .hover(|s| s.bg(hover_bg))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.apply_loadout_entry(index, cx) {
                                this.close(cx);
                            }
                        }))
                })
                .child(
                    crate::icons::icon(icon)
                        .size(px(14.0))
                        .flex_none()
                        .text_color(tint.unwrap_or(theme.text_muted)),
                )
                .child(
                    div()
                        .flex_none()
                        .max_w(px(150.0))
                        .truncate()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(SharedString::from(entry.label.clone())),
                )
                .when_some(entry.effort.clone(), |el, effort| {
                    el.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.text_muted)
                            .child(SharedString::from(effort)),
                    )
                })
                .when(entry.fast, |el| {
                    el.child(
                        crate::icons::icon(crate::icons::FAST_TIER_BOLD)
                            .size(px(11.0))
                            .flex_none()
                            .text_color(theme.text_muted),
                    )
                })
                .child(div().flex_1())
                .child(if selected {
                    crate::icons::icon(crate::icons::CHECK)
                        .size(px(13.0))
                        .flex_none()
                        .text_color(theme.text)
                        .into_any_element()
                } else {
                    div()
                        .flex_none()
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted.opacity(0.6))
                        .child(SharedString::from(slot_label(index + 1)))
                        .into_any_element()
                })
                .into_any_element()
        });

        let empty = entries.is_empty().then(|| {
            div()
                .h(px(ROW_HEIGHT))
                .px(px(8.0))
                .flex()
                .items_center()
                .text_size(crate::typography::text_xs())
                .text_color(theme.text_muted)
                .child("No models pinned")
        });

        let effort_row = effort.map(|effort| {
            let label = effort
                .labels
                .get(effort.selected)
                .cloned()
                .unwrap_or_else(|| "Default".into());
            control_row(&theme, "Effort")
                .id("loadout-effort")
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(format!("Effort: {label}")))
                .cursor_pointer()
                .hover(|s| s.bg(hover_bg))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.cycle_reasoning_effort(cx);
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .text_color(theme.text)
                        .child(label)
                        .child(
                            crate::icons::icon(crate::icons::ALT_ARROW_RIGHT)
                                .size(px(12.0))
                                .text_color(theme.text_muted),
                        ),
                )
        });

        let fast_row = fast.map(|(option, choice, default, on)| {
            control_row(&theme, "Fast")
                .id("loadout-fast")
                .role(gpui::Role::Switch)
                .aria_label("Fast mode")
                .aria_toggled(if on {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .cursor_pointer()
                .hover(|s| s.bg(hover_bg))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.pick_option(option.clone(), choice.clone(), default, cx);
                    cx.notify();
                }))
                .child(crate::settings::widgets::toggle_switch(
                    &theme,
                    on,
                    format!("loadout-fast-{}", cx.entity_id()),
                ))
        });
        let has_controls = effort_row.is_some() || fast_row.is_some();

        let icon_button = |id: &'static str, label: &'static str, icon: &'static str| {
            div()
                .id(id)
                .role(gpui::Role::Button)
                .aria_label(label)
                .h(px(24.0))
                .px(px(6.0))
                .flex()
                .items_center()
                .gap(px(5.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .text_size(crate::typography::text_xs())
                .text_color(theme.text_muted)
                .hover(|s| s.bg(hover_bg).text_color(theme.text))
                .child(
                    crate::icons::icon(icon)
                        .size(px(13.0))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
        };
        let footer = div()
            .flex_none()
            .h(px(FOOTER_HEIGHT))
            .px(px(INSET + 7.0))
            .pb(px(2.0))
            .flex()
            .items_center()
            .justify_between()
            .child(
                icon_button("loadout-edit", "Edit", crate::icons::SETTINGS_MINIMALISTIC)
                    .child("Edit")
                    .on_click(cx.listener(|this, _, _, cx| this.edit_loadout(cx))),
            )
            .child(
                icon_button("loadout-all-models", "All models", crate::icons::MAGNIFER)
                    .tip("All models")
                    .on_click(cx.listener(|this, _, _, cx| this.browse_all_models(cx))),
            );

        let list_height =
            entries.len().max(1) as f32 * (ROW_HEIGHT + popover::MENU_GAP) + 4.0 * INSET + 2.0;
        let controls_height = if has_controls {
            (effort_row.is_some() as u8 + fast_row.is_some() as u8) as f32 * CONTROL_ROW_HEIGHT
        } else {
            0.0
        };
        let height = (list_height + controls_height + FOOTER_HEIGHT + 2.0)
            .min(self.menu_geometry().height.max(160.0));
        self.compact_motion.frame_height = height;

        let content = div()
            .id("model-loadout")
            .role(gpui::Role::ListBox)
            .aria_label("Models")
            .size_full()
            .flex()
            .flex_col()
            .child(
                popover::inner_card(&theme)
                    .id("model-loadout-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .m(px(INSET))
                    .p(px(INSET))
                    .flex()
                    .flex_col()
                    .gap(px(popover::MENU_GAP))
                    .children(rows)
                    .children(empty),
            )
            .when(has_controls, |el| {
                el.child(
                    div()
                        .flex_none()
                        .px(px(INSET + 5.0))
                        .flex()
                        .flex_col()
                        .children(effort_row)
                        .children(fast_row),
                )
            })
            .child(footer)
            .into_any_element();
        self.popover_frame_flush(WIDTH, content, cx)
    }

    /// Settings → Models: the loadout as a reorderable list in a frame,
    /// then each provider's catalog in its own frame to add from.
    pub fn render_loadout_editor(&mut self, cx: &mut Context<Self>) -> AnyElement {
        // The composer edits the same file; pick its changes up when shown.
        let modified = self
            .data_dir
            .as_deref()
            .and_then(|dir| std::fs::metadata(ComposerDefaults::path(dir)).ok())
            .and_then(|meta| meta.modified().ok());
        if modified != self.loadout_read_at {
            self.loadout_read_at = modified;
            self.reload_defaults();
        }
        self.ensure_harnesses(false, cx);
        let descriptors = self
            .harnesses
            .ready()
            .map(|list| self.offered(list))
            .unwrap_or_default();
        for descriptor in &descriptors {
            self.ensure_models(descriptor.id, false, cx);
        }
        let theme = Theme::of(cx).for_settings_surface();
        let entries = self.defaults.loadout.clone();
        let full = entries.len() >= LOADOUT_LIMIT;
        let divider = crate::settings::widgets::row_divider(&theme);
        let frame = || {
            div()
                .flex()
                .flex_col()
                .p(px(4.0))
                .rounded(px(zuse_viewport::settings_style::FRAME_RADIUS))
                .bg(theme.settings_frame_bg())
        };
        let card = || crate::settings::widgets::section_card(&theme).mt_0();
        let frame_header = |title: AnyElement, meta: SharedString| {
            div()
                .h(px(32.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(crate::typography::text_xs())
                .child(title)
                .child(div().flex_1())
                .child(div().text_color(theme.text_muted).child(meta))
        };

        let rows = entries.iter().enumerate().map(|(index, entry)| {
            let model = self.catalog_model(entry.harness, &entry.model).cloned();
            let can_fast = model
                .as_ref()
                .is_some_and(|m| m.options.iter().any(|o| fast_mode_values(o).is_some()));
            let can_effort = model.as_ref().is_some_and(|m| {
                !self.ladder_for(entry.harness, m).is_empty()
                    || zuse_protocol::presentation::model_effort_option(m).is_some()
            });
            let (icon, tint) = harness_brand_icon(entry.harness);
            let quiet_button = |id: (&'static str, usize)| {
                div()
                    .id(id)
                    .role(gpui::Role::Button)
                    .size(px(24.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.muted_fill().opacity(0.7)))
            };
            div()
                .id(("loadout-entry", index))
                .h(px(42.0))
                .pl(px(10.0))
                .pr(px(8.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .text_size(crate::typography::text_xs())
                .when(index > 0, |row| row.border_t_1().border_color(divider))
                .on_drag(LoadoutDrag { from: index }, |_, _, _, cx| {
                    cx.stop_propagation();
                    cx.new(|_| LoadoutGhost)
                })
                .drag_over::<LoadoutDrag>(|style, _, _, _| style.bg(crate::theme::ink(0.05)))
                .on_drop(cx.listener(move |this, drag: &LoadoutDrag, _, cx| {
                    let delta = index as isize - drag.from as isize;
                    this.move_loadout_entry(drag.from, delta, cx);
                }))
                .child(
                    crate::icons::icon(crate::icons::DRAG_HANDLE)
                        .size(px(12.0))
                        .flex_none()
                        .text_color(theme.text_muted.opacity(0.5)),
                )
                .child(
                    crate::icons::icon(icon)
                        .size(px(14.0))
                        .flex_none()
                        .text_color(tint.unwrap_or(theme.text)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(SharedString::from(entry.label.clone())),
                )
                .child(
                    div()
                        .id(("loadout-effort", index))
                        .h(px(22.0))
                        .px(px(8.0))
                        .flex()
                        .flex_none()
                        .items_center()
                        .rounded(px(6.0))
                        .bg(theme.muted_fill().opacity(0.8))
                        .text_color(theme.text)
                        .child(SharedString::from(
                            entry.effort.clone().unwrap_or_else(|| "Default".into()),
                        ))
                        .when(can_effort, |el| {
                            el.cursor_pointer()
                                .tip("Change effort")
                                .hover(|s| s.bg(theme.muted_fill()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.cycle_loadout_effort(index, cx)
                                }))
                        }),
                )
                .child(if can_fast || entry.fast {
                    quiet_button(("loadout-fast", index))
                        .aria_label("Fast mode")
                        .tip(if entry.fast {
                            "Fast mode on"
                        } else {
                            "Fast mode off"
                        })
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.toggle_loadout_fast(index, cx)),
                        )
                        .child(
                            crate::icons::icon(crate::icons::FAST_TIER_BOLD)
                                .size(px(12.0))
                                .text_color(if entry.fast {
                                    theme.text
                                } else {
                                    theme.text_muted.opacity(0.4)
                                }),
                        )
                        .into_any_element()
                } else {
                    div().size(px(24.0)).flex_none().into_any_element()
                })
                .child(
                    div()
                        .w(px(44.0))
                        .flex_none()
                        .flex()
                        .justify_end()
                        .text_color(theme.text_muted.opacity(0.7))
                        .child(SharedString::from(slot_label(index + 1))),
                )
                .child(
                    quiet_button(("loadout-remove", index))
                        .aria_label("Remove from picker")
                        .tip("Remove")
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.remove_loadout_entry(index, cx)),
                        )
                        .child(
                            crate::icons::icon(crate::icons::CLOSE)
                                .size(px(11.0))
                                .text_color(theme.text_muted),
                        ),
                )
                .into_any_element()
        });
        let empty = entries.is_empty().then(|| {
            div()
                .h(px(42.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .text_size(crate::typography::text_xs())
                .text_color(theme.text_muted)
                .child("No models yet. Add some from the lists below.")
        });
        let loadout = frame()
            .child(frame_header(
                div()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child("Picker models")
                    .into_any_element(),
                format!("{} of {LOADOUT_LIMIT}", entries.len()).into(),
            ))
            .child(card().children(rows).children(empty));

        let providers = descriptors.iter().map(|descriptor| {
            let harness = descriptor.id;
            let (icon, tint) = harness_brand_icon(harness);
            let models = self.models.get(&harness);
            let count = match models {
                Some(Loadable::Ready(models)) => models.len().to_string(),
                _ => String::new(),
            };
            let body = match models {
                Some(Loadable::Ready(models)) if !models.is_empty() => {
                    div().p(px(4.0)).flex().flex_col().gap(px(1.0)).children(
                        models.iter().enumerate().map(|(ix, model)| {
                            let added = entries
                                .iter()
                                .any(|e| e.harness == harness && e.model == model.id);
                            let disabled = !added && full;
                            let model_row = model.clone();
                            div()
                                .id(SharedString::from(format!("loadout-pick-{harness:?}-{ix}")))
                                .role(gpui::Role::Button)
                                .aria_label(SharedString::from(format!(
                                    "{} {}",
                                    if added { "Remove" } else { "Add" },
                                    model.label
                                )))
                                .h(px(28.0))
                                .px(px(8.0))
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .rounded(px(6.0))
                                .text_size(crate::typography::text_xs())
                                .text_color(if added { theme.text } else { theme.text_muted })
                                .when(disabled, |el| el.opacity(0.45))
                                .when(!disabled, |el| {
                                    el.cursor_pointer()
                                        .hover(|s| {
                                            s.bg(theme.muted_fill().opacity(0.6))
                                                .text_color(theme.text)
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toggle_loadout_model(harness, &model_row, cx)
                                        }))
                                })
                                .child(
                                    crate::icons::icon(if added {
                                        crate::icons::CHECK
                                    } else {
                                        crate::icons::PLUS
                                    })
                                    .size(px(12.0))
                                    .flex_none()
                                    .text_color(if added {
                                        theme.text
                                    } else {
                                        theme.text_muted.opacity(0.6)
                                    }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child(SharedString::from(model.label.clone())),
                                )
                        }),
                    )
                }
                other => div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .text_size(crate::typography::text_xs())
                    .text_color(theme.text_muted)
                    .child(match other {
                        Some(Loadable::Error(error)) => {
                            SharedString::from(super::model_catalog_notice(error))
                        }
                        Some(Loadable::Ready(_)) => "No models".into(),
                        _ => "Loading models…".into(),
                    }),
            };
            frame()
                .child(frame_header(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_color(theme.text)
                        .child(
                            crate::icons::icon(icon)
                                .size(px(14.0))
                                .text_color(tint.unwrap_or(theme.text_muted)),
                        )
                        .child(SharedString::from(descriptor.name.clone()))
                        .into_any_element(),
                    count.into(),
                ))
                .child(card().child(body))
        });

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(loadout)
            .child(
                div()
                    .grid()
                    .grid_cols(3)
                    .gap(px(12.0))
                    .items_start()
                    .children(providers),
            )
            .into_any_element()
    }
}

/// Dragging a loadout row to a new place in Settings.
#[derive(Clone)]
struct LoadoutDrag {
    from: usize,
}

/// The (invisible) drag preview; the drop target's highlight shows where it lands.
struct LoadoutGhost;
impl Render for LoadoutGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Control row in the loadout card: label left, control right.
fn control_row(theme: &Theme, label: &'static str) -> gpui::Div {
    div()
        .h(px(CONTROL_ROW_HEIGHT))
        .px(px(8.0))
        .flex()
        .items_center()
        .justify_between()
        .rounded(px(MODEL_ROW_RADIUS))
        .text_size(crate::typography::text_xs())
        .text_color(theme.text_muted)
        .child(label)
}

/// The shortcut that picks loadout slot `slot` while the picker is open.
pub fn slot_label(slot: usize) -> String {
    format!(
        "{}{slot}",
        if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl+"
        }
    )
}

#[cfg(feature = "browser-fixture")]
impl Pickers {
    /// Review fixture: the catalog plus a five-entry loadout, so the model
    /// card and Settings → Default models render real entries.
    pub fn fixture_seed_loadout(&mut self, cx: &mut Context<Self>) {
        self.fixture_seed_catalog(cx);
        let entry = |harness, model: &str, label: &str, reasoning, fast: bool| {
            let mut options = ModelOptions::new();
            if !fast && model == "claude-sonnet-4-6" {
                options.insert("serviceTier".into(), "standard".into());
            }
            LoadoutEntry {
                harness,
                model: model.into(),
                label: label.into(),
                reasoning: Some(reasoning),
                options,
                effort: Some(reasoning_label(reasoning).into()),
                fast,
            }
        };
        self.defaults.loadout = vec![
            entry(
                HarnessId::ClaudeCode,
                "claude-sonnet-4-6",
                "claude-sonnet-4-6",
                ReasoningLevel::High,
                true,
            ),
            entry(
                HarnessId::ClaudeCode,
                "claude-opus-4-1",
                "Claude Opus 4.1",
                ReasoningLevel::High,
                false,
            ),
            entry(
                HarnessId::Codex,
                "gpt-5-codex",
                "GPT-5 Codex",
                ReasoningLevel::Medium,
                false,
            ),
            entry(
                HarnessId::ClaudeCode,
                "claude-sonnet-4-6",
                "claude-sonnet-4-6",
                ReasoningLevel::Low,
                false,
            ),
            entry(
                HarnessId::ClaudeCode,
                "claude-haiku-4-5",
                "Claude Haiku 4.5",
                ReasoningLevel::Low,
                false,
            ),
        ];
        self.persist_defaults();
        self.catalog_rev += 1;
        cx.notify();
    }
}
