//! The Desktop surface uses the selected cloud chat's computer. A local chat
//! remembers an explicit choice within the signed-in account and organization.
use super::*;
use cloud_conversation::CloudConversation;

#[derive(Default)]
pub(super) struct DesktopPanel {
    pub(super) choices: std::collections::HashMap<String, String>,
    enabled: bool,
    scope: Option<(String, Option<String>)>,
    live: Option<(String, Entity<CloudConversation>, Subscription)>,
}

impl Shell {
    pub(super) fn open_desktop_panel(&mut self, cx: &mut Context<Self>) {
        if !settings::cloud_desktop_enabled(cx) || !self.has_conversation(cx) {
            return;
        }
        let key = self.panel_key(cx);
        push_unique_right_surface(
            self.right_tabs.entry(key).or_default(),
            RightSurface::Desktop,
        );
        self.set_right_active(RightSurface::Desktop, cx);
        self.set_surfaces_open(true, cx);
    }

    pub(super) fn disconnect_desktop(&mut self, cx: &mut Context<Self>) {
        if let Some((_, view, _)) = self.desktop_panel.live.take() {
            view.update(cx, |v, cx| v.disconnect_desktop(cx));
        }
    }

    pub(super) fn sync_desktop_panel(&mut self, visible: bool, cx: &mut Context<Self>) {
        let enabled = settings::cloud_desktop_enabled(cx);
        if !enabled && !self.desktop_panel.enabled {
            return;
        }
        let state = self.state.read(cx);
        let scope = state.cloud_user_id().map(|user| {
            (
                user.to_owned(),
                state.cloud_organization_id().map(str::to_owned),
            )
        });
        self.desktop_panel.enabled = enabled;
        if scope != self.desktop_panel.scope || !enabled {
            self.disconnect_desktop(cx);
            self.desktop_panel.choices.clear();
            for tabs in self.right_tabs.values_mut() {
                tabs.retain(|s| *s != RightSurface::Desktop);
            }
            self.desktop_panel.scope = scope;
        }
        if !visible
            || self
                .desktop_panel
                .live
                .as_ref()
                .is_some_and(|(key, _, _)| *key != self.panel_key(cx))
        {
            self.disconnect_desktop(cx);
        }
    }

    fn attach_desktop(&mut self, view: Entity<CloudConversation>, cx: &mut Context<Self>) {
        let sub = cx.observe(&view, |_, _, cx| cx.notify());
        self.desktop_panel.live = Some((self.panel_key(cx), view, sub));
    }

    pub(super) fn render_desktop_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !settings::cloud_desktop_enabled(cx) || !self.right_pane_shown(cx) {
            return Empty.into_any_element();
        }
        let key = self.panel_key(cx);
        if self.desktop_panel.live.is_none() {
            if let Some(view) = self.cloud_conversation.clone() {
                self.attach_desktop(view, cx);
            } else {
                let mut picker = div()
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child("Choose a cloud computer");
                let rows = self.state.read(cx).cloud_sidebar_workspaces();
                let mut count = 0;
                for row in rows {
                    let Some(target) = self.cloud_row_target(&row, cx) else {
                        continue;
                    };
                    if self.desktop_panel.choices.get(&key) == Some(&target.workspace) {
                        let view = self.new_cloud_conversation_view(
                            target,
                            row,
                            serde_json::json!({}),
                            cx,
                        );
                        self.attach_desktop(view, cx);
                        break;
                    }
                    count += 1;
                    let workspace = target.workspace;
                    let name = row["displayName"]
                        .as_str()
                        .or(row["title"].as_str())
                        .unwrap_or(&workspace)
                        .to_owned();
                    picker = picker.child(
                        div()
                            .id(SharedString::from(workspace.clone()))
                            .role(gpui::Role::Button)
                            .cursor_pointer()
                            .child(name)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let key = this.panel_key(cx);
                                this.desktop_panel.choices.insert(key, workspace.clone());
                                cx.notify();
                            })),
                    );
                }
                if self.desktop_panel.live.is_none() {
                    if count == 0 {
                        picker = picker.child("No cloud computers available").child(
                            div()
                                .id("desktop-settings")
                                .cursor_pointer()
                                .child("Cloud settings")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_settings(SettingsSection::Cloud, cx)
                                })),
                        );
                    }
                    return picker.into_any_element();
                }
            }
        }
        let view = self.desktop_panel.live.as_ref().unwrap().1.clone();
        if let Some(browser) = view.update(cx, |v, cx| v.desktop_view(window, cx)) {
            return browser.into_any_element();
        }
        let (status, action) = view.read(cx).desktop_status();
        let status = status.to_owned();
        div()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(status)
            .when_some(action, |v, action| {
                v.child(
                    div()
                        .id("desktop-connect")
                        .role(gpui::Role::Button)
                        .cursor_pointer()
                        .child(action)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            view.update(cx, |v, cx| v.connect_desktop(cx))
                        })),
                )
            })
            .into_any_element()
    }
}
