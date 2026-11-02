//! Provider filter focus and dismissal stay inside the dashboard's shared menu.
use super::view::{PROVIDERS, column};
use super::*;
use crate::{assets::icon, palette::rgb, settings_style as style, shell::BORDER};

impl UsageDashboard {
    pub(super) fn provider_filter(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = self
            .provider
            .and_then(|id| PROVIDERS.iter().find(|p| p.1 == id).map(|p| p.2))
            .unwrap_or("All providers");
        let painted = self.provider_bounds.clone();
        let mut trigger = self
            .text_button("usage-provider", name, false, Action::ProviderMenu, cx)
            .role(Role::ComboBox)
            .aria_expanded(self.provider_open)
            .relative()
            .h(px(28.))
            .px(px(8.))
            .text_size(px(12.))
            .bg(style::card_bg())
            .border_1()
            .border_color(rgb(BORDER))
            .child(icon("alt-arrow-down").size(px(12.)).ml(px(6.)))
            .child(
                canvas(move |bounds, _, _| painted.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .inset_0()
                    .size_full(),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if matches!(
                    event.keystroke.key.as_str(),
                    "down" | "up" | "arrowdown" | "arrowup"
                ) {
                    if !this.provider_open {
                        this.action(Action::ProviderMenu, window, cx);
                    } else {
                        this.provider_focus = Some(this.provider_cursor);
                        cx.notify();
                    }
                    cx.stop_propagation();
                }
            }));
        if self.provider_open {
            let trigger_bounds = self.provider_bounds.clone();
            let mut menu = column()
                .id("usage-provider-options")
                .role(Role::ListBox)
                .aria_label("Filter sessions by provider")
                .w(px(160.))
                .p(px(4.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(BORDER))
                .bg(style::card_bg())
                .shadow_lg()
                .on_mouse_down_out(
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        if !trigger_bounds.get().contains(&event.position) {
                            this.provider_open = false;
                            this.provider_focus = None;
                            let previous = window.focused(cx);
                            let trigger = this.focus.get("usage-provider").cloned();
                            window.defer(cx, move |window, cx| {
                                if window.focused(cx) == previous {
                                    if let Some(trigger) = trigger {
                                        window.focus(&trigger, cx);
                                    }
                                }
                            });
                            cx.notify();
                        }
                    }),
                )
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    let count = PROVIDERS.len() + 1;
                    let cursor = match event.keystroke.key.as_str() {
                        "down" | "arrowdown" => (this.provider_cursor + 1) % count,
                        "up" | "arrowup" => (this.provider_cursor + count - 1) % count,
                        "home" => 0,
                        "end" => count - 1,
                        _ => return,
                    };
                    this.provider_cursor = cursor;
                    this.provider_focus = Some(cursor);
                    cx.notify();
                    cx.stop_propagation();
                }));
            for i in 0..=PROVIDERS.len() {
                let (id, name, provider) = if i == 0 {
                    ("usage-provider-all".to_owned(), "All providers", None)
                } else {
                    let (_, id, name, _) = PROVIDERS[i - 1];
                    (format!("usage-filter-{id}"), name, Some(id))
                };
                menu = menu.child(
                    self.text_button(
                        id,
                        name,
                        self.provider_cursor == i,
                        Action::Provider(provider),
                        cx,
                    )
                    .role(Role::ListBoxOption)
                    .aria_selected(self.provider_cursor == i)
                    .h(px(28.))
                    .justify_start()
                    .text_size(px(12.)),
                );
            }
            if let Some(i) = self.provider_focus.take() {
                let id = if i == 0 {
                    "usage-provider-all".to_owned()
                } else {
                    format!("usage-filter-{}", PROVIDERS[i - 1].1)
                };
                if let Some(focus) = self.focus.get(&id) {
                    window.focus(focus, cx);
                }
            }
            trigger = trigger.child(deferred(
                div().absolute().top(px(32.)).right_0().child(menu),
            ));
        }
        trigger.into_any_element()
    }
}
