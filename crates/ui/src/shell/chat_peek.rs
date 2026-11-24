//! The details card a sidebar chat row opens when the pointer rests on it:
//! the full title, where the chat runs and its activity. It reads
//! only what the sidebar already holds (local chat rows, the cloud catalog),
//! so opening it never polls or calls the network.
use super::*;
use crate::tooltip::HoverCardExt;
use serde_json::Value;

/// Where a chat's agent runs.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum PeekPlace {
    /// This computer, or another of the account's computers by name.
    Local { device: Option<String> },
    /// A cloud computer and its machine state, when known. The
    /// infrastructure behind it is never shown.
    Cloud {
        machine: Option<&'static str>,
        online: bool,
    },
}

/// Everything a row's details card can show. Optional fields are absent
/// when the sidebar has no data for them, and the card leaves them out.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ChatPeek {
    pub title: String,
    pub project: Option<String>,
    pub place: PeekPlace,
    pub branch: Option<String>,
    pub agent: Option<zuse_protocol::HarnessId>,
    pub model: Option<String>,
    pub pull_request: Option<zuse_protocol::ChangeRequestSummary>,
    pub diff: Option<(u32, u32)>,
    /// Activity and age, e.g. "Working · 5m ago" or "Active 2h ago".
    pub status: Option<String>,
}

/// The lines a card shows, in order; tests read which are present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PeekLine {
    Project,
    Place,
    Branch,
    Agent,
    PullRequest,
    Diff,
    Status,
}

impl ChatPeek {
    pub(super) fn lines(&self) -> Vec<PeekLine> {
        let mut lines = Vec::new();
        if self.project.is_some() {
            lines.push(PeekLine::Project);
        }
        lines.push(PeekLine::Place);
        if self.branch.is_some() {
            lines.push(PeekLine::Branch);
        }
        if self.agent.is_some() || self.model.is_some() {
            lines.push(PeekLine::Agent);
        }
        if self.pull_request.is_some() {
            lines.push(PeekLine::PullRequest);
        }
        if self.diff.is_some() {
            lines.push(PeekLine::Diff);
        }
        if self.status.is_some() {
            lines.push(PeekLine::Status);
        }
        lines
    }

    /// The place line's words.
    pub(super) fn place_label(&self) -> String {
        match &self.place {
            PeekPlace::Local { device: None } => "Local · this computer".into(),
            PeekPlace::Local {
                device: Some(device),
            } => format!("Local · {device}"),
            PeekPlace::Cloud { machine, .. } => std::iter::once("Cloud")
                .chain(*machine)
                .collect::<Vec<_>>()
                .join(" · "),
        }
    }
}

/// A cloud computer's state as the sidebar words it, and whether it is up.
/// Mirrors the conversation header's reading of the same workspace state.
pub(super) fn cloud_machine(state: Option<&str>) -> (Option<&'static str>, bool) {
    match state {
        Some("ready") => (Some("Running"), true),
        Some("paused") => (Some("Sleeping"), false),
        Some("pausing") => (Some("Going to sleep"), false),
        Some("failed" | "error" | "blocked") => (Some("Needs attention"), false),
        Some("archiving" | "archived") => (Some("Archived"), false),
        Some("deleting" | "deleted") => (Some("Deleted"), false),
        Some(_) => (Some("Starting"), false),
        None => (None, false),
    }
}

/// Whether a listed cloud computer is up: running, and its runtime not
/// reported offline. A sleeping, starting or archived one is not.
pub(super) fn cloud_row_online(row: &serde_json::Value) -> bool {
    cloud_machine(row["state"].as_str()).1 && row["runtimeState"].as_str() != Some("offline")
}

fn non_empty(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// "Working · 5m ago", "Active 2h ago", "Active just now".
fn status_line(label: Option<&str>, time_ago: &str) -> Option<String> {
    let age = match time_ago.trim() {
        "" => None,
        "now" => Some("just now".to_owned()),
        age => Some(format!("{age} ago")),
    };
    match (label, age) {
        (Some(label), Some(age)) => Some(format!("{label} · {age}")),
        (Some(label), None) => Some(label.to_owned()),
        (None, Some(age)) => Some(format!("Active {age}")),
        (None, None) => None,
    }
}

/// The card for a local chat row, from the chat list alone.
pub(super) fn local_peek(
    state: &AppState,
    chat_id: &str,
    status: Option<&str>,
    time_ago: &str,
) -> Option<ChatPeek> {
    let chat = state.chats.iter().find(|chat| chat.id == chat_id)?;
    let local = state.local_device_id.as_deref() == Some(chat.device_id.as_str());
    let device = (!local)
        .then(|| state.device_name(&chat.device_id).map(str::to_owned))
        .flatten();
    Some(ChatPeek {
        title: transcript::single_line(chat.title.as_deref().unwrap_or("New session")),
        project: state
            .space_for_chat(chat)
            .map(|space| space.display_name().to_owned()),
        place: PeekPlace::Local { device },
        branch: non_empty(crate::change_requests::conversation_branch(
            chat,
            &state.spaces,
        )),
        agent: chat.config.as_ref().map(|config| config.harness),
        model: non_empty(chat.config.as_ref().and_then(|c| c.model.as_deref())),
        pull_request: state.change_request_for_chat(chat).cloned(),
        diff: state.diff_stat_for_chat(chat),
        status: status_line(status, time_ago),
    })
}

/// The card for a cloud task row, from its catalog entry alone.
pub(super) fn cloud_peek(
    row: &Value,
    project: Option<String>,
    status: Option<&str>,
    time_ago: &str,
) -> ChatPeek {
    let (machine, online) = cloud_machine(row["state"].as_str());
    ChatPeek {
        title: transcript::single_line(&zuse_protocol::file_mentions::mention_display_text(
            row["title"]
                .as_str()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or("New session"),
        )),
        project,
        place: PeekPlace::Cloud { machine, online },
        branch: non_empty(row["branch"].as_str()),
        agent: super::cloud_conversation::cloud_harness(row),
        model: non_empty(row["model"].as_str()),
        pull_request: None,
        diff: None,
        status: status_line(status, time_ago),
    }
}

impl Shell {
    /// The details card for the sidebar row `id` (a local chat id or a
    /// `cloud:<workspace>` row), from data the sidebar already holds.
    pub(super) fn chat_peek(
        &self,
        id: &str,
        status: Option<&str>,
        time_ago: &str,
        cx: &App,
    ) -> Option<ChatPeek> {
        match id.strip_prefix(CLOUD_ROW_PREFIX) {
            Some(workspace) => {
                let row = self.state.read(cx).cloud_sidebar_workspace(workspace)?;
                let project = Some(self.cloud_row_project_name(row, cx));
                Some(cloud_peek(row, project, status, time_ago))
            }
            None => local_peek(self.state.read(cx), id, status, time_ago),
        }
    }

    /// Attach the details card to a sidebar chat row.
    pub(super) fn with_chat_peek(
        &self,
        row: gpui::Stateful<gpui::Div>,
        id: String,
        status: Option<&'static str>,
        time_ago: SharedString,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let shell = cx.weak_entity();
        row.hover_card(move |_, cx| {
            let peek = shell
                .upgrade()
                .and_then(|shell| shell.read(cx).chat_peek(&id, status, &time_ago, cx));
            cx.new(|_| ChatPeekCard { peek }).into()
        })
    }
}

/// The card's view; renders nothing when the row has gone.
struct ChatPeekCard {
    peek: Option<ChatPeek>,
}

impl Render for ChatPeekCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(peek) = self.peek.as_ref() else {
            return div().into_any_element();
        };
        render_chat_peek(peek, Theme::of(cx))
    }
}

fn render_chat_peek(peek: &ChatPeek, theme: &Theme) -> AnyElement {
    let muted = theme.text_muted;
    let glyph = |path: &'static str, color: gpui::Hsla| {
        icon(path).size(px(14.0)).flex_none().text_color(color)
    };
    let mut card = crate::tooltip::card(theme).child(
        div()
            .mb(px(2.0))
            .w_full()
            .min_w_0()
            .truncate()
            .text_size(px(13.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.text_strong)
            .child(peek.title.clone()),
    );
    for line in peek.lines() {
        card = card.child(match line {
            PeekLine::Project => crate::tooltip::card_line(
                glyph(icons::FOLDER, muted),
                peek.project.clone().unwrap_or_default(),
                theme,
            ),
            PeekLine::Place => {
                let place = match &peek.place {
                    PeekPlace::Local { device: None } => glyph(icons::LAPTOP, muted),
                    PeekPlace::Local { device: Some(_) } => glyph(icons::REMOTE_SERVER, muted),
                    PeekPlace::Cloud { online, .. } => glyph(
                        icons::DITHER_CLOUD,
                        super::cloud_conversation::header_status_color(*online, theme),
                    ),
                };
                crate::tooltip::card_line(place, peek.place_label(), theme)
            }
            PeekLine::Branch => crate::tooltip::card_line(
                glyph(icons::GIT_BRANCH, muted),
                peek.branch.clone().unwrap_or_default(),
                theme,
            ),
            PeekLine::Agent => {
                let name = peek.agent.map(crate::pickers::harness_display_name);
                let text = match (name, peek.model.as_deref()) {
                    (Some(name), Some(model)) => format!("{name} · {model}"),
                    (Some(name), None) => name.to_owned(),
                    (None, Some(model)) => model.to_owned(),
                    (None, None) => String::new(),
                };
                let mark = match peek.agent {
                    Some(agent) => {
                        let (path, tint) = crate::pickers::harness_brand_icon(agent);
                        icons::brand_icon(path, 14.0, tint.unwrap_or(muted))
                    }
                    None => glyph(icons::TERMINAL, muted).into_any_element(),
                };
                crate::tooltip::card_line(mark, text, theme)
            }
            PeekLine::PullRequest => {
                let summary = peek.pull_request.clone().expect("listed only when present");
                let (path, tone) =
                    crate::change_requests::branch_state_glyph(Some(&summary), theme);
                crate::tooltip::card_line(
                    glyph(path, tone),
                    format!("#{} {}", summary.number, summary.title),
                    theme,
                )
            }
            PeekLine::Diff => {
                let (added, removed) = peek.diff.unwrap_or_default();
                crate::tooltip::card_line(
                    glyph(icons::GIT_COMPARE, muted),
                    sidebar_diff_stat(added, removed, 1.0, theme),
                    theme,
                )
            }
            PeekLine::Status => div()
                .pt(px(2.0))
                .text_size(px(11.0))
                .text_color(muted.opacity(0.8))
                .child(peek.status.clone().unwrap_or_default()),
        });
    }
    crate::tooltip::frost(
        card.id("chat-peek-card")
            .debug_selector(|| "chat-peek-card".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chat(id: &str, device: &str) -> zuse_protocol::Chat {
        serde_json::from_value(json!({
            "id": id, "deviceId": device, "title": "Fix the login redirect",
            "archived": false, "cwd": null, "branch": null, "checkoutId": null,
            "config": null, "lastMessagePreview": null, "lastMessageAt": null,
            "createdAt": "2026-11-18T00:00:00Z"
        }))
        .unwrap()
    }

    #[test]
    fn a_bare_local_chat_shows_only_where_it_runs_and_its_age() {
        let mut state = AppState::new();
        state.local_device_id = Some("mac".into());
        state.chats.push(chat("c1", "mac"));
        let peek = local_peek(&state, "c1", None, "5m").unwrap();
        assert_eq!(peek.title, "Fix the login redirect");
        assert_eq!(peek.lines(), vec![PeekLine::Place, PeekLine::Status]);
        assert_eq!(peek.place_label(), "Local · this computer");
        assert_eq!(peek.status.as_deref(), Some("Active 5m ago"));
        assert!(local_peek(&state, "missing", None, "5m").is_none());
    }

    #[test]
    fn a_local_chat_lists_every_field_it_has() {
        let mut state = AppState::new();
        state.local_device_id = Some("mac".into());
        let mut chat = chat("c1", "mac");
        chat.source_context = Some(
            serde_json::from_value(json!({
                "checkoutId": "checkout", "repoRoot": "/projects/zuse", "cwd": "/projects/zuse",
                "branch": "swarajbachu/fix-login", "observedAt": "2026-11-18T00:00:00Z"
            }))
            .unwrap(),
        );
        chat.last_message_preview = Some("Updated the redirect\nand tests".into());
        chat.config = Some(
            serde_json::from_value(json!({
                "harness": "zuse", "model": "test-model", "reasoning": null, "sandbox": "workspace-write"
            }))
            .unwrap(),
        );
        state.chats.push(chat);
        let peek = local_peek(&state, "c1", Some("Working"), "now").unwrap();
        assert_eq!(
            peek.lines(),
            vec![
                PeekLine::Place,
                PeekLine::Branch,
                PeekLine::Agent,
                PeekLine::Status,
            ]
        );
        assert_eq!(peek.status.as_deref(), Some("Working · just now"));
        assert_eq!(peek.model.as_deref(), Some("test-model"));
    }

    #[test]
    fn another_computers_chat_names_that_computer() {
        let mut state = AppState::new();
        state.local_device_id = Some("mac".into());
        state.chats.push(chat("c1", "studio"));
        let peek = local_peek(&state, "c1", None, "").unwrap();
        // The device list is empty here, so its name is unknown.
        assert_eq!(peek.place, PeekPlace::Local { device: None });
        assert_eq!(peek.lines(), vec![PeekLine::Place]);
    }

    #[test]
    fn a_cloud_task_shows_machine_branch_and_agent_but_never_its_provider() {
        let row = json!({
            "workspaceId": "task-1", "providerId": "boxd", "state": "ready",
            "title": "Cloud chat", "branch": "task/branch-1", "agent": "zuse"
        });
        let peek = cloud_peek(&row, Some("Hello-World".into()), None, "1m");
        assert_eq!(
            peek.lines(),
            vec![
                PeekLine::Project,
                PeekLine::Place,
                PeekLine::Branch,
                PeekLine::Agent,
                PeekLine::Status,
            ]
        );
        assert_eq!(peek.place_label(), "Cloud · Running");
        assert!(!zuse_protocol::cloud_billing::plans::names_infrastructure(
            &peek.place_label()
        ));
        assert!(matches!(peek.place, PeekPlace::Cloud { online: true, .. }));

        let bare = cloud_peek(&json!({"workspaceId": "task-2"}), None, None, "");
        assert_eq!(bare.title, "New session");
        assert_eq!(bare.lines(), vec![PeekLine::Place]);
        assert_eq!(bare.place_label(), "Cloud");
    }

    #[test]
    fn a_listed_cloud_computer_is_online_only_while_running() {
        use serde_json::json;
        assert!(cloud_row_online(
            &json!({"state":"ready","runtimeState":"online"})
        ));
        assert!(cloud_row_online(&json!({"state":"ready"})));
        assert!(!cloud_row_online(
            &json!({"state":"ready","runtimeState":"offline"})
        ));
        for state in ["paused", "pausing", "archived", "provisioning", "failed"] {
            assert!(
                !cloud_row_online(&json!({"state":state,"runtimeState":"online"})),
                "{state}"
            );
        }
    }
    #[test]
    fn cloud_machines_read_online_only_while_running() {
        assert_eq!(cloud_machine(Some("ready")), (Some("Running"), true));
        assert_eq!(cloud_machine(Some("paused")), (Some("Sleeping"), false));
        assert_eq!(cloud_machine(Some("setup")), (Some("Starting"), false));
        assert_eq!(cloud_machine(None), (None, false));
    }
}
