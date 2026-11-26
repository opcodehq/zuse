//! Computers a cloud chat may run commands on. The toolbar chip attaches
//! and detaches the owner's enrolled computers (this computer first, matched
//! by its enrollment ID, never by name); approvals appear above the composer
//! the way local permission prompts do, and running commands sit in the
//! tray with Stop. Nothing is attached and nothing runs without a click.
use super::*;
use crate::composer::tray::{TrayPill, TrayTone, tray_text_action};
use crate::{icons, popover};
use gpui::AnyElement;
use serde::Deserialize;
use serde_json::{Value, json};

/// Pending approvals and running output refresh this often while they matter.
const FAST_POLL: Duration = Duration::from_millis(2500);
/// Attachments, the account's computers and this computer's connector.
const SLOW_EVERY: u32 = 6;
const OUTPUT_LINES: usize = 4;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct BridgeStatus {
    pub state: String,
    pub device_id: Option<String>,
    pub name: Option<String>,
    pub error: Option<String>,
    pub running: usize,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Computer {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub connected: bool,
    pub always_allowed: bool,
}

/// A computer the chat may use: its primary (the computer the owner sends
/// from) or a secondary the owner attached.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Attached {
    pub device_id: String,
    pub role: String,
}
impl Attached {
    pub(crate) fn primary(&self) -> bool {
        self.role == "primary"
    }
}

/// `attached` rows; a plain device ID (older servers) is a secondary.
fn parse_attached(value: Value) -> Vec<Attached> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| match row {
            Value::String(id) => Some(Attached {
                device_id: id.clone(),
                role: "secondary".into(),
            }),
            row => serde_json::from_value(row.clone()).ok(),
        })
        .collect()
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Pending {
    pub id: String,
    pub role: String,
    pub device_id: String,
    pub computer_name: String,
    pub os_user: String,
    pub shell: String,
    pub cwd: String,
    pub command: String,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub(crate) struct RunResult {
    pub status: String,
    pub output: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Run {
    pub id: String,
    pub role: String,
    pub device_id: String,
    pub computer: String,
    pub command: String,
    pub state: String,
    pub result: RunResult,
}

/// One computer in the chip's list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ComputerRow {
    pub computer: Computer,
    pub this_computer: bool,
    pub primary: bool,
    pub secondary: bool,
}

/// The cloud chat these computers belong to.
#[derive(Clone)]
struct Scope {
    controller: crate::state::EngineHandle,
    subject: String,
    workspace: String,
    chat: String,
}
impl Scope {
    fn key(&self) -> (String, String, String) {
        (
            self.subject.clone(),
            self.workspace.clone(),
            self.chat.clone(),
        )
    }
    fn params(&self, action: &str) -> Value {
        json!({"expectedAccountId":self.subject,"workspaceId":self.workspace,
            "chatId":self.chat,"action":action})
    }
}

#[derive(Default)]
pub(in crate::composer) struct State {
    scope: Option<Scope>,
    /// Bumped on every scope change; replies for an older one are dropped.
    generation: u64,
    poll: Option<Task<()>>,
    actions: Vec<Task<()>>,
    pub(crate) status: Option<BridgeStatus>,
    pub(crate) computers: Vec<Computer>,
    pub(crate) attached: Vec<Attached>,
    pub(crate) pending: Vec<Pending>,
    pub(crate) runs: Vec<Run>,
    /// Decided here and waiting for the server to agree.
    decided: HashSet<String>,
    /// Stop requested here.
    stopping: HashSet<String>,
    /// Unknown outcomes the owner has dismissed on this computer.
    dismissed: HashSet<String>,
    pub(crate) open: bool,
    starting: bool,
    error: Option<String>,
}
impl State {
    /// Requests and output change quickly only while something is attached
    /// or still running.
    fn watching(&self) -> bool {
        !self.attached.is_empty()
            || self
                .runs
                .iter()
                .any(|run| matches!(run.state.as_str(), "pending" | "approved" | "running"))
    }
    /// The chat's primary computer, if any.
    fn primary(&self) -> Option<&str> {
        self.attached
            .iter()
            .find(|a| a.primary())
            .map(|a| a.device_id.as_str())
    }
    fn secondary(&self, device: &str) -> bool {
        self.attached
            .iter()
            .any(|a| !a.primary() && a.device_id == device)
    }
    /// This computer's enrollment, when its connector knows one.
    fn local_id(&self) -> Option<&str> {
        self.status.as_ref()?.device_id.as_deref()
    }
}

/// What one refresh read; `None` fields were not part of it.
#[derive(Default)]
struct Fetched {
    status: Option<BridgeStatus>,
    computers: Option<Vec<Computer>>,
    attached: Option<Vec<Attached>>,
    pending: Option<Vec<Pending>>,
    runs: Option<Vec<Run>>,
    error: Option<String>,
}

async fn fetch(scope: Scope, slow: bool, watching: bool) -> Fetched {
    let client = scope.controller.client();
    let mut out = Fetched::default();
    let call = |action: &str| client.call("CloudComputerControl", scope.params(action));
    if slow {
        // A missing local connector (older engine) is not an error here.
        out.status = client
            .call("ComputerBridgeStatus", json!({}))
            .await
            .ok()
            .and_then(|v| serde_json::from_value(v).ok());
        match call("list").await {
            Ok(rows) => out.computers = serde_json::from_value(rows).ok(),
            Err(error) => out.error = Some(error.to_string()),
        }
        match call("attached").await {
            Ok(ids) => out.attached = Some(parse_attached(ids)),
            Err(error) => out.error = Some(error.to_string()),
        }
    }
    // Every full read reconciles requests, so detaching the last computer
    // still clears its approvals; the fast reads only run while one matters.
    if slow || watching {
        match call("pending").await {
            Ok(rows) => out.pending = serde_json::from_value(rows).ok(),
            Err(error) => out.error = Some(error.to_string()),
        }
        match call("results").await {
            Ok(rows) => out.runs = serde_json::from_value(rows).ok(),
            Err(error) => out.error = Some(error.to_string()),
        }
    }
    out
}

impl Composer {
    /// The cloud chat in front, when this composer belongs to one.
    fn computers_scope(&self, cx: &App) -> Option<Scope> {
        if self.side_chat {
            return None;
        }
        let target = self.native_cloud_target()?;
        let chat = self.state.read(cx).selected_chat.clone()?;
        Some(Scope {
            controller: target.controller.clone(),
            subject: target.subject.clone(),
            workspace: target.workspace.clone(),
            chat,
        })
    }

    /// Follow the chat in front: a new chat (or account) starts over and
    /// drops anything still in flight for the previous one.
    pub(super) fn sync_computers(&mut self, cx: &mut Context<Self>) {
        let scope = self.computers_scope(cx);
        let same = match (&scope, &self.computers.scope) {
            (Some(a), Some(b)) => a.key() == b.key(),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        let generation = self.computers.generation.wrapping_add(1);
        self.computers = State {
            generation,
            dismissed: std::mem::take(&mut self.computers.dismissed),
            ..Default::default()
        };
        let Some(scope) = scope else {
            return;
        };
        self.computers.scope = Some(scope.clone());
        self.computers.poll = Some(cx.spawn(async move |this, cx| {
            let mut tick = 0u32;
            loop {
                let slow = tick.is_multiple_of(SLOW_EVERY);
                let watching = this
                    .read_with(cx, |c, _| c.computers.watching())
                    .unwrap_or(false);
                if slow || watching {
                    let fetched = fetch(scope.clone(), slow, watching).await;
                    if this
                        .update(cx, |c, cx| c.apply_computers(generation, fetched, cx))
                        .is_err()
                    {
                        return;
                    }
                }
                tick = tick.wrapping_add(1);
                cx.background_executor().timer(FAST_POLL).await;
            }
        }));
    }

    fn apply_computers(&mut self, generation: u64, fetched: Fetched, cx: &mut Context<Self>) {
        let state = &mut self.computers;
        if state.generation != generation {
            return;
        }
        if let Some(status) = fetched.status {
            if status.device_id.is_some() && status.state != "off" {
                state.starting = false;
            }
            state.status = Some(status);
        }
        if let Some(computers) = fetched.computers {
            state.computers = computers;
        }
        if let Some(attached) = fetched.attached {
            state.attached = attached;
        }
        if let Some(pending) = fetched.pending {
            state
                .decided
                .retain(|id| pending.iter().any(|p| &p.id == id));
            state.pending = pending;
        }
        if let Some(runs) = fetched.runs {
            state.stopping.retain(|id| {
                runs.iter()
                    .any(|run| &run.id == id && run.state == "running")
            });
            state.runs = runs;
        }
        if fetched.error.is_some() && state.error.is_none() {
            state.error = fetched.error;
        }
        cx.notify();
    }

    /// Read everything again now (after an action or opening the chip).
    fn refresh_computers(&mut self, cx: &mut Context<Self>) {
        let Some(scope) = self.computers.scope.clone() else {
            return;
        };
        let generation = self.computers.generation;
        let watching = self.computers.watching();
        self.computers.actions.retain(|task| !task.is_ready());
        self.computers.actions.push(cx.spawn(async move |this, cx| {
            let fetched = fetch(scope, true, watching).await;
            let _ = this.update(cx, |c, cx| c.apply_computers(generation, fetched, cx));
        }));
    }

    /// Send one control action; `undo` restores the optimistic change if it fails.
    fn computer_action(
        &mut self,
        method: &'static str,
        params: Value,
        undo: impl FnOnce(&mut State) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(scope) = self.computers.scope.clone() else {
            return;
        };
        let generation = self.computers.generation;
        self.computers.error = None;
        self.computers.actions.retain(|task| !task.is_ready());
        self.computers.actions.push(cx.spawn(async move |this, cx| {
            let result = scope.controller.client().call(method, params).await;
            let _ = this.update(cx, |c, cx| {
                if c.computers.generation != generation {
                    return;
                }
                if let Err(error) = result {
                    undo(&mut c.computers);
                    c.computers.error = Some(error.to_string());
                }
                c.refresh_computers(cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(crate) fn set_computer_attached(
        &mut self,
        device: &str,
        attach: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(scope) = self.computers.scope.clone() else {
            return;
        };
        let was = self.computers.attached.clone();
        if attach {
            if !self.computers.secondary(device) {
                self.computers.attached.push(Attached {
                    device_id: device.to_owned(),
                    role: "secondary".into(),
                });
            }
        } else {
            self.computers
                .attached
                .retain(|a| a.primary() || a.device_id != device);
        }
        let mut params = scope.params(if attach { "attach" } else { "detach" });
        params["deviceId"] = json!(device);
        self.computer_action(
            "CloudComputerControl",
            params,
            move |state| state.attached = was,
            cx,
        );
    }

    pub(crate) fn decide_computer_request(
        &mut self,
        request: &str,
        decision: &'static str,
        cx: &mut Context<Self>,
    ) {
        let Some(scope) = self.computers.scope.clone() else {
            return;
        };
        self.computers.decided.insert(request.to_owned());
        let mut params = scope.params("decide");
        params["requestId"] = json!(request);
        params["decision"] = json!(decision);
        let id = request.to_owned();
        self.computer_action(
            "CloudComputerControl",
            params,
            move |state| {
                state.decided.remove(&id);
            },
            cx,
        );
    }

    pub(crate) fn stop_computer_request(&mut self, request: &str, cx: &mut Context<Self>) {
        let Some(scope) = self.computers.scope.clone() else {
            return;
        };
        self.computers.stopping.insert(request.to_owned());
        let mut params = scope.params("cancel");
        params["requestId"] = json!(request);
        let id = request.to_owned();
        self.computer_action(
            "CloudComputerControl",
            params,
            move |state| {
                state.stopping.remove(&id);
            },
            cx,
        );
    }

    /// Enroll this computer and start its connector. It is not attached.
    fn allow_this_computer(&mut self, cx: &mut Context<Self>) {
        if self.computers.starting {
            return;
        }
        let label = {
            let state = self.state.read(cx);
            state
                .local_device_id
                .as_deref()
                .and_then(|id| state.devices.iter().find(|d| d.id == id))
                .map(|d| d.name.clone())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "This computer".into())
        };
        self.computers.starting = true;
        self.computer_action(
            "StartComputerBridge",
            json!({"label": label}),
            |state| state.starting = false,
            cx,
        );
    }

    fn toggle_computers(&mut self, cx: &mut Context<Self>) {
        self.computers.open = !self.computers.open;
        if self.computers.open {
            self.computers.error = None;
            self.refresh_computers(cx);
        }
        cx.notify();
    }

    /// Rows for the popover: the primary, then attached secondaries, then
    /// the account's other computers; this computer first within each, by
    /// enrollment ID (never by name).
    pub(crate) fn computer_rows(&self) -> Vec<ComputerRow> {
        let state = &self.computers;
        let local = state.local_id();
        let primary = state.primary();
        let mut rows: Vec<ComputerRow> = state
            .computers
            .iter()
            .map(|c| ComputerRow {
                computer: c.clone(),
                this_computer: Some(c.device_id.as_str()) == local,
                primary: Some(c.device_id.as_str()) == primary,
                secondary: state.secondary(&c.device_id),
            })
            .collect();
        rows.sort_by_key(|row| {
            (
                !row.primary,
                !row.secondary,
                !row.this_computer,
                row.computer.name.to_lowercase(),
            )
        });
        rows
    }

    /// The toolbar chip, in a cloud chat only.
    pub(super) fn render_computers_chip(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.computers.scope.as_ref()?;
        let count = self.computers.attached.len();
        let degraded = self
            .computers
            .status
            .as_ref()
            .is_some_and(|s| matches!(s.state.as_str(), "reconnecting" | "paused"));
        let open = self.computers.open;
        let hover_key: SharedString = format!("composer-computers-{}", cx.entity_id()).into();
        let tip = match count {
            0 => "Computers".to_owned(),
            1 => "1 computer attached".to_owned(),
            n => format!("{n} computers attached"),
        };
        let trigger = div()
            .id("composer-computers")
            .debug_selector(|| "composer-computers".into())
            .role(Role::Button)
            .aria_label(tip.clone())
            .aria_expanded(open)
            .tab_index(0)
            .h(px(28.))
            .min_w(px(28.))
            .px(px(if count > 0 { 7. } else { 0. }))
            .gap(px(5.))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .cursor_pointer()
            .relative()
            .text_size(crate::typography::text_xs())
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(if count > 0 {
                theme.text
            } else {
                theme.text_muted
            })
            .bg(if open {
                theme.muted_fill().opacity(0.7)
            } else {
                motion::hover_blend(
                    &hover_key,
                    theme.muted_fill().opacity(0.0),
                    theme.muted_fill().opacity(0.6),
                )
            })
            .on_hover(motion::hover_listener(hover_key.clone()))
            .when(!open, |el| el.tip(tip))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| this.toggle_computers(cx)))
            .child(
                icons::icon(icons::DITHER_MONITOR)
                    .size(px(14.))
                    .text_color(if count > 0 {
                        theme.text
                    } else {
                        motion::hover_blend(&hover_key, theme.text_muted, theme.text)
                    }),
            )
            .when(count > 0, |el| el.child(count.to_string()))
            .when(degraded, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(4.))
                        .right(px(4.))
                        .size(px(6.))
                        .rounded_full()
                        .bg(theme.warning),
                )
            });
        let mut root = div().relative().flex_none().child(trigger);
        if open {
            root = root.child(popover::anchored_menu_above(
                "composer-computers-popup",
                self.render_computers_menu(theme, cx),
                None,
            ));
        }
        Some(root.into_any_element())
    }

    fn render_computers_menu(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let popup = theme.for_popup();
        let status = self.computers.status.clone();
        let local = self.computers.local_id().map(str::to_owned);
        let local_listed = local
            .as_ref()
            .is_some_and(|id| self.computers.computers.iter().any(|c| &c.device_id == id));
        let connector_off = status
            .as_ref()
            .is_none_or(|s| s.state == "off" || s.device_id.is_none());
        let mut menu = popover::popover_card(&popup)
            .id("composer-computers-menu")
            .debug_selector(|| "composer-computers-menu".into())
            .role(Role::List)
            .aria_label("Computers")
            .w(px(300.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.computers.open = false;
                cx.notify();
            }));
        let rows = self.computer_rows();
        menu = menu.child(
            div()
                .px(px(8.))
                .pt(px(4.))
                .pb(px(6.))
                .text_size(crate::typography::text_xs())
                .line_height(px(15.))
                .text_color(popup.text_muted)
                .child("The agent uses your primary computer, and a secondary only when you ask for it."),
        );
        for (index, row) in rows.into_iter().enumerate() {
            let ComputerRow {
                computer,
                this_computer,
                primary,
                secondary,
            } = row;
            let attached = primary || secondary;
            let presence = if this_computer {
                match status.as_ref().map(|s| s.state.as_str()) {
                    Some("online") => "Online",
                    Some("starting") => "Connecting…",
                    Some("reconnecting") => "Reconnecting…",
                    Some("paused") => "Paused",
                    _ => "Off",
                }
            } else if computer.connected {
                "Online"
            } else {
                "Offline"
            };
            let mut detail = Vec::new();
            if primary {
                detail.push("Primary");
            } else if secondary {
                detail.push("Secondary");
            }
            detail.push(if this_computer {
                "This computer"
            } else {
                platform_name(&computer.platform)
            });
            detail.push(presence);
            let detail = detail.join(" · ");
            let device = computer.device_id.clone();
            menu = menu.child(
                popover::menu_row_nav(&popup, attached, false, format!("computer-row-{index}"))
                    .id(("computer-row", index))
                    .debug_selector(move || format!("computer-row-{index}"))
                    .role(Role::CheckBox)
                    .aria_label(computer.name.clone())
                    .aria_selected(attached)
                    .h(px(40.))
                    .py(px(0.))
                    .gap(px(10.))
                    .child(
                        icons::icon(icons::LAPTOP)
                            .flex_none()
                            .size(px(15.))
                            .text_color(popup.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .line_height(px(16.))
                                    .text_size(crate::typography::text_xs())
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(popup.text)
                                    .child(computer.name.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .line_height(px(15.))
                                    .text_size(crate::typography::text_xs())
                                    .text_color(popup.text_muted)
                                    .child(detail),
                            ),
                    )
                    .child(div().flex_none().size(px(14.)).when(attached, |slot| {
                        slot.child(
                            icons::icon(icons::CHECK)
                                .size(px(14.))
                                .text_color(popup.accent),
                        )
                    }))
                    .when(primary, |row| {
                        row.cursor_default()
                            .tip("The computer you send from. Its commands still ask first.")
                    })
                    .when(!primary, |row| {
                        row.on_click(cx.listener(move |this, _, _, cx| {
                            this.set_computer_attached(&device, !secondary, cx)
                        }))
                    }),
            );
        }
        if connector_off || !local_listed {
            let starting =
                self.computers.starting || status.as_ref().is_some_and(|s| s.state == "starting");
            menu = menu.child(
                div()
                    .id("computer-allow-this")
                    .debug_selector(|| "computer-allow-this".into())
                    .role(Role::Button)
                    .cursor_pointer()
                    .rounded(px(8.))
                    .px(px(8.))
                    .py(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .hover(|s| s.bg(popup.element_hover))
                    .child(
                        icons::icon(icons::DITHER_PLUS)
                            .flex_none()
                            .size(px(14.))
                            .text_color(popup.accent),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::text_xs())
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(popup.accent)
                            .child(if starting {
                                "Allowing this computer…"
                            } else {
                                "Allow cloud chats to use this computer"
                            }),
                    )
                    .tip("Each command still asks you first.")
                    .on_click(cx.listener(|this, _, _, cx| this.allow_this_computer(cx))),
            );
        } else if self.computers.computers.is_empty() {
            menu = menu.child(
                div()
                    .px(px(8.))
                    .py(px(8.))
                    .text_size(crate::typography::text_xs())
                    .text_color(popup.text_muted)
                    .child("No computers yet"),
            );
        }
        if let Some(error) = status.as_ref().and_then(|s| s.error.clone()) {
            menu = menu.child(
                div()
                    .px(px(8.))
                    .pb(px(6.))
                    .text_size(crate::typography::text_xs())
                    .text_color(popup.warning)
                    .child(error),
            );
        }
        menu.into_any_element()
    }

    /// The approval card above the composer, one request at a time.
    pub(super) fn render_computer_approval(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let waiting: Vec<&Pending> = self
            .computers
            .pending
            .iter()
            .filter(|p| !self.computers.decided.contains(&p.id))
            .collect();
        let request = (*waiting.first()?).clone();
        let headline = if request.role == "secondary" {
            format!("Run on {} (secondary)?", request.computer_name)
        } else {
            format!("Run on {}?", request.computer_name)
        };
        let more = waiting.len() - 1;
        let minutes = (request.timeout_ms / 60_000).max(1);
        let limit = if request.timeout_ms >= 60_000 {
            format!("up to {minutes} min")
        } else {
            format!("up to {}s", (request.timeout_ms / 1000).max(1))
        };
        let meta = [request.os_user.as_str(), request.shell.as_str(), &limit]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let button = |label: &str, id: &'static str, decision: &'static str| {
            let request = request.id.clone();
            popover::btn_ghost(theme, label, id)
                .id(id)
                .debug_selector(move || id.into())
                .role(Role::Button)
                .tab_index(0)
                .h(px(28.))
                .px(px(8.))
                .py(px(4.))
                .rounded(px(6.))
                .text_size(px(11.))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.decide_computer_request(&request, decision, cx)
                }))
        };
        let deny = button("Deny", "computer-deny", "deny");
        let chat = button("This chat", "computer-allow-chat", "session")
            .tip("Run this chat's later commands on this computer without asking.");
        let always = button("Always", "computer-allow-always", "always")
            .tip("Run commands from any of your chats on this computer without asking.");
        let once_id = request.id.clone();
        let card = div()
            .id("computer-approval-card")
            .debug_selector(|| "computer-approval-card".into())
            .role(Role::Group)
            .aria_label(headline.clone())
            .w_full()
            .rounded(px(12.))
            .p(px(12.))
            .border_1()
            .border_color(theme.border.opacity(0.7))
            .bg(theme.surface_card)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        icons::icon(icons::DITHER_MONITOR)
                            .flex_none()
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text_strong)
                            .child(headline),
                    )
                    .when(more > 0, |row| {
                        row.child(
                            div()
                                .rounded_full()
                                .px(px(8.))
                                .py(px(2.))
                                .text_size(px(10.))
                                .bg(theme.muted_fill())
                                .text_color(theme.text_muted)
                                .child(format!("+{more} more")),
                        )
                    }),
            )
            .child(
                div()
                    .id("computer-request-detail")
                    .mt(px(8.))
                    .max_h(px(96.))
                    .overflow_y_scroll()
                    .rounded(px(6.))
                    .px(px(10.))
                    .py(px(6.))
                    .bg(theme.muted_fill().opacity(0.45))
                    .font_family(theme.font_mono.clone())
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(theme.text)
                    .child(
                        div()
                            .text_color(theme.text_muted)
                            .child(request.cwd.clone()),
                    )
                    .child(format!("$ {}", request.command)),
            )
            .child(
                div()
                    .mt(px(6.))
                    .text_size(px(10.))
                    .line_height(px(16.))
                    .text_color(theme.text_muted)
                    .child(meta),
            )
            .child(
                div()
                    .mt(px(10.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_end()
                    .gap(px(4.))
                    .child(deny)
                    .child(chat)
                    .child(always)
                    .child(
                        popover::btn_primary(theme, "Approve once")
                            .id("computer-allow-once")
                            .debug_selector(|| "computer-allow-once".into())
                            .role(Role::Button)
                            .tab_index(0)
                            .ml(px(4.))
                            .text_size(px(11.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.decide_computer_request(&once_id, "once", cx)
                            })),
                    ),
            );
        Some(card.into_any_element())
    }

    /// Tray rows: running commands with Stop, unknown outcomes, and errors.
    pub(super) fn computer_tray_rows(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<TrayPill> {
        let mut rows = Vec::new();
        if self.computers.scope.is_none() {
            return rows;
        }
        if let Some(error) = self.computers.error.clone() {
            rows.push(
                TrayPill::new("computers-error", "Computers")
                    .subtitle(error)
                    .tone(TrayTone::Warning)
                    .glyph(icons::ALERT_CIRCLE)
                    .action(
                        tray_text_action("computers-error-dismiss", "Dismiss", None, theme)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.computers.error = None;
                                cx.notify();
                            })),
                    ),
            );
        }
        for (index, run) in self
            .computers
            .runs
            .iter()
            .filter(|run| run.state == "running")
            .take(3)
            .enumerate()
        {
            let stopping = self.computers.stopping.contains(&run.id);
            let id = run.id.clone();
            let lines: Vec<&str> = run
                .result
                .output
                .lines()
                .filter(|l| !l.trim().is_empty())
                .collect();
            let tail = lines[lines.len().saturating_sub(OUTPUT_LINES)..].join("\n");
            let output = (!tail.is_empty()).then(|| {
                div()
                    .px(px(12.))
                    .py(px(6.))
                    .font_family(theme.font_mono.clone())
                    .text_size(px(11.))
                    .line_height(px(15.))
                    .text_color(theme.text_muted)
                    .child(tail)
                    .into_any_element()
            });
            let pill = TrayPill::new(
                SharedString::from(format!("computer-run-{index}")),
                format!("Running on {}", run.computer),
            )
            .subtitle(run.command.clone())
            .icon_element(crate::loaders::mini_mono_spinner(
                SharedString::from(format!("computer-run-spinner-{index}")),
                2.,
                theme.text_muted,
                cx.entity_id(),
                cx,
            ))
            .expanded(output);
            rows.push(if stopping {
                pill.action(
                    div()
                        .px(px(8.))
                        .text_color(theme.text_muted)
                        .child("Stopping…"),
                )
            } else {
                pill.action(
                    tray_text_action(
                        SharedString::from(format!("computer-stop-{index}")),
                        "Stop",
                        None,
                        theme,
                    )
                    .debug_selector(move || format!("computer-stop-{index}"))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.stop_computer_request(&id, cx)),
                    ),
                )
            });
        }
        for (index, run) in self
            .computers
            .runs
            .iter()
            .filter(|run| run.state == "unknown" && !self.computers.dismissed.contains(&run.id))
            .take(2)
            .enumerate()
        {
            let id = run.id.clone();
            rows.push(
                TrayPill::new(
                    SharedString::from(format!("computer-unknown-{index}")),
                    "Outcome unknown",
                )
                .subtitle("It may have run. It won't run again.")
                .tone(TrayTone::Warning)
                .glyph(icons::ALERT_CIRCLE)
                .action(
                    tray_text_action(
                        SharedString::from(format!("computer-unknown-dismiss-{index}")),
                        "Dismiss",
                        None,
                        theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.computers.dismissed.insert(id.clone());
                        cx.notify();
                    })),
                ),
            );
        }
        rows
    }
}

impl Composer {
    /// The chat a send claims its primary computer for: only once this
    /// engine has answered for its connector (older engines have none).
    pub(super) fn primary_claim(
        &self,
        request: &Value,
        cx: &App,
    ) -> Option<(crate::state::CloudTarget, String)> {
        self.computers.status.as_ref()?;
        let target = self.native_cloud_target()?.clone();
        let chat = request["sessionId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .or_else(|| self.state.read(cx).selected_chat.clone())?;
        Some((target, chat))
    }
}

/// Make the computer the owner is typing on the chat's primary (the local
/// engine proves it with this computer's own key, or clears the primary when
/// its connector is off). A send never waits more than a moment for it and
/// never fails because of it.
pub(super) async fn claim_primary(
    target: crate::state::CloudTarget,
    chat: String,
    executor: gpui::BackgroundExecutor,
) {
    use futures::future::{Either, select};
    let call = target.controller.client().call(
        "ClaimComputerPrimary",
        json!({"expectedAccountId":target.subject,"workspaceId":target.workspace,"chatId":chat}),
    );
    let timer = executor.timer(Duration::from_secs(3));
    match select(std::pin::pin!(call), std::pin::pin!(timer)).await {
        Either::Left((Ok(_), _)) => {}
        Either::Left((Err(error), _)) => {
            tracing::info!(%error, "computers: primary not claimed");
        }
        Either::Right(_) => tracing::info!("computers: primary claim timed out"),
    }
}

fn platform_name(platform: &str) -> &str {
    match platform {
        "macos" => "macOS",
        "linux" => "Linux",
        "windows" => "Windows",
        other => other,
    }
}

#[cfg(test)]
mod tests;
