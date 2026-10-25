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
    Cloud,
    Providers,
    Defaults,
    Devices,
    Plugins,
    Usage,
    Shortcuts,
    Archived,
}
impl Section {
    const NAV: [Self; 8] = [
        Self::General,
        Self::Providers,
        Self::Defaults,
        Self::Plugins,
        Self::Devices,
        Self::Cloud,
        Self::Shortcuts,
        Self::Archived,
    ];
    fn title(self) -> &'static str {
        match self {
            Self::Cloud => "Cloud workspaces",
            Self::General => "General",
            Self::Providers => "Providers",
            Self::Defaults => "Default models",
            Self::Devices => "Remote access",
            Self::Plugins => "Plugins",
            Self::Usage => "Accounts & usage",
            Self::Shortcuts => "Keyboard shortcuts",
            Self::Archived => "Archived chats",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Self::Cloud => "cloud",
            Self::General => "settings-general",
            Self::Providers => "settings-providers",
            Self::Defaults => "settings-defaults",
            Self::Devices => "settings-remote",
            Self::Plugins => "settings-plugins",
            Self::Usage => "clock-circle",
            Self::Shortcuts => "keyboard",
            Self::Archived => "archive-minimalistic",
        }
    }
    fn description(self) -> &'static str {
        match self {
            Self::Cloud => {
                "Connect GitHub and your coding agents, then keep work running when this app is closed."
            }
            Self::General => "Workspace defaults and your personal app preferences.",
            Self::Providers => {
                "Verify what’s installed, signed in, and which subscription each provider runs on."
            }
            Self::Defaults => "Choose how new chats start.",
            Self::Devices => "Use this computer from your phone, a browser, or another computer.",
            Self::Plugins => "Your Zuse account and the tools shared across your agents.",
            Self::Usage => "Provider accounts, plans and current usage.",
            Self::Shortcuts => "Keyboard shortcuts for the browser workspace.",
            Self::Archived => "Read saved conversations or restore them to your workspace.",
        }
    }
}
impl WorkspaceView {
    pub(super) fn select_settings_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self.settings_scroll.set_offset(point(px(0.), px(0.)));
        if section != Section::Archived && self.project_archive.global {self.close_project_archives();}
        self.close_plugins_browse();
        self.plugins_add_menu = false;
        self.settings_section = section;
        self.completion_preset_menu = false;
        self.default_permission_menu = false;
        self.model_picker = false;
        if section == Section::Cloud {
            self.refresh_billing(false);
        }
        if section == Section::General {
            self.refresh_worktree_settings();
            self.refresh_title_settings();
        }
        if section == Section::Devices {
            self.refresh_pairings();
            self.refresh_managed_access();
        }
        if section == Section::Providers { self.refresh_custom_acp(cx); }
        if matches!(section, Section::Providers | Section::Usage) {
            self.call(methods::LIST_AGENT_ACCOUNTS, json!({"forceUsage":false}));
        }
        cx.notify();
    }
    fn on_settings_nav_key(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "up" => {
                self.settings_nav_active =
                    (self.settings_nav_active + Section::NAV.len() - 1) % Section::NAV.len()
            }
            "down" => {
                self.settings_nav_active = (self.settings_nav_active + 1) % Section::NAV.len()
            }
            "home" => self.settings_nav_active = 0,
            "end" => self.settings_nav_active = Section::NAV.len() - 1,
            "enter" | "space" if !event.is_held => {}
            _ => return,
        }
        self.select_settings_section(Section::NAV[self.settings_nav_active], cx);
        cx.stop_propagation();
    }
    fn shortcuts_settings(&self) -> AnyElement {
        let groups = [
            (
                "Navigation",
                vec![
                    ("Search sessions", "Cmd K / Ctrl K"),
                    ("New chat", "Cmd N / Ctrl N"),
                    ("Close settings or a preview", "Esc"),
                ],
            ),
            (
                "Composer",
                vec![
                    ("Send message or save queued edit", "Enter"),
                    ("Insert a new line", "Shift Enter"),
                    ("Cancel a queued-message edit", "Esc"),
                ],
            ),
            (
                "Workspace files",
                vec![("Save the open file", "Cmd S / Ctrl S")],
            ),
            (
                "Menus and settings",
                vec![
                    ("Move between options", "↑ / ↓"),
                    ("First or last option", "Home / End"),
                    ("Choose a focused menu option", "Enter / Space"),
                    ("Dismiss a menu", "Esc"),
                ],
            ),
        ];
        column()
            .gap_4()
            .children(groups.into_iter().map(|(title, items)| {
                s::frame().child(s::header(title, "")).child(
                    s::card().children(items.into_iter().enumerate().map(
                        |(index, (name, shortcut))| {
                            s::setting_row(index == 0)
                                .child(s::title_text(name).flex_1().min_w_0())
                                .child(
                                    row()
                                        .h(px(24.))
                                        .px_2()
                                        .rounded(px(5.))
                                        .border_1()
                                        .border_color(rgb(BORDER))
                                        .bg(s::frame_bg())
                                        .text_size(px(11.))
                                        .child(shortcut),
                                )
                        },
                    )),
                )
            }))
            .into_any_element()
    }
    pub(super) fn settings_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.suspend_usage(cx) {
            window.focus(&self.workspace_focus, cx);
        }
        let section = self.settings_section;
        if section == Section::Archived {
            self.load_cloud_archive();
        }
        let compact = self.content_width < 800.;
        let navigation = column()
            .id("settings-navigation")
            .role(Role::TabList)
            .aria_label("Settings sections")
            .track_focus(&self.settings_nav_focus)
            .tab_index(0)
            .on_key_down(cx.listener(Self::on_settings_nav_key))
            .overflow_y_scroll()
            .w(px(if compact { 48. } else { 208. }))
            .h_full()
            .flex_none()
            .bg(s::sidebar_bg())
            .border_r_1()
            .border_color(s::sidebar_border())
            .px(px(if compact { 6. } else { 10. }))
            .py_3()
            .gap(px(2.))
            .children(Section::NAV.into_iter().enumerate().map(|(index, item)| {
                row()
                    .id(("settings-section", index))
                    .role(Role::Tab)
                    .aria_selected(
                        item == section
                            || (item == Section::Providers && section == Section::Usage),
                    )
                    .aria_label(item.title())
                    .cursor_pointer()
                    .min_h(px(28.))
                    .px(px(if compact { 6. } else { 10. }))
                    .when(compact, |row| row.justify_center())
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
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.settings_nav_active = index;
                        window.focus(&this.settings_nav_focus, cx);
                        this.select_settings_section(item, cx);
                    }))
            }));
        let content = match section {
            Section::General => column()
                .gap_4()
                .child(self.account_settings(cx))
                .child(self.appearance_settings(cx))
                .child(self.completion_sound_settings(cx))
                .child(column().gap_4()
                    .child(s::disclosure("general-more-preferences", "More preferences", "Motion, thread naming and worktree storage.", self.general_more)
                        .on_click(cx.listener(|this, _, _, cx| {this.general_more=!this.general_more;cx.notify();})))
                    .when(self.general_more, |view| view
                        .child(self.motion_settings(cx))
                        .child(self.worktree_settings_card(cx))
                        .child(self.thread_naming_settings_card(cx))))
                .into_any_element(),
            Section::Defaults => self.default_model_settings(cx),
            Section::Providers => self.providers_settings(cx),
            Section::Cloud => self.billing_settings(cx),
            Section::Devices => self.pairing_settings(cx),
            Section::Plugins => self.cloud_settings(cx),
            Section::Usage => self.usage_settings(cx),
            Section::Shortcuts => self.shortcuts_settings(),
            Section::Archived => self.archive_list(window, cx),
        };
        let content_panel =
            if section == Section::Archived && self.project_archive.global {
                self.project_archives_view(window,cx)
            } else if section == Section::Archived && self.cloud_archive.state.head.is_some() {
                self.cloud_archive_history(window, cx)
            } else {
                column()
                    .id("settings-content")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.settings_scroll)
                    .p(px(if compact { 16. } else { 24. }))
                    .child(
                        column()
                            .w_full()
                            .max_w(px(if section == Section::Shortcuts {
                                1152.
                            } else {
                                768.
                            }))
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
                                            .child(if section == Section::Cloud {
                                                "Cloud workspaces (Beta)"
                                            } else {
                                                section.title()
                                            }),
                                    )
                                    .child(
                                        s::description_text(section.description())
                                            .text_size(px(12.))
                                            .line_height(px(20.)),
                                    ),
                            )
                            .child(content),
                    )
                    .into_any_element()
            };
        column()
            .id("workspace-settings")
            .relative()
            .tab_group()
            .tab_stop(false)
            .on_key_down(|event: &KeyDownEvent, window, cx| {
                let modifiers = event.keystroke.modifiers;
                if event.keystroke.key == "tab"
                    && !modifiers.control
                    && !modifiers.alt
                    && !modifiers.platform
                {
                    if modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                }
            })
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
                            .tab_index(0)
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
                                if this.project_archive.global {this.close_project_archives();}
                                this.settings = false;
                                this.completion_preset_menu = false;
                                this.default_permission_menu = false;
                                this.model_picker = false;
                                this.picker_target = model_picker::PickerTarget::Composer;
                                window.focus(&this.input.focus_handle(cx), cx);
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        row()
                            .gap_2()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(
                                icon(if self.paired || self.account_remote {
                                    "globe"
                                } else {
                                    "monitor"
                                })
                                .size(px(14.)),
                            )
                            .child(if self.account_remote {
                                "Account computer"
                            } else if self.paired {
                                "Paired computer"
                            } else {
                                "Local workspace"
                            }),
                    ),
            )
            .child(
                row()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(navigation)
                    .child(content_panel),
            )
            .when_some((section == Section::Archived).then(|| self.archive_error.clone()).flatten(), |view, error| {
                view.child(row().id("archive-error-notice").role(Role::Alert)
                    .absolute().bottom(px(16.)).right(px(16.)).w(px(360.))
                    .items_start().gap(px(8.)).p(px(12.)).rounded(px(10.))
                    .border_1().border_color(s::destructive_text().opacity(0.35))
                    .bg(s::card_bg()).shadow_lg().occlude()
                    .child(div().flex_1().min_w_0().text_size(px(12.)).line_height(px(16.))
                        .text_color(s::destructive_text()).child(error))
                    .child(s::button("archive-error-dismiss", "").size(px(24.)).px_0()
                        .tab_index(0).aria_label("Dismiss archive error")
                        .child(icon("close").size(px(12.)))
                        .on_click(cx.listener(|this, _, _, cx| {this.archive_error = None; cx.notify();}))))
            })
            .into_any_element()
    }
    fn account_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut status = self.cloud_account.clone();
        if let Some(error) = &self.account_action_error {
            if !status.is_object() {
                status = json!({});
            }
            status["error"] = json!(error);
        }
        let properties = crate::account_settings::Properties {
            status,
            display_name: self.preferences.display_name.clone(),
            loaded: !self.cloud_account.is_null() || self.account_action_error.is_some(),
            available: self.connected,
            busy: self.pending.values().any(|method| {
                matches!(
                    method.as_str(),
                    methods::CLOUD_ACCOUNT_SIGN_IN | methods::CLOUD_ACCOUNT_SIGN_OUT
                )
            }),
            colors: crate::account_settings::Colors {
                text: rgb(TEXT).into(),
                muted: rgb(MUTED).into(),
                background: s::card_bg(),
                border: s::card_border(),
                danger: s::destructive_text(),
            },
        };
        self.account_card
            .update(cx, |card, cx| card.set_properties(properties, cx));
        self.account_card.clone().into_any_element()
    }
    fn appearance_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        use zuse_protocol::presentation::AppearanceMode;
        let selected = self.preferences.appearance;
        let choices = row()
            .p(px(2.))
            .rounded(px(8.))
            .border_1()
            .border_color(rgb(BORDER).opacity(0.6))
            .bg(s::frame_bg())
            .children(
                AppearanceMode::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, mode)| {
                        s::choice(("general-theme", index), mode.label(), selected == mode)
                            .when(selected == mode, |view| view.bg(rgb(BG)))
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
                        .child(row().h(px(28.)).flex_none().child(choices)),
                ),
            )
            .into_any_element()
    }
    fn motion_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let motion = row()
            .id("reduce-motion-options")
            .role(Role::RadioGroup)
            .aria_label("Reduce motion")
            .gap_1()
            .children(
                zuse_protocol::presentation::motion::ReduceMotion::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(i, mode)| {
                        s::choice(
                            ("reduce-motion", i),
                            mode.label(),
                            self.preferences.reduce_motion == mode,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.preferences.reduce_motion = mode;
                            this.save_preferences();
                            cx.notify();
                        }))
                    }),
            );
        let paused = self.preferences.pause_animations_in_background;
        s::frame().child(s::header("Motion","Choose how the app animates."))
            .child(s::card()
                .child(s::setting_row(true).child(s::details("Reduce motion","Follow system motion preferences, keep animations still, or allow motion.")).child(motion))
                .child(s::setting_row(false).child(s::details("Pause animations in background","Keep animations still while the app is out of focus."))
                    .child(s::toggle("pause-animations-background",paused,"Pause animations in background").tab_index(0)
                        .on_click(cx.listener(|this,_,_,cx|{this.preferences.pause_animations_in_background=!this.preferences.pause_animations_in_background;this.save_preferences();cx.notify();}))))).into_any_element()
    }
    fn default_model_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        use super::model_picker::TriggerBounds;
        let model_bounds = self.picker_menu.trigger.clone();
        let model_bounds_measure = model_bounds.clone();
        let permission_bounds = TriggerBounds::default();
        let permission_bounds_measure = permission_bounds.clone();
        let provider = self.preferences.default_provider.as_deref();
        let model = provider.and_then(|provider| self.preferences.default_models.get(provider));
        let label = model
            .and_then(|id| {
                provider
                    .and_then(|provider| self.picker_catalog.providers.get(provider))
                    .and_then(|catalog| catalog.models.iter().find(|model| model.id == *id))
            })
            .map(|model| model.label.as_str())
            .or_else(|| model.map(String::as_str).filter(|id| *id != "default"))
            .unwrap_or("Agent default")
            .to_owned();
        let trigger = column()
            .relative()
            .w(px(256.))
            .child(
                canvas(
                    move |bounds, _, _| model_bounds_measure.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                s::button("settings-default-model", "")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .w_full()
                    .px(px(10.))
                    .text_size(px(12.))
                    .justify_between()
                    .role(Role::ComboBox)
                    .aria_label(format!("Default model: {label}"))
                    .aria_expanded(
                        self.model_picker
                            && self.picker_target == model_picker::PickerTarget::Defaults,
                    )
                    .track_focus(&self.default_model_focus)
                    .tab_index(0)
                    .child(
                        row()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .when_some(provider, |view, provider| {
                                view.child(icon(provider_icon(provider)).size(px(14.)))
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(label),
                            ),
                    )
                    .child(icon("alt-arrow-down").size(px(14.)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_settings_model_picker(
                            model_picker::PickerTarget::Defaults,
                            window,
                            cx,
                        );
                        this.default_permission_menu = false;
                        if !this.model_picker {
                            window.focus(&this.default_model_focus, cx);
                        }
                        cx.notify();
                    })),
            )
            .when(
                self.model_picker && self.picker_target == model_picker::PickerTarget::Defaults,
                |view| {
                    view.child(
                        deferred(self.composer_picker(cx, Some(model_bounds.clone()))).priority(1),
                    )
                },
            );
        use zuse_protocol::presentation::PermissionMode;
        let selected = self.preferences.default_permission_mode;
        let permission = column()
            .id("default-permission-control")
            .track_focus(&self.default_permission_focus)
            .on_key_down(cx.listener(Self::on_default_permission_key))
            .relative()
            .child(
                canvas(
                    move |bounds, _, _| permission_bounds_measure.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                s::button("settings-default-permission", selected.label())
                    .w(px(256.))
                    .px(px(10.))
                    .text_size(px(12.))
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
                            .on_mouse_down_out(cx.listener(
                                move |this, event: &MouseDownEvent, window, cx| {
                                    if !permission_bounds
                                        .get()
                                        .is_some_and(|bounds| bounds.contains(&event.position))
                                    {
                                        this.default_permission_menu = false;
                                        window.focus(&this.default_permission_focus, cx);
                                        cx.notify();
                                    }
                                },
                            ))
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
        let description = provider
            .and_then(|id| self.providers.iter().find(|provider| provider["id"] == id))
            .and_then(|provider| provider["name"].as_str())
            .map(|name| format!("Model for new chats · {name}"))
            .unwrap_or_else(|| "Model for new chats.".into());
        let card = s::card()
            .child(s::setting_row(true).child(s::details("Default model", description)).child(trigger))
            .child(s::setting_row(false).child(s::details("Default permission mode", "How new chats handle tool calls. Each chat can override this from the composer.")).child(permission));
        column()
            .child(s::frame().child(s::header("Chat defaults", "These choices apply when you start a new chat. You can still change either one from the composer.")).child(card))
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
                let count = PermissionMode::ALL.len();
                self.default_permission_active = if event.keystroke.key == "up" {
                    (self.default_permission_active + count - 1) % count
                } else {
                    (self.default_permission_active + 1) % count
                };
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
