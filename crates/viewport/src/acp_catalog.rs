//! The selected computer supplies registry metadata; both hosts render this picker.
use crate::{
    assets::icon,
    input::{self, TextInput},
    palette::rgb,
    settings_style as s,
    shell::{MUTED, TEXT},
};
use gpui::{prelude::*, *};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use zuse_protocol::{
    HarnessId,
    custom_acp::{
        CustomAcpAgent,
        catalog::{self, Catalog, Offer, RegistryAgent},
    },
};

#[derive(Clone)]
pub enum Event {
    Add(CustomAcpAgent),
    Manual(CustomAcpAgent),
    Custom,
    Dismiss,
}

pub struct CatalogDialog {
    data: Option<Catalog>,
    search: Entity<TextInput>,
    _search: Vec<Subscription>,
    installed: HashSet<HarnessId>,
    icons: HashMap<String, String>,
    tinted: HashMap<String, (bool, Arc<Image>)>,
    busy: bool,
    adding: Option<HarnessId>,
    error: Option<String>,
    root: FocusHandle,
    done: FocusHandle,
    window: AnyWindowHandle,
    return_focus: Option<FocusHandle>,
}
impl EventEmitter<Event> for CatalogDialog {}

pub fn popup_bg() -> Hsla {
    crate::palette::tone(hsla(132. / 360., 0.05, 0.105, 1.), white())
}

pub fn spinner(id: impl Into<ElementId>, size: f32) -> AnyElement {
    icon("refresh")
        .size(px(size))
        .with_animation(
            id,
            Animation::new(std::time::Duration::from_secs(1)).repeat(),
            |svg, progress| svg.with_transformation(Transformation::rotate(percentage(progress))),
        )
        .into_any_element()
}

impl CatalogDialog {
    pub fn new(
        installed: impl IntoIterator<Item = HarnessId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        input::bind_keys(cx);
        let return_focus = window.focused(cx);
        let search = cx.new(|cx| TextInput::new("Search agents…", window, cx));
        let subscriptions = vec![
            cx.subscribe(&search, |_, _, _: &input::Changed, cx| cx.notify()),
            cx.subscribe(&search, |_, _, _: &input::FocusChanged, cx| cx.notify()),
        ];
        let focus = search.read(cx).focus_handle(cx);
        window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        Self {
            data: None,
            search,
            _search: subscriptions,
            installed: installed.into_iter().collect(),
            icons: HashMap::new(),
            tinted: HashMap::new(),
            busy: false,
            adding: None,
            error: None,
            root: cx.focus_handle(),
            done: cx.focus_handle().tab_stop(true),
            window: window.window_handle(),
            return_focus,
        }
    }
    pub fn set_catalog(&mut self, data: Catalog, cx: &mut Context<Self>) {
        self.error = data.error.clone();
        self.data = Some(data);
        cx.notify();
    }
    pub fn reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.return_focus = window.focused(cx);
        self.busy = false;
        self.adding = None;
        let focus = self.search.read(cx).focus_handle(cx);
        window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        cx.notify();
    }
    pub fn set_installed(
        &mut self,
        installed: impl IntoIterator<Item = HarnessId>,
        cx: &mut Context<Self>,
    ) {
        self.installed = installed.into_iter().collect();
        cx.notify();
    }
    pub fn set_icon(&mut self, id: String, svg: Option<String>, cx: &mut Context<Self>) {
        if let Some(svg) = svg {
            self.tinted.remove(&id);
            self.icons.insert(id, svg);
            cx.notify();
        }
    }
    pub fn set_status(&mut self, busy: bool, error: Option<String>, cx: &mut Context<Self>) {
        self.busy = busy;
        if !busy {
            self.adding = None;
        }
        self.error = error;
        cx.notify();
    }
    pub fn return_focus(&self) -> Option<FocusHandle> {
        self.return_focus.clone()
    }
    pub fn restore_focus(&self, cx: &mut Context<Self>) {
        if let Some(focus) = self.return_focus.clone() {
            let window = self.window;
            cx.defer(move |cx| {
                let _ = cx.update_window(window, |_, window, cx| window.focus(&focus, cx));
            });
        }
    }
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.restore_focus(cx);
        cx.emit(Event::Dismiss);
    }
    fn logo(&mut self, agent: &RegistryAgent) -> AnyElement {
        let light = crate::palette::is_light();
        let image = if let Some((theme, image)) = self
            .tinted
            .get(&agent.id)
            .filter(|(theme, _)| *theme == light)
        {
            let _ = theme;
            Some(image.clone())
        } else if let Some(svg) = self.icons.get(&agent.id) {
            let c = Rgba::from(rgb(TEXT));
            let hex = format!(
                "#{:02x}{:02x}{:02x}",
                (c.r * 255.).round() as u8,
                (c.g * 255.).round() as u8,
                (c.b * 255.).round() as u8
            );
            let image = Arc::new(Image::from_bytes(
                ImageFormat::Svg,
                catalog::tint_icon(svg, &hex).into_bytes(),
            ));
            self.tinted.insert(agent.id.clone(), (light, image.clone()));
            Some(image)
        } else {
            None
        };
        let image = image
            .map(|image| img(image).size(px(16.)).into_any_element())
            .unwrap_or_else(|| {
                icon("bot")
                    .size(px(16.))
                    .text_color(rgb(TEXT))
                    .into_any_element()
            });
        div()
            .size(px(28.))
            .flex_none()
            .rounded(px(6.))
            .bg(s::frame_bg().opacity(0.6))
            .flex()
            .items_center()
            .justify_center()
            .child(image)
            .into_any_element()
    }
    fn entry(&mut self, index: usize, agent: RegistryAgent, cx: &mut Context<Self>) -> AnyElement {
        let offer = self
            .data
            .as_ref()
            .and_then(|data| catalog::offer(&agent, &data.platform));
        let action = match offer {
            Some(Offer::BuiltIn(_)) => div()
                .text_size(px(10.))
                .text_color(rgb(MUTED))
                .child("Built in")
                .into_any_element(),
            Some(Offer::Add(ref config) | Offer::Manual(ref config))
                if self.installed.contains(&config.id) =>
            {
                div()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child("Added")
                    .into_any_element()
            }
            Some(offer @ (Offer::Add(_) | Offer::Manual(_))) => {
                let (config, manual) = match offer {
                    Offer::Add(config) => (config, false),
                    Offer::Manual(config) => (config, true),
                    _ => unreachable!(),
                };
                let pending = self.adding == Some(config.id);
                s::button(
                    ("acp-catalog-add", index),
                    if manual { "Set up" } else { "Add" },
                )
                .tab_index(0)
                .px(px(12.))
                .bg(s::frame_bg().opacity(0.5))
                .border_0()
                .aria_label(format!(
                    "{} {}",
                    if manual { "Set up" } else { "Add" },
                    agent.name
                ))
                .when(self.busy, |v| v.opacity(0.64).cursor_default())
                .when(pending, |v| {
                    v.child(spinner(("acp-catalog-adding", index), 12.))
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.busy {
                        if manual {
                            cx.emit(Event::Manual(config.clone()));
                        } else {
                            this.busy = true;
                            this.adding = Some(config.id);
                            cx.emit(Event::Add(config.clone()));
                            cx.notify();
                        }
                    }
                }))
                .into_any_element()
            }
            None => div()
                .text_size(px(10.))
                .text_color(rgb(MUTED))
                .child("Unavailable")
                .into_any_element(),
        };
        let source = agent
            .repository
            .clone()
            .filter(|url| url.starts_with("https://"))
            .unwrap_or_else(|| "https://agentclientprotocol.com/registry".into());
        div()
            .id(("acp-catalog-entry", index))
            .group("acp-catalog-entry")
            .h(px(44.))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(10.))
            .rounded(px(6.))
            .hover(|v| v.bg(s::frame_bg().opacity(0.4)))
            .child(self.logo(&agent))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(s::title_text(agent.name.clone()).truncate())
                    .child(s::description_text(agent.description).truncate()),
            )
            .child(
                s::button(("acp-catalog-source", index), "")
                    .size(px(28.))
                    .px_0()
                    .border_0()
                    .bg(transparent_black())
                    .opacity(0.)
                    .group_hover("acp-catalog-entry", |v| v.opacity(1.))
                    .focus_visible(|v| v.opacity(1.))
                    .tab_index(0)
                    .aria_label(format!("View source for {}", agent.name))
                    .child(icon("arrow-up-right").size(px(14.)))
                    .on_click(move |_, _, cx| cx.open_url(&source)),
            )
            .child(action)
            .into_any_element()
    }
}

impl Render for CatalogDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search.read(cx).value(cx);
        let agents: Vec<_> = self
            .data
            .as_ref()
            .into_iter()
            .flat_map(|data| &data.agents)
            .filter(|agent| catalog::matches(agent, &query))
            .cloned()
            .collect();
        let loading = self.data.is_none() && self.error.is_none();
        let empty = agents.is_empty();
        let entries = agents
            .into_iter()
            .enumerate()
            .map(|(i, agent)| self.entry(i, agent, cx))
            .collect::<Vec<_>>();
        let focused = self.search.read(cx).focus_handle(cx).is_focused(window);
        let footer = div()
            .px(px(16.))
            .py(px(10.))
            .border_t_1()
            .border_color(s::card_border().opacity(0.6))
            .bg(s::frame_bg().opacity(0.72))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.))
            .child(
                s::button("acp-catalog-custom", "Use a custom command…")
                    .tab_index(0)
                    .border_0()
                    .bg(transparent_black())
                    .px(px(10.))
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(Event::Custom);
                    })),
            )
            .child(
                s::button("acp-catalog-done", "Done")
                    .track_focus(&self.done)
                    .tab_index(0)
                    .border_0()
                    .bg(transparent_black())
                    .px(px(10.))
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
            );
        let panel = div()
            .id("acp-catalog-dialog")
            .role(Role::Dialog)
            .aria_label("Add ACP agent")
            .track_focus(&self.root)
            .tab_group()
            .tab_stop(false)
            .w(px(448.).min(window.viewport_size().width - px(32.)))
            .max_h(window.viewport_size().height - px(32.))
            .flex_none()
            .flex()
            .flex_col()
            .rounded(px(12.))
            .when(!crate::palette::is_light(), |v| {
                v.border_1().border_color(white().opacity(0.1))
            })
            .bg(popup_bg())
            .shadow_lg()
            .text_color(rgb(TEXT))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.dismiss(cx);
                    cx.stop_propagation();
                }
                if event.keystroke.key == "enter"
                    && this.search.read(cx).focus_handle(cx).is_focused(window)
                {
                    cx.stop_propagation();
                }
                if event.keystroke.key == "tab"
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt
                    && !event.keystroke.modifiers.platform
                {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    if !this.root.contains_focused(window, cx) {
                        let next = if event.keystroke.modifiers.shift {
                            this.done.clone()
                        } else {
                            this.search.read(cx).focus_handle(cx)
                        };
                        window.focus(&next, cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .px(px(16.))
                    .pt(px(16.))
                    .pb(px(10.))
                    .text_size(px(15.))
                    .line_height(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Add ACP agent"),
            )
            .child(
                div().px(px(16.)).pb(px(8.)).child(
                    div()
                        .h(px(28.))
                        .px(px(9.))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .rounded(px(6.))
                        .border_1()
                        .bg(s::card_bg())
                        .border_color(if focused {
                            crate::palette::tone(
                                rgba(0xd9c5108c).into(),
                                hsla(83. / 360., 0.74, 0.43, 1.),
                            )
                        } else {
                            crate::palette::tone(
                                hsla(136. / 360., 0.05, 0.17, 1.),
                                s::card_border(),
                            )
                        })
                        .child(icon("magnifer").size(px(14.)))
                        .child(self.search.clone()),
                ),
            )
            .child(
                div()
                    .id("acp-catalog-entries")
                    .min_h_0()
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .px(px(8.))
                    .pb(px(16.))
                    .children(entries)
                    .when(empty, |v| {
                        v.child(
                            div()
                                .py(px(if loading {
                                    24.
                                } else if self
                                    .data
                                    .as_ref()
                                    .is_some_and(|data| !data.agents.is_empty())
                                {
                                    12.
                                } else {
                                    36.
                                }))
                                .flex()
                                .justify_center()
                                .child(if loading {
                                    spinner("acp-catalog-loading", 16.)
                                } else {
                                    s::description_text(
                                        if self
                                            .data
                                            .as_ref()
                                            .is_some_and(|data| !data.agents.is_empty())
                                        {
                                            "No matching agents"
                                        } else {
                                            "The registry is unavailable"
                                        },
                                    )
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(TEXT).opacity(0.85))
                                    .into_any_element()
                                }),
                        )
                    }),
            )
            .when_some(self.error.clone(), |v, error| {
                v.child(
                    s::description_text(error)
                        .text_color(s::destructive_text())
                        .px(px(16.))
                        .pb(px(8.)),
                )
            })
            .child(footer);
        let mut bottom = div().flex_1();
        bottom.style().flex_grow = Some(3.);
        deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .position(point(px(0.), px(0.)))
                .child(crate::glass::backdrop(
                    div()
                        .id("acp-catalog-backdrop")
                        .w(window.viewport_size().width)
                        .h(window.viewport_size().height)
                        .p(px(16.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .occlude()
                        .bg(black().opacity(0.32))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.dismiss(cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(div().flex_1())
                        .child(panel)
                        .child(bottom),
                )),
        )
        .priority(3)
    }
}
