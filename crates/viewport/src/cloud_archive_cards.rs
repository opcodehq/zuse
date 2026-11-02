//! Cloud archive presentation uses only acknowledged workspace metadata.
use crate::localization::message as tr;
use crate::{
    assets::icon,
    palette::rgb,
    settings_style as s,
    shell::{MUTED, TEXT},
};
use gpui::{prelude::*, *};
use zuse_i18n::messages as msg;
use zuse_model::cloud::Workspace;
use zuse_protocol::cloud::{ArchivePhase, CloudArchiveStatus};

#[derive(Clone, Debug)]
pub enum Action {
    Open(String),
    Restore(String),
    Delete(String),
    RetryDelete(String),
    Cleanup(String),
    Refresh,
}
impl Action {
    pub fn target(&self) -> Option<&str> {
        match self {
            Self::Open(id)
            | Self::Restore(id)
            | Self::Delete(id)
            | Self::RetryDelete(id)
            | Self::Cleanup(id) => Some(id),
            Self::Refresh => None,
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
pub enum Access {
    Loading,
    SignedOut,
    PersonalAccount,
    Ready,
}
pub struct State<'a> {
    pub access: Access,
    pub loading: bool,
    pub busy: bool,
    pub opening: Option<&'a str>,
    pub pending: Option<&'a Action>,
    pub error: Option<&'a str>,
    pub error_target: Option<&'a str>,
}

fn status(row: &Workspace) -> (&'static str, &'static str, bool) {
    if row.state == "deleted" {
        (
            "Cloud deleted",
            if row.cleanup_failed() {
                "Cloud data is deleted. Local cleanup still needs attention."
            } else {
                "Cloud data is deleted. Finishing local cleanup."
            },
            row.cleanup_failed(),
        )
    } else if row.deleting() {
        (
            if row.delete_failed() {
                "Deletion needs attention"
            } else {
                "Deleting"
            },
            row.deletion_label(),
            row.delete_failed(),
        )
    } else if row.status_code.contains("failed")
        || row.status_code.contains("incomplete")
        || matches!(
            row.status_code.as_str(),
            "archive-metadata-invalid"
                | "archive-source-missing"
                | "archive-source-shared"
                | "cloud_archive_operation_fenced"
        )
    {
        (
            "Archive needs attention",
            "Archive preparation could not finish. Refresh to check its current state.",
            true,
        )
    } else if row.state != "archived" {
        (
            "Archiving",
            "Saving this chat before it moves to the archive.",
            false,
        )
    } else {
        ("Archived", "Saved history is available.", false)
    }
}
pub fn matches(row: &Workspace, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    row.archive_visible()
        && (query.is_empty()
            || format!(
                "{} {} {} {} {}",
                row.label(),
                row.branch,
                status(row).0,
                if row.cleanup_failed() { "cleanup" } else { "" },
                row.archive_status().map(recovery_title).unwrap_or("")
            )
            .to_lowercase()
            .contains(&query))
}
fn recovery_title(status: &CloudArchiveStatus) -> &'static str {
    if status.error_code.is_some() {
        return "Recovery needs attention";
    }
    match status.phase {
        Some(ArchivePhase::Uploading) => "Uploading backup",
        Some(ArchivePhase::Verifying) => "Verifying backup",
        Some(ArchivePhase::Retiring) => "Retiring compute",
        Some(ArchivePhase::Restoring) => "Restoring files",
        _ if status.backup_verified => "Backup verified",
        _ => "Backup not yet verified",
    }
}
fn recovery_details(row: &Workspace) -> Option<Div> {
    let status = row.archive_status()?;
    let mut details = div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .p(px(10.))
        .rounded(px(6.))
        .bg(s::frame_bg().opacity(0.5))
        .child(
            div()
                .text_size(px(12.))
                .line_height(px(16.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if status.error_code.is_some() {
                    s::destructive_text()
                } else {
                    rgb(TEXT).into()
                })
                .child(recovery_title(status)),
        )
        .child(s::description_text(if status.compute_retired {
            "Compute retired"
        } else if status.machine_retained {
            "Machine retained"
        } else {
            "No retained machine"
        }));
    if status.backup_verified && recovery_title(status) != "Backup verified" {
        details = details.child(s::description_text("Backup verified"));
    }
    if status.phase == Some(ArchivePhase::Verifying)
        && status.total_chunks > 0
        && status.verified_chunks <= status.total_chunks
    {
        details = details.child(s::description_text(format!(
            "{} of {} backup parts verified",
            status.verified_chunks, status.total_chunks
        )));
    }
    if let Some(deadline) = status
        .retention_deadline
        .and_then(chrono::DateTime::from_timestamp_millis)
    {
        details = details.child(s::description_text(format!(
            "Compute retention deadline: {}",
            deadline.format("%b %-d, %Y at %H:%M UTC")
        )));
        if !status.compute_retired {
            details = details.child(s::description_text(
                "Retirement waits for a verified backup.",
            ));
        }
    }
    if status.error_code.is_some() {
        details = details.child(s::description_text("Refresh to check this archive again."));
    }
    Some(details)
}
fn control<C: 'static>(
    key: &'static str,
    index: usize,
    label: impl Into<SharedString>,
    enabled: bool,
    danger: bool,
    action: Action,
    handler: fn(&mut C, Action, &mut Window, &mut Context<C>),
    cx: &Context<C>,
) -> Stateful<Div> {
    let label = label.into();
    div()
        .id((key, index))
        .role(Role::Button)
        .tab_index(0)
        .aria_label(label.clone())
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .h(px(28.))
        .px(px(12.))
        .rounded(px(7.))
        .border_1()
        .border_color(s::card_border())
        .bg(s::card_bg())
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(if danger {
            s::destructive_text()
        } else {
            rgb(TEXT).into()
        })
        .when(enabled, |v| {
            v.cursor_pointer().hover(|v| v.bg(s::frame_bg()))
        })
        .when(!enabled, |v| v.opacity(0.5))
        .child(label)
        .on_click(cx.listener(move |this, _, window, cx| {
            if enabled {
                handler(this, action.clone(), window, cx);
            }
        }))
}
fn error(key: &'static str, text: &str) -> Stateful<Div> {
    div()
        .id(key)
        .role(Role::Alert)
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(s::destructive_text())
        .child(text.to_owned())
}
pub fn panel<C: 'static>(
    rows: &[Workspace],
    query: &str,
    state: State<'_>,
    handler: fn(&mut C, Action, &mut Window, &mut Context<C>),
    cx: &Context<C>,
) -> Div {
    let rows = rows
        .iter()
        .filter(|row| matches(row, query))
        .collect::<Vec<_>>();
    let ready = state.access == Access::Ready;
    let enabled = ready && !state.loading && !state.busy && state.opening.is_none();
    let refresh = control(
        "cloud-archive-refresh",
        0,
        if state.loading {
            "Refreshing…"
        } else {
            "Refresh"
        },
        enabled,
        false,
        Action::Refresh,
        handler,
        cx,
    );
    let title = if ready && !rows.is_empty() {
        format!("Cloud archives · {}", rows.len())
    } else {
        "Cloud archives".into()
    };
    let mut body = div().flex().flex_col().gap(px(8.));
    let gate = match state.access {
        Access::Loading => Some("Loading your account…"),
        Access::SignedOut => Some("Sign in through General settings to view your Cloud archives."),
        Access::PersonalAccount => {
            Some("Switch to your personal account in General settings to view Cloud archives.")
        }
        Access::Ready => None,
    };
    if let Some(text) = gate {
        body = body.child(s::description_text(text).p(px(12.)));
    } else {
        if let Some(message) = state.error.filter(|_| {
            !rows
                .iter()
                .any(|row| Some(row.workspace_id.as_str()) == state.error_target)
        }) {
            body = body.child(
                error("cloud-archive-load-error", message)
                    .px(px(12.))
                    .py(px(8.)),
            );
        }
        if rows.is_empty() {
            body = body.child(
                s::description_text(if state.loading {
                    "Loading Cloud archives…"
                } else if state.error.is_some() {
                    "Refresh to load your Cloud archives."
                } else if query.trim().is_empty() {
                    "No archived Cloud chats."
                } else {
                    "No Cloud archives match your search."
                })
                .p(px(12.)),
            );
        }
        for (index, row) in rows.into_iter().enumerate() {
            let id = &row.workspace_id;
            let (label, description, attention) = status(row);
            let deleted = row.state == "deleted";
            let deleting = row.deleting();
            let opening = state.opening == Some(id.as_str());
            let pending = state
                .pending
                .filter(|action| action.target() == Some(id.as_str()));
            let restoring = matches!(pending, Some(Action::Restore(_)));
            let deleting_request =
                matches!(pending, Some(Action::Delete(_) | Action::RetryDelete(_)));
            let cleaning = matches!(pending, Some(Action::Cleanup(_)));
            let mut actions = div().flex().justify_end().gap(px(8.));
            if !deleting && !deleted {
                actions = actions
                    .child(control(
                        "cloud-archive-open",
                        index,
                        if opening {
                            "Opening…"
                        } else {
                            "View history"
                        },
                        enabled,
                        false,
                        Action::Open(id.clone()),
                        handler,
                        cx,
                    ))
                    .child(control(
                        "cloud-archive-restore",
                        index,
                        if restoring {
                            "Restoring…"
                        } else {
                            "Restore chat"
                        },
                        enabled && row.state == "archived",
                        false,
                        Action::Restore(id.clone()),
                        handler,
                        cx,
                    ));
            }
            if !deleted {
                actions = actions.child(control(
                    "cloud-archive-delete",
                    index,
                    if deleting_request {
                        "Deleting…"
                    } else if row.delete_failed() {
                        "Retry deletion"
                    } else if deleting {
                        "Deleting…"
                    } else {
                        "Delete…"
                    },
                    enabled && (!deleting || row.delete_failed()),
                    true,
                    if row.delete_failed() {
                        Action::RetryDelete(id.clone())
                    } else {
                        Action::Delete(id.clone())
                    },
                    handler,
                    cx,
                ));
            }
            if row.cleanup_failed() {
                actions = actions.child(control(
                    "cloud-archive-cleanup",
                    index,
                    if cleaning {
                        "Retrying…"
                    } else {
                        "Retry local cleanup"
                    },
                    enabled,
                    false,
                    Action::Cleanup(id.clone()),
                    handler,
                    cx,
                ));
            }
            let mut card = div()
                .id(("cloud-archive-card", index))
                .flex()
                .flex_col()
                .gap(px(8.))
                .p(px(12.))
                .rounded(px(8.))
                .border_1()
                .border_color(s::card_border())
                .bg(s::card_bg())
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(10.))
                        .child(
                            div()
                                .size(px(32.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(8.))
                                .bg(s::frame_bg())
                                .child(icon("cloud").size(px(16.)).text_color(rgb(MUTED))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(13.))
                                        .line_height(px(18.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(TEXT))
                                        .child(row.label().to_owned()),
                                )
                                .when(!row.branch.is_empty(), |v| {
                                    v.child(
                                        s::description_text(format!("Branch {}", row.branch))
                                            .text_size(px(11.))
                                            .truncate(),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex_none()
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(5.))
                                .bg(s::frame_bg())
                                .text_size(px(10.))
                                .line_height(px(16.))
                                .text_color(if attention {
                                    s::destructive_text()
                                } else {
                                    rgb(MUTED).into()
                                })
                                .child(label),
                        ),
                )
                .child(
                    s::description_text(description)
                        .text_size(px(12.))
                        .line_height(px(16.)),
                );
            card = card.children(recovery_details(row));
            if let Some(cleanup) = &row.local_mirror_cleanup {
                if cleanup.state == "pending" {
                    card = card.child(
                        s::description_text(tr(cx, msg::native::cleaning_up_local_files))
                            .text_size(px(12.)),
                    );
                } else if cleanup.state == "error" {
                    card = card.child(error(
                        "cloud-archive-cleanup-error",
                        cleanup
                            .error
                            .as_deref()
                            .unwrap_or("Local cleanup could not finish. Retry to continue."),
                    ));
                }
                if cleanup.removed > 0 || cleanup.preserved > 0 {
                    card = card.child(
                        s::description_text(format!(
                            "Local files: {} removed · {} kept",
                            cleanup.removed, cleanup.preserved
                        ))
                        .text_size(px(11.)),
                    );
                }
            }
            if let Some(message) = state
                .error
                .filter(|_| state.error_target == Some(id.as_str()))
            {
                card = card.child(error("cloud-archive-action-error", message));
            }
            body = body.child(card.child(actions));
        }
    }
    s::frame()
        .child(s::header_with_action(
            title,
            "Saved conversations and cleanup progress. Restored chats stay paused.",
            Some(refresh.into_any_element()),
        ))
        .child(body)
}

/// Keep the root conversation available while switching saved sessions.
pub fn sessions<C: 'static>(
    chats: &[zuse_protocol::Chat],
    selected: &str,
    busy: bool,
    handler: fn(&mut C, String, &mut Window, &mut Context<C>),
    cx: &Context<C>,
) -> Stateful<Div> {
    div()
        .id("cloud-archive-sessions")
        .flex_none()
        .h(px(44.))
        .flex()
        .items_stretch()
        .px(px(12.))
        .gap(px(4.))
        .border_b_1()
        .border_color(s::card_border())
        .overflow_x_scroll()
        .children(chats.iter().enumerate().map(|(index, chat)| {
            let id = chat.id.clone();
            let title = chat
                .title
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("Conversation {}", index + 1));
            div()
                .id(("cloud-archive-session", index))
                .role(Role::Button)
                .tab_index(0)
                .aria_label(title.clone())
                .flex_none()
                .flex()
                .items_center()
                .max_w(px(224.))
                .px(px(12.))
                .border_b_2()
                .border_color(if chat.id == selected {
                    rgb(TEXT).into()
                } else {
                    transparent_black()
                })
                .text_size(px(12.))
                .text_color(if chat.id == selected {
                    rgb(TEXT)
                } else {
                    rgb(MUTED)
                })
                .when(!busy, |v| v.cursor_pointer().hover(|v| v.bg(s::frame_bg())))
                .when(busy, |v| v.opacity(0.5))
                .child(div().truncate().child(title))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !busy {
                        handler(this, id.clone(), window, cx);
                    }
                }))
        }))
}
