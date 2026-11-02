//! Installed plugin settings and the separate discovery page.
use super::*;
use crate::{
    assets::icon,
    plugin_catalog as catalog, settings_style as s,
    shell::{BG, BORDER, MUTED, TEXT, column, control, icon_button, row},
};
use zuse_protocol::plugins::PluginDefinition;

impl WorkspaceView {
    pub(super) fn close_plugins_browse(&mut self) {
        self.plugin_setup = None;
        self.plugin_setup_events = None;
        self.plugins_browse = false;
        self.plugins_add_menu = false;
        self.plugin_detail = None;
        self.plugin_detail_menu = Default::default();
        if let Some(right_open) = self.plugins_return_right.take() {
            self.right_open = right_open;
        }
    }
    pub(super) fn open_plugins_browse(&mut self, cx: &mut Context<Self>) {
        self.close_usage(cx);
        if self.plugins_return_right.is_none() {
            self.plugins_return_right = Some(self.right_open);
        }
        self.right_open = false;
        self.settings = false;
        self.plugins_browse = true;
        self.plugins_add_menu = false;
        self.plugin_detail = None;
        self.plugin_detail_menu = Default::default();
        if self.cloud_account["user"].is_object() && self.plugin_request.is_none() {
            self.plugin_action(self.plugin_state.refresh());
        }
        cx.notify();
    }
    fn connect_plugin(
        &mut self,
        definition: PluginDefinition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.connected || self.plugin_request.is_some() {
            return;
        }
        if !self.cloud_account["user"].is_object() {
            self.call(methods::CLOUD_ACCOUNT_SIGN_IN, json!({}));
            cx.notify();
            return;
        }
        if self.plugin_state.snapshot["canManage"] == false {
            return;
        }
        let name = catalog::next_label(&self.plugin_state.snapshot, &definition);
        if definition.api_key_header.is_some() {
            self.plugin_detail_menu
                .connect_focus
                .get_or_insert_with(|| cx.focus_handle());
            self.plugin_detail = Some(definition.id.clone());
            self.open_plugin_setup(
                crate::plugin_setup::SetupTarget::Catalog(definition),
                window,
                cx,
            );
            if let Some(form) = &self.plugin_setup {
                form.update(cx, |form, cx| {
                    form.set_connection_label(name, window, cx);
                    form.set_return_focus(self.plugin_detail_menu.connect_focus.clone());
                });
            }
        } else {
            self.plugin_action(json!({"action":"connect","tenantId":self.plugin_state.snapshot["tenantId"],"pluginId":definition.id,"label":name,"requestId":uuid::Uuid::new_v4().to_string(),"returnTo":if self.browser_local {json!({"kind":"desktop","port":0})}else{json!({"kind":"web"})}}));
        }
        cx.notify();
    }
    fn plugin_detail_action(
        &mut self,
        action: crate::plugin_detail::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::plugin_detail::Action;
        let definition = self.plugin_detail.as_ref().and_then(|id| {
            catalog::catalog(&self.plugin_state.snapshot, "", false)
                .into_iter()
                .find(|p| &p.id == id)
        });
        match action {
            Action::Back=>{self.plugin_setup=None;self.plugin_setup_events=None;self.plugin_detail=None;self.plugin_detail_menu=Default::default();},
            Action::MenuToggle=>self.plugin_detail_menu.toggle(),
            Action::MenuClose=>self.plugin_detail_menu.close(),
            Action::AccountMenu(id)=>self.plugin_detail_menu.toggle_account(id,window,cx),
            Action::AccountClose=>self.plugin_detail_menu.close_account(true,window,cx),
            Action::AccountDismiss => self.plugin_detail_menu.close_account(false,window,cx),
            Action::Connect=>if let Some(definition)=definition {self.connect_plugin(definition,window,cx);},
            Action::Toggle{id,enabled}=>self.plugin_action(json!({"action":"setEnabled","tenantId":self.plugin_state.snapshot["tenantId"],"connectionId":id,"enabled":enabled})),
            Action::Disconnect(id)=>self.plugin_action(json!({"action":"disconnect","tenantId":self.plugin_state.snapshot["tenantId"],"connectionId":id})),
            Action::RemoveServer=>if let Some(definition)=definition {
                self.ask_plugin_removal(zuse_protocol::plugin_state::Removal::Server,&definition.id,cx);
                if self.plugin_state.confirmation.is_some() {
                    let colors=crate::plugin_setup::SetupColors{text:rgb(TEXT).into(),muted:rgb(MUTED).into(),background:rgb(crate::shell::RAISED).into(),border:rgb(BORDER).into(),danger:rgb(0xf87171).into()};
                    let dialog=cx.new(|cx|crate::plugin_removal::ServerRemoval::new(definition.name,colors,window,cx));
                    self.plugin_detail_menu.removal_events=Some(cx.subscribe(&dialog,|this,_,event,cx| {
                        match event {crate::plugin_removal::Event::Cancel=>this.plugin_state.confirmation=None,crate::plugin_removal::Event::Remove=>this.confirm_plugin_removal(cx)}cx.notify();
                    }));
                    self.plugin_detail_menu.removal=Some(dialog);
                }
            },
        }
        cx.notify();
    }
    fn plugin_has_status(&self, cx: &App) -> bool {
        self.plugin_setup
            .as_ref()
            .is_some_and(|form| !form.read(cx).is_inline())
            || (self.cloud_account["user"].is_object()
                && self.plugin_state.snapshot["canManage"] == false)
            || self.plugin_state.notice.is_some()
            || self.cloud_account["error"].is_string()
            || self.cloud_account["login"]["url"].is_string()
            || self.plugin_state.confirmation.is_some()
            || self.plugin_state.active_attempt()
    }
    fn plugin_status(&self, cx: &mut Context<Self>) -> Div {
        let only_overlay = (self
            .plugin_setup
            .as_ref()
            .is_some_and(|form| !form.read(cx).is_inline())
            || self.plugin_state.confirmation.is_some())
            && self.plugin_state.notice.is_none()
            && !self.cloud_account["error"].is_string()
            && !self.cloud_account["login"]["url"].is_string()
            && !self.plugin_state.active_attempt();
        let mut view = column().gap_3().when(only_overlay, |view| view.absolute());
        if let Some(form) = self
            .plugin_setup
            .as_ref()
            .filter(|form| !form.read(cx).is_inline())
        {
            view = view.child(div().absolute().child(form.clone()));
        }
        if let Some(notice) = &self.plugin_state.notice {
            view = view.child(
                div()
                    .id("plugin-notice")
                    .role(Role::Status)
                    .child(s::description_text(notice.clone())),
            );
        }
        if let Some(error) = self.cloud_account["error"].as_str() {
            view = view.child(s::description_text(error.to_owned()));
        }
        if let Some(url) = self.cloud_account["login"]["url"]
            .as_str()
            .filter(|url| url.starts_with("https://"))
            .map(str::to_owned)
        {
            view = view.child(
                row()
                    .gap_2()
                    .child(s::description_text(format!(
                        "Authorization code: {}",
                        self.cloud_account["login"]["userCode"]
                            .as_str()
                            .unwrap_or_default()
                    )))
                    .child(
                        s::button("authorize-account", "Continue in browser")
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    ),
            );
        }
        if self.plugin_state.confirmation.is_some() {
            if let Some(dialog) = &self.plugin_detail_menu.removal {
                dialog.update(cx, |dialog, cx| {
                    dialog.sync(
                        self.plugin_request.is_some(),
                        self.plugin_state.notice.clone(),
                        cx,
                    )
                });
                view = view.child(div().absolute().child(dialog.clone()));
            }
        }
        if self.plugin_state.active_attempt() {
            let id = self.plugin_state.attempt["pluginId"]
                .as_str()
                .unwrap_or_default();
            let name = catalog::catalog(&self.plugin_state.snapshot, "", false)
                .into_iter()
                .find(|p| p.id == id)
                .map(|p| p.name)
                .unwrap_or_else(|| "plugin".into());
            view=view.child(crate::plugin_detail::pending(&name,self.plugin_state.attempt["authorizationUrl"].as_str(),self.plugin_request.is_some(),
                crate::plugin_setup::SetupColors{text:rgb(TEXT).into(),muted:rgb(MUTED).into(),background:s::card_bg(),border:rgb(BORDER).into(),danger:rgb(0xf87171).into()},cx,
                |this,_,cx| {this.plugin_action(json!({"action":"cancel","tenantId":this.plugin_state.snapshot["tenantId"],"attemptId":this.plugin_state.attempt["id"]}));cx.notify();}));
        }

        view
    }
    fn plugin_add(&self, cx: &mut Context<Self>) -> Div {
        column()
            .relative()
            .child(
                s::button("plugins-add", "Add")
                    .child(icon("alt-arrow-down").size(px(12.)))
                    .aria_expanded(self.plugins_add_menu)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| {
                        let just_closed = this.plugins_add_closed.is_some_and(|time| {
                            time.elapsed() < std::time::Duration::from_millis(300)
                        });
                        this.plugins_add_menu = !this.plugins_add_menu && !just_closed;
                        this.plugins_add_closed = None;
                        cx.notify();
                    })),
            )
            .when(self.plugins_add_menu, |view| {
                view.child(
                    deferred(
                        s::card()
                            .absolute()
                            .top_full()
                            .right_0()
                            .mt(px(4.))
                            .w(px(224.))
                            .p_1()
                            .bg(rgb(crate::shell::RAISED))
                            .shadow_lg()
                            .occlude()
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.plugins_add_menu = false;
                                this.plugins_add_closed = Some(web_time::Instant::now());
                                cx.notify();
                            }))
                            .child(
                                control("plugins-add-browse", "Browse plugins")
                                    .h(px(28.))
                                    .justify_start()
                                    .w_full()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.open_plugins_browse(cx)),
                                    ),
                            )
                            .when(self.cloud_account["user"].is_object(), |view| {
                                view.child(
                                    control("add-plugin-server", "Add MCP server")
                                        .h(px(28.))
                                        .justify_start()
                                        .w_full()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.plugins_add_menu = false;
                                            this.open_plugin_setup(
                                                crate::plugin_setup::SetupTarget::Server,
                                                window,
                                                cx,
                                            );
                                        })),
                                )
                            }),
                    )
                    .priority(2),
                )
            })
    }
    pub(super) fn cloud_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self
            .plugin_installed_search
            .read(cx)
            .value(cx)
            .trim()
            .to_lowercase();
        let definitions = catalog::catalog(&self.plugin_state.snapshot, "", false);
        let all = self.plugin_state.snapshot["connections"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let connections: Vec<_> = all
            .iter()
            .filter(|connection| {
                let definition = definitions.iter().find(|p| connection["pluginId"] == p.id);
                format!(
                    "{} {}",
                    connection["label"].as_str().unwrap_or_default(),
                    definition
                        .map(|p| p.description.as_str())
                        .unwrap_or_default()
                )
                .to_lowercase()
                .contains(&query)
            })
            .cloned()
            .collect();
        let signed_in = self.cloud_account["user"].is_object();
        let busy = self.plugin_request.is_some();
        let mut view = column()
            .gap_4()
            .child(
                row()
                    .flex_wrap()
                    .justify_between()
                    .gap_2()
                    .child(
                        row()
                            .h(px(28.))
                            .p(px(2.))
                            .rounded(px(7.))
                            .bg(s::frame_bg().opacity(0.55))
                            .child(
                                control("plugins-settings-tab", format!("Plugins  {}", all.len()))
                                    .h(px(24.))
                                    .rounded(px(5.))
                                    .bg(rgb(BG)),
                            ),
                    )
                    .child(
                        row()
                            .gap(px(6.))
                            .child(
                                row()
                                    .w(px(192.))
                                    .min_w_0()
                                    .h(px(28.))
                                    .px(px(10.))
                                    .gap_2()
                                    .rounded(px(7.))
                                    .bg(s::frame_bg().opacity(0.55))
                                    .child(icon("magnifer").size(px(14.)).text_color(rgb(MUTED)))
                                    .child(self.plugin_installed_search.clone()),
                            )
                            .child(self.plugin_add(cx)),
                    ),
            )
            .when(self.plugin_has_status(cx), |view| {
                view.child(self.plugin_status(cx))
            });
        if connections.is_empty() {
            view = view.child(
                column()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .py(px(48.))
                    .child(
                        s::description_text(if query.is_empty() {
                            "No plugins connected yet"
                        } else {
                            "No connected plugins match your search"
                        })
                        .text_size(px(12.)),
                    )
                    .when(query.is_empty(), |view| {
                        view.child(
                            row()
                                .gap_2()
                                .child(s::button("plugins-browse", "Browse plugins").on_click(
                                    cx.listener(|this, _, _, cx| this.open_plugins_browse(cx)),
                                ))
                                .when(!signed_in, |view| {
                                    view.child(s::button("plugins-sign-in", "Sign in").on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.call(methods::CLOUD_ACCOUNT_SIGN_IN, json!({}));
                                            cx.notify();
                                        }),
                                    ))
                                }),
                        )
                    }),
            );
        } else {
            let mut card = s::card();
            for (index, connection) in connections.iter().enumerate() {
                let definition = definitions.iter().find(|p| connection["pluginId"] == p.id);
                let name = connection["label"]
                    .as_str()
                    .filter(|n| !n.is_empty())
                    .or_else(|| definition.map(|p| p.name.as_str()))
                    .unwrap_or("Plugin")
                    .to_owned();
                let description = definition
                    .map(|p| {
                        if p.custom {
                            p.url.as_deref().unwrap_or(&p.description)
                        } else {
                            &p.description
                        }
                    })
                    .unwrap_or_default()
                    .to_owned();
                let id = connection["id"].as_str().unwrap_or_default().to_owned();
                let remove = id.clone();
                let enabled = connection["enabled"] != false;
                let can_manage = !busy && self.plugin_state.snapshot["canManage"] != false;
                card=card.child(s::setting_row(index==0).items_center().flex_nowrap().gap_3()
                    .child(catalog::logo(definition.map(|p|p.domain.as_str()).unwrap_or_default(),&name,32.,s::frame_bg(),rgb(MUTED).into()))
                    .child(column().flex_1().min_w_0().gap(px(2.)).child(s::title_text(name).text_size(px(13.)).truncate()).child(s::description_text(description).text_size(px(12.)).truncate()))
                    .child(icon_button(("disconnect-plugin",index),"link-broken","Disconnect plugin")
                        .when(!can_manage,|view|view.opacity(0.5)).on_click(cx.listener(move|this,_,_,cx|if can_manage{this.plugin_action(json!({"action":"disconnect","tenantId":this.plugin_state.snapshot["tenantId"],"connectionId":remove}));cx.notify();})))
                    .child(s::toggle(("toggle-plugin",index),enabled,"Enable plugin").tab_index(0)
                        .when(!can_manage,|view|view.opacity(0.5)).on_click(cx.listener(move|this,_,_,cx|if can_manage{this.plugin_action(json!({"action":"setEnabled","tenantId":this.plugin_state.snapshot["tenantId"],"connectionId":id,"enabled":!enabled}));cx.notify();}))));
            }
            view = view.child(s::frame().child(s::header("Connected", "")).child(card));
        }
        view.into_any_element()
    }
    fn catalog_rows(
        &self,
        title: &'static str,
        key: &'static str,
        items: Vec<PluginDefinition>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = items
            .into_iter()
            .enumerate()
            .map(|(index, definition)| {
                let id = definition.id.clone();
                let connect = definition.clone();
                let connected = catalog::connected(&self.plugin_state.snapshot, &id);
                row()
                    .id((key, index))
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .p_2()
                    .rounded(px(10.))
                    .hover(|view| view.bg(s::frame_bg().opacity(0.45)))
                    .child(
                        row()
                            .id((SharedString::from(format!("{key}-open")), index))
                            .role(Role::Button)
                            .aria_label(format!("Open {} details", definition.name))
                            .tab_index(0)
                            .cursor_pointer()
                            .flex_1()
                            .min_w_0()
                            .gap_3()
                            .child(catalog::logo(
                                &definition.domain,
                                &definition.name,
                                36.,
                                s::frame_bg(),
                                rgb(MUTED).into(),
                            ))
                            .child(catalog::title(
                                &definition,
                                rgb(TEXT).into(),
                                rgb(MUTED).into(),
                                s::frame_bg(),
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.plugin_detail = Some(id.clone());
                                this.plugin_detail_menu = Default::default();
                                cx.notify();
                            })),
                    )
                    .child(
                        icon_button(
                            (SharedString::from(format!("{key}-connect")), index),
                            if connected { "check" } else { "plus" },
                            if connected {
                                "Connected"
                            } else {
                                "Connect plugin"
                            },
                        )
                        .when(
                            !connected
                                && (self.plugin_request.is_some()
                                    || self.plugin_state.active_attempt()
                                    || self.plugin_state.snapshot["canManage"] == false),
                            |v| v.opacity(0.5).cursor_default(),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if connected {
                                    this.plugin_detail = Some(connect.id.clone());
                                    this.plugin_detail_menu = Default::default();
                                    cx.notify();
                                } else {
                                    this.connect_plugin(connect.clone(), window, cx);
                                }
                            },
                        )),
                    )
            })
            .collect::<Vec<_>>();
        let mut entries = rows.into_iter();
        let mut grid = column().gap(px(2.));
        while let Some(left) = entries.next() {
            if self.viewport_width < 1024. {
                grid = grid.child(left);
            } else {
                grid = grid.child(
                    row()
                        .gap(px(16.))
                        .child(div().flex_1().min_w_0().child(left))
                        .child(div().flex_1().min_w_0().children(entries.next())),
                );
            }
        }
        column()
            .gap(px(4.))
            .child(
                div()
                    .px(px(8.))
                    .pb(px(4.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(title),
            )
            .child(grid)
            .into_any_element()
    }
    pub(super) fn plugins_page_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let listing = catalog::browse(
            &self.plugin_state.snapshot,
            &self.plugin_search.read(cx).value(cx),
            self.plugin_connected_only,
            self.plugin_catalog_page,
        );
        let connected_count = catalog::connected_count(&self.plugin_state.snapshot);
        let query = self.plugin_search.read(cx).value(cx);
        let detail = self.plugin_detail.as_ref().and_then(|id| {
            catalog::catalog(&self.plugin_state.snapshot, "", false)
                .into_iter()
                .find(|p| &p.id == id)
        });
        let mut content = column()
            .flex_none()
            .text_size(px(12.))
            .line_height(px(16.))
            .w_full()
            .max_w(px(768.))
            .mx_auto()
            .px(px(if self.content_width < 800. { 16. } else { 32. }))
            .py(px(32.))
            .gap_5();
        let status = self.plugin_has_status(cx).then(|| self.plugin_status(cx));
        if let Some(definition) = detail {
            content = content.children(status).child(crate::plugin_detail::render(
                &definition,
                &self.plugin_state.snapshot,
                self.plugin_request.is_some() || self.plugin_state.active_attempt(),
                &mut self.plugin_detail_menu,
                self.plugin_setup
                    .clone()
                    .filter(|form| form.read(cx).is_inline()),
                crate::plugin_setup::SetupColors {
                    text: rgb(TEXT).into(),
                    muted: rgb(MUTED).into(),
                    background: s::card_bg(),
                    border: rgb(BORDER).into(),
                    danger: rgb(0xf87171).into(),
                },
                cx,
                Self::plugin_detail_action,
            ));
        } else {
            content=content.child(column().items_start().gap_4()
                .child(column().min_w_0().gap_1().child(div().text_size(px(20.)).line_height(px(28.)).font_weight(FontWeight::MEDIUM).child("Plugins")).child(s::description_text("Connect your tools so agents can work across them in any chat.").text_size(px(12.))))
                .child(row().gap(px(6.)).child(row().w(px(224.)).h(px(28.)).px(px(10.)).gap_2().rounded(px(7.)).bg(s::frame_bg().opacity(0.55)).child(icon("magnifer").size(px(14.)).text_color(rgb(MUTED))).child(self.plugin_search.clone()))
                    .when(self.cloud_account["user"].is_object(),|view|view.child(s::button("browse-add-server","Add MCP server").on_click(cx.listener(|this,_,window,cx|this.open_plugin_setup(crate::plugin_setup::SetupTarget::Server,window,cx)))))
                    .child(icon_button("refresh-plugins","refresh","Refresh plugins").on_click(cx.listener(|this,_,_,cx|{this.plugin_action(this.plugin_state.refresh());cx.notify();})))))
                .child(row().h(px(28.)).p(px(2.)).rounded(px(7.)).bg(s::frame_bg().opacity(0.55)).self_start()
                    .children([(false,"Browse"),(true,"Connected")].map(|(connected,title)|control(if connected{"plugin-tab-connected"}else{"plugin-tab-browse"},title).h(px(24.)).rounded(px(5.))
                        .when(connected && connected_count > 0, |view|view.gap(px(6.)).child(div().text_color(rgb(MUTED)).child(connected_count.to_string())))
                        .when(self.plugin_connected_only==connected,|view|view.bg(rgb(BG)))
                        .on_click(cx.listener(move|this,_,_,cx|{this.plugin_connected_only=connected;this.plugin_catalog_page=0;cx.notify();})))));
            content = content.children(status);
            if !self.cloud_account["user"].is_object() {
                let busy = self.cloud_account.is_null()
                    || self.cloud_account["login"].is_object()
                    || self
                        .pending
                        .values()
                        .any(|method| method == methods::CLOUD_ACCOUNT_SIGN_IN);
                content = content.child(catalog::signed_out(
                    busy,
                    |this, _, cx| {
                        if this.cloud_account["user"].is_object()
                            || this.cloud_account["login"].is_object()
                            || this
                                .pending
                                .values()
                                .any(|method| method == methods::CLOUD_ACCOUNT_SIGN_IN)
                        {
                            return;
                        }
                        this.call(methods::CLOUD_ACCOUNT_SIGN_IN, json!({}));
                        cx.notify();
                    },
                    cx,
                ));
            } else if !self.plugin_state.snapshot["catalog"].is_array() {
                content = content.child(div().py(px(32.)).text_color(rgb(MUTED)).child(
                    if self.plugin_request.is_some() {
                        "Loading plugins…"
                    } else {
                        "Plugins are unavailable. Refresh to try again."
                    },
                ));
            } else if listing.sections.is_empty() {
                content = content.child(
                    div()
                        .py(px(48.))
                        .text_center()
                        .text_color(rgb(MUTED))
                        .child(if query.trim().is_empty() {
                            "No plugins connected yet"
                        } else {
                            "No plugins match your search"
                        }),
                );
            } else {
                for section in listing.sections {
                    content = content.child(self.catalog_rows(
                        section.title,
                        section.key,
                        section.items,
                        cx,
                    ));
                }
                if listing.more {
                    content = content.child(
                        s::button("plugins-show-more", "Show more")
                            .self_start()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.plugin_catalog_page += 1;
                                cx.notify();
                            })),
                    );
                }
            }
        }
        column()
            .size_full()
            .min_h_0()
            .child(
                row().h(px(36.)).flex_none().px_3().child(
                    control("plugins-back-to-settings", "Back to settings")
                        .child(icon("alt-arrow-left").size(px(14.)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_plugins_browse();
                            this.settings = true;
                            this.select_settings_section(
                                crate::settings_view::Section::Plugins,
                                cx,
                            );
                        })),
                ),
            )
            .child(
                column()
                    .id("plugins-page")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
            .into_any_element()
    }
}
