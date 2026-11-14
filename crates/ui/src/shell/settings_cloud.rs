//! Settings → Cloud computer and Plan & usage. One Cloud page entity backs
//! every Cloud section; Usage pairs model usage with Cloud compute.
use super::*;
use gpui::{Div, Stateful};
use settings::cloud_billing::{CloudBillingPage, CloudView, Navigate};

impl Shell {
    pub(super) fn ensure_cloud_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<CloudBillingPage> {
        if let Some(page) = &self.cloud_billing_page {
            return page.clone();
        }
        let state = self.state.clone();
        let page = cx.new(|cx| CloudBillingPage::new(state, window, cx));
        self.cloud_page_sub = Some(cx.subscribe(
            &page,
            |this, _, event: &Navigate, cx| match *event {
                Navigate::Cloud(CloudView::Compute) => {
                    this.set_usage_settings_compute(true, cx);
                    this.open_settings(SettingsSection::Usage, cx);
                }
                Navigate::Cloud(view) => {
                    this.open_settings(SettingsSection::from_cloud_view(view), cx)
                }
            },
        ));
        self.cloud_billing_page = Some(page.clone());
        page
    }

    /// Review fixtures open Usage on its Cloud compute tab.
    #[cfg(feature = "cloud-computer-fixture")]
    pub fn fixture_usage_compute(&mut self, cx: &mut Context<Self>) {
        self.set_usage_settings_compute(true, cx);
    }

    fn set_usage_settings_compute(&mut self, compute: bool, cx: &mut Context<Self>) {
        // Local-only builds have no Cloud compute tab to switch to.
        let compute = compute && zuse_viewport::cloud_gate::enabled();
        if self.usage_settings_compute == compute {
            return;
        }
        let here = self.route == Route::Settings(SettingsSection::Usage);
        self.usage_settings_compute = compute;
        if here {
            if compute {
                if let Some(dashboard) = self.usage_dashboard.clone() {
                    dashboard.update(cx, |dashboard, cx| dashboard.hide(cx));
                }
            } else {
                self.usage_pending = true;
            }
        }
        cx.notify();
    }

    fn usage_tab(
        &self,
        id: &'static str,
        label: &'static str,
        selected: bool,
        compute: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .role(gpui::Role::Tab)
            .aria_label(label)
            .aria_selected(selected)
            .tab_index(0)
            .h(px(26.))
            .px(px(10.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .cursor_pointer()
            .text_size(crate::typography::text_xs())
            .text_color(if selected {
                theme.text
            } else {
                theme.text_muted
            })
            .when(selected, |v| {
                v.bg(theme.settings_frame_bg())
                    .border_1()
                    .border_color(theme.border)
            })
            .hover(|v| v.text_color(theme.text))
            .focus_visible(|v| v.border_2().border_color(theme.accent))
            .on_click(
                cx.listener(move |this, _, _, cx| this.set_usage_settings_compute(compute, cx)),
            )
            .child(label)
    }

    /// Usage: model usage from this computer's agents, or Cloud compute.
    pub(super) fn usage_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_settings_surface();
        // Local-only builds show model usage alone: no Cloud compute tab.
        let cloud = zuse_viewport::cloud_gate::enabled();
        let compute = cloud && self.usage_settings_compute;
        let tabs = div()
            .id("usage-settings-tabs")
            .role(gpui::Role::TabList)
            .aria_label("Usage")
            .mt(px(16.))
            .flex()
            .gap(px(4.))
            .child(self.usage_tab("usage-tab-models", "Models", !compute, false, &theme, cx))
            .child(self.usage_tab(
                "usage-tab-compute",
                "Cloud compute",
                compute,
                true,
                &theme,
                cx,
            ));
        let header = settings::widgets::settings_page_column(window)
            .pb(px(12.))
            .child(settings::widgets::section_title(
                &theme,
                "Usage",
                if compute {
                    "What your cloud computers used this cycle, by computer and by chat."
                } else if cloud {
                    "Tokens and cost by model, provider and chat, from the agents you run here and in Cloud."
                } else {
                    "Tokens and cost by model, provider and chat, from the agents you run on this computer."
                },
            ))
            .when(cloud, |header| header.child(tabs));
        let body = if compute {
            let page = self.ensure_cloud_page(window, cx);
            page.update(cx, |page, cx| page.set_view(CloudView::Compute, cx));
            page.into_any_element()
        } else {
            let padding = settings::widgets::page_padding(window);
            div()
                .size_full()
                .max_w(px(768. + 2. * padding))
                .mx_auto()
                .px(px((padding - 24.).max(0.)))
                .child(self.usage_view(window, cx))
                .into_any_element()
        };
        div()
            .id("usage-settings")
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex_none().child(header))
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}
