//! Commands a cloud chat runs on the owner's computers. The computer the
//! owner types on is used automatically, and their other enrolled online
//! computers are available without attaching anything. Every command asks
//! first: its approval replaces the composer the way a local permission
//! prompt does, and running commands sit in the tray with Stop.
use super::*;
use crate::composer::tray::{TrayPill, TrayTone, tray_text_action};
use crate::{icons, popover};
use gpui::AnyElement;
use serde::Deserialize;
use serde_json::{Value, json};

/// Requests refresh this often while one is waiting or running.
const FAST_POLL: Duration = Duration::from_millis(2500);
/// Otherwise every fourth round (about every 10 s).
const IDLE_EVERY: u32 = 4;
/// The account's computers and the chat's primary, about every 15 s.
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

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Pending {
    pub id: String,
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
    pub device_id: String,
    pub computer: String,
    pub command: String,
    pub state: String,
    pub result: RunResult,
}

/// The account's computers by enrollment ID, and the computer the chat in
/// front runs on by default. Transcript rows name a command's computer
/// from here.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ComputerDirectory {
    pub computers: Vec<Computer>,
    pub primary: Option<String>,
}
impl gpui::Global for ComputerDirectory {}
impl ComputerDirectory {
    /// The name and icon for a command's computer; no ID means the
    /// chat's own (the one the owner types on).
    pub(crate) fn label(&self, device: Option<&str>) -> Option<(String, &'static str)> {
        let id = device.or(self.primary.as_deref())?;
        let computer = self.computers.iter().find(|c| c.device_id == id)?;
        Some((computer.name.clone(), platform_icon(&computer.platform)))
    }
}

/// A laptop for macOS, a desktop for the others.
pub(crate) fn platform_icon(platform: &str) -> &'static str {
    match platform {
        "macos" => icons::LAPTOP,
        _ => icons::MONITOR,
    }
}

/// The cloud chat these requests belong to.
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
    pub(crate) computers: Vec<Computer>,
    pub(crate) primary: Option<String>,
    pub(crate) pending: Vec<Pending>,
    pub(crate) runs: Vec<Run>,
    /// Decided here and waiting for the server to agree.
    decided: HashSet<String>,
    /// Stop requested here.
    stopping: HashSet<String>,
    /// Unknown outcomes the owner has dismissed on this computer.
    dismissed: HashSet<String>,
    error: Option<String>,
}
impl State {
    /// A request is waiting for the owner or a command is running.
    fn busy(&self) -> bool {
        !self.pending.is_empty()
            || self
                .runs
                .iter()
                .any(|run| matches!(run.state.as_str(), "pending" | "approved" | "running"))
    }
    /// Requests still waiting for a decision here, oldest first.
    fn waiting(&self) -> Vec<&Pending> {
        self.pending
            .iter()
            .filter(|p| !self.decided.contains(&p.id))
            .collect()
    }
}

/// What one refresh read; `None` fields were not part of it.
#[derive(Default)]
struct Fetched {
    computers: Option<Vec<Computer>>,
    primary: Option<Option<String>>,
    pending: Option<Vec<Pending>>,
    runs: Option<Vec<Run>>,
    error: Option<String>,
}

async fn fetch(scope: Scope, slow: bool, requests: bool) -> Fetched {
    let client = scope.controller.client();
    let mut out = Fetched::default();
    let call = |action: &str| client.call("CloudComputerControl", scope.params(action));
    if slow {
        match call("list").await {
            Ok(rows) => out.computers = serde_json::from_value(rows).ok(),
            Err(error) => out.error = Some(error.to_string()),
        }
        if let Ok(primary) = call("primary").await {
            out.primary = Some(primary["deviceId"].as_str().map(str::to_owned));
        }
    }
    if slow || requests {
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
                let busy = this
                    .read_with(cx, |c, _| c.computers.busy())
                    .unwrap_or(false);
                let requests = busy || tick.is_multiple_of(IDLE_EVERY);
                if slow || requests {
                    let fetched = fetch(scope.clone(), slow, requests).await;
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
        let mut directory = None;
        if let Some(computers) = fetched.computers {
            state.computers = computers;
            directory = Some(());
        }
        if let Some(primary) = fetched.primary {
            state.primary = primary;
            directory = Some(());
        }
        if let Some(pending) = fetched.pending {
            let had = state.waiting().is_empty();
            state
                .decided
                .retain(|id| pending.iter().any(|p| &p.id == id));
            state.pending = pending;
            if had && !state.waiting().is_empty() {
                // A new request takes the composer's place; take focus with it.
                self.focus_computer_approval = true;
            }
        }
        let state = &mut self.computers;
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
        if directory.is_some() {
            let next = ComputerDirectory {
                computers: state.computers.clone(),
                primary: state.primary.clone(),
            };
            cx.set_global(next);
        }
        cx.notify();
    }

    /// Read everything again now (after an action).
    fn refresh_computers(&mut self, cx: &mut Context<Self>) {
        let Some(scope) = self.computers.scope.clone() else {
            return;
        };
        let generation = self.computers.generation;
        self.computers.actions.retain(|task| !task.is_ready());
        self.computers.actions.push(cx.spawn(async move |this, cx| {
            let fetched = fetch(scope, true, true).await;
            let _ = this.update(cx, |c, cx| c.apply_computers(generation, fetched, cx));
        }));
    }

    /// Send one control action; `undo` restores the optimistic change if it fails.
    fn computer_action(
        &mut self,
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
            let result = scope
                .controller
                .client()
                .call("CloudComputerControl", params)
                .await;
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
            params,
            move |state| {
                state.decided.remove(&id);
            },
            cx,
        );
        // The composer comes back once nothing else is waiting.
        self.focus_pending = self.computers.waiting().is_empty();
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
            params,
            move |state| {
                state.stopping.remove(&id);
            },
            cx,
        );
    }

    /// The first request still waiting, which the approval shows.
    pub(crate) fn computer_request_waiting(&self) -> Option<&Pending> {
        self.computers.waiting().into_iter().next()
    }

    /// Keys on the approval: Escape denies, ⌘/Ctrl+Enter allows once.
    fn on_computer_approval_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(request) = self.computer_request_waiting().map(|p| p.id.clone()) else {
            return;
        };
        let decision = match event.keystroke.key.as_str() {
            "escape" => "deny",
            "enter" if event.keystroke.modifiers.platform || event.keystroke.modifiers.control => {
                "once"
            }
            _ => return,
        };
        self.decide_computer_request(&request, decision, cx);
        cx.stop_propagation();
    }

    /// The approval that takes the composer's place while a command waits,
    /// laid out like a local permission prompt.
    pub(super) fn render_computer_approval(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let waiting = self.computers.waiting();
        let request = (*waiting.first()?).clone();
        let more = waiting.len() - 1;
        let theme = Theme::of(cx).clone();
        if std::mem::take(&mut self.focus_computer_approval)
            || self.input.focus_handle(cx).is_focused(window)
        {
            window.focus(&self.computer_focus, cx);
        }
        let platform = self
            .computers
            .computers
            .iter()
            .find(|c| c.device_id == request.device_id)
            .map(|c| c.platform.clone())
            .unwrap_or_default();
        let limit = if request.timeout_ms >= 60_000 {
            format!("up to {} min", request.timeout_ms / 60_000)
        } else {
            format!("up to {}s", (request.timeout_ms / 1000).max(1))
        };
        let meta = [request.os_user.as_str(), request.shell.as_str(), &limit]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let ghost = |label: &str, id: &'static str, decision: &'static str| {
            let request = request.id.clone();
            popover::btn_ghost(&theme, label, id)
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
        let deny = ghost("Deny", "computer-deny", "deny").tip("Escape".to_owned());
        let chat = ghost("Allow for this chat", "computer-allow-chat", "session")
            .tip("Later commands from this chat on this computer run without asking.");
        let always = ghost("Allow always", "computer-allow-always", "always")
            .tip("Commands from any of your chats on this computer run without asking.");
        let once_id = request.id.clone();
        let card = div()
            .id("computer-approval-card")
            .debug_selector(|| "computer-approval-card".into())
            .role(Role::Group)
            .aria_label("Run this command on your computer?")
            .track_focus(&self.computer_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.on_computer_approval_key(event, cx)
            }))
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
                        div()
                            .size(px(24.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(theme.muted_fill())
                            .child(
                                icons::icon(platform_icon(&platform))
                                    .size(px(14.))
                                    .text_color(theme.text_muted),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(theme.text_muted)
                            .child("Computer"),
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
                    .mt(px(10.))
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_strong)
                    .child("Run this command on your computer?"),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(theme.text_muted)
                    .child(request.computer_name.clone()),
            )
            .child(
                div()
                    .id("computer-request-detail")
                    .mt(px(10.))
                    .max_h(px(120.))
                    .overflow_y_scroll()
                    .rounded(px(6.))
                    .px(px(10.))
                    .py(px(8.))
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
                    .mt(px(10.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(10.))
                            .text_color(theme.text_muted)
                            .child(meta),
                    )
                    .child(deny)
                    .child(chat)
                    .child(always)
                    .child(
                        popover::btn_primary(&theme, "Allow once")
                            .id("computer-allow-once")
                            .debug_selector(|| "computer-allow-once".into())
                            .role(Role::Button)
                            .tab_index(0)
                            .ml(px(4.))
                            .text_size(px(11.))
                            .tip("⌘ / Ctrl + Enter".to_owned())
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
    /// The chat a send claims its primary computer for, through the local
    /// engine bound to this cloud chat.
    pub(super) fn primary_claim(
        &self,
        request: &Value,
        cx: &App,
    ) -> Option<(crate::state::CloudTarget, String)> {
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

/// Test RPC channels: answer each send's primary claim the way an engine
/// without this computer's connector does, and pass every other frame on.
#[cfg(test)]
pub(crate) fn answering_primary_claims(
    send: tokio::sync::mpsc::Sender<String>,
    replies: tokio::sync::mpsc::Sender<String>,
) -> tokio::sync::mpsc::Sender<String> {
    let (frames, mut received) = tokio::sync::mpsc::channel::<String>(64);
    tokio::spawn(async move {
        while let Some(line) = received.recv().await {
            let frame: Value = serde_json::from_str(line.trim()).unwrap_or_default();
            if frame["method"] == "ClaimComputerPrimary" {
                let reply = json!({"id":frame["id"],"ok":{"deviceId":null}});
                let _ = replies.send(format!("{reply}\n")).await;
            } else if send.send(line).await.is_err() {
                return;
            }
        }
    });
    frames
}

#[cfg(test)]
mod tests;
