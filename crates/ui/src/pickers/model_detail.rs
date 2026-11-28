//! The model menu's detail card: what the hovered (or keyboard-highlighted)
//! model offers, and its reasoning, fast mode and other options.
//!
//! The card always describes one row. Its values are what picking that row
//! would run with; changing any of them applies to that model and selects it,
//! so a click on the row afterwards (or closing the menu) keeps them.
use super::*;
use crate::tooltip::TooltipExt as _;
use zuse_protocol::{ModelOption, ModelOptionChoiceDetails};

pub(super) const CARD_WIDTH: f32 = 240.;
const PAD: f32 = 12.;
const HEADER: f32 = 40.;
/// Option rows and toggle chips share one height.
const ROW: f32 = 30.;
const TOGGLE: f32 = 28.;
/// The reasoning slider: a pill track, a soft pill knob and dot stops.
const TRACK: f32 = 20.;
const KNOB_W: f32 = 22.;
const KNOB_H: f32 = 14.;
const DOT: f32 = 3.;

/// Everything the card shows for one row.
pub(super) struct Detail {
    pub row: ModelRowData,
    pub effort: Option<compact::CompactEffort>,
    /// Fast mode offered by the model, and whether it is on.
    pub fast: Option<bool>,
    /// The long-context switch, for models whose catalog offers one.
    pub long_context: Option<LongContext>,
    /// The remaining options (context window, ...), excluding the ones the
    /// slider and the fast-mode switch own.
    pub options: Vec<SettingGroup>,
    fusion: Option<FusionControls>,
    pub favorite: bool,
    pub is_default: bool,
}

#[derive(Clone)]
struct FusionChoice {
    id: String,
    label: String,
    effort: Option<ReasoningLevel>,
}

struct FusionControls {
    leads: Vec<FusionChoice>,
    lead: usize,
    sidekicks: Vec<FusionChoice>,
    sidekick: usize,
}

fn selected_option<'a>(
    option: &'a ModelOption,
    options: &'a serde_json::Map<String, serde_json::Value>,
) -> &'a str {
    options
        .get(&option.id)
        .and_then(|v| v.as_str())
        .unwrap_or(&option.default_choice)
}

fn fusion_options(model: &Model) -> Option<(&ModelOption, &ModelOption)> {
    (model.id == "fusion").then_some(())?;
    Some((
        model.options.iter().find(|o| o.id == "lead")?,
        model.options.iter().find(|o| o.id == "sidekick")?,
    ))
}

pub(super) fn fusion_lead_details<'a>(
    model: &'a Model,
    options: &serde_json::Map<String, serde_json::Value>,
) -> Option<&'a ModelOptionChoiceDetails> {
    let (lead, _) = fusion_options(model)?;
    lead.choice_details
        .get(selected_option(lead, options))
        .or_else(|| lead.choice_details.get(&lead.default_choice))
}

impl FusionControls {
    fn new(model: &Model, options: &serde_json::Map<String, serde_json::Value>) -> Option<Self> {
        let (lead_option, sidekick_option) = fusion_options(model)?;
        let lead = lead_option
            .choices
            .iter()
            .position(|c| c.id == selected_option(lead_option, options))
            .unwrap_or(0);
        let leads: Vec<_> = lead_option
            .choices
            .iter()
            .map(|c| FusionChoice {
                id: c.id.clone(),
                label: c.label.clone(),
                effort: None,
            })
            .collect();
        let allowed = lead_option
            .choice_details
            .get(&leads.get(lead)?.id)
            .and_then(|details| details.compatible_options.get("sidekick"));
        let sidekicks: Vec<_> = sidekick_option
            .choices
            .iter()
            .filter(|c| allowed.is_none_or(|ids| ids.contains(&c.id)))
            .map(|c| {
                let details = sidekick_option.choice_details.get(&c.id);
                FusionChoice {
                    id: c.id.clone(),
                    label: details
                        .and_then(|d| d.model_label.clone())
                        .unwrap_or_else(|| c.label.clone()),
                    effort: details.and_then(|d| d.reasoning_levels.first().copied()),
                }
            })
            .collect();
        let sidekick = sidekicks
            .iter()
            .position(|c| c.id == selected_option(sidekick_option, options))
            .or_else(|| {
                sidekicks
                    .iter()
                    .position(|c| c.id == sidekick_option.default_choice)
            })
            .or_else(|| {
                let first = allowed?.first()?;
                sidekicks.iter().position(|c| &c.id == first)
            })
            .unwrap_or(0);
        (!sidekicks.is_empty()).then_some(Self {
            leads,
            lead,
            sidekicks,
            sidekick,
        })
    }

    fn sidekick_efforts(&self) -> Vec<&FusionChoice> {
        let selected = &self.sidekicks[self.sidekick];
        let mut efforts: Vec<_> = self
            .sidekicks
            .iter()
            .filter(|c| c.label == selected.label && c.effort.is_some())
            .collect();
        efforts.sort_by_key(|c| c.effort);
        efforts
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DetailSlider {
    Reasoning,
    Sidekick,
}

/// A model's long-context option as one switch ("1M context").
#[derive(Clone, Debug)]
pub(super) struct LongContext {
    pub option: String,
    pub on_value: String,
    pub off_value: String,
    pub on_label: String,
    pub off_label: String,
    pub on: bool,
    pub off_default: bool,
}

/// "200k" / "1m" as a token count.
fn token_count(value: &str) -> Option<f64> {
    let value = value.trim().to_ascii_lowercase();
    let (number, scale) = if let Some(n) = value.strip_suffix('m') {
        (n, 1_000_000.)
    } else {
        (value.strip_suffix('k')?, 1_000.)
    };
    number.parse::<f64>().ok().map(|n| n * scale)
}

/// The model's long-context switch, read from its catalog options: a
/// two-choice context option whose larger window is at least a million
/// tokens. Models whose catalog has no such option get no switch.
fn long_context(
    model: &Model,
    options: &serde_json::Map<String, serde_json::Value>,
) -> Option<LongContext> {
    model.options.iter().find_map(|option| {
        if !option.id.to_ascii_lowercase().contains("context") {
            return None;
        }
        let (on, off) = zuse_protocol::presentation::binary_switch_values(
            option.choices.iter().map(|c| c.id.as_str()),
        )?;
        if token_count(on)? < 1_000_000. || token_count(off)? >= 1_000_000. {
            return None;
        }
        let current = options
            .get(&option.id)
            .and_then(|v| v.as_str())
            .unwrap_or(&option.default_choice);
        Some(LongContext {
            option: option.id.clone(),
            on_value: on.to_owned(),
            off_value: off.to_owned(),
            on_label: option
                .choices
                .iter()
                .find(|c| c.id == on)
                .map(|c| c.label.clone())
                .unwrap_or_else(|| on.to_uppercase()),
            off_label: option
                .choices
                .iter()
                .find(|c| c.id == off)
                .map(|c| c.label.clone())
                .unwrap_or_else(|| off.to_uppercase()),
            on: current == on,
            off_default: option.default_choice == off,
        })
    })
}

/// An icon-only toggle in the card: the pixel icon, muted when off and in
/// the accent on a faint wash when on. The tooltip names it and its state.
fn icon_toggle(
    id: &'static str,
    icon: &'static str,
    label: &'static str,
    on: bool,
    tip: &'static str,
    theme: &Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let hover = theme.muted_fill().opacity(0.8);
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label)
        .aria_toggled(if on {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .tab_index(0)
        .size(px(TOGGLE))
        .flex_none()
        .rounded(px(7.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(on, |el| el.bg(theme.accent.opacity(0.12)))
        .when(!on, |el| el.hover(move |s| s.bg(hover)))
        .tip(tip)
        .on_click(on_click)
        .child(
            crate::icons::icon(icon)
                .size(px(16.))
                .flex_none()
                .text_color(if on { theme.accent } else { theme.text_muted }),
        )
        .into_any_element()
}

/// The card's tooltip on the name: the description, or for a model that
/// shares its name with another, its id. The list shows names only.
fn card_note(row: &ModelRowData) -> Option<SharedString> {
    row.model
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| SharedString::from(d.to_owned()))
        .or_else(|| row.ambiguous.then(|| row.model.id.clone().into()))
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
    /// chat (or bound cloud task) keeps its resolved level, clamped, on the
    /// same provider and takes the
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
        let existing = self.edits_saved_choice(cx);
        let same_provider = self.effective_harness(cx) == Some(row.harness);
        let (reasoning, options) = if existing && same_provider {
            // A pick restages the resolved configuration, then clamps it.
            let resolved = self.resolved(cx);
            (resolved.reasoning, resolved.model_options)
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
            let (levels, reasoning) = if row.harness == HarnessId::Devin
                && let Some(details) = fusion_lead_details(&row.model, &options)
            {
                (
                    details.reasoning_levels.clone(),
                    clamp_reasoning(reasoning, &details.reasoning_levels),
                )
            } else {
                (levels, reasoning)
            };
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
        let long = configurable
            .then(|| long_context(&row.model, &options))
            .flatten();
        let fusion = (configurable && row.harness == HarnessId::Devin)
            .then(|| FusionControls::new(&row.model, &options))
            .flatten();
        let hidden: Vec<&str> = effort
            .as_ref()
            .and_then(|e| e.option_id())
            .into_iter()
            .chain(fast_option.map(|(o, _)| o.id.as_str()))
            .chain(long.as_ref().map(|l| l.option.as_str()))
            .chain(
                fusion
                    .as_ref()
                    .into_iter()
                    .flat_map(|_| ["lead", "sidekick"]),
            )
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
            long_context: long,
            options: groups,
            fusion,
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

    fn set_fusion_model(&mut self, role: &'static str, choice: &str, cx: &mut Context<Self>) {
        if !self.select_detail_model(HarnessId::Devin, "fusion", cx) {
            return;
        }
        let Some(model) = self.selected_model(cx) else {
            return;
        };
        let Some((lead, sidekick)) = fusion_options(model) else {
            return;
        };
        let mut options = self.explicit_options(cx);
        if role == "lead" {
            if let Some(current) = FusionControls::new(model, &options) {
                options.insert(
                    "sidekick".into(),
                    current.sidekicks[current.sidekick].id.clone().into(),
                );
            }
            options.insert("lead".into(), choice.into());
        } else {
            options.insert("sidekick".into(), choice.into());
        }
        let Some(fusion) = FusionControls::new(model, &options) else {
            return;
        };
        let lead_id = fusion.leads[fusion.lead].id.clone();
        let sidekick_id = fusion.sidekicks[fusion.sidekick].id.clone();
        let picks = [
            (
                "lead".into(),
                lead_id.clone(),
                lead_id == lead.default_choice,
            ),
            (
                "sidekick".into(),
                sidekick_id.clone(),
                sidekick_id == sidekick.default_choice,
            ),
        ];
        self.model_menu.fusion_selector = None;
        self.pick_options(picks, cx);
    }

    fn fusion_selector(
        &self,
        role: &'static str,
        label: &'static str,
        choices: &[FusionChoice],
        selected: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = &choices[selected];
        let open = self.model_menu.fusion_selector == Some(role);
        let mut names = HashSet::new();
        let rows = choices
            .iter()
            .enumerate()
            .filter(|(_, c)| names.insert(c.label.clone()))
            .map(|(ix, choice)| {
                let picked = choice.label == current.label;
                // Keep the current reasoning level when the newly selected
                // Sidekick supports it.
                let id = choices
                    .iter()
                    .find(|c| c.label == choice.label && c.effort == current.effort)
                    .unwrap_or(choice)
                    .id
                    .clone();
                crate::settings::widgets::select_row(
                    theme,
                    picked,
                    false,
                    format!("fusion-{role}-{ix}"),
                )
                .id((role, ix))
                .debug_selector(move || format!("fusion-{role}-choice-{ix}"))
                .role(gpui::Role::MenuItemRadio)
                .tab_index(0)
                .aria_label(choice.label.clone())
                .aria_selected(picked)
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.set_fusion_model(role, &id, cx);
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(choice.label.clone()),
                )
                .child(crate::settings::widgets::select_check(theme, picked))
            });
        div()
            .id(role)
            .flex()
            .flex_col()
            .gap(px(4.))
            .text_size(crate::typography::text_xs())
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if this.model_menu.fusion_selector == Some(role) && event.keystroke.key == "escape"
                {
                    this.model_menu.fusion_selector = None;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(div().text_color(theme.text_muted).child(label))
            .child(
                crate::settings::widgets::select_trigger(theme, format!("fusion-{role}"), open)
                    .debug_selector(move || format!("fusion-{role}"))
                    .aria_label(format!("{label}: {}", current.label))
                    .tip(current.label.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.model_menu.fusion_selector = if open { None } else { Some(role) };
                        cx.notify();
                    }))
                    .child(crate::settings::widgets::select_value(
                        current.label.clone(),
                    ))
                    .child(crate::settings::widgets::select_chevron(theme, open)),
            )
            .when(open, |el| {
                el.child(
                    crate::settings::widgets::select_menu(theme)
                        .id(SharedString::from(format!("fusion-{role}-choices")))
                        .max_h(px(114.))
                        .overflow_y_scroll()
                        .children(rows),
                )
            })
            .into_any_element()
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

    /// Stop starting new chats with a particular model.
    pub(super) fn clear_detail_default(&mut self, cx: &mut Context<Self>) {
        crate::settings::update(crate::settings::SavePolicy::Immediate, cx, |settings| {
            settings.default_model = None;
        });
        self.catalog_rev += 1;
        cx.refresh_windows();
        cx.notify();
    }

    pub(super) fn render_model_detail(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let detail = self.model_detail(cx)?;
        if detail.fusion.is_none() {
            self.model_menu.fusion_selector = None;
        }
        let theme = Theme::of(cx).for_popup();
        let ink = popover::dither_ink(&theme);
        let harness = detail.row.harness;
        let model_id: SharedString = detail.row.model.id.clone().into();
        let label: SharedString = detail.row.model.label.clone().into();
        let (path, tint) = harness_brand_icon(harness);
        let divider = || div().h(px(1.)).flex_none().bg(theme.border.opacity(0.5));
        let header = div()
            .relative()
            .h(px(HEADER))
            .px(px(PAD))
            .flex()
            .items_center()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .w(px(130.))
                    .h(px(HEADER))
                    .child(zuse_viewport::dither::corner(
                        ink.opacity(0.55),
                        popover::CARD_RADIUS,
                    )),
            )
            .child(
                div()
                    .id("model-detail-name")
                    .relative()
                    .min_w_0()
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
                            .flex_none()
                            .max_w(px(CARD_WIDTH - 2. * PAD - 96.))
                            .truncate()
                            .text_size(crate::typography::text_sm())
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text_strong)
                            .child(label.clone()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::text_xs())
                            .text_color(theme.text_muted)
                            .child(detail.row.harness_name.clone()),
                    )
                    .when_some(card_note(&detail.row), |el, note| el.tip(note)),
            );

        let mut body = div()
            .px(px(PAD))
            .pt(px(10.))
            .pb(px(PAD))
            .flex()
            .flex_col()
            .gap(px(8.));
        if let Some(fusion) = &detail.fusion {
            body = body.child(self.fusion_selector(
                "lead",
                "Lead",
                &fusion.leads,
                fusion.lead,
                &theme,
                cx,
            ));
        }
        if let Some(effort) = detail.effort.as_ref() {
            body = body.child(self.detail_slider(
                DetailSlider::Reasoning,
                if detail.fusion.is_some() {
                    "Lead reasoning"
                } else {
                    "Reasoning"
                },
                &effort.labels,
                effort.selected,
                harness,
                model_id.clone(),
                &theme,
                cx,
            ));
            if detail.fusion.is_none() {
                body = body.child(div().h(px(1.)));
            }
        }
        if let Some(fusion) = &detail.fusion {
            body = body.child(self.fusion_selector(
                "sidekick",
                "Sidekick",
                &fusion.sidekicks,
                fusion.sidekick,
                &theme,
                cx,
            ));
            let efforts = fusion.sidekick_efforts();
            if let Some(selected) = efforts
                .iter()
                .position(|c| c.id == fusion.sidekicks[fusion.sidekick].id)
            {
                let labels: Vec<_> = efforts
                    .iter()
                    .filter_map(|c| c.effort.map(|level| reasoning_label(level).into()))
                    .collect();
                body = body.child(self.detail_slider(
                    DetailSlider::Sidekick,
                    "Sidekick reasoning",
                    &labels,
                    selected,
                    harness,
                    model_id.clone(),
                    &theme,
                    cx,
                ));
            }
        }

        // One row under the slider: icon toggles on the left, the context
        // window switch on the right. Hover names each toggle.
        let mut toggles = div().flex().items_center().gap(px(2.));
        if let Some(on) = detail.fast {
            let model = model_id.clone();
            toggles = toggles.child(icon_toggle(
                "model-detail-fast",
                crate::icons::DITHER_BOLT,
                "Fast mode",
                on,
                if on {
                    "Fast mode · On"
                } else {
                    "Fast mode · Off"
                },
                &theme,
                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.set_detail_fast(harness, &model, !on, cx);
                }),
            ));
        }
        let favorite = detail.favorite;
        let favorite_model = model_id.clone();
        toggles = toggles.child(icon_toggle(
            "model-detail-favorite",
            crate::icons::DITHER_BOOKMARK,
            "Favorite",
            favorite,
            if favorite {
                "Remove from favorites"
            } else {
                "Add to favorites"
            },
            &theme,
            cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                cx.stop_propagation();
                this.toggle_detail_favorite(harness, &favorite_model, cx);
            }),
        ));
        let is_default = detail.is_default;
        let default_model = model_id.clone();
        let default_label = label.clone();
        toggles = toggles.child(icon_toggle(
            "model-detail-default",
            crate::icons::DITHER_FLAG,
            "Default model",
            is_default,
            if is_default {
                "Default model · New chats start here"
            } else {
                "Set as default"
            },
            &theme,
            cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                cx.stop_propagation();
                if is_default {
                    this.clear_detail_default(cx);
                } else {
                    this.set_detail_default(harness, &default_model, &default_label, cx);
                }
            }),
        ));
        let context = detail.long_context.clone().map(|long| {
            let segment =
                |id: &'static str, text: String, value: String, default: bool, on: bool| {
                    let model = model_id.clone();
                    let option = long.option.clone();
                    div()
                        .id(id)
                        .role(gpui::Role::RadioButton)
                        .aria_label(SharedString::from(text.clone()))
                        .aria_selected(on)
                        .tab_index(0)
                        .h(px(22.))
                        .min_w(px(40.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_color(if on {
                            theme.text_strong
                        } else {
                            theme.text_muted
                        })
                        .when(on, |el| model_selected(el, &theme))
                        .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                            cx.stop_propagation();
                            if !on {
                                this.set_detail_option(
                                    harness,
                                    &model,
                                    option.clone(),
                                    value.clone(),
                                    default,
                                    cx,
                                );
                            }
                        }))
                        .child(text)
                };
            div()
                .id("model-detail-context")
                .role(gpui::Role::RadioGroup)
                .aria_label("Context window")
                .tip("Context window")
                .flex_none()
                .p(px(2.))
                .rounded(px(8.))
                .bg(theme.muted_fill().opacity(0.45))
                .flex()
                .gap(px(2.))
                .text_size(crate::typography::text_xs())
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(segment(
                    "model-detail-context-off",
                    long.off_label.clone(),
                    long.off_value.clone(),
                    long.off_default,
                    !long.on,
                ))
                .child(segment(
                    "model-detail-context-on",
                    long.on_label.clone(),
                    long.on_value.clone(),
                    !long.off_default,
                    long.on,
                ))
        });
        body = body.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(toggles)
                .children(context),
        );
        for (gx, group) in detail.options.iter().enumerate() {
            body = body.child(self.detail_option(gx, group, harness, &model_id, &theme, cx));
        }

        let entity = cx.entity().downgrade();
        Some(
            popover::popover_card_flush(&theme)
                .id("model-detail-card")
                .w(px(CARD_WIDTH))
                .flex()
                .flex_col()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(header)
                .child(divider())
                .child(body)
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
        slider: DetailSlider,
        name: &'static str,
        labels: &[SharedString],
        selected: usize,
        harness: HarnessId,
        model: SharedString,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = labels.len();
        let label = labels.get(selected).cloned().unwrap_or_default();
        let heading = div()
            .flex()
            .items_center()
            .justify_between()
            .text_size(crate::typography::text_xs())
            .child(div().text_color(theme.text_muted).child(name))
            .child(div().text_color(theme.text).child(label.clone()));
        if count <= 1 {
            return heading.into_any_element();
        }
        let width = CARD_WIDTH - 2. * PAD;
        let inset = compact::THUMB_INSET;
        let stop_x = |index: usize| {
            inset + index as f32 / count.saturating_sub(1).max(1) as f32 * (width - 2. * inset)
        };
        let knob_x = stop_x(selected);
        let (track, knob_w, knob_h) = (TRACK, KNOB_W, KNOB_H);
        let radius = track / 2.;
        // The hit area stays comfortable when the track is slim.
        let height = track.max(knob_h) + 4.;
        let top = (height - track) / 2.;
        let ink = popover::dither_ink(theme);
        let entity = cx.entity().downgrade();
        let down_model = model.clone();
        let key_model = model.clone();
        let control = div()
            .id(match slider {
                DetailSlider::Reasoning => "model-detail-effort",
                DetailSlider::Sidekick => "model-detail-sidekick-effort",
            })
            .debug_selector(move || name.to_owned())
            .role(gpui::Role::Slider)
            .tab_index(0)
            .aria_label(SharedString::from(format!("{name}: {label}")))
            .aria_numeric_value(selected as f64)
            .aria_min_numeric_value(0.)
            .aria_max_numeric_value(count.saturating_sub(1) as f64)
            .relative()
            .w(px(width))
            .h(px(height))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if this.select_detail_model(harness, &down_model, cx) {
                        this.set_detail_dragging(slider, true);
                        this.pick_detail_effort_at(slider, event.position.x, cx);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                    let dragging = match slider {
                        DetailSlider::Reasoning => this.effort_dragging,
                        DetailSlider::Sidekick => this.model_menu.sidekick_dragging,
                    };
                    if dragging && event.pressed_button == Some(gpui::MouseButton::Left) {
                        this.pick_detail_effort_at(slider, event.position.x, cx);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.set_detail_dragging(slider, false);
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.set_detail_dragging(slider, false);
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                let last = count - 1;
                let index = match event.keystroke.key.as_str() {
                    "left" | "down" => selected.saturating_sub(1),
                    "right" | "up" => (selected + 1).min(last),
                    "home" => 0,
                    "end" => last,
                    _ => return,
                };
                cx.stop_propagation();
                match slider {
                    DetailSlider::Reasoning => {
                        this.set_detail_effort(harness, &key_model, index, cx)
                    }
                    DetailSlider::Sidekick => this.set_sidekick_effort(index, cx),
                }
            }))
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        let _ = entity.update(cx, |this, _| match slider {
                            DetailSlider::Reasoning => this.effort_bounds = Some(bounds),
                            DetailSlider::Sidekick => {
                                this.model_menu.sidekick_slider_bounds = Some(bounds)
                            }
                        });
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
                    .top(px(top))
                    .w(px(width))
                    .h(px(track))
                    .rounded_full()
                    .bg(theme.muted_fill().opacity(0.55)),
            )
            // The filled span: a faint accent wash with a light dither
            // texture, rounded at both ends like the track.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(top))
                    .h(px(track))
                    .w(px((knob_x + radius.max(knob_w / 2.)).min(width)))
                    .rounded_full()
                    .bg(theme.accent.opacity(0.12))
                    .child(zuse_viewport::dither::ramp(ink.opacity(0.32), radius)),
            )
            .children(((selected + 1)..count).map(|index| {
                div()
                    .absolute()
                    .left(px(stop_x(index) - DOT / 2.))
                    .top(px((height - DOT) / 2.))
                    .size(px(DOT))
                    .rounded_full()
                    .bg(theme.text_muted.opacity(0.5))
            }))
            .child(
                div()
                    .absolute()
                    .left(px(knob_x - knob_w / 2.))
                    .top(px((height - knob_h) / 2.))
                    .w(px(knob_w))
                    .h(px(knob_h))
                    .rounded_full()
                    .bg(gpui::white())
                    .shadow_sm(),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(heading)
            .child(control)
            .into_any_element()
    }

    fn set_detail_dragging(&mut self, slider: DetailSlider, dragging: bool) {
        match slider {
            DetailSlider::Reasoning => self.effort_dragging = dragging,
            DetailSlider::Sidekick => self.model_menu.sidekick_dragging = dragging,
        }
    }

    fn pick_detail_effort_at(
        &mut self,
        slider: DetailSlider,
        x: gpui::Pixels,
        cx: &mut Context<Self>,
    ) {
        if slider == DetailSlider::Reasoning {
            return self.pick_effort_at(x, cx);
        }
        let Some(bounds) = self.model_menu.sidekick_slider_bounds else {
            return;
        };
        let Some(fusion) = self.model_detail(cx).and_then(|detail| detail.fusion) else {
            return;
        };
        if let Some(index) = compact::effort_index(
            f32::from(x - bounds.left()),
            f32::from(bounds.size.width),
            fusion.sidekick_efforts().len(),
        ) {
            self.set_sidekick_effort(index, cx);
        }
    }

    fn set_sidekick_effort(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(fusion) = self.model_detail(cx).and_then(|detail| detail.fusion) else {
            return;
        };
        if let Some(choice) = fusion.sidekick_efforts().get(index)
            && choice.id != fusion.sidekicks[fusion.sidekick].id
        {
            self.set_fusion_model("sidekick", &choice.id, cx);
        }
    }

    /// One more option of the card's model: a switch for an on/off option,
    /// otherwise a compact segmented choice (a context window reads
    /// "200K | 1M", not a switch).
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
        if is_toggle(group) && is_on_off(group) {
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
                .h(px(ROW))
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
        // Two choices sit beside the label (Context window  200K | 1M);
        // longer lists take their own line.
        let inline = group.choices.len() <= 2;
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
                .when(inline, |el| el.flex_none().min_w(px(52.)))
                .when(!inline, |el| el.flex_1().min_w_0())
                .h(px(22.))
                .px(px(8.))
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
            .when(inline, |el| el.h(px(ROW)).items_center().gap(px(12.)))
            .when(!inline, |el| el.py(px(8.)).flex_col().gap(px(6.)))
            .text_size(crate::typography::text_xs())
            .child(label)
            .child(
                div()
                    .when(inline, |el| el.flex_none())
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

/// Whether an option's two choices read as off and on, so a switch says it
/// all; named values (sizes, tiers) stay visible as segments.
fn is_on_off(group: &SettingGroup) -> bool {
    group.choices.iter().all(|choice| {
        matches!(
            choice.label.trim().to_ascii_lowercase().as_str(),
            "on" | "off" | "enabled" | "disabled" | "true" | "false" | "yes" | "no"
        )
    })
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
    fn fusion_selectors_and_reasoning_controls_apply_independent_picks(
        cx: &mut gpui::TestAppContext,
    ) {
        use ReasoningLevel::{High, Low, Medium};

        let (_dir, handle) = picker_window(cx);
        handle
            .update(cx, |picker, window, cx| {
                picker.harnesses = Loadable::Ready(vec![descriptor(HarnessId::Devin, "Devin")]);
                let fusion: Model = serde_json::from_value(serde_json::json!({
                    "id": "fusion", "label": "Fusion", "reasoningLevels": ["low", "medium", "high"],
                    "options": [
                        {"id": "lead", "label": "Lead", "defaultChoice": "opus",
                         "choices": [{"id":"opus","label":"Claude Opus 5.5"},
                                     {"id":"sol","label":"GPT-6.2 Sol"}],
                         "choiceDetails": {
                             "opus":{"reasoningLevels":["medium","high"],"compatibleOptions":{"sidekick":["haiku-medium","swe-medium","swe-high"]}},
                             "sol":{"reasoningLevels":["low","medium"],"compatibleOptions":{"sidekick":["swe-medium","swe-high"]}}
                         }},
                        {"id": "sidekick", "label": "Sidekick", "defaultChoice": "haiku-medium",
                         "choices": [{"id":"haiku-medium","label":"Haiku Medium"},
                                     {"id":"swe-medium","label":"SWE-2 Medium"},
                                     {"id":"swe-high","label":"SWE-2 High"}],
                         "choiceDetails": {
                             "haiku-medium":{"modelLabel":"Haiku","reasoningLevels":["medium"],"compatibleOptions":{}},
                             "swe-medium":{"modelLabel":"SWE-2","reasoningLevels":["medium"],"compatibleOptions":{}},
                             "swe-high":{"modelLabel":"SWE-2","reasoningLevels":["high"],"compatibleOptions":{}}
                         }}
                    ]
                }))
                .unwrap();
                picker.models.insert(HarnessId::Devin, Loadable::Ready(vec![fusion]));
                picker.config.harness = Some(HarnessId::Devin);
                picker.config.model = Some("fusion".into());
                picker.open_model_menu(window, cx);
            })
            .unwrap();
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(gpui::size(px(1180.), px(760.)));
        let draw = |cx: &mut gpui::VisualTestContext| {
            // The second frame positions the detail card from the measured list.
            cx.update(|window, cx| {
                window.draw(cx).clear();
                window.draw(cx).clear();
            });
        };
        let click = |cx: &mut gpui::VisualTestContext, selector: &'static str, right: bool| {
            draw(cx);
            let bounds = cx.debug_bounds(selector).expect(selector);
            let mut point = bounds.center();
            if right {
                point.x = bounds.right() - px(4.);
            }
            cx.simulate_click(point, gpui::Modifiers::default());
        };
        click(cx, "fusion-sidekick", false);
        click(cx, "fusion-sidekick-choice-1", false);
        click(cx, "Sidekick reasoning", true);
        handle
            .update(cx, |picker, _, cx| {
                let run = picker.resolved(cx);
                assert_eq!(run.model_options["sidekick"], "swe-high");
                assert_eq!(run.reasoning, Some(High));
            })
            .unwrap();

        click(cx, "fusion-lead", false);
        click(cx, "fusion-lead-choice-1", false);
        draw(cx);
        let bounds = cx.debug_bounds("Lead reasoning").unwrap();
        cx.simulate_click(
            gpui::point(bounds.left() + px(4.), bounds.center().y),
            gpui::Modifiers::default(),
        );
        handle
            .update(cx, |picker, _, cx| {
                let run = picker.resolved(cx);
                assert_eq!(run.model_options["lead"], "sol");
                assert_eq!(run.model_options["sidekick"], "swe-high");
                assert_eq!(run.reasoning, Some(Low));
                assert_eq!(picker.trait_ladder(cx), [Low, Medium]);
            })
            .unwrap();
        click(cx, "fusion-sidekick", false);
        draw(cx);
        assert!(
            cx.debug_bounds("fusion-sidekick-choice-1").is_none(),
            "one compatible Sidekick model, with two reasoning levels"
        );
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
                assert!(detail.long_context.is_none());
                assert!(detail.options.is_empty());
                assert_eq!(detail.effort.as_ref().unwrap().labels.len(), 3);

                picker.active = row_index(picker, "opus", cx);
                let detail = picker.model_detail(cx).unwrap();
                assert_eq!(detail.fast, Some(false));
                // Fast mode and the 1M window are switches of their own.
                let long = detail.long_context.unwrap();
                assert_eq!(long.option, "contextWindow");
                assert_eq!(long.on_label, "1M");
                assert!(!long.on);
                assert!(detail.options.is_empty());

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

                picker.clear_detail_default(cx);
                assert!(!picker.model_detail(cx).unwrap().is_default);
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

    #[gpui::test]
    fn card_preview_matches_what_a_pick_restages_in_an_existing_chat(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_dir, handle) = picker_window(cx);
        handle
            .update(cx, |picker, _, cx| {
                // Model A offers only Medium; B offers Medium and High.
                let mut a = bare_model("a", "A");
                a.reasoning_levels = vec![ReasoningLevel::Medium];
                let mut b = bare_model("b", "B");
                b.reasoning_levels = vec![ReasoningLevel::Medium, ReasoningLevel::High];
                picker
                    .models
                    .insert(HarnessId::Codex, Loadable::Ready(vec![a, b]));
                picker.catalog_rev += 1;
                picker.state.update(cx, |state, cx| {
                    state.chats = vec![
                        serde_json::from_value(serde_json::json!({
                            "id":"saved", "deviceId":"local", "archived":false,
                            "createdAt":"2026-01-01T00:00:00Z",
                            "config":{"harness":"codex","model":"a","sandbox":"workspace-write"}
                        }))
                        .unwrap(),
                    ];
                    state.selected_chat = Some("saved".into());
                    cx.notify();
                });
                picker.config = DraftConfig::default();
                assert_eq!(picker.resolved(cx).reasoning, Some(ReasoningLevel::Medium));
                picker.active = row_index(picker, "b", cx);
                let effort = picker.model_detail(cx).unwrap().effort.unwrap();
                // Not B's own default (High): the chat's resolved Medium.
                assert_eq!(effort.labels[effort.selected].as_ref(), "Medium");
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_card_pick_on_a_bound_cloud_task_updates_its_saved_choice(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (send, _requests) = tokio::sync::mpsc::channel(1);
        let (_responses, receive) = tokio::sync::mpsc::channel(1);
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, receive));
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
        });
        // No selected chat: the task's saved choice is all that names a model.
        let state = cx.new(|_| AppState::new());
        let pickers = cx.new(|cx| Pickers::new(state, cx));
        pickers.update(cx, |picker, cx| {
            let target = crate::state::CloudTarget {
                controller,
                subject: "owner".into(),
                organization: None,
                workspace: "task".into(),
                initial_session: "session".into(),
            };
            let choice = ChatConfig {
                harness: HarnessId::Codex,
                model: Some("bound".into()),
                reasoning: Some(ReasoningLevel::High),
                model_options: Default::default(),
                sandbox: SandboxLevel::WorkspaceWrite,
                plan: false,
            };
            picker.bind_cloud_task(target, "boxd".into(), Some(choice), cx);
            picker.receive_cloud_models(
                serde_json::json!({"providers":[{"id":"codex","name":"Codex","defaultModel":"bound",
                "models":[
                    {"id":"bound","label":"Bound","reasoningLevels":["low","high"]},
                    {"id":"other","label":"Other","reasoningLevels":["low","medium","high"]}
                ]}]}),
                cx,
            );
            assert_eq!(picker.effective_model_id(cx), Some("bound"));
            picker.active = row_index(picker, "other", cx);
            assert_eq!(picker.detail_row(cx).unwrap().model.id, "other");
            picker.set_detail_effort(HarnessId::Codex, "other", 0, cx);
            let saved = picker.cloud_task_choice().unwrap();
            assert_eq!(saved.model.as_deref(), Some("other"));
            assert_eq!(saved.reasoning, Some(ReasoningLevel::Low));
            assert_eq!(picker.effective_model_id(cx), Some("other"));
            assert_eq!(picker.resolved(cx).model.as_deref(), Some("other"));
        });
    }

    #[gpui::test]
    fn a_deferred_hover_never_lands_on_a_model_that_moved_into_its_row(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_dir, handle) = picker_window(cx);
        let start = handle
            .update(cx, |picker, window, cx| {
                picker.open_model_menu(window, cx);
                picker.model_menu.card_left = false;
                picker.model_menu.card_bounds = Some(gpui::Bounds::new(
                    gpui::point(px(400.), px(0.)),
                    gpui::size(px(272.), px(300.)),
                ));
                let opus = row_index(picker, "opus", cx);
                let gpt = row_index(picker, "gpt-a", cx);
                picker.hover_model_row(opus, gpui::point(px(100.), px(10.)), true, cx);
                picker.hover_model_row(gpt, gpui::point(px(160.), px(30.)), true, cx);
                assert_eq!(picker.model_menu.hover.pending(), Some(&gpt));
                // A refresh moves another model into the pending row.
                picker.models.insert(
                    HarnessId::Codex,
                    Loadable::Ready(vec![
                        bare_model("gpt-new", "GPT New"),
                        laddered("gpt-a", "GPT A", false),
                        bare_model("gpt-plain", "GPT Plain"),
                    ]),
                );
                picker.catalog_rev += 1;
                picker.active
            })
            .unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(400));
        cx.run_until_parked();
        handle
            .update(cx, |picker, _, cx| {
                assert_eq!(picker.active, start, "the card did not jump");
                assert_ne!(picker.detail_row(cx).unwrap().model.id, "gpt-new");

                // A catalog landing cancels any pending hover outright.
                let gpt = row_index(picker, "gpt-a", cx);
                let current = picker.active;
                picker.model_menu.card_left = false;
                picker.model_menu.card_bounds = Some(gpui::Bounds::new(
                    gpui::point(px(400.), px(0.)),
                    gpui::size(px(272.), px(300.)),
                ));
                picker.hover_model_row(current, gpui::point(px(100.), px(10.)), true, cx);
                picker.hover_model_row(gpt, gpui::point(px(160.), px(30.)), true, cx);
                assert_eq!(picker.model_menu.hover.pending(), Some(&gpt));
                picker.apply_model_catalog(
                    HarnessId::Codex,
                    Loadable::Ready(vec![laddered("gpt-a", "GPT A", false)]),
                    cx,
                );
                assert!(picker.model_menu.hover.pending().is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_vanished_provider_scope_falls_back_to_every_model(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = picker_window(cx);
        handle
            .update(cx, |picker, window, cx| {
                picker.open_model_menu(window, cx);
                picker.scope_model_menu(Some(HarnessId::ClaudeCode), cx);
                assert!(
                    picker
                        .model_rows(cx)
                        .iter()
                        .all(|r| r.harness == HarnessId::ClaudeCode)
                );
                // The refreshed catalog no longer offers the scoped provider.
                picker.harnesses = Loadable::Ready(vec![descriptor(HarnessId::Codex, "Codex")]);
                picker.catalog_rev += 1;
                picker.heal_model_scope(cx);
                assert_eq!(picker.model_scope, None);
                assert_eq!(picker.model_rail, ModelRail::All);
                assert!(picker.model_rows(cx).iter().any(|r| r.model.id == "gpt-a"));
                picker
                    .search
                    .update(cx, |input, cx| input.set_text("plain", cx));
                assert_eq!(picker.model_rows(cx).len(), 1);
                assert_eq!(picker.model_rows(cx)[0].model.id, "gpt-plain");
            })
            .unwrap();
    }

    #[gpui::test]
    fn only_models_whose_catalog_offers_1m_get_the_context_switch(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = picker_window(cx);
        handle
            .update(cx, |picker, window, cx| {
                // A service-tier model (no context option) beside the
                // long-context one.
                let mut tiered = laddered("gpt-tier", "GPT Tier", false);
                tiered.options = serde_json::from_value(serde_json::json!([
                    {"id":"serviceTier","label":"Service Tier","defaultChoice":"default",
                     "choices":[{"id":"default","label":"Standard"},{"id":"fast","label":"Fast"}]}
                ]))
                .unwrap();
                picker
                    .models
                    .insert(HarnessId::Codex, Loadable::Ready(vec![tiered]));
                picker.config.model = Some("gpt-tier".into());
                picker.catalog_rev += 1;
                picker.open_model_menu(window, cx);
                let detail = picker.model_detail(cx).unwrap();
                assert_eq!(detail.row.model.id, "gpt-tier");
                assert!(detail.long_context.is_none());
                assert_eq!(detail.fast, Some(false));

                // The Claude model's switch turns 1M on, and off again.
                picker.active = row_index(picker, "opus", cx);
                picker.set_detail_option(
                    HarnessId::ClaudeCode,
                    "opus",
                    "contextWindow".into(),
                    "1m".into(),
                    false,
                    cx,
                );
                assert_eq!(picker.resolved(cx).model_options["contextWindow"], "1m");
                assert!(picker.model_detail(cx).unwrap().long_context.unwrap().on);
            })
            .unwrap();
    }

    #[gpui::test]
    fn same_named_models_are_told_apart_in_the_card_not_the_list(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = picker_window(cx);
        handle
            .update(cx, |picker, _, cx| {
                // An alias without a description beside the full entry.
                let mut full = laddered("claude-opus", "Opus", false);
                full.description = Some("Best for complex work".into());
                picker.models.insert(
                    HarnessId::ClaudeCode,
                    Loadable::Ready(vec![bare_model("opus", "Opus"), full]),
                );
                picker.catalog_rev += 1;
                let rows = picker.model_rows(cx);
                let alias = rows.iter().find(|r| r.model.id == "opus").unwrap();
                let full = rows.iter().find(|r| r.model.id == "claude-opus").unwrap();
                assert!(alias.ambiguous && full.ambiguous);
                assert_eq!(card_note(alias).as_deref(), Some("opus"));
                assert_eq!(card_note(full).as_deref(), Some("Best for complex work"));
                let plain = rows.iter().find(|r| r.model.id == "gpt-plain").unwrap();
                assert_eq!(card_note(plain), None);
            })
            .unwrap();
    }

    #[test]
    fn long_context_needs_a_window_of_a_million_tokens() {
        let model = |choices: serde_json::Value| -> Model {
            let mut model = bare_model("m", "M");
            model.options = serde_json::from_value(serde_json::json!([
                {"id":"contextWindow","label":"Context Window","defaultChoice":"200k",
                 "choices": choices}
            ]))
            .unwrap();
            model
        };
        let empty = serde_json::Map::new();
        let one_m = model(serde_json::json!([
            {"id":"200k","label":"200K"},{"id":"1m","label":"1M"}
        ]));
        let long = long_context(&one_m, &empty).unwrap();
        assert_eq!(
            (long.on_value.as_str(), long.off_value.as_str()),
            ("1m", "200k")
        );
        assert!(!long.on && long.off_default);
        let small = model(serde_json::json!([
            {"id":"128k","label":"128K"},{"id":"200k","label":"200K"}
        ]));
        assert!(long_context(&small, &empty).is_none());
        assert!(long_context(&bare_model("plain", "Plain"), &empty).is_none());
    }

    #[test]
    fn named_two_value_options_stay_segments_and_on_off_options_are_switches() {
        let group = |labels: [&str; 2]| SettingGroup {
            id: ModelSetting::Option("x".into()),
            label: "X".into(),
            choices: labels
                .iter()
                .map(|label| SettingChoice {
                    label: (*label).into(),
                    reasoning: None,
                    value: label.to_lowercase(),
                    selected: false,
                    default: false,
                })
                .collect(),
        };
        assert!(!is_on_off(&group(["200K", "1M"])));
        assert!(!is_on_off(&group(["Standard", "1M tokens"])));
        assert!(is_on_off(&group(["Off", "On"])));
    }
}
