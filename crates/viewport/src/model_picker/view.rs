//! Model menu projection. Search, pointer selection and shortcuts share one order.
use super::*;
use crate::model_picker_style as ui;
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};

#[derive(Clone)]
pub(super) struct Entry {
    pub provider: String,
    name: String,
    pub model: Model,
}
#[derive(Clone)]
enum Item {
    Label(String),
    Header {
        provider: String,
        name: String,
        count: usize,
        current: bool,
    },
    Model {
        entry: Entry,
        index: usize,
        provider: bool,
        bottom: f32,
    },
}
struct Cache {
    key: String,
    items: Arc<Vec<Item>>,
    height: f32,
}
pub(crate) struct MenuState {
    pub(crate) trigger: TriggerBounds,
    list: ListState,
    generation: Cell<u64>,
    cache: RefCell<Option<Cache>>,
    controls: RefCell<HashMap<String, FocusHandle>>,
}
impl Default for MenuState {
    fn default() -> Self {
        Self {
            trigger: Default::default(),
            list: ListState::new(0, ListAlignment::Top, px(256.)),
            generation: Cell::new(0),
            cache: RefCell::new(None),
            controls: Default::default(),
        }
    }
}
impl MenuState {
    pub(crate) fn invalidate(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
    }
    pub(crate) fn control_focused(&self, window: &Window) -> bool {
        self.controls
            .borrow()
            .values()
            .any(|f| f.is_focused(window))
    }
    fn focus(&self, key: String, cx: &mut Context<WorkspaceView>) -> FocusHandle {
        self.controls
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }
}

impl WorkspaceView {
    fn picker_providers_visible(&self) -> Vec<&Value> {
        let current = self.picker_current_provider();
        self.picker_providers()
            .iter()
            .filter(|p| {
                (p["installed"] != false && p["enabled"] != false)
                    || (!self.picker_in_settings() && self.selected.is_some() && p["id"] == current)
            })
            .collect()
    }
    pub(super) fn picker_entries(&self, cx: &App) -> Vec<Entry> {
        let query = self.model_search.read(cx).value(cx).trim().to_lowercase();
        let mut providers = self.picker_providers_visible();
        if self.picker_scope.is_none() && query.is_empty() {
            let current = self.picker_current_provider();
            providers.sort_by_key(|p| p["id"] != current);
        }
        providers
            .into_iter()
            .filter(|p| {
                self.picker_scope
                    .as_deref()
                    .is_none_or(|scope| p["id"] == scope)
            })
            .flat_map(|p| {
                let provider = p["id"].as_str().unwrap_or_default();
                let name = p["name"].as_str().unwrap_or(provider);
                self.active_picker_catalog()
                    .providers
                    .get(provider)
                    .into_iter()
                    .flat_map(|c| c.models.iter())
                    .filter(|m| {
                        query.is_empty()
                            || m.id.to_lowercase().contains(&query)
                            || m.label.to_lowercase().contains(&query)
                    })
                    .map(|m| Entry {
                        provider: provider.into(),
                        name: name.into(),
                        model: m.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }
    fn picker_selected_model(&self, provider: &str) -> Option<String> {
        let configured = match self.picker_target {
            PickerTarget::Composer => self.model.clone(),
            PickerTarget::Defaults => self.preferences.default_models.get(provider).cloned(),
            PickerTarget::ThreadNaming => self
                .title_settings
                .settings
                .as_ref()
                .and_then(|s| s.model.clone()),
        };
        let models = &self.active_picker_catalog().providers.get(provider)?.models;
        match configured {
            Some(id) if id != "default" || models.iter().any(|m| m.id == id) => Some(id),
            _ => models.first().map(|m| m.id.clone()),
        }
    }
    pub(crate) fn refresh_picker_placeholder(&self, cx: &mut Context<Self>) {
        let total = self
            .picker_providers_visible()
            .iter()
            .map(|p| {
                self.active_picker_catalog()
                    .providers
                    .get(p["id"].as_str().unwrap_or_default())
                    .map_or(0, |c| c.models.len())
            })
            .sum::<usize>();
        let placeholder = self
            .picker_scope
            .as_ref()
            .map(|scope| {
                format!(
                    "in {}…",
                    self.picker_providers()
                        .iter()
                        .find(|p| p["id"] == scope.as_str())
                        .and_then(|p| p["name"].as_str())
                        .unwrap_or(scope)
                )
            })
            .unwrap_or_else(|| format!("Search {total} models"));
        self.model_search
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
    }
    fn picker_items(&self, cx: &App) -> (Arc<Vec<Item>>, f32) {
        let query = self.model_search.read(cx).value(cx).trim().to_lowercase();
        let current = self.picker_current_provider();
        let providers = self
            .picker_providers_visible()
            .into_iter()
            .map(|p| (p["id"].clone(), p["name"].clone()))
            .collect::<Vec<_>>();
        let key = json!([
            self.picker_menu.generation.get(),
            query,
            self.picker_scope,
            current,
            providers,
            self.picker_in_settings()
        ])
        .to_string();
        if let Some(cache) = self
            .picker_menu
            .cache
            .borrow()
            .as_ref()
            .filter(|c| c.key == key)
        {
            return (cache.items.clone(), cache.height);
        }
        let entries = self.picker_entries(cx);
        let grouped = self.picker_scope.is_none() && query.is_empty();
        let show_provider = self.picker_scope.is_none() && !grouped;
        let mut items = vec![];
        let mut height = 10.;
        if !entries.is_empty() {
            let label = if grouped {
                "MODELS".into()
            } else {
                format!(
                    "{} {}",
                    entries.len(),
                    if self.picker_scope.is_none() {
                        if entries.len() == 1 {
                            "MATCH"
                        } else {
                            "MATCHES"
                        }
                    } else if entries.len() == 1 {
                        "MODEL"
                    } else {
                        "MODELS"
                    }
                )
            };
            items.push(Item::Label(label));
            height += ui::LABEL_HEIGHT;
            for (index, entry) in entries.iter().enumerate() {
                if grouped && (index == 0 || entries[index - 1].provider != entry.provider) {
                    let count = entries[index..]
                        .iter()
                        .take_while(|r| r.provider == entry.provider)
                        .count();
                    items.push(Item::Header {
                        provider: entry.provider.clone(),
                        name: entry.name.clone(),
                        count,
                        current: entry.provider == current,
                    });
                    height += ui::GROUP_HEADER_HEIGHT;
                }
                let last = index + 1 == entries.len()
                    || grouped && entries[index + 1].provider != entry.provider;
                let bottom = if grouped && last {
                    8.
                } else if last {
                    0.
                } else {
                    2.
                };
                height += if show_provider {
                    ui::PROVIDER_ROW_HEIGHT
                } else {
                    ui::ROW_HEIGHT
                };
                height += bottom;
                items.push(Item::Model {
                    entry: entry.clone(),
                    index,
                    provider: show_provider,
                    bottom,
                });
            }
        } else {
            height += 64.;
        }
        let items = Arc::new(items);
        self.picker_menu.list.reset(items.len());
        *self.picker_menu.cache.borrow_mut() = Some(Cache {
            key,
            items: items.clone(),
            height,
        });
        (items, height)
    }
    pub(super) fn scroll_picker_row(&self, choice: &ModelSelection, cx: &App) {
        let (items, _) = self.picker_items(cx);
        if let Some(index)=items.iter().position(|item|matches!(item,Item::Model{entry,..}if entry.provider==choice.provider&&entry.model.id==choice.model)){self.picker_menu.list.scroll_to_reveal_item(index);}
    }
    fn model_menu_item(&self, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        match item {
            Item::Label(label) => ui::section(label.clone()).into_any_element(),
            Item::Header {
                provider,
                name,
                count,
                current,
            } => ui::provider_heading(
                name.clone(),
                icon(provider_icon(provider)).text_color(rgb(TEXT)),
                *count,
                *current,
            )
            .into_any_element(),
            Item::Model {
                entry,
                index,
                provider,
                bottom,
            } => {
                let default = self.preferences.default_provider.as_deref() == Some(&entry.provider)
                    && self.preferences.default_models.get(&entry.provider)
                        == Some(&entry.model.id);
                let selection = ModelSelection {
                    provider: entry.provider.clone(),
                    model: entry.model.id.clone(),
                };
                let click = selection.clone();
                let keyboard = selection.clone();
                let star = ui::default_button(
                    SharedString::from(format!(
                        "model-default-{}-{}",
                        entry.provider, entry.model.id
                    )),
                    default,
                )
                .track_focus(
                    &self
                        .picker_menu
                        .focus(format!("{}:{}", entry.provider, entry.model.id), cx),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.preferences.set_default(click.clone());
                    this.save_preferences();
                    cx.notify();
                }))
                .on_key_down(cx.listener(move |this, e: &KeyDownEvent, _, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        this.preferences.set_default(keyboard.clone());
                        this.save_preferences();
                        cx.notify();
                    }
                }));
                let active = self.picker_active.as_ref() == Some(&selection);
                let selected = self.picker_current_provider() == entry.provider
                    && self.picker_selected_model(&entry.provider).as_deref()
                        == Some(&entry.model.id);
                let choice = selection.clone();
                let keyboard_choice = choice.clone();
                let focus = self
                    .picker_menu
                    .focus(format!("row:{}:{}", entry.provider, entry.model.id), cx);
                let hovered = selection;
                div()
                    .pb(px(*bottom))
                    .child(
                        ui::model_row(
                            SharedString::from(format!(
                                "model-row-{}-{}",
                                entry.provider, entry.model.id
                            )),
                            entry.model.label.clone(),
                            provider.then(|| entry.name.clone().into()),
                            icon(provider_icon(&entry.provider)).text_color(rgb(MUTED)),
                            selected,
                            active,
                            (*index < 9).then_some(index + 1),
                            star,
                        )
                        .track_focus(&focus)
                        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                cx.stop_propagation();
                                let target = this.picker_target;
                                this.choose_model(
                                    keyboard_choice.provider.clone(),
                                    Some(keyboard_choice.model.clone()),
                                    cx,
                                );
                                if !this.model_picker {
                                    this.return_model_picker_focus(target, window, cx);
                                }
                            }
                        }))
                        .on_hover(cx.listener(move |this, hovered_row: &bool, _, cx| {
                            if *hovered_row && this.picker_active.as_ref() != Some(&hovered) {
                                this.picker_active = Some(hovered.clone());
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                let target = this.picker_target;
                                this.choose_model(
                                    choice.provider.clone(),
                                    Some(choice.model.clone()),
                                    cx,
                                );
                                if !this.model_picker {
                                    this.return_model_picker_focus(target, window, cx);
                                }
                            },
                        )),
                    )
                    .into_any_element()
            }
        }
    }
    fn scope_model_picker(&mut self, scope: Option<String>, cx: &mut Context<Self>) {
        self.picker_scope = scope;
        self.picker_active = None;
        self.refresh_picker_placeholder(cx);
        cx.notify();
    }
    pub(crate) fn composer_picker(
        &self,
        cx: &mut Context<Self>,
        trigger: Option<TriggerBounds>,
    ) -> AnyElement {
        let (items, body_height) = self.picker_items(cx);
        let empty = items.is_empty();
        let providers = self.picker_providers_visible();
        let options = !self.picker_in_settings() && self.cloud_model_configuration_available()
            && self
                .selected_model()
                .is_some_and(|m| !m.reasoning_levels.is_empty() || !m.options.is_empty());
        let footer = if options { 88. } else { 0. };
        let rail_height = 12. + providers.len() as f32 * 36. + 32.;
        let height = (ui::SEARCH_HEIGHT + body_height + footer)
            .max(rail_height)
            .min(ui::MAX_HEIGHT)
            .min((self.content_height - 120.).max(180.));
        let list = gpui::list(
            self.picker_menu.list.clone(),
            cx.processor(move |this, index, _, cx| this.model_menu_item(&items[index], cx)),
        )
        .size_full();
        let current = self.picker_current_provider();
        let mut rail = column()
            .w(px(44.))
            .flex_none()
            .items_center()
            .p(px(6.))
            .gap(px(4.))
            .border_r_1()
            .border_color(rgb(BORDER).opacity(0.5))
            .bg(crate::settings_style::frame_bg().opacity(0.2))
            .child(
                ui::provider_tab(
                    "models-all",
                    "All models",
                    self.picker_scope.is_none(),
                    false,
                    None,
                )
                .track_focus(&self.picker_menu.focus("tab:all".into(), cx))
                .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        this.scope_model_picker(None, cx);
                    }
                }))
                .on_click(cx.listener(|this, _, _, cx| this.scope_model_picker(None, cx))),
            );
        for (index, p) in providers.into_iter().enumerate() {
            let provider = p["id"].as_str().unwrap_or_default().to_owned();
            let live = self
                .active_picker_catalog()
                .providers
                .get(&provider)
                .is_some_and(|c| c.loaded && c.error.is_none());
            let name = p["name"].as_str().unwrap_or(&provider).to_owned();
            let keyboard_provider = provider.clone();
            rail = rail.child(
                ui::provider_tab(
                    ("model-provider", index),
                    format!("{name} models"),
                    self.picker_scope.as_deref() == Some(&provider),
                    live,
                    Some(icon(provider_icon(&provider))),
                )
                .track_focus(&self.picker_menu.focus(format!("tab:{provider}"), cx))
                .on_key_down(cx.listener(move |this, e: &KeyDownEvent, _, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        this.scope_model_picker(Some(keyboard_provider.clone()), cx);
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.scope_model_picker(Some(provider.clone()), cx)
                })),
            );
        }
        let mut errors = self
            .active_picker_catalog()
            .providers
            .iter()
            .filter_map(|(id, c)| {
                c.error
                    .as_ref()
                    .filter(|_| self.picker_scope.as_ref().is_none_or(|scope| scope == id))
                    .map(|e| e.clone())
            })
            .collect::<Vec<_>>();
        if !self.picker_in_settings() {
            errors.extend(if self.cloud_models_active() { self.cloud_models.error.clone() } else { self.model_host.error.clone() });
        }
        let loading = (!self.picker_in_settings() && (if self.cloud_models_active() { self.cloud_models_loading() } else { self.model_host.request.is_some() }))
            || self
                .active_picker_catalog()
                .providers
                .iter()
                .any(|(id, catalog)| {
                    self.picker_scope.as_ref().is_none_or(|scope| scope == id)
                        && catalog.request.is_some()
                });
        let bounds = trigger
            .clone()
            .unwrap_or_else(|| self.picker_menu.trigger.clone());
        let anchor = bounds.get();
        let below = anchor.is_some_and(|b| {
            f32::from(b.top()) < height + 14.
                && self.content_height - f32::from(b.bottom()) > height + 14.
        });
        let popup = row()
            .id("composer-picker")
            .w(px((self.content_width - 48.).clamp(272., ui::WIDTH)))
            .h(px(height))
            .items_stretch()
            .overflow_hidden()
            .rounded(px(16.))
            .border_1()
            .border_color(crate::settings_style::card_border())
            .bg(crate::palette::tone(
                hsla(132. / 360., 0.05, 0.105, 1.),
                white(),
            ))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .capture_key_down(cx.listener(Self::on_picker_key))
            .when_some(Some(bounds), |v, trigger| {
                v.on_mouse_down_out(cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    if trigger.get().is_some_and(|b| b.contains(&e.position)) {
                        return;
                    }
                    let target = this.picker_target;
                    this.model_picker = false;
                    this.picker_target = PickerTarget::Composer;
                    let previous = window.focused(cx);
                    let entity = cx.entity().downgrade();
                    window.defer(cx, move |window, cx| {
                        if window.focused(cx) == previous {
                            let _ = entity.update(cx, |this, cx| {
                                this.return_model_picker_focus(target, window, cx)
                            });
                        }
                    });
                    cx.notify();
                }))
            })
            .child(rail)
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(
                        div()
                            .flex_none()
                            .p(px(10.))
                            .border_b_1()
                            .border_color(rgb(BORDER).opacity(0.5))
                            .child(
                                row()
                                    .h(px(32.))
                                    .px(px(10.))
                                    .gap(px(8.))
                                    .rounded(px(7.))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .bg(rgb(crate::shell::BG))
                                    .child(icon("magnifer").size(px(14.)).text_color(rgb(MUTED)))
                                    .child(
                                        div().flex_1().min_w_0().child(self.model_search.clone()),
                                    ),
                            ),
                    )
                    .children(errors.into_iter().map(|error| {
                        row()
                            .px(px(10.))
                            .py(px(6.))
                            .gap(px(6.))
                            .child(label(error).text_size(px(11.)).flex_1())
                            .child(control("retry-models", "Retry").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.load_picker_catalogs(true);
                                    cx.notify();
                                },
                            )))
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .px(px(10.))
                            .pb(px(10.))
                            .when(empty, |v| {
                                v.child(
                                    div()
                                        .px(px(12.))
                                        .py(px(24.))
                                        .text_size(px(12.))
                                        .text_color(rgb(MUTED))
                                        .child(if loading {
                                            "Checking available agents…"
                                        } else if current.is_empty() {
                                            "No authenticated agents available."
                                        } else {
                                            "No models match your search."
                                        }),
                                )
                            })
                            .when(!empty, |v| v.child(list)),
                    )
                    .when(options, |v| v.child(self.model_options_view(cx))),
            )
            .into_any_element();
        deferred(
            anchored()
                .anchor(if below {
                    Anchor::TopRight
                } else {
                    Anchor::BottomRight
                })
                .when_some(anchor, |v, b| {
                    v.position(point(b.right(), if below { b.bottom() } else { b.top() }))
                })
                .offset(point(px(0.), px(if below { 6. } else { -6. })))
                .snap_to_window_with_margin(px(8.))
                .child(popup),
        )
        .priority(1)
        .into_any_element()
    }
}
