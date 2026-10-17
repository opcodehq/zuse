//! Settings uses the same rail, framed groups and card rows as Zuse.
use super::*;
use crate::{
    assets::icon,
    settings_style as s,
    shell::{BG, BORDER, MUTED, TEXT, column, provider_icon, row},
};
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    General,
    Providers,
    Defaults,
    Devices,
    Plugins,
    Usage,
}
impl Section {
    fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Providers => "Providers",
            Self::Defaults => "Default models",
            Self::Devices => "Remote access",
            Self::Plugins => "Plugins",
            Self::Usage => "Accounts & usage",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Self::General => "settings-general",
            Self::Providers => "settings-providers",
            Self::Defaults => "settings-defaults",
            Self::Devices => "settings-remote",
            Self::Plugins => "settings-plugins",
            Self::Usage => "clock-circle",
        }
    }
    fn description(self) -> &'static str {
        match self {
            Self::General => "Workspace defaults and your personal app preferences.",
            Self::Providers => {
                "Verify what’s installed, signed in, and which subscription each provider runs on."
            }
            Self::Defaults => "Choose how new chats start.",
            Self::Devices => "Use this computer from your phone, a browser, or another computer.",
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
            .bg(s::sidebar_bg())
            .border_r_1()
            .border_color(s::sidebar_border())
            .px(px(if compact { 6. } else { 10. }))
            .py_3()
            .gap(px(2.))
            .children(
                [
                    Section::General,
                    Section::Providers,
                    Section::Defaults,
                    Section::Plugins,
                    Section::Devices,
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
                        .min_h(px(28.))
                        .px(px(10.))
                        .py(px(4.))
                        .gap_2()
                        .rounded(px(7.))
                        .when(item == section, |row| row.bg(s::sidebar_active()))
                        .text_color(rgb(if item == section { TEXT } else { MUTED }))
                        .hover(|row| row.bg(s::sidebar_active().opacity(0.6)))
                        .child(
                            icon(item.icon())
                                .size(px(16.))
                                .text_color(rgb(if item == section { TEXT } else { MUTED })),
                        )
                        .when(!compact, |row| {
                            row.child(div().text_size(px(12.)).child(item.title()))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.settings_section = item;
                            this.completion_preset_menu = false;
                            this.default_permission_menu = false;
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
            Section::General => column()
                .gap_4()
                .child(self.account_settings(cx))
                .child(self.appearance_settings(cx))
                .child(self.completion_sound_settings(cx))
                .into_any_element(),
            Section::Defaults => self.default_model_settings(cx),
            Section::Providers => column()
                .gap_5()
                .child(self.providers_settings(cx))
                .child(self.model_accounts_settings(cx))
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
                    .child(div().w(px(64.)).flex_none())
                    .child(
                        row()
                            .id("settings-back")
                            .role(Role::Button)
                            .aria_label("Back to app")
                            .cursor_pointer()
                            .rounded(px(4.))
                            .p(px(4.))
                            .gap_1()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .hover(|view| view.bg(s::frame_bg().opacity(0.4)).text_color(rgb(TEXT)))
                            .child(icon("alt-arrow-left").size(px(14.)).text_color(rgb(MUTED)))
                            .child("Back to app")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.settings = false;
                                this.completion_preset_menu = false;
                                this.default_permission_menu = false;
                                window.focus(&this.input.focus_handle(cx), cx);
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1()),
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
                            .p(px(if compact { 16. } else { 24. }))
                            .child(
                                column()
                                    .w_full()
                                    .max_w(px(768.))
                                    .mx_auto()
                                    .gap_5()
                                    .child(
                                        column()
                                            .gap_1()
                                            .border_b_1()
                                            .border_color(rgb(BORDER))
                                            .pb(px(16.))
                                            .child(
                                                div()
                                                    .text_size(px(20.))
                                                    .line_height(px(28.))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(section.title()),
                                            )
                                            .child(
                                                s::description_text(section.description())
                                                    .text_size(px(12.))
                                                    .line_height(px(20.)),
                                            ),
                                    )
                                    .child(content),
                            ),
                    ),
            )
            .into_any_element()
    }
    fn account_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let user = &self.cloud_account["user"];
        let signed_in = user.is_object();
        let name = user["name"]
            .as_str()
            .or_else(|| user["email"].as_str())
            .unwrap_or("Not signed in");
        let action = s::button(
            "general-account-sign-in",
            if signed_in { "Sign out" } else { "Sign in" },
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.call(
                if signed_in {
                    methods::CLOUD_ACCOUNT_SIGN_OUT
                } else {
                    methods::CLOUD_ACCOUNT_SIGN_IN
                },
                json!({}),
            );
            cx.notify();
        }));
        let account = s::setting_row(true)
            .child(s::details(name.to_owned(), if signed_in {user["email"].as_str().unwrap_or("")}else{"You're using Zuse (Beta) locally without an account. Sign in to sync and unlock remote agents."}))
            .child(action);
        s::frame()
            .child(s::header("Account", "Sign in to sync your account across devices and (soon) drive remote agents from your phone."))
            .child(s::card().child(account))
            .when_some(self.cloud_account["error"].as_str().map(str::to_owned), |view,error| view.child(s::description_text(error).px_3().py_2()))
            .when_some(self.cloud_account["login"]["url"].as_str().filter(|url|url.starts_with("https://")).map(str::to_owned), |view,url| {
                view.child(s::button("general-authorize-account","Open sign-in").on_click(move|_,_,cx|cx.open_url(&url)))
            }).into_any_element()
    }
    fn appearance_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        use zuse_protocol::presentation::AppearanceMode;
        let selected = self.preferences.appearance;
        let choices = row()
            .p(px(2.))
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(BORDER).opacity(0.6))
            .bg(s::frame_bg())
            .children(
                AppearanceMode::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, mode)| {
                        row()
                            .id(("general-theme", index))
                            .role(Role::Button)
                            .aria_label(mode.label())
                            .aria_selected(selected == mode)
                            .cursor_pointer()
                            .h(px(28.))
                            .px(px(10.))
                            .rounded(px(7.))
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(if selected == mode { TEXT } else { MUTED }))
                            .when(selected == mode, |view| view.bg(rgb(BG)))
                            .child(mode.label())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.preferences.appearance = mode;
                                this.save_preferences();
                                cx.notify();
                            }))
                    }),
            );
        s::frame()
            .child(s::header(
                "Appearance",
                "Choose the app theme, or follow your system setting.",
            ))
            .child(
                s::card().child(
                    s::setting_row(true)
                        .child(s::details(
                            "Theme",
                            "Choose the app theme, or follow your system setting.",
                        ))
                        .child(choices),
                ),
            )
            .into_any_element()
    }
    fn default_model_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let provider = self.preferences.default_provider.as_deref();
        let model = provider.and_then(|provider| self.preferences.default_models.get(provider));
        let label = model
            .and_then(|id| {
                provider
                    .and_then(|provider| self.picker_catalog.providers.get(provider))
                    .and_then(|catalog| catalog.models.iter().find(|model| model.id == *id))
            })
            .map(|model| model.label.as_str())
            .or_else(|| model.map(String::as_str))
            .unwrap_or("Agent default")
            .to_owned();
        let trigger = s::button("settings-default-model", label)
            .w(px(256.))
            .justify_between()
            .child(icon("alt-arrow-down").size(px(14.)))
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_model_picker(window, cx);
                this.model_picker_default = true;
                this.default_permission_menu = false;
                cx.notify();
            }));
        use zuse_protocol::presentation::PermissionMode;
        let selected = self.preferences.default_permission_mode;
        let permission = column()
            .id("default-permission-control")
            .track_focus(&self.default_permission_focus)
            .on_key_down(cx.listener(Self::on_default_permission_key))
            .relative()
            .child(
                s::button("settings-default-permission", selected.label())
                    .w(px(256.))
                    .justify_between()
                    .role(Role::ComboBox)
                    .tab_index(0)
                    .aria_label("Default permission mode")
                    .aria_expanded(self.default_permission_menu)
                    .child(icon("alt-arrow-down").size(px(14.)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.default_permission_focus, cx);
                        this.default_permission_active = PermissionMode::ALL
                            .iter()
                            .position(|mode| *mode == selected)
                            .unwrap_or(0);
                        this.default_permission_menu = !this.default_permission_menu;
                        this.model_picker = false;
                        cx.notify();
                    })),
            )
            .when(self.default_permission_menu, |view| {
                view.child(
                    deferred(
                        column()
                            .id("default-permission-options")
                            .role(Role::ListBox)
                            .absolute()
                            .top_full()
                            .right_0()
                            .mt_1()
                            .w(px(256.))
                            .p_1()
                            .rounded(px(10.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(s::card_bg())
                            .shadow_lg()
                            .occlude()
                            .children(PermissionMode::ALL.into_iter().enumerate().map(
                                |(index, mode)| {
                                    row()
                                        .id(("default-permission-mode", index))
                                        .role(Role::ListBoxOption)
                                        .aria_selected(selected == mode)
                                        .when(self.default_permission_active == index, |view| {
                                            view.bg(s::frame_bg())
                                        })
                                        .cursor_pointer()
                                        .h(px(28.))
                                        .px_2()
                                        .gap_2()
                                        .rounded(px(7.))
                                        .text_size(px(12.))
                                        .hover(|view| view.bg(s::frame_bg()))
                                        .child(div().flex_1().child(mode.label()))
                                        .when(selected == mode, |view| {
                                            view.child(icon("check").size(px(14.)))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.preferences.default_permission_mode = mode;
                                            this.default_permission_menu = false;
                                            this.save_preferences();
                                            cx.notify();
                                        }))
                                },
                            )),
                    )
                    .priority(1),
                )
            });
        let card = s::card()
            .child(s::setting_row(true).child(s::details("Default model", "Model for new chats.")).child(trigger))
            .child(s::setting_row(false).child(s::details("Default permission mode", "How new chats handle tool calls. Each chat can override this from the composer.")).child(permission));
        column().relative()
            .child(s::frame().child(s::header("Chat defaults", "These choices apply when you start a new chat. You can still change either one from the composer.")).child(card))
            .when(self.model_picker,|view|view.child(self.composer_picker(cx)))
            .into_any_element()
    }
    fn on_default_permission_key(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use zuse_protocol::presentation::PermissionMode;
        match event.keystroke.key.as_str() {
            "escape" => self.default_permission_menu = false,
            "enter" | "space" if !event.is_held => {
                if self.default_permission_menu {
                    self.preferences.default_permission_mode =
                        PermissionMode::ALL[self.default_permission_active];
                    self.save_preferences();
                } else {
                    self.default_permission_active = PermissionMode::ALL
                        .iter()
                        .position(|mode| *mode == self.preferences.default_permission_mode)
                        .unwrap_or(0);
                }
                self.default_permission_menu = !self.default_permission_menu;
            }
            "up" | "down" if self.default_permission_menu => {
                self.default_permission_active = 1 - self.default_permission_active
            }
            "home" if self.default_permission_menu => self.default_permission_active = 0,
            "end" if self.default_permission_menu => {
                self.default_permission_active = PermissionMode::ALL.len() - 1
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
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
                                                                .bg(rgb(0x89ca21)),
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
