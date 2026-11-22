//! The model menu: a provider rail, a searchable list (favorites first, then
//! one group per provider) and a detail card for the hovered model.
use super::*;
use crate::tooltip::TooltipExt as _;
use std::{cell::RefCell, sync::Arc};
use zuse_viewport::model_picker_style as ui;

/// The list card's width (rail included) and tallest height.
pub(super) const MENU_WIDTH: f32 = 328.;
pub(super) const MENU_HEIGHT: f32 = 380.;
const RAIL_WIDTH: f32 = 44.;
const ROW_HEIGHT: f32 = compact::COMPACT_ROW_HEIGHT;
const HEADER_HEIGHT: f32 = 30.;

fn picker_brand(harness: HarnessId, fallback: gpui::Hsla) -> (&'static str, gpui::Hsla) {
    let (path, tint) = harness_brand_icon(harness);
    (path, tint.unwrap_or(fallback))
}

#[derive(Clone)]
enum Item {
    /// The starred models' section label.
    Favorites,
    /// A provider's group heading. `first` drops the separator above it.
    Header {
        provider: HarnessId,
        name: SharedString,
        first: bool,
    },
    Row {
        row: ModelRowData,
        index: usize,
    },
}
struct Cache {
    key: ModelRowsKey,
    signature: String,
    items: Arc<Vec<Item>>,
}
pub(super) struct MenuState {
    list: gpui::ListState,
    cache: RefCell<Option<Cache>>,
    /// Pointer intent from a row toward the detail card: moving diagonally
    /// across other rows on the way into the card keeps the card's model.
    pub(super) hover: popover::HoverIntent<usize>,
    pub(super) card_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    pub(super) card_left: bool,
    frame_bounds: Option<gpui::Bounds<gpui::Pixels>>,
}
impl Default for MenuState {
    fn default() -> Self {
        Self {
            list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(256.)),
            cache: RefCell::new(None),
            hover: popover::HoverIntent::default(),
            card_bounds: None,
            card_left: false,
            frame_bounds: None,
        }
    }
}

impl Pickers {
    pub(super) fn settings_menu_height(&self, _cx: &App) -> f32 {
        MENU_HEIGHT
    }

    /// A provider's discovery failure belongs to that provider's tab. The
    /// All view keeps listing every healthy provider instead of leading with
    /// one agent's raw error (e.g. an agent that was never signed in).
    fn scoped_model_refresh_error(&self, cx: &App) -> Option<String> {
        let scope = self.model_scope?;
        self.rail_descriptors(cx)
            .iter()
            .any(|p| p.id == scope)
            .then(|| self.model_refresh_errors.get(&scope))
            .flatten()
            .map(|error| super::model_catalog_notice(error))
    }

    fn reference_key(&self, cx: &App) -> ModelRowsKey {
        ModelRowsKey {
            query: self.search.read(cx).text().trim().to_lowercase(),
            rail: self.model_rail,
            scope: self.model_scope,
            effective: self.effective_harness(cx),
            locked: self.harness_locked(cx),
            catalog_rev: self.catalog_rev,
            selected: self.effective_model_id(cx).map(str::to_owned),
        }
    }

    /// The menu's rows in display order: starred models first, then every
    /// other model grouped by provider (the current provider's group first
    /// while browsing). A provider picked on the rail narrows the list to it;
    /// search matches names and ids within the current view.
    pub(super) fn reference_model_rows(&self, cx: &App) -> Vec<ModelRowData> {
        let query = self.search.read(cx).text().trim().to_lowercase();
        let current = self.effective_harness(cx);
        let mut providers = self.rail_descriptors(cx);
        if self.model_scope.is_none() && query.is_empty() {
            providers.sort_by_key(|p| Some(p.id) != current);
        }
        let matches = |id: &str, label: &str| {
            query.is_empty()
                || id.to_lowercase().contains(&query)
                || label.to_lowercase().contains(&query)
        };
        let mut favorites = vec![];
        let mut rows = vec![];
        for provider in providers {
            if self.model_scope.is_some_and(|scope| scope != provider.id) {
                continue;
            }
            let Some(models) = self
                .model_catalog(cx)
                .get(&provider.id)
                .and_then(Loadable::ready)
            else {
                continue;
            };
            let mut push = |row: ModelRowData| {
                if self.defaults.is_favorite(row.harness, &row.model.id) {
                    favorites.push(row);
                } else {
                    rows.push(row);
                }
            };
            for model in models {
                if matches(&model.id, &model.label) {
                    push(ModelRowData {
                        harness: provider.id,
                        harness_name: provider.name.clone().into(),
                        model: model.clone(),
                        selected_only: false,
                        ambiguous: false,
                    });
                }
            }
            if Some(provider.id) == current
                && let Some(id) = self
                    .effective_model_id(cx)
                    .filter(|id| !models.iter().any(|m| m.id == *id))
            {
                let label = self.selected_model_label(cx).unwrap_or_else(|| id.into());
                if matches(id, &label) {
                    push(ModelRowData {
                        harness: provider.id,
                        harness_name: provider.name.clone().into(),
                        model: Model {
                            id: id.into(),
                            label,
                            description: None,
                            reasoning_levels: vec![],
                            options: vec![],
                        },
                        selected_only: true,
                        ambiguous: false,
                    });
                }
            }
        }
        favorites.append(&mut rows);
        let mut rows = favorites;
        // Catalogs can contain repeated entries for the same model ID. Keep
        // separate IDs, but make same-named choices distinguishable below.
        let mut seen = std::collections::HashSet::new();
        rows.retain(|row| seen.insert((row.harness, row.model.id.clone())));
        mark_ambiguous(&mut rows);
        rows
    }

    /// The leading run of starred rows in [`Self::model_rows`].
    pub(super) fn favorite_row_count(&self, cx: &App) -> usize {
        self.model_rows(cx)
            .iter()
            .take_while(|row| self.defaults.is_favorite(row.harness, &row.model.id))
            .count()
    }

    fn reference_items(&self, cx: &App) -> Arc<Vec<Item>> {
        let key = self.reference_key(cx);
        if let Some(cache) = self
            .model_menu
            .cache
            .borrow()
            .as_ref()
            .filter(|c| c.key == key)
        {
            return cache.items.clone();
        }
        let rows = self.model_rows(cx);
        let starred = self.favorite_row_count(cx);
        let signature = serde_json::json!([
            key.query,
            self.model_scope,
            starred,
            rows.iter()
                .map(|r| (&r.harness, &r.model.id, &r.model.label, &r.harness_name))
                .collect::<Vec<_>>()
        ])
        .to_string();
        let mut items = vec![];
        if starred > 0 {
            items.push(Item::Favorites);
        }
        for (index, row) in rows.iter().enumerate() {
            if index >= starred && (index == starred || rows[index - 1].harness != row.harness) {
                items.push(Item::Header {
                    provider: row.harness,
                    name: row.harness_name.clone(),
                    first: items.is_empty(),
                });
            }
            items.push(Item::Row {
                row: row.clone(),
                index,
            });
        }
        let items = Arc::new(items);
        if self
            .model_menu
            .cache
            .borrow()
            .as_ref()
            .is_none_or(|c| c.signature != signature)
        {
            self.model_menu.list.reset(items.len());
        }
        *self.model_menu.cache.borrow_mut() = Some(Cache {
            key,
            signature,
            items: items.clone(),
        });
        items
    }
    pub(super) fn scroll_reference_row(&self, row: usize, cx: &App) {
        let items = self.reference_items(cx);
        if let Some(index) = items
            .iter()
            .position(|i| matches!(i,Item::Row{index,..}if *index==row))
        {
            self.model_menu.list.scroll_to_reveal_item(index);
        }
    }
    pub(super) fn reset_reference_scroll(&self) {
        self.model_menu.list.scroll_to(gpui::ListOffset::default());
    }

    /// Hovering a row shows its card; moving toward an open card across
    /// other rows keeps the card until the pointer settles.
    pub(super) fn hover_model_row(
        &mut self,
        index: usize,
        pointer: gpui::Point<gpui::Pixels>,
        entered: bool,
        cx: &mut Context<Self>,
    ) {
        let current = self.active;
        let bounds = self.model_menu.card_bounds;
        let left = self.model_menu.card_left;
        let action = if entered {
            self.model_menu
                .hover
                .enter(Some(&current), &index, pointer, bounds, left)
        } else {
            self.model_menu
                .hover
                .moved(Some(&current), &index, pointer, bounds, left)
        };
        match action {
            popover::HoverAction::None => {}
            popover::HoverAction::Open => {
                self.model_menu.hover.record_origin(pointer);
                if self.active != index || self.compact_keyboard {
                    self.active = index;
                    self.compact_keyboard = false;
                    cx.notify();
                }
            }
            popover::HoverAction::Defer => {
                // The row's identity, checked again when the grace ends: a
                // catalog refresh can move another model to this index.
                let hovered = self
                    .model_rows(cx)
                    .get(index)
                    .map(|row| (row.harness, row.model.id.clone()));
                self.model_menu.hover.defer(cx, move |this, cx| {
                    let still_there = this
                        .model_rows(cx)
                        .get(index)
                        .map(|row| (row.harness, row.model.id.clone()))
                        == hovered;
                    if this.is_open()
                        && this.active == current
                        && this.model_menu.hover.pending() == Some(&index)
                        && still_there
                    {
                        this.model_menu.hover.record_origin(pointer);
                        this.active = index;
                        this.compact_keyboard = false;
                        cx.notify();
                    }
                });
            }
        }
    }

    fn reference_item(&mut self, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        match item {
            Item::Favorites => div()
                .h(px(HEADER_HEIGHT))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(crate::typography::text_xs())
                .text_color(theme.text_muted)
                .child(
                    crate::icons::icon(crate::icons::STAR)
                        .size(px(13.))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .child("Favorites")
                .into_any_element(),
            Item::Header {
                provider,
                name,
                first,
            } => {
                let (path, tint) = picker_brand(*provider, theme.text_muted);
                div()
                    .w_full()
                    .when(!first, |el| {
                        el.mt(px(4.))
                            .border_t_1()
                            .border_color(theme.border.opacity(0.5))
                    })
                    .h(px(HEADER_HEIGHT))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(crate::typography::text_xs())
                    .text_color(theme.text_muted)
                    .child(
                        div()
                            .opacity(0.8)
                            .child(crate::icons::brand_icon(path, 13., tint)),
                    )
                    .child(div().min_w_0().truncate().child(name.clone()))
                    .into_any_element()
            }
            Item::Row { row, index } => self.reference_row(row, *index, cx),
        }
    }

    fn reference_row(
        &mut self,
        row: &ModelRowData,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let (path, tint) = picker_brand(row.harness, theme.text_muted);
        let selected = self.row_is_selected(row, cx);
        let highlighted = self.active == index;
        let is_default = crate::settings::default_model(cx)
            .is_some_and(|model| model.provider == row.harness && model.model == row.model.id);
        // A second muted line only tells same-named rows apart.
        let attribution: Option<SharedString> = row.ambiguous.then(|| {
            row.model
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .unwrap_or(&row.model.id)
                .to_owned()
                .into()
        });
        let label: SharedString = row.model.label.clone().into();
        let item = div()
            .id(("model-row", index))
            .role(gpui::Role::ListBoxOption)
            .aria_label(SharedString::from(match &attribution {
                Some(attribution) => {
                    format!("{} ({attribution}) · {}", row.model.label, row.harness_name)
                }
                None => format!("{} · {}", row.model.label, row.harness_name),
            }))
            .aria_selected(selected)
            .tab_index(0)
            .relative()
            .h(px(ROW_HEIGHT))
            .px(px(8.))
            .rounded(px(MODEL_ROW_RADIUS))
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .text_size(crate::typography::text_xs())
            .text_color(theme.text)
            .when(selected, |el| model_selected(el, &theme))
            .when(!selected && highlighted, |el| {
                el.bg(theme.muted_fill().opacity(0.6))
            })
            .when(self.compact_keyboard && highlighted, |el| {
                el.aria_active_descendant()
            })
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                if *hovered {
                    this.hover_model_row(index, window.mouse_position(), true, cx);
                } else {
                    this.model_menu.hover.leave(&index);
                }
            }))
            .on_mouse_move(
                cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                    this.hover_model_row(index, event.position, false, cx);
                }),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    this.activate_model_index(index, cx);
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.activate_model_index(index, cx)))
            .child(crate::icons::brand_icon(path, 15., tint))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(px(6.))
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
                                .text_color(theme.text_muted)
                                .child(attribution),
                        )
                    }),
            )
            .when(is_default, |el| {
                el.child(
                    div()
                        .flex_none()
                        .text_color(theme.text_muted)
                        .child("Default"),
                )
            })
            .when(selected, |el| {
                el.child(model_check(&theme))
                    .child(compact::selected_bar(&theme))
            });
        div()
            .pb(px(popover::MENU_GAP))
            .child(item)
            .into_any_element()
    }

    /// A provider picked on the rail that the refreshed catalog no longer
    /// offers would hide every other model (search included): fall back to
    /// listing every model.
    pub(super) fn heal_model_scope(&mut self, cx: &mut Context<Self>) {
        let Some(scope) = self.model_scope else {
            return;
        };
        if !matches!(self.harness_catalog(cx), Loadable::Ready(_)) {
            return;
        }
        if !self.rail_descriptors(cx).iter().any(|p| p.id == scope) {
            self.model_scope = None;
            self.model_rail = ModelRail::All;
            self.model_menu.hover.reset();
            self.active = self.selected_model_index(cx);
            cx.notify();
        }
    }

    /// Narrow the list to one provider, or (`None`) list every model.
    pub(super) fn scope_model_menu(&mut self, scope: Option<HarnessId>, cx: &mut Context<Self>) {
        self.model_scope = scope;
        self.model_rail = if scope.is_none() {
            ModelRail::All
        } else {
            ModelRail::Harness
        };
        if let Some(provider) = scope {
            self.ensure_models(provider, false, cx);
        } else {
            self.prefetch_models(false, cx);
        }
        self.model_menu.hover.reset();
        self.active = self.selected_model_index(cx);
        self.reset_reference_scroll();
        self.refresh_compact_placeholder(cx);
        cx.notify();
    }

    /// A rail click narrows the list to that provider; clicking the
    /// highlighted provider again returns to every model.
    fn toggle_model_scope(&mut self, scope: Option<HarnessId>, cx: &mut Context<Self>) {
        let scope = scope.filter(|scope| self.model_scope != Some(*scope));
        self.scope_model_menu(scope, cx);
    }

    fn rail_tab(
        &self,
        id: impl Into<gpui::ElementId>,
        label: SharedString,
        active: bool,
        glyph: AnyElement,
        scope: Option<HarnessId>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        ui::provider_tab(id, label.clone(), active, false, Some(glyph))
            .tip(label)
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, _, cx| {
                if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    this.toggle_model_scope(scope, cx);
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_model_scope(scope, cx)))
            .into_any_element()
    }

    pub(super) fn render_reference_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.compact_model_list = true;
        self.heal_model_scope(cx);
        let theme = Theme::of(cx).for_popup();
        let items = self.reference_items(cx);
        let empty = items.is_empty();
        let error = if self.cloud_models_active(cx) {
            self.cloud.model_error.clone()
        } else {
            None
        }
        .or_else(|| match self.harness_catalog(cx) {
            Loadable::Error(message) => Some(message.clone()),
            _ => (!self.cloud_models_active(cx))
                .then(|| self.scoped_model_refresh_error(cx))
                .flatten(),
        });
        let loading = matches!(self.harness_catalog(cx), Loadable::Loading | Loadable::Idle)
            || self.rail_descriptors(cx).iter().any(|p| {
                self.model_scope.is_none_or(|scope| scope == p.id)
                    && matches!(
                        self.model_catalog(cx).get(&p.id),
                        None | Some(Loadable::Loading | Loadable::Idle)
                    )
            });
        let providers = self.rail_descriptors(cx);
        self.compact_motion.frame_height = MENU_HEIGHT.min(self.menu_geometry().height);
        // A narrowed list always keeps its way back to every model.
        let rail = (providers.len() > 1 || self.model_scope.is_some()).then(|| {
            let mut rail = div()
                .id("model-provider-rail")
                .overflow_y_scroll()
                .w(px(RAIL_WIDTH))
                .flex_none()
                .p(px(6.))
                .border_r_1()
                .border_color(theme.border.opacity(0.5))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.))
                .child(
                    self.rail_tab(
                        "model-tab-all",
                        "All models".into(),
                        self.model_scope.is_none(),
                        crate::icons::icon(crate::icons::LIST)
                            .size(px(15.))
                            .text_color(theme.text_muted)
                            .into_any_element(),
                        None,
                        cx,
                    ),
                );
            for (index, p) in providers.iter().enumerate() {
                let (path, tint) = picker_brand(p.id, theme.text_muted);
                rail = rail.child(self.rail_tab(
                    ("harness-tab", index),
                    p.name.clone().into(),
                    self.model_scope == Some(p.id),
                    crate::icons::brand_icon(path, 16., tint),
                    Some(p.id),
                    cx,
                ));
            }
            rail
        });
        let list = gpui::list(
            self.model_menu.list.clone(),
            cx.processor(move |this, index, _, cx| this.reference_item(&items[index], cx)),
        )
        .size_full();
        let search = div()
            .flex_none()
            .h(px(ui::SEARCH_HEIGHT))
            .px(px(14.))
            .border_b_1()
            .border_color(theme.border.opacity(0.5))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                crate::icons::icon(crate::icons::MAGNIFER)
                    .size(px(14.))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .child(div().flex_1().min_w_0().child(self.search.clone()));
        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .when_some(error, |body, message| {
                body.child(self.retry_row(
                    "model-menu-retry",
                    &message,
                    PickerKind::HarnessModel,
                    &theme,
                    cx,
                ))
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px(px(ui::LIST_INSET))
                    .pt(px(4.))
                    .pb(px(6.))
                    .when(!empty, |v| v.child(list))
                    .when(empty, |v| {
                        v.child(
                            div()
                                .px(px(12.))
                                .py(px(24.))
                                .text_size(px(12.))
                                .text_color(theme.text_muted)
                                .child(if loading {
                                    crate::localization::text(
                                        "providers-model_picker_checking_available_agents",
                                        cx,
                                    )
                                } else {
                                    "No models match your search.".to_string()
                                }),
                        )
                    }),
            );
        // The card opens on whichever side of the list has room.
        let viewport = window.viewport_size().width;
        if let Some(frame) = self.model_menu.frame_bounds {
            let room_right = f32::from(viewport - frame.right());
            self.model_menu.card_left = room_right < super::model_detail::CARD_WIDTH + 24.;
        }
        let card = self.render_model_detail(cx);
        if card.is_none() {
            self.model_menu.card_bounds = None;
        }
        let card =
            card.map(|card| popover::nested_menu("model-detail", card, self.model_menu.card_left));
        let entity = cx.entity().downgrade();
        let content = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(search)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_stretch()
                    .children(rail)
                    .child(body),
            )
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        let _ = entity.update(cx, |this, cx| {
                            if this.model_menu.frame_bounds != Some(bounds) {
                                this.model_menu.frame_bounds = Some(bounds);
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .children(card)
            .into_any_element();
        self.popover_frame_flush(MENU_WIDTH, content, cx)
    }
}
