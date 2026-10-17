//! Settings uses the same rail, framed groups and card rows as Zuse.
use super::*;
use crate::{
    assets::icon,
    settings_style as s,
    shell::{BG, BORDER, MUTED, TEXT, column, provider_icon, row},
};
use zuse_protocol::presentation::SidebarView;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    General,
    Providers,
    Devices,
    Plugins,
    Usage,
}
impl Section {
    fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Providers => "Providers",
            Self::Devices => "Devices",
            Self::Plugins => "Account & plugins",
            Self::Usage => "Accounts & usage",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Self::General => "settings-minimalistic",
            Self::Providers => "terminal",
            Self::Devices => "globe",
            Self::Plugins => "split-columns",
            Self::Usage => "clock-circle",
        }
    }
    fn description(self) -> &'static str {
        match self {
            Self::General => "Make Zuse feel at home in your workflow.",
            Self::Providers => "Manage your model connections and coding agents.",
            Self::Devices => "Connect trusted devices to this workspace.",
            Self::Plugins => "Your Zuse account and the tools shared across your agents.",
            Self::Usage => "Provider accounts, plans and current usage.",
        }
    }
}
impl WorkspaceView {
    pub(super) fn settings_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let section = self.settings_section;
        let compact = self.content_width < 800.;
        let navigation = column()
            .w(px(if compact { 48. } else { 208. }))
            .h_full()
            .flex_none()
            .bg(rgb(0x151515))
            .border_r_1()
            .border_color(rgb(BORDER))
            .px(px(if compact { 6. } else { 10. }))
            .py_3()
            .gap(px(2.))
            .children(
                [
                    Section::General,
                    Section::Providers,
                    Section::Devices,
                    Section::Plugins,
                    Section::Usage,
                ]
                .into_iter()
                .enumerate()
                .map(|(index, item)| {
                    row()
                        .id(("settings-section", index))
                        .role(Role::Button)
                        .aria_label(item.title())
                        .cursor_pointer()
                        .h(px(32.))
                        .px_2()
                        .gap_2()
                        .rounded(px(6.))
                        .when(item == section, |row| row.bg(rgb(0x292929)))
                        .text_color(rgb(if item == section { TEXT } else { MUTED }))
                        .hover(|row| row.bg(rgb(0x232323)))
                        .child(
                            icon(item.icon())
                                .size(px(15.))
                                .text_color(rgb(if item == section { TEXT } else { MUTED })),
                        )
                        .when(!compact, |row| {
                            row.child(div().text_size(px(12.)).child(item.title()))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.settings_section = item;
                            if item == Section::Devices {
                                this.refresh_pairings();
                            }
                            if item == Section::Usage {
                                this.call(
                                    methods::LIST_AGENT_ACCOUNTS,
                                    json!({"forceUsage":false}),
                                );
                            }
                            cx.notify();
                        }))
                }),
            );
        let content = match section {
            Section::General => self.general_settings(cx),
            Section::Providers => column()
                .gap_5()
                .child(self.model_accounts_settings(cx))
                .child(self.providers_settings(cx))
                .into_any_element(),
            Section::Devices => self.pairing_settings(cx),
            Section::Plugins => self.cloud_settings(cx),
            Section::Usage => self.usage_settings(cx),
        };
        column()
            .flex_1()
            .size_full()
            .min_h_0()
            .bg(rgb(BG))
            .child(
                row()
                    .h(px(36.))
                    .flex_none()
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        s::button("settings-back", "Back to Zuse")
                            .border_0()
                            .bg(gpui::transparent_black())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.settings = false;
                                window.focus(&this.input.focus_handle(cx), cx);
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(s::description_text(if self.paired {
                        "Paired workspace"
                    } else {
                        "Local workspace"
                    })),
            )
            .child(
                row()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(navigation)
                    .child(
                        column()
                            .id("settings-content")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_y_scroll()
                            .p(px(24.))
                            .child(
                                column()
                                    .w_full()
                                    .max_w(px(768.))
                                    .mx_auto()
                                    .gap_5()
                                    .child(
                                        column()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_size(px(20.))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(section.title()),
                                            )
                                            .child(s::description_text(section.description())),
                                    )
                                    .child(content),
                            ),
                    ),
            )
            .into_any_element()
    }
    fn general_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.preferences.sidebar_view;
        let default = self
            .preferences
            .default_provider
            .as_deref()
            .and_then(|id| self.providers.iter().find(|provider| provider["id"] == id));
        let default_title = default
            .and_then(|provider| provider["name"].as_str())
            .unwrap_or("No default agent");
        let default_model = self
            .preferences
            .default_provider
            .as_ref()
            .and_then(|id| self.preferences.default_models.get(id))
            .cloned()
            .unwrap_or_else(|| "Choose a default using the star in the model picker.".into());
        column()
            .gap_5()
            .child(
                s::frame()
                    .child(s::header(
                        "Workspace",
                        "How conversations are organized in your sidebar.",
                    ))
                    .child(
                        s::card().child(
                            s::setting_row(true)
                                .child(s::details(
                                    "Sidebar view",
                                    "Switch between recent threads and projects.",
                                ))
                                .children(
                                    [
                                        (SidebarView::Threads, "Threads"),
                                        (SidebarView::Projects, "Projects"),
                                    ]
                                    .into_iter()
                                    .enumerate()
                                    .map(
                                        |(index, (view, name))| {
                                            s::button(("default-sidebar-view", index), name)
                                                .when(selected == view, |button| {
                                                    button
                                                        .bg(rgb(0x373737))
                                                        .border_color(rgb(0x505050))
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.preferences.sidebar_view = view;
                                                    this.save_preferences();
                                                    cx.notify();
                                                }))
                                        },
                                    ),
                                ),
                        ),
                    ),
            )
            .child(
                s::frame()
                    .child(s::header(
                        "Conversation defaults",
                        "Defaults apply when you start a new conversation.",
                    ))
                    .child(
                        s::card().child(
                            s::setting_row(true)
                                .child(s::details(default_title.to_owned(), default_model))
                                .when(self.preferences.default_provider.is_some(), |row| {
                                    row.child(
                                        s::button("clear-model-default", "Clear default").on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.preferences.default_provider = None;
                                                this.preferences.default_models.clear();
                                                this.save_preferences();
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                }),
                        ),
                    ),
            )
            .into_any_element()
    }
    fn providers_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        s::frame()
            .child(s::header(
                "Coding agents",
                "Choose the agent used for new work. Model accounts are managed above.",
            ))
            .child(s::card().children(self.providers.iter().enumerate().map(
                |(index, provider)| {
                    let installed = provider["installed"] == true;
                    s::setting_row(index == 0)
                        .id(("settings-provider", index))
                        .cursor_pointer()
                        .hover(|row| row.bg(rgb(0x181818)))
                        .child(
                            icon(provider_icon(provider["id"].as_str().unwrap_or_default()))
                                .size(px(18.)),
                        )
                        .child(s::details(
                            provider["name"].as_str().unwrap_or("Agent").to_owned(),
                            if installed {
                                if self.paired {
                                    "Installed on your paired engine"
                                } else {
                                    "Installed on this machine"
                                }
                            } else {
                                "CLI installation required"
                            },
                        ))
                        .when(index == self.provider_index, |row| {
                            row.child(icon("check").size(px(16.)).text_color(rgb(TEXT)))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.choose_provider(index, cx);
                        }))
                },
            )))
            .into_any_element()
    }
    fn usage_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let accounts = self.accounts["accounts"].as_array();
        s::frame()
            .child(
                s::header(
                    "Provider usage",
                    "Usage and plan information reported by your signed-in agents.",
                )
                .child(s::button("refresh-settings-usage", "Refresh").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.call(methods::LIST_AGENT_ACCOUNTS, json!({"forceUsage":true}));
                        cx.notify();
                    }),
                )),
            )
            .child(
                s::card()
                    .when(
                        accounts.is_none_or(|accounts| accounts.is_empty()),
                        |card| {
                            card.child(s::setting_row(true).child(s::details(
                                "No provider accounts yet",
                                "Sign in to an agent to see its plan and usage here.",
                            )))
                        },
                    )
                    .children(accounts.into_iter().flatten().enumerate().map(
                        |(index, account)| {
                            column()
                                .when(index > 0, |row| row.border_t_1().border_color(rgb(BORDER)))
                                .child(
                                    s::setting_row(true)
                                        .child(
                                            icon(provider_icon(
                                                account["harness"].as_str().unwrap_or_default(),
                                            ))
                                            .size(px(18.)),
                                        )
                                        .child(s::details(
                                            account["email"]
                                                .as_str()
                                                .or_else(|| account["displayName"].as_str())
                                                .unwrap_or("CLI account")
                                                .to_owned(),
                                            account["planLabel"]
                                                .as_str()
                                                .unwrap_or_default()
                                                .to_owned(),
                                        )),
                                )
                                .children(
                                    account["usageWindows"]
                                        .as_array()
                                        .into_iter()
                                        .flatten()
                                        .map(|window| {
                                            let fraction = window["usedFraction"]
                                                .as_f64()
                                                .unwrap_or(0.)
                                                .clamp(0., 1.)
                                                as f32;
                                            column()
                                                .px_3()
                                                .pb_3()
                                                .gap_1()
                                                .child(
                                                    row()
                                                        .child(s::description_text(
                                                            window["label"]
                                                                .as_str()
                                                                .unwrap_or("Usage")
                                                                .to_owned(),
                                                        ))
                                                        .child(div().flex_1())
                                                        .child(s::description_text(format!(
                                                            "{:.0}%",
                                                            fraction * 100.
                                                        ))),
                                                )
                                                .child(
                                                    div()
                                                        .h(px(3.))
                                                        .rounded_full()
                                                        .bg(rgb(BORDER))
                                                        .child(
                                                            div()
                                                                .h_full()
                                                                .w(relative(fraction))
                                                                .rounded_full()
                                                                .bg(rgb(0xb4f327)),
                                                        ),
                                                )
                                        }),
                                )
                                .when_some(
                                    account["usageError"].as_str().map(str::to_owned),
                                    |row, error| {
                                        row.child(s::description_text(error).px_3().pb_3())
                                    },
                                )
                        },
                    )),
            )
            .into_any_element()
    }
}
