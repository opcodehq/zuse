//! Terminal tabs remain attached to their owning chat and engine across navigation.
use super::*;
use crate::shell::{BORDER, RAISED, column, control, icon_button, row};
pub(super) const LOCAL_MIRROR_PREFIX: &str = "cloud-local/";
#[derive(Clone)]
pub(super) struct TerminalTab {
    pub id: String,
    pub host: Option<String>,
    scope: String,
    name: String,
    draft: String,
}
pub(super) struct OpeningTerminal {
    id: u64,
    host: Option<String>,
    scope: String,
    catalog: bool,
}
impl WorkspaceView {
    pub(super) fn terminal_scope(&self) -> String {
        let scope = match &self.selected {
            Some(chat) => format!(
                "{}:{}",
                chat["deviceId"].as_str().unwrap_or_default(),
                chat["id"].as_str().unwrap_or_default()
            ),
            None => format!("canvas:{}", self.cwd()),
        };
        if self.cloud.guest && self.cloud.local_terminals {
            format!("local:{scope}")
        } else {
            scope
        }
    }
    fn terminal_target_host(&self) -> Option<String> {
        if self.cloud.guest && self.cloud.local_terminals {
            Some(format!(
                "{LOCAL_MIRROR_PREFIX}{}",
                self.selected
                    .as_ref()
                    .and_then(|c| c["id"].as_str())
                    .unwrap_or("browser-canvas")
            ))
        } else {
            self.selected
                .as_ref()
                .and_then(|c| c["deviceId"].as_str())
                .map(str::to_owned)
        }
    }
    fn select_terminal_environment(&mut self, local: bool, cx: &mut Context<Self>) {
        if self.cloud.local_terminals == local {
            return;
        }
        self.detach_browser_terminal(cx);
        if let Some(opening) = self.terminal_open.take() {
            self.cancel(opening.id);
            self.pending.remove(&opening.id);
        }
        self.cloud.local_terminals = local;
        if !self.restore_browser_terminal(cx) {
            self.discover_browser_terminals(cx);
        }
        cx.notify();
    }
    pub(super) fn open_browser_terminal(&mut self, cx: &mut Context<Self>) {
        if !self.require_connection(cx) || self.terminal_open.is_some() {
            return;
        }
        let scope = self.terminal_scope();
        let host = self.terminal_target_host();
        let id = self.call(methods::OPEN_TERMINAL, json!({"chatId":self.selected.as_ref().map(|chat| chat["id"].clone()).unwrap_or(json!("browser-canvas")),"cwd":self.cwd(),"cols":80,"rows":28,"targetDeviceId":host}));
        self.terminal_open = Some(OpeningTerminal {
            id,
            host,
            scope,
            catalog: false,
        });
        cx.notify();
    }
    pub(super) fn discover_browser_terminals(&mut self, cx: &mut Context<Self>) {
        if !self.require_connection(cx) || self.terminal_open.is_some() {
            return;
        }
        let scope = self.terminal_scope();
        let host = self.terminal_target_host();
        let id = self.call(methods::LIST_TERMINALS,json!({"chatId":self.selected.as_ref().map(|c|c["id"].clone()).unwrap_or(json!("browser-canvas")),"targetDeviceId":host}));
        self.terminal_open = Some(OpeningTerminal {
            id,
            host,
            scope,
            catalog: true,
        });
        cx.notify();
    }
    pub(super) fn receive_terminal_open(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .terminal_open
            .as_ref()
            .is_some_and(|request| request.id == id)
        {
            return false;
        }
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        let request = self.terminal_open.take().unwrap();
        self.pending.remove(&id);
        if request.catalog {
            match serde_json::from_value::<Vec<zuse_protocol::TerminalSession>>(frame["ok"].clone()) {
                Ok(sessions) => {
                    let empty=sessions.is_empty();
                    for session in sessions {
                        self.attach_browser_terminal(session.id,session.shell,request.scope.clone(),request.host.clone(),cx);
                    }
                    if empty && request.scope==self.terminal_scope() { self.open_browser_terminal(cx); }
                }
                Err(_) => self.error=Some(frame["err"].as_str().unwrap_or("Could not load retained terminals. Reopen the terminal panel to retry.").into()),
            }
            cx.notify();
            return true;
        }
        match serde_json::from_value::<zuse_protocol::TerminalSession>(frame["ok"].clone()) {
            Ok(terminal) => self.attach_browser_terminal(terminal.id, terminal.shell, request.scope, request.host, cx),
            Err(_) => self.error = Some(frame["err"].as_str().unwrap_or("The terminal could not be confirmed. Check the engine before opening another.").into()),
        }
        cx.notify();
        true
    }
    pub(super) fn attach_browser_terminal(
        &mut self,
        id: String,
        name: String,
        scope: String,
        host: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self
            .terminal_tabs
            .iter()
            .any(|tab| tab.id == id && tab.host == host)
        {
            self.terminal_tabs.push(TerminalTab {
                id: id.clone(),
                host: host.clone(),
                scope: scope.clone(),
                name,
                draft: String::new(),
            });
        }
        if scope == self.terminal_scope() {
            self.switch_browser_terminal(&id, host.as_deref(), cx);
        }
    }
    pub(super) fn detach_browser_terminal(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.terminal_tabs.iter_mut().find(|tab| {
            Some(&tab.id) == self.terminal_id.as_ref() && tab.host == self.terminal_host
        }) {
            tab.draft = self.terminal_input.read(cx).value(cx);
        }
        if let Some(id) = self.terminal_watch.take() {
            self.cancel(id);
        }
        self.terminal_id = None;
        self.terminal_host = None;
        self.terminal_write = None;
        self.terminal_last_seq = None;
        self.terminal_text.clear();
        self.terminal = vt100::Parser::new(28, 80, 2000);
    }
    fn switch_browser_terminal(&mut self, id: &str, host: Option<&str>, cx: &mut Context<Self>) {
        let Some(tab) = self
            .terminal_tabs
            .iter()
            .find(|tab| {
                tab.id == id && tab.host.as_deref() == host && tab.scope == self.terminal_scope()
            })
            .cloned()
        else {
            return;
        };
        if !self.connected {
            return;
        }
        self.detach_browser_terminal(cx);
        self.terminal_id = Some(tab.id.clone());
        self.terminal_host = tab.host.clone();
        self.terminal_input
            .update(cx, |input, cx| input.set_value(tab.draft, cx));
        self.terminal_watch = Some(self.call(
            methods::SUBSCRIBE_TERMINAL,
            json!({"terminalId":tab.id,"targetDeviceId":tab.host}),
        ));
        cx.notify();
    }
    pub(super) fn restore_browser_terminal(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(tab) = self
            .terminal_tabs
            .iter()
            .rev()
            .find(|tab| tab.scope == self.terminal_scope())
            .cloned()
        else {
            return false;
        };
        self.switch_browser_terminal(&tab.id, tab.host.as_deref(), cx);
        true
    }
    fn close_browser_terminal(&mut self, id: &str, host: Option<&str>, cx: &mut Context<Self>) {
        if !self.require_connection(cx) {
            return;
        }
        let Some(tab) = self
            .terminal_tabs
            .iter()
            .find(|tab| tab.id == id && tab.host.as_deref() == host)
            .cloned()
        else {
            return;
        };
        if self
            .terminal_closing
            .values()
            .any(|closing| closing.id == tab.id && closing.host == tab.host)
        {
            return;
        }
        let request = self.call(
            methods::CLOSE_TERMINAL,
            json!({"terminalId":id,"targetDeviceId":host}),
        );
        self.terminal_closing.insert(request, tab);
        cx.notify();
    }
    pub(super) fn receive_terminal_close(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.terminal_closing.contains_key(&id) {
            return false;
        }
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        let tab = self.terminal_closing.remove(&id).unwrap();
        self.pending.remove(&id);
        if frame.get("ok").is_some() {
            let active = self.terminal_id.as_deref() == Some(tab.id.as_str())
                && self.terminal_host == tab.host;
            if active {
                self.detach_browser_terminal(cx);
            }
            self.terminal_tabs
                .retain(|other| !(other.id == tab.id && other.host == tab.host));
            if active {
                self.restore_browser_terminal(cx);
            }
        } else {
            self.error = Some(frame["err"].as_str().unwrap_or("Could not confirm the terminal closed. The tab is retained; check the process before retrying.").into());
        }
        cx.notify();
        true
    }
    pub(super) fn terminal_tabs_view(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .child(
                row()
                    .id("terminal-tabs")
                    .overflow_x_scroll()
                    .gap_1()
                    .p_1()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .when(self.cloud.guest, |view| {
                        view.child(
                            control("terminal-cloud-environment", "Cloud")
                                .role(Role::Tab)
                                .aria_selected(!self.cloud.local_terminals)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.select_terminal_environment(false, cx)
                                })),
                        )
                        .child(
                            control("terminal-local-environment", "Local (synced)")
                                .role(Role::Tab)
                                .aria_selected(self.cloud.local_terminals)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.select_terminal_environment(true, cx)
                                })),
                        )
                    })
                    .children(
                        self.terminal_tabs
                            .iter()
                            .filter(|tab| tab.scope == self.terminal_scope())
                            .enumerate()
                            .map(|(index, tab)| {
                                let select = tab.clone();
                                let close = tab.clone();
                                row()
                                    .rounded(px(6.))
                                    .when(
                                        self.terminal_id.as_deref() == Some(tab.id.as_str()),
                                        |view| view.bg(rgb(RAISED)),
                                    )
                                    .child(
                                        control(("terminal-tab", index), tab.name.clone())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.switch_browser_terminal(
                                                    &select.id,
                                                    select.host.as_deref(),
                                                    cx,
                                                )
                                            })),
                                    )
                                    .child(
                                        icon_button(
                                            ("close-terminal-tab", index),
                                            "close",
                                            "Close terminal and stop its process",
                                        )
                                        .size(px(20.))
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.close_browser_terminal(
                                                    &close.id,
                                                    close.host.as_deref(),
                                                    cx,
                                                )
                                            }),
                                        ),
                                    )
                            }),
                    )
                    .child(
                        icon_button("add-terminal-tab", "plus", "New terminal")
                            .on_click(cx.listener(|this, _, _, cx| this.open_browser_terminal(cx))),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn workflow_terminal_navigation_detaches_and_explicit_close_targets_owner(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut received) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window
            .update(cx, |view, _, cx| {
                view.connected = true;
                view.selected = Some(json!({"id":"chat-a","deviceId":"peer-a"}));
                let scope = view.terminal_scope();
                view.attach_browser_terminal(
                    "a".into(),
                    "Build".into(),
                    scope,
                    Some("peer-a".into()),
                    cx,
                );
                view.clear_workspace(cx);
                assert!(view.terminal_id.is_none());
                assert_eq!(view.terminal_tabs.len(), 1);
                assert!(view.restore_browser_terminal(cx));
                assert_eq!(view.terminal_id.as_deref(), Some("a"));
                view.selected = Some(json!({"id":"chat-b","deviceId":"peer-b"}));
                assert!(!view.restore_browser_terminal(cx));
                view.close_browser_terminal("a", Some("peer-a"), cx);
                assert_eq!(view.terminal_tabs.len(), 1);
                let close = *view.terminal_closing.keys().next().unwrap();
                view.receive(json!({"id":close,"ok":null}), cx);
                assert!(view.terminal_tabs.is_empty());
            })
            .unwrap();
        let calls = std::iter::from_fn(|| received.try_recv().ok()).collect::<Vec<_>>();
        let closes = calls
            .iter()
            .filter(|request| request["method"] == methods::CLOSE_TERMINAL)
            .collect::<Vec<_>>();
        assert_eq!(closes.len(), 1);
        assert_eq!(closes[0]["params"]["targetDeviceId"], "peer-a");
    }
}
