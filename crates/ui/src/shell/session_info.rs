//! The session card: where this chat runs (device, project, checkout,
//! branch, and the branch's pull request), a new side chat, and the
//! project's run actions, in one toggleable card. Its
//! rows follow the environment summary's idiom: 28px icon rows, 12px text,
//! a glass card with no dividers.
//! While the column has room, the card shows at its right on its own (until
//! the user hides it), the transcript and composer shifting left as needed
//! to clear it. Once clearing it would narrow the transcript, the card hides,
//! and opening it floats it over the transcript instead.
use super::*;
use crate::localization::message as tr;
use zuse_i18n::messages as msg;

use crate::project_actions::{ProjectActionsStatus, action_icon};

/// The card's width, its inset from the column's right edge, and the
/// least gap the reading column keeps from it.
pub(super) const SESSION_CARD_WIDTH: f32 = 288.0;
const SESSION_CARD_INSET: f32 = 12.0;
const SESSION_CARD_GAP: f32 = 24.0;
/// The transcript's own side gutters (its rows pad 48px each side).
const TRANSCRIPT_GUTTER: f32 = 48.0;

/// The room the transcript holds free on its right for the card docked in a
/// main column `main` wide, for a configured reading width `transcript`: none
/// while the centered column already clears the card, else just enough to
/// shift it left. `None` once that would narrow the column: the card no
/// longer fits beside it.
pub(super) fn session_card_reserve(main: f32, transcript: f32) -> Option<f32> {
    let card_left = main - SESSION_CARD_WIDTH - SESSION_CARD_INSET;
    // Centered in `main - reserve`, the column's right edge is
    // (main - reserve) / 2 + transcript / 2; keep it at `card_left - gap`.
    let reserve = (main + transcript - 2.0 * (card_left - SESSION_CARD_GAP)).max(0.0);
    (main - reserve - 2.0 * TRANSCRIPT_GUTTER >= transcript - 0.5).then_some(reserve)
}

/// The card's row height, padding and corner radius.
const SUMMARY_ROW_HEIGHT: f32 = 28.0;
const SUMMARY_CARD_PADDING: f32 = 6.0;
const SUMMARY_CARD_RADIUS: f32 = 12.0;
/// Where a row's label starts (padding + icon + gap): sub-rows indent to it.
const SUMMARY_LABEL_INSET: f32 = 34.0;

/// One summary row: 16px icon slot, 12px label, trailing meta by the caller.
pub(super) fn summary_row(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .w_full()
        .min_w_0()
        .min_h(px(SUMMARY_ROW_HEIGHT))
        .px(px(10.0))
        .rounded(px(6.0))
        .text_size(crate::typography::text_xs())
        .text_color(theme.text)
}

/// A non-interactive card row: icon slot and label.
fn info_row(theme: &Theme, glyph: AnyElement, label: SharedString) -> gpui::Div {
    summary_row(theme)
        .child(popover::picker_icon_slot(glyph))
        .child(div().flex_1().min_w_0().truncate().child(label))
}

/// A clickable card row, washed on hover.
pub(super) fn action_row(theme: &Theme, id: impl Into<SharedString>) -> gpui::Stateful<gpui::Div> {
    let wash = theme.ink(0.06);
    summary_row(theme)
        .id(id.into())
        .cursor_pointer()
        .hover(move |style| style.bg(wash))
}

/// A quiet sub-row under a section, indented to its labels.
pub(super) fn sub_row(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .min_h(px(24.0))
        .pl(px(SUMMARY_LABEL_INSET))
        .pr(px(10.0))
        .text_size(crate::typography::text_xs())
        .text_color(theme.text_muted)
}

pub(super) fn row_glyph(path: &'static str, theme: &Theme) -> AnyElement {
    icon(path)
        .size(px(16.0))
        .text_color(theme.text_muted)
        .into_any_element()
}

/// Trailing status on a row: counts, states, diff sizes.
pub(super) fn row_meta(theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(11.0))
        .text_color(theme.text_muted)
}

/// The faint rule between the workspace rows and the session's activity:
/// a hairline that fades out toward both edges.
fn fade_rule(theme: &Theme) -> gpui::Div {
    let ink = theme.text.opacity(0.15);
    let half = |from: gpui::Hsla, to: gpui::Hsla| {
        div().flex_1().h_full().bg(gpui::linear_gradient(
            90.,
            gpui::linear_color_stop(from, 0.0),
            gpui::linear_color_stop(to, 1.0),
        ))
    };
    div()
        .flex_none()
        .mx(px(10.0))
        .my(px(6.0))
        .h(px(1.0))
        .flex()
        .child(half(ink.opacity(0.0), ink))
        .child(half(ink, ink.opacity(0.0)))
}

/// A spawned subagent of this chat, from its spawn chip.
struct SummaryAgent {
    doc_id: String,
    title: SharedString,
    status: zuse_doc::SubagentStatus,
}

/// The chat's subagents in spawn order, one per subagent doc.
fn chat_agents(entries: &[zuse_doc::SessionMessageEntry]) -> Vec<SummaryAgent> {
    use zuse_doc::{MessagePart, SubagentStatus};
    let mut agents: Vec<SummaryAgent> = Vec::new();
    for part in entries.iter().flat_map(|entry| &entry.parts) {
        let MessagePart::Tool {
            call,
            is_error,
            resolved,
            subagent_ref: Some(doc_id),
            subagent_status,
            ..
        } = part
        else {
            continue;
        };
        if !call.is_subagent_spawn() {
            continue;
        }
        let status = subagent_status.unwrap_or(if *is_error {
            SubagentStatus::Failed
        } else if *resolved {
            SubagentStatus::Done
        } else {
            SubagentStatus::Running
        });
        let agent = SummaryAgent {
            doc_id: doc_id.clone(),
            title: crate::transcript::subagent_tab_title(call),
            status,
        };
        match agents.iter_mut().find(|known| known.doc_id == agent.doc_id) {
            Some(known) => *known = agent,
            None => agents.push(agent),
        }
    }
    agents
}

/// A subagent's state at a glance: a live dot, a check, or a cross.
fn agent_status_glyph(status: zuse_doc::SubagentStatus, theme: &Theme) -> AnyElement {
    use zuse_doc::SubagentStatus;
    let slot = div()
        .size(px(12.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    match status {
        SubagentStatus::Running => slot.child(
            div()
                .size(px(9.0))
                .rounded_full()
                .bg(theme.success.opacity(0.3))
                .flex()
                .items_center()
                .justify_center()
                .child(div().size(px(5.0)).rounded_full().bg(theme.success)),
        ),
        SubagentStatus::Done => {
            slot.child(icon(icons::CHECK).size(px(12.0)).text_color(theme.success))
        }
        SubagentStatus::Failed => {
            slot.child(icon(icons::CLOSE).size(px(12.0)).text_color(theme.danger))
        }
    }
    .into_any_element()
}

impl Shell {
    /// The card's reserve at the main column's LIVE width (see
    /// [`session_card_reserve`]): while the sidebar or a pane animates, the
    /// settled width would move or hide the card ahead of the column.
    fn session_info_reserve(&self, cx: &App) -> Option<f32> {
        let main = (self.viewport_width
            - self.sidebar_now()
            - self.right_visible_width(cx)
            - self.files_visible_width(cx))
        .max(0.0);
        session_card_reserve(main, crate::settings::transcript_width(cx))
    }

    /// Whether the card fits beside the transcript without narrowing it.
    pub(super) fn session_info_fits(&self, cx: &App) -> bool {
        self.session_info_reserve(cx).is_some()
    }

    /// Beside the transcript it shows unless hidden; over it, only while
    /// opened.
    pub(super) fn session_info_visible(&self, cx: &App) -> bool {
        if self.session_info_fits(cx) {
            !self.settings.session_info_hidden
        } else {
            self.session_info_overlay
        }
    }

    /// The toggle's mouse-down: note whether the card showed, so the click
    /// that follows closes it rather than reopening it. Reads the last
    /// painted state, which the overlay's click-outside dismissal (handled
    /// on the same press, in either order) leaves alone.
    pub(super) fn note_session_info_press(&mut self) {
        self.session_info_pressed_open = Some(self.session_info_shown);
    }

    pub(super) fn toggle_session_info(&mut self, cx: &mut Context<Self>) {
        let show = !self
            .session_info_pressed_open
            .take()
            .unwrap_or(self.session_info_shown);
        if self.session_info_fits(cx) {
            self.settings.session_info_hidden = !show;
            self.schedule_save(cx);
        } else {
            self.session_info_overlay = show;
        }
        cx.notify();
    }

    /// Dismiss the card floating over the transcript; one shown beside it
    /// stays put.
    pub(super) fn close_session_info_overlay(&mut self, cx: &mut Context<Self>) {
        if self.session_info_overlay {
            self.session_info_overlay = false;
            cx.notify();
        }
    }

    /// This frame's card: docked beside the transcript (else floating over
    /// it), its reveal, 0 (hidden) → 1 (shown), and the room the transcript
    /// holds free for it. The reveal eases on the shell's resize clock
    /// whenever the card shows or hides, by toggle or by the column crossing
    /// the fit, and the reserve eases with it.
    pub(super) fn session_info_frame(&mut self, has_selection: bool, cx: &App) -> (bool, f32, f32) {
        let reserve = self.session_info_reserve(cx);
        if let Some(reserve) = reserve {
            // An overlay opened on a tighter column ends once the card fits.
            self.session_info_overlay = false;
            self.session_info_last_reserve = reserve;
        }
        let shown = has_selection && self.session_info_visible(cx);
        if shown {
            self.session_info_docked = reserve.is_some();
        }
        let target = if shown { 1.0 } else { 0.0 };
        if shown != self.session_info_shown {
            let from = self.eval_tween(self.session_info_tween, 1.0 - target);
            self.session_info_tween = Some(WidthTween::new(from, target));
            self.session_info_shown = shown;
        }
        let reveal = self.eval_tween(self.session_info_tween, target);
        // Hiding as the column stops fitting, the card keeps its docked look
        // and the transcript eases back from the last room it held.
        let docked = self.session_info_docked;
        let reserve = if docked {
            reserve.unwrap_or(self.session_info_last_reserve) * reveal
        } else {
            0.0
        };
        (docked, reveal, reserve)
    }

    /// The card, positioned within the main column; `None` while closed or
    /// with no chat selected.
    pub(super) fn render_session_info(
        &mut self,
        docked: bool,
        reveal: f32,
        max_height: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if reveal <= 0.001 {
            return None;
        }
        let theme = Theme::of(cx).for_popup();
        let now = Utc::now();
        let (
            chat,
            device,
            local,
            online,
            project,
            checkout,
            branch,
            change_request,
            diff_stat,
            agents,
            background,
        ) = {
            let state = self.state.read(cx);
            let chat = state.selected_chat_row()?.clone();
            let device: SharedString = state
                .device_name(&chat.device_id)
                .unwrap_or("Unknown device")
                .to_owned()
                .into();
            let local = state.local_device_id.as_deref() == Some(chat.device_id.as_str());
            let online = state.device_online(&chat.device_id, now);
            let project = chat
                .space_id
                .as_deref()
                .and_then(|id| state.space_row(id))
                .map(|space| {
                    (
                        SharedString::from(space.display_name().to_owned()),
                        SharedString::from(space.path.clone()),
                    )
                });
            let branch = crate::change_requests::conversation_branch(&chat, &state.spaces)
                .map(str::trim)
                .filter(|branch| !branch.is_empty())
                .map(|branch| SharedString::from(branch.to_owned()));
            // Git projects name the checkout kind: the project folder itself,
            // or a worktree of it.
            let checkout = chat
                .space_id
                .as_deref()
                .and_then(|id| state.space_row(id))
                .filter(|space| space.git_detected)
                .map(|space| {
                    let cwd = chat.cwd.clone().unwrap_or_else(|| space.path.clone());
                    (cwd != space.path, SharedString::from(cwd))
                });
            let change_request = state.change_request_for_chat(&chat).cloned();
            let diff_stat = state.diff_stat_for_chat(&chat);
            let agents = chat_agents(&state.transcript);
            // Detached agents are listed with the other agents; this lists
            // processes (shells, dev servers, monitors).
            let background: Vec<zuse_protocol::BackgroundTask> =
                zuse_protocol::view::background_tasks(state.session_for(&chat.id), now)
                    .iter()
                    .filter(|task| task.kind != zuse_protocol::BackgroundTaskKind::Agent)
                    .cloned()
                    .collect();
            (
                chat,
                device,
                local,
                online,
                project,
                checkout,
                branch,
                change_request,
                diff_stat,
                agents,
                background,
            )
        };

        // ── Where it runs: the device (a remote one shows whether it is
        // reachable; its tooltip names the project folder) and the branch.
        let device_row = info_row(
            &theme,
            row_glyph(if local { icons::LAPTOP } else { icons::MONITOR }, &theme),
            device,
        )
        .id("session-info-device")
        .when_some(project.map(|(_, path)| path), |row, path| {
            row.tooltip(crate::settings::widgets::text_tooltip(path))
        })
        .when(!local, |row| {
            row.child(
                row_meta(&theme).child(div().size(px(6.0)).rounded_full().bg(if online {
                    theme.success
                } else {
                    theme.warning
                })),
            )
        });
        // Without a branch the checkout keeps a row of its own.
        let git = checkout.is_some();
        let checkout_path = checkout.as_ref().map(|(_, path)| path.clone());
        let branch_row = match branch {
            Some(branch) => Some(
                info_row(&theme, row_glyph(icons::GIT_BRANCH, &theme), branch.clone())
                    .id("session-info-branch")
                    .tooltip(crate::settings::widgets::text_tooltip(
                        checkout_path.clone().unwrap_or(branch),
                    ))
                    .into_any_element(),
            ),
            None => checkout.map(|(worktree, path)| {
                let (glyph, label) = if worktree {
                    (icons::WORKTREE, "Worktree")
                } else {
                    (icons::FOLDER, "Local checkout")
                };
                info_row(&theme, row_glyph(glyph, &theme), label.into())
                    .id("session-info-checkout")
                    .debug_selector(|| "session-info-checkout".into())
                    .tooltip(crate::settings::widgets::text_tooltip(path))
                    .into_any_element()
            }),
        };

        // ── What it changed: the checkout's diff (opens the review) and the
        // branch's pull request.
        let changes_row = git.then(|| {
            action_row(&theme, "session-info-changes")
                .debug_selector(|| "session-info-changes".into())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_session_info_overlay(cx);
                    this.open_review_tab(window, cx);
                }))
                .child(popover::picker_icon_slot(row_glyph(
                    icons::GIT_COMPARE,
                    &theme,
                )))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(tr(cx, msg::chat::environment_summary_branch_changes)),
                )
                .when_some(diff_stat, |row, (additions, deletions)| {
                    row.child(
                        sidebar_diff_stat(additions, deletions, 1.0, &theme).text_size(px(11.0)),
                    )
                })
        });
        // The branch's pull request, with its menu (view, repair, merge,
        // add to chat, status, GitHub) and its next step.
        let pull_request_rows = match change_request {
            Some(summary) => {
                let target = (!local).then(|| chat.device_id.clone());
                self.render_summary_pull_request(summary, target, &theme, cx)
            }
            None => Vec::new(),
        };

        // ── What runs for it: background processes, then the chat's
        // subagents, running ones first.
        let mut activity = self.render_session_info_background(&chat.id, background, &theme, cx);
        activity.extend(self.render_session_info_agents(&chat, agents, &theme, cx));

        // ── This conversation. Forking lives on each reply's footer.
        let busy = self.side_chat_creating;
        let side_chat = action_row(&theme, "session-info-side-chat")
            .debug_selector(|| "session-info-side-chat".into())
            .when(busy, |row| row.opacity(0.5))
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_session_info_overlay(cx);
                this.create_child_chat(None, cx)
            }))
            .child(popover::picker_icon_slot(row_glyph(icons::PLUS, &theme)))
            .child(div().flex_1().child(tr(cx, msg::native::new_side_chat)));

        // ── The project's run actions.
        let actions = self.render_session_info_actions(&theme, cx);

        let body = div()
            .id("session-info-body")
            .max_h(px(max_height.max(120.0)))
            .overflow_y_scroll()
            .p(px(SUMMARY_CARD_PADDING))
            .flex()
            .flex_col()
            .child(device_row)
            .children(branch_row)
            .children(changes_row)
            .children(pull_request_rows)
            // The session's activity sits under a rule, apart from the
            // workspace it runs in.
            .when(!activity.is_empty(), |body| {
                body.child(fade_rule(&theme)).children(activity)
            })
            .child(fade_rule(&theme))
            .child(side_chat)
            .children(actions)
            .child(
                // Its text lines up with the rows' labels.
                div()
                    .pt(px(6.0))
                    .pl(px(SUMMARY_LABEL_INSET - 16.0))
                    .pr(px(4.0))
                    .pb(px(2.0))
                    .child(self.recovery.clone()),
            );

        let card = div()
            .w(px(SESSION_CARD_WIDTH))
            .rounded(px(SUMMARY_CARD_RADIUS))
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .bg(popover::surface_bg(&theme))
            .when(!docked && !theme.is_frost(), |card| card.shadow_lg())
            .child(body);

        // Docked, the card slides in from the right beside the column;
        // floating, it drops in from just above. Either way it is placed from
        // the column's right edge, so it rides that edge while panes animate.
        let ease = crate::motion::RESIZE.progress(reveal.clamp(0.0, 1.0));
        let (dx, dy) = if docked {
            (12.0 * (1.0 - ease), 0.0)
        } else {
            (0.0, -6.0 * (1.0 - ease))
        };
        Some(
            div()
                .id("session-info")
                .debug_selector(|| "session-info".into())
                .absolute()
                .top(px(Theme::TITLEBAR_HEIGHT + 12.0 + dy))
                .right(px(SESSION_CARD_INSET - dx))
                .opacity(ease)
                .occlude()
                // Over the transcript it is a popover: a press elsewhere, or
                // choosing one of its rows, dismisses it.
                .when(!docked, |card| {
                    card.on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        // The pull request menu floats beside the card and
                        // dismisses itself first.
                        if !this.summary_pr.menu_open {
                            this.close_session_info_overlay(cx);
                        }
                    }))
                })
                .child(crate::frost::frosted(
                    SUMMARY_CARD_RADIUS,
                    crate::frost::MENU_BLUR,
                    card,
                ))
                .into_any_element(),
        )
    }

    /// A collapsible section's header: an ordinary icon row whose chevron
    /// folds the rows beneath it. Returns the row and whether it is open.
    /// `session_info_folded` records the sections toggled away from
    /// `default_open`.
    #[allow(clippy::too_many_arguments)]
    fn session_info_section(
        &mut self,
        key: &'static str,
        default_open: bool,
        glyph: AnyElement,
        label: &'static str,
        meta: Option<SharedString>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> (AnyElement, bool) {
        let open = default_open != self.session_info_folded.contains(key);
        let row = action_row(
            theme,
            SharedString::from(format!("session-info-section-{key}")),
        )
        .debug_selector(move || format!("session-info-section-{key}"))
        .role(gpui::Role::Button)
        .aria_label(label)
        .aria_expanded(open)
        .on_click(cx.listener(move |this, _, _, cx| {
            if !this.session_info_folded.remove(key) {
                this.session_info_folded.insert(key);
            }
            cx.notify();
        }))
        .child(popover::picker_icon_slot(glyph))
        .child(div().flex_1().min_w_0().truncate().child(label))
        .child(
            row_meta(theme).children(meta).child(
                icon(icons::ALT_ARROW_DOWN)
                    .size(px(12.0))
                    .text_color(theme.text_muted)
                    .with_transformation(gpui::Transformation::rotate(gpui::radians(if open {
                        0.0
                    } else {
                        -std::f32::consts::FRAC_PI_2
                    }))),
            ),
        )
        .into_any_element();
        (row, open)
    }

    /// The Background work section, folded by default: running processes
    /// first, each with its own stop control when the runtime offers one,
    /// then the recently ended ones with how they ended.
    fn render_session_info_background(
        &mut self,
        chat_id: &str,
        tasks: Vec<zuse_protocol::BackgroundTask>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if tasks.is_empty() {
            return Vec::new();
        }
        let now = Utc::now();
        let (running, ended): (Vec<_>, Vec<_>) = tasks
            .into_iter()
            .partition(zuse_protocol::BackgroundTask::running);
        // A requested stop lasts until the task leaves the live set.
        self.background_stops
            .retain(|id| running.iter().any(|task| &task.id == id));
        let (header, open) = self.session_info_section(
            "background",
            false,
            row_glyph(icons::SUMMARY_BACKGROUND, theme),
            "Background work",
            (!running.is_empty()).then(|| format!("{} running", running.len()).into()),
            theme,
            cx,
        );
        let mut rows = vec![header];
        if !open {
            return rows;
        }
        let elapsed = |task: &zuse_protocol::BackgroundTask| {
            let end = task.ended_at.unwrap_or(now);
            SharedString::from(crate::transcript::format_elapsed(
                end.signed_duration_since(task.started_at).num_seconds(),
            ))
        };
        let label = |task: &zuse_protocol::BackgroundTask| -> SharedString {
            let text = task.description.trim();
            if text.is_empty() {
                "Background process".into()
            } else {
                text.to_owned().into()
            }
        };
        let group = |text: &'static str, action: Option<AnyElement>| {
            sub_row(theme)
                .min_h(px(22.0))
                .text_size(px(11.0))
                .text_color(theme.text_muted.opacity(0.8))
                .child(div().flex_1().min_w_0().truncate().child(text))
                .children(action)
                .into_any_element()
        };
        if !running.is_empty() {
            let stoppable: Vec<String> = running
                .iter()
                .filter(|task| task.stoppable && !self.background_stops.contains(&task.id))
                .map(|task| task.id.clone())
                .collect();
            let stop_all = (stoppable.len() > 1).then(|| {
                let chat_id = chat_id.to_owned();
                div()
                    .id("session-info-background-stop-all")
                    .flex_none()
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.text))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        for id in &stoppable {
                            this.stop_background_task(&chat_id, id, cx);
                        }
                    }))
                    .child(tr(cx, msg::chat::background_stop_all))
                    .into_any_element()
            });
            rows.push(group("Running", stop_all));
            for task in &running {
                let stopping = self.background_stops.contains(&task.id);
                let control: AnyElement = if stopping {
                    crate::loaders::ring_spinner(
                        SharedString::from(format!("session-info-background-stopping-{}", task.id)),
                        12.0,
                        1.5,
                        theme.text_muted.opacity(0.25),
                        theme.text_muted,
                    )
                } else if task.stoppable {
                    let (chat_id, id) = (chat_id.to_owned(), task.id.clone());
                    let group =
                        SharedString::from(format!("session-info-background-stop-{}", task.id));
                    div()
                        .id(group.clone())
                        .group(group.clone())
                        .size(px(16.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .tooltip(crate::settings::widgets::text_tooltip("Stop"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.stop_background_task(&chat_id, &id, cx);
                        }))
                        // A filled square: the stop mark, unmistakable at
                        // this size where an outlined one reads as a checkbox.
                        .child(
                            div()
                                .size(px(8.0))
                                .rounded(px(2.0))
                                .bg(theme.text_muted)
                                .group_hover(group, |style| style.bg(theme.text)),
                        )
                        .into_any_element()
                } else {
                    agent_status_glyph(zuse_doc::SubagentStatus::Running, theme)
                };
                rows.push(
                    sub_row(theme)
                        .min_h(px(SUMMARY_ROW_HEIGHT))
                        .text_color(theme.text)
                        .gap(px(8.0))
                        .child(div().flex_1().min_w_0().truncate().child(label(task)))
                        .child(row_meta(theme).child(elapsed(task)).child(control))
                        .into_any_element(),
                );
            }
        }
        if !ended.is_empty() {
            rows.push(group("Stopped", None));
            for task in ended.iter().rev() {
                let glyph = match task.outcome {
                    Some(zuse_protocol::BackgroundTaskOutcome::Failed) => {
                        agent_status_glyph(zuse_doc::SubagentStatus::Failed, theme)
                    }
                    // Stopped on request: a quiet dash, neither success nor failure.
                    Some(zuse_protocol::BackgroundTaskOutcome::Stopped) => {
                        icon(icons::TASK_STOPPED)
                            .size(px(12.0))
                            .text_color(theme.text_muted)
                            .into_any_element()
                    }
                    _ => agent_status_glyph(zuse_doc::SubagentStatus::Done, theme),
                };
                rows.push(
                    sub_row(theme)
                        .min_h(px(SUMMARY_ROW_HEIGHT))
                        .gap(px(8.0))
                        .child(div().flex_1().min_w_0().truncate().child(label(task)))
                        .child(row_meta(theme).child(elapsed(task)).child(glyph))
                        .into_any_element(),
                );
            }
        }
        rows
    }

    /// Ask the chat's host to stop one background task: a session command,
    /// so it reaches the device the chat runs on wherever this one is.
    fn stop_background_task(&mut self, chat_id: &str, task_id: &str, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        if !self.background_stops.insert(task_id.to_owned()) {
            return;
        }
        cx.notify();
        let params = serde_json::json!({
            "chatId": chat_id,
            "command": { "kind": "stopBackgroundTask", "taskId": task_id },
        });
        let task_id = task_id.to_owned();
        cx.spawn(async move |this, cx| {
            if let Err(error) = engine.client().call(methods::QUEUE_COMMAND, params).await {
                tracing::warn!(%error, "background task stop failed");
                this.update(cx, |this, cx| {
                    this.background_stops.remove(&task_id);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// The Agents section: the chat's subagents, running first, each opening
    /// its transcript beside the chat. Settled ones past the first few fold
    /// behind "Show more".
    fn render_session_info_agents(
        &mut self,
        chat: &zuse_protocol::Chat,
        agents: Vec<SummaryAgent>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        /// Settled agents listed before the rest fold away.
        const SETTLED_PREVIEW: usize = 3;
        if agents.is_empty() {
            return Vec::new();
        }
        use zuse_doc::SubagentStatus;
        let (running, settled): (Vec<_>, Vec<_>) = agents
            .into_iter()
            .partition(|agent| agent.status == SubagentStatus::Running);
        let meta = if running.is_empty() {
            (running.len() + settled.len()).to_string()
        } else {
            format!("{} running", running.len())
        };
        let (header, open) = self.session_info_section(
            "agents",
            true,
            row_glyph(icons::SUMMARY_AGENTS, theme),
            "Agents",
            Some(meta.into()),
            theme,
            cx,
        );
        let mut rows = vec![header];
        if !open {
            return rows;
        }
        // Native subagents run inside the chat's own provider.
        let brand = chat
            .config
            .as_ref()
            .map(|config| crate::pickers::harness_brand_icon(config.harness));
        let show_all = self.session_info_folded.contains("agents-all");
        let hidden = if show_all {
            0
        } else {
            settled.len().saturating_sub(SETTLED_PREVIEW)
        };
        let visible = running
            .into_iter()
            .chain(settled.into_iter().take(if show_all {
                usize::MAX
            } else {
                SETTLED_PREVIEW
            }));
        for agent in visible {
            let chat_id = chat.id.clone();
            let doc_id = agent.doc_id.clone();
            let title = agent.title.clone();
            let settled = agent.status != SubagentStatus::Running;
            rows.push(
                action_row(
                    theme,
                    SharedString::from(format!("session-info-agent-{doc_id}")),
                )
                .when(settled, |row| row.text_color(theme.text_muted))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.close_session_info_overlay(cx);
                    this.add_subagent_surface(
                        chat_id.clone(),
                        doc_id.clone(),
                        title.to_string(),
                        settled,
                        cx,
                    );
                }))
                .child(popover::picker_icon_slot(match brand {
                    Some((path, tint)) => icon(path)
                        .size(px(14.0))
                        .text_color(tint.unwrap_or(theme.text_muted))
                        .into_any_element(),
                    None => row_glyph(icons::TOOL_AGENT, theme),
                }))
                .child(div().flex_1().min_w_0().truncate().child(agent.title))
                .child(row_meta(theme).child(agent_status_glyph(agent.status, theme)))
                .into_any_element(),
            );
        }
        if hidden > 0 {
            rows.push(
                sub_row(theme)
                    .id("session-info-agents-more")
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.text))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.session_info_folded.insert("agents-all");
                        cx.notify();
                    }))
                    .child(SharedString::from(format!("Show {hidden} more")))
                    .into_any_element(),
            );
        }
        rows
    }

    /// The Actions section: a header row with the action count, a row per
    /// project action (click runs it; the gear edits it), imports from
    /// zuse.json, and a quiet Add action under them.
    fn render_session_info_actions(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        self.ensure_project_actions(cx);
        let (Some(status), Some(snapshot), Some(key)) = (
            self.project_actions.active_status().cloned(),
            self.project_actions.visible_snapshot(),
            self.project_actions.active.clone(),
        ) else {
            return Vec::new();
        };
        let count = snapshot.actions.len();
        let (header, open) = self.session_info_section(
            "actions",
            false,
            row_glyph(icons::ACTION_PLAY, theme),
            "Actions",
            (count > 0).then(|| SharedString::from(count.to_string())),
            theme,
            cx,
        );
        let mut rows: Vec<AnyElement> = vec![header];
        if !open {
            return rows;
        }
        let note = |text: SharedString, color| {
            sub_row(theme)
                .py(px(4.0))
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        match &status {
            ProjectActionsStatus::Idle | ProjectActionsStatus::Loading => {
                rows.push(note("Loading actions…".into(), theme.text_muted));
                return rows;
            }
            ProjectActionsStatus::Unavailable { message, .. } => {
                rows.push(note(message.clone().into(), theme.text_muted));
                rows.push(
                    sub_row(theme)
                        .id("session-info-actions-retry")
                        .cursor_pointer()
                        .hover(|style| style.text_color(theme.text))
                        .on_click(cx.listener(|this, _, _, cx| this.retry_project_actions(cx)))
                        .child(tr(cx, msg::chat::attempts_retry))
                        .into_any_element(),
                );
            }
            _ => {}
        }
        let can_run = status.can_run();
        for action in snapshot.actions.clone() {
            let row_id = SharedString::from(format!("session-info-action-{}", action.id));
            let group = SharedString::from(format!("session-info-action-group-{}", action.id));
            let key = key.clone();
            let run = action.clone();
            let edit = action.clone();
            let label: SharedString = if action.run_on_worktree_create {
                format!("{} (setup)", action.name).into()
            } else {
                action.name.clone().into()
            };
            rows.push(
                action_row(theme, row_id)
                    .group(group.clone())
                    .when(!can_run, |row| row.opacity(0.5).cursor_default())
                    .when(can_run, |row| {
                        row.on_click(cx.listener(move |this, _, _, cx| {
                            this.close_session_info_overlay(cx);
                            this.run_project_action(&key, run.clone(), cx)
                        }))
                    })
                    .child(popover::picker_icon_slot(row_glyph(
                        action_icon(action.icon),
                        theme,
                    )))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "session-info-edit-action-{}",
                                edit.id
                            )))
                            .size(px(20.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .invisible()
                            .group_hover(group, |style| style.visible())
                            .hover(|style| style.bg(crate::theme::ink(0.08)))
                            .tooltip(crate::settings::widgets::text_tooltip("Edit action"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_session_info_overlay(cx);
                                this.open_project_action_editor(Some(edit.clone()), None, cx)
                            }))
                            .child(
                                icon(icons::SETTINGS_MINIMALISTIC)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
                            ),
                    )
                    .into_any_element(),
            );
        }
        for draft in snapshot.importable_actions.clone() {
            let row_id = SharedString::from(format!("session-info-import-{}", draft.name));
            let import = draft.clone();
            rows.push(
                action_row(theme, row_id)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.close_session_info_overlay(cx);
                        this.open_project_action_editor(None, Some(import.clone()), cx)
                    }))
                    .child(popover::picker_icon_slot(row_glyph(
                        action_icon(draft.icon),
                        theme,
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(draft.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(crate::typography::text_xs())
                            .text_color(theme.text_muted)
                            .child(tr(cx, msg::settings::settings_page_import)),
                    )
                    .into_any_element(),
            );
        }
        if let Some(issue) = snapshot.project_file_issue.clone() {
            rows.push(note(issue.into(), theme.text_muted));
        }
        rows.push(
            sub_row(theme)
                .id("session-info-add-action")
                .cursor_pointer()
                .hover(|style| style.text_color(theme.text))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.close_session_info_overlay(cx);
                    this.open_project_action_editor(None, None, cx)
                }))
                .child(tr(cx, msg::native::add_action))
                .into_any_element(),
        );
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_column_keeps_the_transcript_centered_beside_the_card() {
        assert_eq!(session_card_reserve(1600.0, 736.0), Some(0.0));
        // The centered column's right edge clears the card by the gap.
        let main = 1600.0;
        let card_left = main - SESSION_CARD_WIDTH - SESSION_CARD_INSET;
        assert!(main / 2.0 + 736.0 / 2.0 <= card_left - SESSION_CARD_GAP);
    }

    #[test]
    fn a_tighter_column_shifts_the_transcript_just_enough() {
        let main = 1200.0;
        let reserve = session_card_reserve(main, 736.0).expect("it still fits");
        assert!(reserve > 0.0);
        let card_left = main - SESSION_CARD_WIDTH - SESSION_CARD_INSET;
        let right = (main - reserve) / 2.0 + 736.0 / 2.0;
        assert!((right - (card_left - SESSION_CARD_GAP)).abs() < 0.01);
        // At full width: the column keeps its configured width.
        assert!(main - reserve - 2.0 * TRANSCRIPT_GUTTER >= 736.0);
    }

    #[test]
    fn the_card_stops_fitting_where_the_transcript_would_narrow() {
        // Gutter, column, gap, card, inset.
        let least =
            TRANSCRIPT_GUTTER + 736.0 + SESSION_CARD_GAP + SESSION_CARD_WIDTH + SESSION_CARD_INSET;
        assert!(session_card_reserve(least, 736.0).is_some());
        assert!(session_card_reserve(least - 1.0, 736.0).is_none());
        assert!(session_card_reserve(700.0, 736.0).is_none());
        // A narrower reading column leaves room sooner.
        assert!(session_card_reserve(least - 1.0, 560.0).is_some());
    }
}
