//! The find-in-chat bar: "n of m" over the transcript rows, Enter and
//! Shift+Enter to step, highlights painted by the markdown renderer.
use super::*;
use crate::chat_find::{self, FindMatch};
use crate::popover;
use gpui::{Focusable, KeyDownEvent};
use zuse_viewport::input::{Changed, ShiftSubmitted, Submitted, TextInput};

pub(super) struct FindBar {
    input: Entity<TextInput>,
    needle: Vec<char>,
    matches: Vec<FindMatch>,
    active: Option<usize>,
    /// Per-row match counts for `needle`, keyed by the row's content
    /// identity so streaming and remounts only rescan what changed.
    counts: HashMap<SharedString, (usize, usize)>,
    _subscriptions: [Subscription; 3],
}

impl FindBar {
    /// An identity for a row's searchable content; `None` for rows that
    /// show no prose of their own (tools, chips, cards).
    fn row_identity(row: &Row) -> Option<usize> {
        match &row.kind {
            RowKind::User { text, .. } => Some(text.as_ptr() as usize ^ text.len()),
            RowKind::Markdown { tree, block_ix } | RowKind::LiveMarkdown { tree, block_ix } => tree
                .blocks
                .get(*block_ix)
                .map(|top| Arc::as_ptr(top) as usize),
            _ => None,
        }
    }

    /// The text elements a row paints, in order.
    fn row_texts(row: &Row) -> Vec<String> {
        let mut texts = Vec::new();
        match &row.kind {
            RowKind::User { text, .. } => texts.push(text.to_string()),
            RowKind::Markdown { tree, block_ix } | RowKind::LiveMarkdown { tree, block_ix } => {
                if let Some(top) = tree.blocks.get(*block_ix) {
                    chat_find::block_texts(&top.block, &mut texts);
                }
            }
            _ => {}
        }
        texts
    }
}

impl Transcript {
    pub fn find_open(&self) -> bool {
        self.find.is_some()
    }

    /// Open the bar, or focus and select its query when it is already open.
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(find) = &self.find {
            let input = find.input.clone();
            window.focus(&input.read(cx).focus_handle(cx), cx);
            window.dispatch_action(Box::new(gpui_base::input::SelectAll), cx);
            return;
        }
        let placeholder = tr(cx, msg::chat::find_in_chat_placeholder);
        let input = cx.new(|cx| TextInput::new(&placeholder, window, cx));
        let subscriptions = [
            cx.subscribe(&input, |this, _, _: &Changed, cx| {
                this.refresh_find(true, cx)
            }),
            cx.subscribe(&input, |this, _, _: &Submitted, cx| this.step_find(1, cx)),
            cx.subscribe(&input, |this, _, _: &ShiftSubmitted, cx| {
                this.step_find(-1, cx)
            }),
        ];
        input.update(cx, |input, cx| input.focus(cx));
        self.find = Some(FindBar {
            input,
            needle: Vec::new(),
            matches: Vec::new(),
            active: None,
            counts: HashMap::new(),
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    pub fn close_find(&mut self, cx: &mut Context<Self>) {
        if self.find.take().is_some() {
            chat_find::clear_highlight(cx.entity_id().as_u64());
            cx.emit(TranscriptEvent::FindClosed);
            cx.notify();
        }
    }

    /// Recompute the matches. A new query lands on the first match at or
    /// below what the reader is looking at; otherwise the active match
    /// follows its row as the chat changes.
    pub(super) fn refresh_find(&mut self, query_changed: bool, cx: &mut Context<Self>) {
        let top_row = self.list.logical_scroll_top().item_ix;
        let Some(find) = self.find.as_mut() else {
            return;
        };
        if query_changed {
            let needle = chat_find::normalize_query(&find.input.read(cx).value(cx));
            if needle == find.needle {
                return;
            }
            find.needle = needle;
            find.counts.clear();
        }
        let previous_index = find.active;
        let previous = previous_index.and_then(|ix| find.matches.get(ix)).cloned();
        let mut matches = Vec::new();
        let mut live = HashSet::new();
        if !find.needle.is_empty() {
            for (row_ix, row) in self.rows.iter().enumerate() {
                let Some(identity) = FindBar::row_identity(row) else {
                    continue;
                };
                live.insert(row.id.clone());
                let count = match find.counts.get(&row.id) {
                    Some((cached, count)) if *cached == identity => *count,
                    _ => {
                        let texts = FindBar::row_texts(row);
                        let count = chat_find::count_matches(&texts, &find.needle);
                        find.counts.insert(row.id.clone(), (identity, count));
                        count
                    }
                };
                matches.extend((0..count).map(|ordinal| FindMatch {
                    row_ix,
                    row_id: row.id.clone(),
                    ordinal,
                }));
            }
        }
        find.counts.retain(|id, _| live.contains(id));
        let changed = matches != find.matches;
        find.active = if query_changed {
            let from = previous.as_ref().map_or(top_row, |m| m.row_ix);
            chat_find::first_index_from_row(from, &matches)
        } else {
            chat_find::resolve_active(previous.as_ref(), previous_index, &matches)
        };
        find.matches = matches;
        if query_changed {
            self.reveal_find_match(cx);
        }
        if changed || query_changed {
            cx.notify();
        }
    }

    pub(super) fn step_find(&mut self, direction: isize, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        let current = find.active.map_or(-1, |ix| ix as isize);
        let start = if find.active.is_none() && direction < 0 {
            0
        } else {
            current
        };
        find.active = chat_find::wrap_index(start + direction, find.matches.len());
        self.reveal_find_match(cx);
        cx.notify();
    }

    fn reveal_find_match(&mut self, cx: &mut Context<Self>) {
        let Some(row_ix) = self
            .find
            .as_ref()
            .and_then(|find| find.active.and_then(|ix| find.matches.get(ix)))
            .map(|m| m.row_ix)
        else {
            return;
        };
        self.begin_scroll_navigation();
        self.list.scroll_to_reveal_item(row_ix);
        cx.notify();
    }

    /// The bar's overlay, and this frame's highlight for the painter.
    pub(super) fn render_find_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let surface = cx.entity_id().as_u64();
        if self.find.is_some() {
            // Rows may have changed since the last frame (streaming, replay).
            self.refresh_find(false, cx);
        }
        let Some(find) = self.find.as_ref() else {
            chat_find::clear_highlight(surface);
            return None;
        };
        let theme = Theme::of(cx).for_popup();
        let total = find.matches.len();
        let active = find.active.and_then(|ix| find.matches.get(ix));
        chat_find::set_highlight(chat_find::Highlight {
            surface,
            needle: find.needle.clone(),
            active: active.map(|m| (m.row_id.clone(), m.ordinal)),
            wash: theme.accent.opacity(0.26),
            active_wash: theme.accent.opacity(0.78),
        });
        let count: SharedString = if find.needle.is_empty() {
            SharedString::default()
        } else if total == 0 {
            tr(cx, msg::chat::find_in_chat_no_results).into()
        } else {
            let current = find.active.map_or(0, |ix| ix + 1);
            tr(cx, |l10n| {
                msg::chat::find_in_chat_count(l10n, current as i64, total as i64)
            })
            .into()
        };
        let button = |id: &'static str, glyph: &'static str, label: String, enabled: bool| {
            div()
                .id(id)
                .flex_none()
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.0))
                .role(gpui::Role::Button)
                .aria_label(label.clone())
                .tooltip(crate::settings::widgets::text_tooltip(label))
                .when(enabled, |el| {
                    el.cursor_pointer()
                        .hover(|style| style.bg(theme.element_hover))
                })
                .child(
                    crate::icons::icon(glyph)
                        .size(px(16.0))
                        .text_color(if enabled {
                            theme.text_muted
                        } else {
                            theme.text_faint
                        }),
                )
        };
        let has_matches = total > 0;
        let bar = popover::popover_card(&theme)
            .id("chat-find-bar")
            .key_context("ChatFind")
            .occlude()
            .h(px(36.0))
            .rounded(px(12.0))
            .p(px(4.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close_find(cx);
                }
            }))
            .child(
                div()
                    .w(px(176.0))
                    .min_w_0()
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .px(px(8.0))
                    .rounded(px(8.0))
                    .bg(crate::theme::ink(0.04))
                    .child(find.input.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .min_w(px(56.0))
                    .px(px(6.0))
                    .text_size(px(12.0))
                    .text_center()
                    .text_color(if total == 0 {
                        theme.text_muted.opacity(0.7)
                    } else {
                        theme.text_muted
                    })
                    .child(count),
            )
            .child(
                button(
                    "chat-find-previous",
                    crate::icons::ALT_ARROW_UP,
                    tr(cx, msg::chat::find_in_chat_previous),
                    has_matches,
                )
                .on_click(cx.listener(|this, _, _, cx| this.step_find(-1, cx))),
            )
            .child(
                button(
                    "chat-find-next",
                    crate::icons::ALT_ARROW_DOWN,
                    tr(cx, msg::chat::find_in_chat_next),
                    has_matches,
                )
                .on_click(cx.listener(|this, _, _, cx| this.step_find(1, cx))),
            )
            .child(
                button(
                    "chat-find-close",
                    crate::icons::CLOSE,
                    tr(cx, msg::chat::find_in_chat_close),
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| this.close_find(cx))),
            );
        Some(
            div()
                .absolute()
                .top(px(Theme::TITLEBAR_HEIGHT + 8.0))
                .right(px(12.0))
                .child(bar)
                .into_any_element(),
        )
    }

    #[cfg(test)]
    pub(crate) fn bulk_tool_details(&self) -> Option<bool> {
        self.bulk_tool_details
    }

    #[cfg(test)]
    pub(crate) fn find_state(&self) -> Option<(usize, Option<usize>)> {
        self.find
            .as_ref()
            .map(|find| (find.matches.len(), find.active))
    }

    #[cfg(any(test, feature = "browser-fixture"))]
    pub(crate) fn set_find_query(&mut self, query: &str, cx: &mut Context<Self>) {
        if let Some(find) = &self.find {
            find.input
                .update(cx, |input, cx| input.set_value(query.into(), cx));
        }
        if let Some(find) = self.find.as_mut() {
            find.needle = chat_find::normalize_query(query);
            find.counts.clear();
            find.active = None;
        }
        self.refresh_find(false, cx);
        if let Some(find) = self.find.as_mut() {
            find.active = chat_find::first_index_from_row(0, &find.matches);
        }
        #[cfg(feature = "browser-fixture")]
        self.step_find(0, cx);
    }
}
