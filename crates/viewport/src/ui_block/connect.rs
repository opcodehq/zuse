//! Live plugin cards (`Connect`, `ConnectedPlugins`) and the agent's own MCP
//! server rows (`McpServer`).
//!
//! A spec only names a plugin; whether it is connected is read from the
//! host's [`PluginDirectory`] on every render, so a card made yesterday shows
//! today's state. Buttons emit [`UiBlockEvent::Plugin`]; the host performs
//! the sign-in, so credentials never pass through the agent or the spec.

use super::{UiBlockEvent, UiBlockPalette, UiBlockView, eid, icon};
use gpui::{prelude::*, *};
use serde_json::Value;
use zuse_protocol::plugins::PluginDefinition;
use zuse_ui_block::ServerStatus;

/// What the host knows about the reader's Zuse plugins.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginDirectory {
    /// The plugin snapshot (`catalog`, `connections`); `Null` before the
    /// first list. The bundled catalog still names and brands plugins then.
    pub snapshot: Value,
    /// Signed in to the Zuse account that owns plugins.
    pub signed_in: bool,
    /// The account state is known (signed in or out).
    pub ready: bool,
    /// The plugin whose sign-in page is open, waiting for the user.
    pub connecting: Option<String>,
    /// A plugin request is in flight; buttons wait for it.
    pub busy: bool,
    /// This build or workspace cannot manage plugins.
    pub unavailable: bool,
}

/// What a plugin card asks its host to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginAction {
    /// Start connecting the plugin (OAuth in the browser, or its key form).
    Connect { plugin_id: String },
    /// Connect another account of an already connected plugin.
    AddAccount { plugin_id: String },
    /// Open the plugin's page in Plugins.
    Manage { plugin_id: String },
    /// Open the plugin catalog, searching for `query`.
    Browse { query: String },
    /// Sign in to the Zuse account first.
    SignIn,
}

/// One plugin's live state for a card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardState {
    SignedOut,
    /// The account or catalog has not loaded yet.
    Loading,
    /// Nothing in the catalog matches the requested plugin.
    Unknown,
    NotConnected,
    Connecting,
    /// Connected accounts (enabled and signed in).
    Connected(usize),
    /// Every account needs signing in again, or failed.
    NeedsAttention,
    Unavailable,
}

/// A requested plugin resolved against the directory.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub definition: Option<PluginDefinition>,
    pub state: CardState,
}

impl PluginDirectory {
    /// Look `query` up by id, then name, then domain (case-insensitive),
    /// then by a connection's label, and join it with its connections.
    pub fn resolve(&self, query: &str) -> Resolved {
        let definition = find_definition(&self.snapshot, query);
        let state = match &definition {
            _ if self.unavailable => CardState::Unavailable,
            _ if self.ready && !self.signed_in => CardState::SignedOut,
            None if !self.ready || self.snapshot.is_null() => CardState::Loading,
            None => CardState::Unknown,
            Some(definition) => self.state_of(&definition.id),
        };
        Resolved { definition, state }
    }

    fn state_of(&self, plugin_id: &str) -> CardState {
        if !self.ready || self.snapshot.is_null() {
            return CardState::Loading;
        }
        let rows: Vec<&Value> = self.snapshot["connections"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| row["pluginId"] == plugin_id)
            .collect();
        let connected = rows
            .iter()
            .filter(|row| row["state"] == "connected" && row["enabled"] != false)
            .count();
        if connected > 0 {
            CardState::Connected(connected)
        } else if self.connecting.as_deref() == Some(plugin_id)
            || rows.iter().any(|row| row["state"] == "connecting")
        {
            CardState::Connecting
        } else if rows
            .iter()
            .any(|row| row["state"] == "needs-auth" || row["state"] == "error")
        {
            CardState::NeedsAttention
        } else {
            CardState::NotConnected
        }
    }

    /// Connected plugins, one per catalog entry, with their account counts,
    /// in the order they were first connected.
    pub fn connected(&self) -> Vec<(PluginDefinition, CardState)> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for row in self.snapshot["connections"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let Some(id) = row["pluginId"].as_str() else {
                continue;
            };
            if !seen.insert(id.to_owned()) {
                continue;
            }
            let definition =
                find_definition(&self.snapshot, id).unwrap_or_else(|| PluginDefinition {
                    id: id.to_owned(),
                    name: row["label"].as_str().unwrap_or(id).to_owned(),
                    description: String::new(),
                    domain: String::new(),
                    category: None,
                    featured: false,
                    custom: true,
                    url: None,
                    api_key_header: None,
                    command: None,
                });
            let state = self.state_of(id);
            out.push((definition, state));
        }
        out
    }
}

fn find_definition(snapshot: &Value, query: &str) -> Option<PluginDefinition> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return None;
    }
    let items = crate::plugin_catalog::catalog(snapshot, "", false);
    let bare = |domain: &str| {
        domain
            .trim()
            .trim_start_matches("www.")
            .trim_end_matches('/')
            .to_lowercase()
    };
    let by = |test: &dyn Fn(&PluginDefinition) -> bool| items.iter().find(|item| test(item));
    by(&|item| item.id.to_lowercase() == query)
        .or_else(|| by(&|item| item.name.to_lowercase() == query))
        .or_else(|| {
            by(&|item| {
                let domain = bare(&item.domain);
                !domain.is_empty()
                    && (domain == query || domain.split('.').next() == Some(query.as_str()))
            })
        })
        .or_else(|| {
            let id = snapshot["connections"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|row| {
                    row["label"]
                        .as_str()
                        .is_some_and(|label| label.trim().to_lowercase() == query)
                })?["pluginId"]
                .as_str()?
                .to_owned();
            by(&|item| item.id == id)
        })
        .cloned()
}

/// "1 account", "3 accounts".
fn accounts(count: usize) -> String {
    if count == 1 {
        "1 account".into()
    } else {
        format!("{count} accounts")
    }
}

/// The card's status line, under the plugin name.
pub fn status_line(state: &CardState) -> String {
    match state {
        CardState::SignedOut => "Sign in to Zuse to connect plugins".into(),
        CardState::Loading => "Checking connection…".into(),
        CardState::Unknown => "Not found in your plugins".into(),
        CardState::NotConnected => "Not connected".into(),
        CardState::Connecting => "Waiting for you to finish signing in…".into(),
        CardState::Connected(count) => format!("Connected · {}", accounts(*count)),
        CardState::NeedsAttention => "Needs you to sign in again".into(),
        CardState::Unavailable => "Plugins aren't available here".into(),
    }
}

/// The status line of a card the agent made to ask for signing in again:
/// a connection that looks fine still fails the agent's calls.
fn reconnect_status_line(state: &CardState) -> String {
    match state {
        CardState::Connected(_) => "Access stopped working · sign in again".into(),
        state => status_line(state),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    /// Filled pill: the one action the card exists for.
    Primary,
    /// Tonal pill.
    Secondary,
    /// Text only.
    Quiet,
}

fn pill(
    p: &UiBlockPalette,
    id: ElementId,
    label: &str,
    look: Look,
    disabled: bool,
) -> Stateful<Div> {
    let (bg, fg, hover) = match look {
        Look::Primary => (p.foreground, p.background, p.foreground.opacity(0.85)),
        Look::Secondary => (p.muted, p.foreground, p.input.opacity(0.6)),
        Look::Quiet => (
            gpui::transparent_black(),
            p.muted_foreground,
            p.muted.opacity(0.6),
        ),
    };
    let quiet_fg = p.foreground;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(32.))
        .px(px(14.))
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_size(px(13.))
        .line_height(px(16.))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .role(Role::Button)
        .aria_label(SharedString::from(label.to_string()))
        .when(disabled, |d| d.opacity(0.5))
        .when(!disabled, |d| {
            d.cursor_pointer().tab_index(0).hover(move |style| {
                let style = style.bg(hover);
                if look == Look::Quiet {
                    style.text_color(quiet_fg)
                } else {
                    style
                }
            })
        })
        .child(label.to_string())
}

impl UiBlockView {
    fn plugin_button(
        &self,
        path: &str,
        key: &str,
        label: &str,
        look: Look,
        action: PluginAction,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let disabled = !self.plugin_actions || self.plugins.busy;
        pill(&self.palette, eid(path, key), label, look, disabled)
            .when(!disabled, |d| {
                d.on_click(cx.listener(move |this, _, _, cx| {
                    if this.plugin_actions && !this.plugins.busy {
                        cx.emit(UiBlockEvent::Plugin(action.clone()));
                    }
                }))
            })
            .into_any_element()
    }

    /// The buttons a card offers in `state`. `reconnect` (the agent asked the
    /// user to sign in again) leads a connected card with Reconnect.
    fn card_actions(
        &self,
        path: &str,
        query: &str,
        resolved: &Resolved,
        reconnect: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let id = resolved
            .definition
            .as_ref()
            .map(|definition| definition.id.clone())
            .unwrap_or_default();
        let manage = |this: &Self, cx: &mut Context<Self>| {
            this.plugin_button(
                path,
                "manage",
                "Manage",
                Look::Secondary,
                PluginAction::Manage {
                    plugin_id: id.clone(),
                },
                cx,
            )
        };
        let reconnect_button = |this: &Self, cx: &mut Context<Self>| {
            this.plugin_button(
                path,
                "reconnect",
                "Reconnect",
                Look::Primary,
                PluginAction::Connect {
                    plugin_id: id.clone(),
                },
                cx,
            )
        };
        match &resolved.state {
            CardState::SignedOut => vec![self.plugin_button(
                path,
                "sign-in",
                "Sign in",
                Look::Primary,
                PluginAction::SignIn,
                cx,
            )],
            CardState::Unknown => vec![self.plugin_button(
                path,
                "browse",
                "Browse plugins",
                Look::Secondary,
                PluginAction::Browse {
                    query: query.to_string(),
                },
                cx,
            )],
            CardState::NotConnected => vec![self.plugin_button(
                path,
                "connect",
                "Connect",
                Look::Primary,
                PluginAction::Connect {
                    plugin_id: id.clone(),
                },
                cx,
            )],
            CardState::Connecting => vec![manage(self, cx)],
            CardState::Connected(_) if reconnect => {
                vec![reconnect_button(self, cx), manage(self, cx)]
            }
            CardState::Connected(_) => vec![
                self.plugin_button(
                    path,
                    "add",
                    "Add account",
                    Look::Quiet,
                    PluginAction::AddAccount {
                        plugin_id: id.clone(),
                    },
                    cx,
                ),
                manage(self, cx),
            ],
            CardState::NeedsAttention => vec![reconnect_button(self, cx), manage(self, cx)],
            // Nothing to decide yet; the catalog page always works.
            CardState::Loading => vec![self.plugin_button(
                path,
                "open",
                "Open plugins",
                Look::Quiet,
                PluginAction::Browse {
                    query: query.to_string(),
                },
                cx,
            )],
            CardState::Unavailable => Vec::new(),
        }
    }

    /// A `Connect(plugin, reason?, reconnect?)` card.
    pub(super) fn connect_card(
        &mut self,
        query: &str,
        reason: Option<&str>,
        reconnect: bool,
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.clone();
        let resolved = self.plugins.resolve(query);
        let (name, domain) = resolved.definition.as_ref().map_or_else(
            || (query.to_string(), String::new()),
            |definition| (definition.name.clone(), definition.domain.clone()),
        );
        let connected = matches!(resolved.state, CardState::Connected(_)) && !reconnect;
        let status = if reconnect {
            reconnect_status_line(&resolved.state)
        } else {
            status_line(&resolved.state)
        };
        let footnote = reason.map(str::to_owned).unwrap_or_else(|| {
            format!(
                "You sign in with {name} directly; the agent never sees your credentials. Disconnect any time."
            )
        });
        let actions = self.card_actions(path, query, &resolved, reconnect, cx);
        let card = div()
            .flex()
            .items_center()
            .gap(px(14.))
            .min_w_0()
            .px(px(16.))
            .py(px(14.))
            .rounded(px(18.))
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .child(crate::plugin_catalog::logo(
                &domain,
                &name,
                44.,
                p.muted,
                p.muted_foreground,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(15.))
                                    .line_height(px(20.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(p.foreground)
                                    .child(name.clone()),
                            )
                            .when(connected, |d| {
                                d.child(icon("check-circle", 14.0, p.success))
                            }),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(13.))
                            .line_height(px(18.))
                            .text_color(p.muted_foreground)
                            .child(status.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .children(actions),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .min_w_0()
            .id(eid(path, "connect"))
            .role(Role::Group)
            .aria_label(SharedString::from(format!("{name}: {status}")))
            .child(card)
            .child(
                div()
                    .px(px(16.))
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .text_color(p.muted_foreground)
                    .child(footnote),
            )
            .into_any_element()
    }

    /// The `ConnectedPlugins()` list: each connected plugin with Manage,
    /// then a way to find more.
    pub(super) fn connected_plugins(&mut self, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette.clone();
        let directory = self.plugins.clone();
        let mut body = div().flex().flex_col().min_w_0();
        let empty_line = |text: &str| {
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(13.))
                .line_height(px(18.))
                .text_color(p.muted_foreground)
                .child(text.to_string())
        };
        let footer = |this: &Self, text: &str, cx: &mut Context<Self>| {
            let (key, label, look, action) = if directory.ready && !directory.signed_in {
                ("sign-in", "Sign in", Look::Primary, PluginAction::SignIn)
            } else {
                (
                    "browse",
                    "Browse plugins",
                    Look::Secondary,
                    PluginAction::Browse {
                        query: String::new(),
                    },
                )
            };
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(14.))
                .py(px(10.))
                .child(empty_line(text))
                .child(this.plugin_button(path, key, label, look, action, cx))
        };
        let rows = directory.connected();
        if directory.unavailable {
            body = body.child(footer(self, "Plugins aren't available here.", cx));
        } else if directory.ready && !directory.signed_in {
            body = body.child(footer(self, "Sign in to Zuse to connect plugins.", cx));
        } else if !directory.ready || directory.snapshot.is_null() {
            body = body.child(
                div()
                    .px(px(14.))
                    .py(px(12.))
                    .child(empty_line("Loading your plugins…")),
            );
        } else {
            let divider = p.border.opacity(0.6);
            for (index, (definition, state)) in rows.iter().enumerate() {
                let manage = self.plugin_button(
                    path,
                    &format!("manage-{index}"),
                    "Manage",
                    Look::Quiet,
                    PluginAction::Manage {
                        plugin_id: definition.id.clone(),
                    },
                    cx,
                );
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .min_w_0()
                        .px(px(14.))
                        .py(px(10.))
                        .border_b_1()
                        .border_color(divider)
                        .child(crate::plugin_catalog::logo(
                            &definition.domain,
                            &definition.name,
                            28.,
                            p.muted,
                            p.muted_foreground,
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(13.))
                                        .line_height(px(18.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(p.foreground)
                                        .child(definition.name.clone()),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.))
                                        .line_height(px(16.))
                                        .text_color(match state {
                                            CardState::NeedsAttention => p.warning,
                                            _ => p.muted_foreground,
                                        })
                                        .child(status_line(state)),
                                ),
                        )
                        .child(manage),
                );
            }
            let text = if rows.is_empty() {
                "No plugins connected yet."
            } else {
                "Connect another app for every agent to use."
            };
            body = body.child(footer(self, text, cx));
        }
        div()
            .min_w_0()
            .rounded(px(14.))
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .overflow_hidden()
            .child(body)
            .into_any_element()
    }
}

fn server_look(p: &UiBlockPalette, status: ServerStatus) -> (&'static str, Hsla) {
    match status {
        ServerStatus::Connected => ("Connected", p.success),
        ServerStatus::NeedsAuth => ("Needs sign-in", p.warning),
        ServerStatus::Failed => ("Failed", p.destructive),
        ServerStatus::Pending => ("Connecting", p.muted_foreground),
        ServerStatus::Disabled => ("Disabled", p.muted_foreground),
        ServerStatus::Configured => ("Configured", p.muted_foreground),
    }
}

/// One `McpServer` row. Siblings inside a Card are grouped by the caller.
pub(super) fn server_row(
    p: &UiBlockPalette,
    name: &str,
    status: ServerStatus,
    detail: Option<&str>,
    divided: bool,
) -> AnyElement {
    let (label, tint) = server_look(p, status);
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .min_w_0()
        .px(px(14.))
        .py(px(9.))
        .when(divided, |d| {
            d.border_b_1().border_color(p.border.opacity(0.6))
        })
        .child(div().flex_none().size(px(8.)).rounded_full().bg(tint))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_size(px(13.))
                        .line_height(px(18.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.foreground)
                        .child(name.to_string()),
                )
                .when_some(detail, |d, detail| {
                    d.child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(p.muted_foreground)
                            .child(detail.to_string()),
                    )
                }),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(12.))
                .line_height(px(16.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if status == ServerStatus::Connected {
                    p.muted_foreground
                } else {
                    tint
                })
                .child(label),
        )
        .into_any_element()
}

/// Consecutive `McpServer` rows as one bordered list.
pub(super) fn server_list(p: &UiBlockPalette, rows: &[&zuse_ui_block::Component]) -> AnyElement {
    let last = rows.len().saturating_sub(1);
    div()
        .min_w_0()
        .rounded(px(14.))
        .border_1()
        .border_color(p.border)
        .bg(p.card)
        .overflow_hidden()
        .children(rows.iter().enumerate().filter_map(|(index, row)| {
            let zuse_ui_block::Component::McpServer {
                name,
                status,
                detail,
            } = row
            else {
                return None;
            };
            Some(server_row(
                p,
                name,
                *status,
                detail.as_deref(),
                index != last,
            ))
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's glob brings its own `test` attribute.
    use super::{CardState, PluginDirectory, status_line};
    use serde_json::Value;

    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    fn directory(connections: Value) -> PluginDirectory {
        PluginDirectory {
            snapshot: {
                let mut snapshot = json(
                    r#"{"tenantId":"personal:u","catalog":[
                        {"id":"linear","name":"Linear","description":"Issues","domain":"linear.app"},
                        {"id":"vidiq","name":"vidIQ","description":"YouTube","domain":"www.vidiq.com"}
                    ]}"#,
                );
                snapshot["connections"] = connections;
                snapshot
            },
            signed_in: true,
            ready: true,
            ..Default::default()
        }
    }

    #[test]
    fn resolves_by_id_name_domain_and_label() {
        let plugins = directory(json(
            r#"[
            {"id":"c1","pluginId":"linear","label":"Work tracker","state":"connected","enabled":true}
        ]"#,
        ));
        for query in ["linear", "Linear", " LINEAR ", "linear.app", "Work tracker"] {
            let resolved = plugins.resolve(query);
            assert_eq!(resolved.definition.unwrap().id, "linear", "{query}");
            assert_eq!(resolved.state, CardState::Connected(1));
        }
        assert_eq!(plugins.resolve("vidiq.com").definition.unwrap().id, "vidiq");
        assert_eq!(plugins.resolve("vidiq").state, CardState::NotConnected);
        assert_eq!(plugins.resolve("nope").state, CardState::Unknown);
    }

    #[test]
    fn state_follows_connections_and_the_account() {
        let mut plugins = directory(json(
            r#"[
            {"id":"c1","pluginId":"linear","label":"Linear","state":"needs-auth","enabled":true},
            {"id":"c2","pluginId":"vidiq","label":"vidIQ","state":"connected","enabled":false}
        ]"#,
        ));
        assert_eq!(plugins.resolve("linear").state, CardState::NeedsAttention);
        // A turned-off account is not a usable connection.
        assert_eq!(plugins.resolve("vidiq").state, CardState::NotConnected);
        plugins.connecting = Some("vidiq".into());
        assert_eq!(plugins.resolve("vidiq").state, CardState::Connecting);
        plugins.signed_in = false;
        assert_eq!(plugins.resolve("linear").state, CardState::SignedOut);
        plugins.ready = false;
        plugins.snapshot = Value::Null;
        // The bundled catalog still names the plugin while loading.
        let loading = plugins.resolve("linear");
        assert_eq!(loading.state, CardState::Loading);
        assert_eq!(loading.definition.unwrap().name, "Linear");
    }

    #[test]
    fn connected_lists_each_plugin_once_with_its_account_count() {
        let plugins = directory(json(
            r#"[
            {"id":"c1","pluginId":"linear","label":"Linear","state":"connected","enabled":true},
            {"id":"c2","pluginId":"linear","label":"Linear 2","state":"connected","enabled":true},
            {"id":"c3","pluginId":"local-abc","label":"Files","state":"connected","enabled":true}
        ]"#,
        ));
        let rows = plugins.connected();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0.name, "Linear");
        assert_eq!(rows[0].1, CardState::Connected(2));
        // A connection whose plugin isn't in the catalog keeps its label.
        assert_eq!(rows[1].0.name, "Files");
        assert_eq!(status_line(&rows[0].1), "Connected · 2 accounts");
    }
}
