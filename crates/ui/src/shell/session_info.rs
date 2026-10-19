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
fn summary_row(theme: &Theme) -> gpui::Div {
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
fn action_row(theme: &Theme, id: impl Into<SharedString>) -> gpui::Stateful<gpui::Div> {
    let wash = theme.ink(0.06);
    summary_row(theme)
        .id(id.into())
        .cursor_pointer()
        .hover(move |style| style.bg(wash))
}

/// A quiet sub-row under a section, indented to its labels.
fn sub_row(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .min_h(px(24.0))
        .pl(px(SUMMARY_LABEL_INSET))
        .pr(px(10.0))
        .text_size(crate::typography::text_xs())
        .text_color(theme.text_muted)
}

fn row_glyph(path: &'static str, theme: &Theme) -> AnyElement {
    icon(path)
        .size(px(16.0))
        .text_color(theme.text_muted)
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
    fn close_session_info_overlay(&mut self, cx: &mut Context<Self>) {
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
        let (chat, device, local, online, project, checkout, branch, change_request) = {
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
            (
                chat,
                device,
                local,
                online,
                project,
                checkout,
                branch,
                change_request,
            )
        };

        // ── Where it runs.
        let device_row = info_row(
            &theme,
            row_glyph(if local { icons::LAPTOP } else { icons::MONITOR }, &theme),
            device,
        )
        .when(!online, |row| {
            row.child(
                icon(icons::WIFI_OFF)
                    .size(px(12.0))
                    .flex_none()
                    .text_color(theme.warning.opacity(0.8)),
            )
        });
        let project_row = match project {
            Some((name, path)) => info_row(
                &theme,
                self.render_project_icon(&chat.id, 16.0, false, cx),
                name,
            )
            .id("session-info-project")
            .tooltip(crate::settings::widgets::text_tooltip(path))
            .into_any_element(),
            None => info_row(&theme, row_glyph(icons::HOME, &theme), "No project".into())
                .into_any_element(),
        };
        let checkout_row = checkout.map(|(worktree, path)| {
            let (glyph, label) = if worktree {
                (icons::WORKTREE, "Worktree")
            } else {
                (icons::FOLDER, "Local checkout")
            };
            info_row(&theme, row_glyph(glyph, &theme), label.into())
                .id("session-info-checkout")
                .debug_selector(|| "session-info-checkout".into())
                .tooltip(crate::settings::widgets::text_tooltip(path))
        });
        let branch_row = branch.map(|branch| {
            info_row(&theme, row_glyph(icons::GIT_BRANCH, &theme), branch.clone())
                .id("session-info-branch")
                .tooltip(crate::settings::widgets::text_tooltip(branch))
        });
        // The branch's pull request: its title, and the state-toned number
        // badge. The row (or the badge) opens it.
        let pull_request_row = change_request.map(|summary| {
            let model = crate::change_requests::ChangeRequestBadgeModel::from_summary(&summary);
            let url = summary.url.clone();
            let device = Some(chat.device_id.clone());
            let open_device = device.clone();
            action_row(&theme, "session-info-pull-request")
                .debug_selector(|| "session-info-pull-request".into())
                .role(gpui::Role::Link)
                .aria_label(SharedString::from(format!(
                    "Open pull request {}: {}",
                    model.number, model.title
                )))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close_session_info_overlay(cx);
                    crate::pull_request_detail::open_on_device(
                        &url,
                        open_device.clone(),
                        window,
                        cx,
                    );
                }))
                .child(popover::picker_icon_slot(
                    icon(model.tone.glyph())
                        .size(px(16.0))
                        .text_color(model.tone.color(&theme))
                        .into_any_element(),
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(model.title.clone()),
                )
                .child(crate::change_requests::pull_request_badge(
                    "session-info-pull-request-badge".into(),
                    summary,
                    crate::change_requests::ChangeRequestBadgeSurface::Composer,
                    device,
                    &theme,
                ))
        });

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
            .child(div().flex_1().child("New side chat"));

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
            .child(project_row)
            .children(checkout_row)
            .children(branch_row)
            .children(pull_request_row)
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
                    card.on_mouse_down_out(
                        cx.listener(|this, _, _, cx| this.close_session_info_overlay(cx)),
                    )
                })
                .child(crate::frost::frosted(
                    SUMMARY_CARD_RADIUS,
                    crate::frost::MENU_BLUR,
                    card,
                ))
                .into_any_element(),
        )
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
        let mut rows: Vec<AnyElement> = vec![
            info_row(
                theme,
                row_glyph(icons::ACTION_PLAY, theme),
                "Actions".into(),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(crate::typography::text_xs())
                    .text_color(theme.text_muted)
                    .when(count > 0, |meta| {
                        meta.child(SharedString::from(count.to_string()))
                    }),
            )
            .into_any_element(),
        ];
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
                        .child("Retry")
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
                            .child("Import"),
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
                .child("Add action")
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
