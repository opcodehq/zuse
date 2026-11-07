//! Message badges: pills standing in for structured context a user message
//! carries.
//!
//! A feature stages context on the composer, folds it into the prompt as plain
//! text, and registers an [`Extractor`]. [`split`] lifts that block back out of
//! the sent message so the transcript draws a pill instead of the bullets the
//! agent reads. Nothing here knows what a diff comment is.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, Entity, SharedString, Task, Window, div, prelude::*, px};

use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageBadge {
    /// An `icons::*` asset name.
    pub icon: &'static str,
    pub label: SharedString,
    /// Empty means the label says everything and the pill carries no card.
    pub details: Vec<BadgeDetail>,
    /// Markdown the card renders in place of `details`.
    pub markdown: Option<SharedString>,
}

/// One row of a hover card. Three generic slots, so a new badge kind fills them
/// without this module learning what it cites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadgeDetail {
    /// `src/main.rs:42`, a URL, a commit subject.
    pub location: SharedString,
    /// `L`/`R` for a diff side, a status, a count.
    pub tag: Option<SharedString>,
    pub body: SharedString,
}

/// Returns the text with this feature's blocks removed, plus the pills
/// replacing them. `None` when the message carries nothing of that kind.
pub type Extractor = fn(&str) -> Option<(String, Vec<MessageBadge>)>;

fn comment_badges(text: &str) -> Option<(String, Vec<MessageBadge>)> {
    crate::comments::extract_badge(text).map(|(text, badge)| (text, vec![badge]))
}

/// Leading context blocks come off first; the comment block trails the
/// user's text.
const EXTRACTORS: &[Extractor] = &[crate::context_items::extract_badges, comment_badges];

/// Each extractor sees what the previous ones left behind, so two features can
/// ride the same prompt.
pub fn split(text: &str) -> (String, Vec<MessageBadge>) {
    let mut rest = text.to_string();
    let mut badges = Vec::new();
    for extract in EXTRACTORS {
        if let Some((stripped, lifted)) = extract(&rest) {
            rest = stripped;
            badges.extend(lifted);
        }
    }
    (rest, badges)
}

pub const BADGE_HEIGHT: f32 = 28.0;

const ICON_SIZE: f32 = 14.0;
const TEXT_SIZE: f32 = 12.0;
const CARD_WIDTH: f32 = 416.0;
const CARD_MAX_HEIGHT: f32 = 320.0;
const HOVER_DELAY: Duration = Duration::from_millis(200);
/// Grace for the pointer to cross from the pill into its card.
const LEAVE_DELAY: Duration = Duration::from_millis(150);

/// The chip shell every pill above a user bubble shares: context items,
/// attachments, files.
pub fn pill(id: impl Into<gpui::ElementId>, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(BADGE_HEIGHT))
        .max_w_full()
        .min_w_0()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .px(px(10.0))
        .rounded_full()
        .border_1()
        .border_color(theme.border.opacity(0.5))
        .bg(theme.muted_fill().opacity(0.6))
        .text_size(px(TEXT_SIZE))
        .text_color(theme.text)
}

/// Hover/click state for one pill's card.
#[derive(Default)]
struct BadgePopover {
    open: bool,
    /// Opened by a click: stays until clicked again or dismissed outside.
    pinned: bool,
    trigger_hovered: bool,
    card_hovered: bool,
    task: Option<Task<()>>,
    tree: Option<Arc<crate::markdown::BlockTree>>,
}

impl BadgePopover {
    fn schedule(&mut self, open: bool, delay: Duration, cx: &mut gpui::Context<Self>) {
        self.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |popover, cx| {
                popover.task = None;
                if open {
                    popover.open = popover.trigger_hovered || popover.card_hovered;
                } else if !popover.pinned && !popover.trigger_hovered && !popover.card_hovered {
                    popover.open = false;
                }
                cx.notify();
            });
        }));
    }

    fn hover_trigger(&mut self, hovered: bool, cx: &mut gpui::Context<Self>) {
        self.trigger_hovered = hovered;
        if hovered && !self.open {
            self.schedule(true, HOVER_DELAY, cx);
        } else if !hovered && self.open && !self.pinned {
            self.schedule(false, LEAVE_DELAY, cx);
        } else if !hovered {
            self.task = None;
        }
    }

    fn hover_card(&mut self, hovered: bool, cx: &mut gpui::Context<Self>) {
        self.card_hovered = hovered;
        if !hovered && !self.pinned {
            self.schedule(false, LEAVE_DELAY, cx);
        }
    }

    fn toggle(&mut self, cx: &mut gpui::Context<Self>) {
        self.task = None;
        if self.open && self.pinned {
            self.open = false;
            self.pinned = false;
        } else {
            self.open = true;
            self.pinned = true;
        }
        cx.notify();
    }

    fn dismiss(&mut self, cx: &mut gpui::Context<Self>) {
        self.task = None;
        self.open = false;
        self.pinned = false;
        cx.notify();
    }
}

/// `id` scopes the card's state and must be stable per rendered pill. Hovering
/// opens the card after a beat; a click pins it open or closes it.
pub fn render(
    id: impl Into<SharedString>,
    badge: &MessageBadge,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    render_with(id, badge, theme, None, window, cx)
}

/// [`render`] with `trailing` (a remove ×) inside the pill, after the label.
pub fn render_with(
    id: impl Into<SharedString>,
    badge: &MessageBadge,
    theme: &Theme,
    trailing: Option<AnyElement>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id: SharedString = id.into();
    let has_trailing = trailing.is_some();
    let has_card = !badge.details.is_empty() || badge.markdown.is_some();
    let body = div()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(
            crate::icons::icon(badge.icon)
                .flex_none()
                .size(px(ICON_SIZE))
                .text_color(theme.text_muted),
        )
        .child(div().min_w_0().truncate().child(badge.label.clone()));
    let body = div()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(2.0))
        .child(body)
        .children(trailing);
    if !has_card {
        return pill(id, theme)
            .when(has_trailing, |pill| pill.pr(px(4.0)))
            .child(body)
            .into_any_element();
    }
    let popover = window.use_keyed_state(
        SharedString::from(format!("{id}-popover")),
        cx,
        |_, _| BadgePopover::default(),
    );
    let open = popover.read(cx).open;
    let card = open.then(|| card(&id, badge, &popover, theme, window, cx));
    let hover_state = popover.clone();
    let click_state = popover.clone();
    div()
        .relative()
        .max_w_full()
        .min_w_0()
        .flex_none()
        .child(
            pill(id.clone(), theme)
                .when(has_trailing, |pill| pill.pr(px(4.0)))
                .cursor_pointer()
                .role(gpui::Role::Button)
                .aria_label(badge.label.clone())
                .on_hover(move |hovered, _, cx| {
                    hover_state.update(cx, |popover, cx| popover.hover_trigger(*hovered, cx))
                })
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    click_state.update(cx, |popover, cx| popover.toggle(cx))
                })
                .child(body),
        )
        .children(card.map(|card| {
            crate::popover::anchored_menu_above(format!("{id}-card"), card, None)
        }))
        .into_any_element()
}

fn card(
    id: &SharedString,
    badge: &MessageBadge,
    popover: &Entity<BadgePopover>,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = &theme.for_popup();
    let viewport = f32::from(window.viewport_size().width);
    let width = CARD_WIDTH.min(viewport - 32.0).max(120.0);
    let content: AnyElement = match &badge.markdown {
        Some(markdown) => {
            let tree = popover.update(cx, |popover, _| {
                popover
                    .tree
                    .get_or_insert_with(|| {
                        Arc::new(crate::markdown::parse_full(
                            &crate::context_items::display_markdown(markdown),
                        ))
                    })
                    .clone()
            });
            let options = crate::markdown::render::RenderOptions {
                tasks: None,
                media: None,
                row_key: format!("{id}-md").into(),
                veil: None,
                cache: None,
                now: Instant::now(),
                copy: None,
                link: None,
                workspace_root: None,
                code: None,
            };
            div()
                .min_w_0()
                .px(px(4.0))
                .child(crate::markdown::render::render_tree(
                    &tree,
                    &options,
                    theme,
                    window,
                    &|_| None,
                ))
                .into_any_element()
        }
        None => div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .children(badge.details.iter().map(|detail| detail_row(detail, theme)))
            .into_any_element(),
    };
    let hover_state = popover.clone();
    let out_state = popover.clone();
    crate::popover::popover_card(theme)
        .id(SharedString::from(format!("{id}-card-surface")))
        .debug_selector(|| "message-badge-card".into())
        .w(px(width))
        .p(px(8.0))
        .on_hover(move |hovered, _, cx| {
            hover_state.update(cx, |popover, cx| popover.hover_card(*hovered, cx))
        })
        .on_mouse_down_out(move |_, _, cx| {
            // A press on the pill itself toggles; everything else dismisses.
            if !out_state.read(cx).trigger_hovered {
                out_state.update(cx, |popover, cx| popover.dismiss(cx));
            }
        })
        .child(
            div()
                .id(SharedString::from(format!("{id}-card-scroll")))
                .max_h(px(CARD_MAX_HEIGHT))
                .overflow_y_scroll()
                .child(content),
        )
        .into_any_element()
}

fn detail_row(detail: &BadgeDetail, theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .gap(px(8.0))
        .p(px(8.0))
        .rounded(px(6.0))
        .bg(crate::theme::ink(0.05))
        .child(
            div()
                .flex_none()
                .w(px(2.0))
                .rounded(px(1.0))
                .bg(theme.solid.opacity(0.35)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .font_family(theme.font_mono.clone())
                        .text_size(px(10.0))
                        .text_color(theme.text_faint)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(detail.location.clone()),
                        )
                        .children(detail.tag.clone().map(|tag| {
                            div()
                                .flex_none()
                                .px(px(4.0))
                                .rounded(px(3.0))
                                .bg(crate::theme::ink(0.10))
                                .child(tag)
                        })),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(TEXT_SIZE))
                        .line_height(px(16.0))
                        .text_color(theme.text)
                        .child(detail.body.clone()),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comments::{CommentSide, ReviewComment, with_comments};

    #[test]
    fn a_plain_message_carries_no_badges() {
        let (text, badges) = split("just a prompt");
        assert_eq!(text, "just a prompt");
        assert!(badges.is_empty());
    }

    #[test]
    fn a_sent_comment_block_becomes_one_pill() {
        let staged = vec![
            ReviewComment::new("a.rs", CommentSide::New, 3, "fix"),
            ReviewComment::new("b.rs", CommentSide::Old, 9, "why"),
        ];
        let (text, badges) = split(&with_comments("do", &staged));
        assert_eq!(text, "do");
        assert_eq!(badges.len(), 1);
        assert_eq!(badges[0].label.as_ref(), "2 comments");
    }

    #[test]
    fn the_card_carries_one_row_per_comment() {
        let staged = vec![
            ReviewComment::new("src/main.rs", CommentSide::New, 42, "early-return here"),
            ReviewComment::new("src/lib.rs", CommentSide::Old, 7, "why was this dropped?"),
        ];
        let (_, badges) = split(&with_comments("look", &staged));
        let details = &badges[0].details;
        assert_eq!(details.len(), 2);
        assert_eq!(details[0].location.as_ref(), "src/main.rs:42");
        assert_eq!(details[0].tag.as_deref(), Some("R"));
        assert_eq!(details[0].body.as_ref(), "early-return here");
        assert_eq!(details[1].location.as_ref(), "src/lib.rs:7");
        assert_eq!(details[1].tag.as_deref(), Some("L"));
    }

    #[test]
    fn a_multiline_body_rejoins_its_continuation_lines() {
        let staged = vec![ReviewComment::new(
            "a.rs",
            CommentSide::New,
            3,
            "first\nsecond",
        )];
        let (_, badges) = split(&with_comments("x", &staged));
        assert_eq!(badges[0].details[0].body.as_ref(), "first\nsecond");
    }

    #[test]
    fn a_path_with_a_colon_still_splits_on_the_side_marker() {
        let staged = vec![ReviewComment::new("odd:name.rs", CommentSide::New, 5, "hm")];
        let (_, badges) = split(&with_comments("x", &staged));
        assert_eq!(badges[0].details[0].location.as_ref(), "odd:name.rs:5");
        assert_eq!(badges[0].details[0].body.as_ref(), "hm");
    }

    #[test]
    fn a_comment_only_send_keeps_its_stand_in_body() {
        let staged = vec![ReviewComment::new("a.rs", CommentSide::New, 3, "fix")];
        let (text, badges) = split(&with_comments("", &staged));
        assert_eq!(text, crate::comments::COMMENT_ONLY_TEXT);
        assert_eq!(badges[0].label.as_ref(), "1 comment");
    }
}
