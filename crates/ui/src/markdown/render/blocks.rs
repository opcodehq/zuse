//! Lay out Markdown blocks and assign stable keys to nested elements.

use std::hash::{DefaultHasher, Hash, Hasher};

use gpui::{AnyElement, Div, FontWeight, Hsla, SharedString, Window, div, prelude::*, px};
use zuse_syntax::HighlightedDocument;

use crate::markdown::parser::{AlertKind, Block, BlockTree};
use crate::theme::{Appearance, Theme};

use super::{
    CodeHighlight, RenderOptions,
    code_block::{FenceLabel, render_code_block},
    table::render_table,
    text::{TextStyle, text_element, text_element_styled},
};

/// Gap between markdown blocks inside one message (zuse mdBlockGap).
pub const MD_BLOCK_GAP: f32 = 12.0;
/// Body text size / line height (zuse: 14px / 22px).
pub const MD_TEXT_SIZE: f32 = crate::typography::TEXT_MD;
pub const MD_LINE_HEIGHT: f32 = 22.0;
/// Transcript block rhythm (T3 `.chat-markdown`): headings open a section
/// with room above and hug their content below; fences, tables and rules get a
/// little more air than prose.
pub(crate) const TRANSCRIPT_GAP_BEFORE_MAJOR_HEADING: f32 = 28.0;
pub(crate) const TRANSCRIPT_GAP_BEFORE_MINOR_HEADING: f32 = 22.0;
pub(crate) const TRANSCRIPT_GAP_AFTER_HEADING: f32 = 8.0;
pub(crate) const TRANSCRIPT_GAP_AROUND_RULE: f32 = 18.0;
pub(crate) const TRANSCRIPT_GAP_AROUND_FRAME: f32 = 14.0;
pub(crate) const TRANSCRIPT_GAP_DEFAULT: f32 = 12.0;
/// Gap between a list item's paragraph and the nested list below it.
const LIST_NESTED_LIST_GAP: f32 = 6.0;
const LIST_ITEM_CHILD_GAP: f32 = 4.0;
pub(crate) const TRANSCRIPT_TEXT_SIZE: f32 = 14.0;
pub(crate) const TRANSCRIPT_LINE_HEIGHT: f32 = 22.0;
/// Vertical gap between two adjacent top-level transcript blocks. Both the
/// unsplit tree ([`render_tree`]) and the per-block transcript rows
/// (`transcript::top_gap_for`) use it, so the two paint identically.
/// Precedence: a heading below wins, then a heading above, then a rule, then
/// a code block or table on either side.
pub(crate) fn block_gap(prev: &Block, next: &Block) -> f32 {
    let is_frame = |block: &Block| matches!(block, Block::CodeBlock { .. } | Block::Table { .. });
    match (prev, next) {
        (_, Block::Heading { level: 1 | 2, .. }) => TRANSCRIPT_GAP_BEFORE_MAJOR_HEADING,
        (_, Block::Heading { .. }) => TRANSCRIPT_GAP_BEFORE_MINOR_HEADING,
        (Block::Heading { .. }, _) => TRANSCRIPT_GAP_AFTER_HEADING,
        (Block::Rule, _) | (_, Block::Rule) => TRANSCRIPT_GAP_AROUND_RULE,
        _ if is_frame(prev) || is_frame(next) => TRANSCRIPT_GAP_AROUND_FRAME,
        _ => TRANSCRIPT_GAP_DEFAULT,
    }
}

/// Tokens for the fence rendered with element discriminator `ix` (the `ix`
/// [`render_block`] receives; see [`code_block_indices`]). Nested fences in
/// lists and quotes resolve through the same lookup as top-level ones.
pub type HighlightLookup<'a, 'tokens> = &'a dyn Fn(usize) -> CodeHighlight<'tokens>;

/// A lookup with no tokens (plain code).
pub fn no_highlight(_: usize) -> CodeHighlight<'static> {
    None
}

/// Render a whole tree stacked with the md block gap. `highlight` resolves
/// tokens for `(top-level block index, element discriminator)` of each fence.
pub fn render_tree(
    tree: &BlockTree,
    opts: &RenderOptions,
    theme: &Theme,
    window: &Window,
    highlight: &dyn Fn(usize, usize) -> Option<std::sync::Arc<HighlightedDocument>>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .when(!opts.transcript, |stack| stack.gap(px(MD_BLOCK_GAP)))
        .children(tree.blocks.iter().enumerate().map(|(ix, top)| {
            let documents: Vec<(usize, std::sync::Arc<HighlightedDocument>)> =
                code_block_indices(&top.block, ix)
                    .into_iter()
                    .filter_map(|element| Some((element, highlight(ix, element)?)))
                    .collect();
            let lookup = |element: usize| {
                documents
                    .iter()
                    .find(|(ix, _)| *ix == element)
                    .map(|(_, document)| document.lines.as_slice())
            };
            let block = render_block(&top.block, ix, ix, opts, theme, window, &lookup);
            // Transcript rhythm depends on the pair of blocks, so it is each
            // block's top padding rather than one flat stack gap.
            match ix.checked_sub(1).filter(|_| opts.transcript) {
                Some(prev) => div()
                    .w_full()
                    .min_w_0()
                    .pt(px(block_gap(&tree.blocks[prev].block, &top.block)))
                    .child(block)
                    .into_any_element(),
                None => block,
            }
        }))
        .into_any_element()
}

pub(super) fn nested_ix(path: impl Hash) -> usize {
    let mut hash = DefaultHasher::new();
    path.hash(&mut hash);
    hash.finish() as usize | (1usize << (usize::BITS - 1))
}

pub(super) fn quote_child_ix(ix: usize, child_ix: usize) -> usize {
    nested_ix((ix, 0u8, child_ix))
}

pub(super) fn list_child_ix(ix: usize, item_ix: usize, child_ix: usize) -> usize {
    nested_ix((ix, 1u8, item_ix, child_ix))
}

/// Element discriminators for every code block below `block`. Transcript rows
/// use this to provision one independent scroll handle per nested fence before
/// the renderer recursively reaches it.
pub(crate) fn code_block_indices(block: &Block, ix: usize) -> Vec<usize> {
    code_blocks(block, ix)
        .into_iter()
        .map(|fence| fence.ix)
        .collect()
}

/// One fenced code block at any depth below a top-level block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CodeFence<'a> {
    /// Element discriminator, as passed to [`render_block`].
    pub ix: usize,
    pub language: Option<&'a str>,
    pub code: &'a str,
}

/// Every fence below `block` (itself included), in render order. Highlight
/// owners key their tokens by [`CodeFence::ix`] so nested fences in lists and
/// quotes are highlighted like top-level ones.
pub(crate) fn code_blocks(block: &Block, ix: usize) -> Vec<CodeFence<'_>> {
    fn collect<'a>(block: &'a Block, ix: usize, out: &mut Vec<CodeFence<'a>>) {
        match block {
            Block::CodeBlock { language, code, .. } => out.push(CodeFence {
                ix,
                language: language.as_deref(),
                code,
            }),
            Block::BlockQuote { children, .. }
            | Block::Details { children, .. }
            | Block::FootnoteDefinition { children, .. } => {
                for (child_ix, child) in children.iter().enumerate() {
                    collect(child, quote_child_ix(ix, child_ix), out);
                }
            }
            Block::List { items, .. } => {
                for (item_ix, item) in items.iter().enumerate() {
                    for (child_ix, child) in item.iter().enumerate() {
                        collect(child, list_child_ix(ix, item_ix, child_ix), out);
                    }
                }
            }
            Block::Paragraph { .. } | Block::Heading { .. } | Block::Table { .. } | Block::Rule => {
            }
        }
    }

    let mut fences = Vec::new();
    collect(block, ix, &mut fences);
    fences
}

/// Render one block (top-level or nested). `top_ix` is the enclosing top-level
/// block index (cache invalidation scope); `ix` the per-element discriminator.
#[allow(clippy::too_many_arguments)]
pub fn render_block(
    block: &Block,
    top_ix: usize,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
    window: &Window,
    highlight: HighlightLookup<'_, '_>,
) -> AnyElement {
    render_block_at_depth(block, top_ix, ix, 0, opts, theme, window, highlight)
}

#[allow(clippy::too_many_arguments)]
fn render_block_at_depth(
    block: &Block,
    top_ix: usize,
    ix: usize,
    list_depth: usize,
    opts: &RenderOptions,
    theme: &Theme,
    window: &Window,
    highlight: HighlightLookup<'_, '_>,
) -> AnyElement {
    let prose_line_height = opts.prose_line_height();
    match block {
        Block::Paragraph { runs } => text_element(
            runs,
            opts.prose_text_size(),
            prose_line_height,
            false,
            top_ix,
            ix,
            opts,
            theme,
        ),
        Block::Heading { level, runs } => {
            let (size, line) = heading_metrics(*level);
            text_element_styled(
                runs,
                size,
                line,
                heading_style(*level, theme),
                top_ix,
                ix,
                opts,
                theme,
            )
        }
        Block::CodeBlock {
            language,
            title,
            code,
            fence_closed,
        } => render_code_block(
            FenceLabel {
                language: language.as_deref(),
                title: title.as_deref(),
                closed: *fence_closed,
            },
            code,
            top_ix,
            ix,
            opts,
            theme,
            highlight(ix),
        ),
        Block::BlockQuote { kind, children } => {
            let color = kind.map(alert_color);
            div()
                .border_l_2()
                .border_color(color.map_or(theme.text.opacity(0.35), |color| color.opacity(0.7)))
                .pl(px(13.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .when_some(*kind, |quote, kind| {
                    let color = alert_label_color(kind, theme.appearance);
                    quote.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color)
                            .child(
                                crate::icons::icon(crate::icons::ALERT_CIRCLE)
                                    .size(px(14.))
                                    .text_color(color),
                            )
                            .child(kind.label()),
                    )
                })
                .child(container_children(
                    children, top_ix, ix, list_depth, opts, theme, window, highlight,
                ))
                .into_any_element()
        }
        Block::Details { summary, children } => DetailsDisclosure {
            id: format!("{}-details-{top_ix}-{ix}", opts.row_key).into(),
            label: if summary.iter().all(|run| run.text.trim().is_empty()) {
                div().child("Details").into_any_element()
            } else {
                let label_options = RenderOptions {
                    link: None,
                    media: None,
                    cache: None,
                    veil: None,
                    workspace_root: None,
                    ..opts.clone()
                };
                text_element_styled(
                    summary,
                    opts.prose_text_size(),
                    prose_line_height,
                    TextStyle {
                        weight: FontWeight::NORMAL,
                        color: Some(theme.text_muted),
                    },
                    top_ix,
                    ix,
                    &label_options,
                    theme,
                )
            },
            body: container_children(
                children, top_ix, ix, list_depth, opts, theme, window, highlight,
            ),
            color: theme.text_muted,
        }
        .into_any_element(),
        Block::FootnoteDefinition {
            label,
            children,
            section_start,
        } => div()
            .flex()
            .items_start()
            .gap(px(8.))
            .when(*section_start, |section| {
                section.border_t_1().border_color(theme.border).pt(px(8.))
            })
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.text_muted)
                    .child(format!("[{label}]")),
            )
            .child(div().min_w_0().flex_1().child(container_children(
                children, top_ix, ix, list_depth, opts, theme, window, highlight,
            )))
            .into_any_element(),
        Block::List {
            ordered_start,
            items,
        } => {
            let decimal = opts.user || list_depth % 3 == 0;
            let marker_label = |index: usize| {
                ordered_start.map(|start| {
                    let number = start.saturating_add(index as u64);
                    if decimal {
                        format!("{number}.")
                    } else {
                        nested_ordered_label(number, list_depth)
                    }
                })
            };
            let marker_width = (0..items.len())
                .filter_map(marker_label)
                .map(|label| label.len() as f32 * 8.)
                .fold(18., f32::max);
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .children(items.iter().enumerate().map(|(item_ix, item)| {
                    let marker_color = if opts.transcript {
                        theme.text.opacity(0.75)
                    } else {
                        theme.accent
                    };
                    let line_height = crate::typography::ui_rems(prose_line_height);
                    // Use a drawn disc for unordered lists so its size is independent
                    // of the selected body font.
                    let task = item.first().and_then(|block| match block {
                        Block::Paragraph { runs } => runs.first()?.style.task.as_ref(),
                        _ => None,
                    });
                    let marker: gpui::AnyElement = if let Some(task) = task {
                        let toggle = opts.tasks.as_ref().and_then(|ui| ui.toggle.clone());
                        let marker = task.clone();
                        let label: String = match &item[0] {
                            Block::Paragraph { runs } => {
                                runs.iter().skip(1).map(|r| r.text.as_str()).collect()
                            }
                            _ => String::new(),
                        };
                        div()
                            .flex_none()
                            .w(px(marker_width))
                            .h(line_height)
                            .flex()
                            .items_center()
                            .child(
                                gpui_base::Checkbox::new(SharedString::from(format!(
                                    "{}-task-{}",
                                    opts.row_key, task.range.start
                                )))
                                .checked(task.checked)
                                .disabled(toggle.is_none())
                                .when(toggle.is_some(), |checkbox| checkbox.cursor_pointer())
                                .focus_visible(|style| style.border_color(theme.text))
                                .when(task.checked, |checkbox| {
                                    checkbox.styles(|styles| {
                                        styles.disabled(|style| style.opacity(0.5))
                                    })
                                })
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .accessibility_label(label)
                                .size(px(16.0))
                                .border_1()
                                .rounded(px(3.0))
                                .border_color(if task.checked {
                                    theme.accent
                                } else if toggle.is_none() {
                                    theme.text_muted
                                } else {
                                    theme.border
                                })
                                .bg(if task.checked {
                                    theme.accent
                                } else {
                                    gpui::transparent_black()
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(task.checked, |checkbox| {
                                    checkbox.child(
                                        crate::icons::icon(crate::icons::CHECK)
                                            .size(px(12.0))
                                            .text_color(theme.bg),
                                    )
                                })
                                .on_change(
                                    move |_, _, window, cx| {
                                        cx.stop_propagation();
                                        if let Some(toggle) = &toggle {
                                            toggle(&marker, window, cx);
                                        }
                                    },
                                ),
                            )
                            .into_any_element()
                    } else {
                        match marker_label(item_ix) {
                            Some(label) => div()
                                .flex_none()
                                .w(px(marker_width))
                                .text_right()
                                .text_size(crate::typography::ui_rems(opts.prose_text_size()))
                                .line_height(line_height)
                                .text_color(marker_color)
                                .child(label)
                                .into_any_element(),
                            None => {
                                let shape = list_depth % 3;
                                div()
                                    .flex_none()
                                    .w(px(marker_width))
                                    .h(line_height)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .size(px(5.))
                                            .when(shape != 2, |dot| dot.rounded_full())
                                            .when(shape == 1, |dot| {
                                                dot.border_1().border_color(marker_color)
                                            })
                                            .when(shape != 1, |dot| dot.bg(marker_color)),
                                    )
                                    .into_any_element()
                            }
                        }
                    };
                    div().flex().flex_row().gap(px(8.0)).child(marker).child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(LIST_ITEM_CHILD_GAP))
                            .children(item.iter().enumerate().map(|(ci, child)| {
                                // A nested list sits a touch further from the
                                // paragraph that introduces it.
                                let nested_list = ci > 0
                                    && matches!(child, Block::List { .. })
                                    && matches!(item[ci - 1], Block::Paragraph { .. });
                                let task_body;
                                let child = if ci == 0 && task.is_some() {
                                    let Block::Paragraph { runs } = child else {
                                        unreachable!()
                                    };
                                    task_body = Block::Paragraph {
                                        runs: runs.iter().skip(1).cloned().collect(),
                                    };
                                    &task_body
                                } else {
                                    child
                                };
                                let rendered = render_block_at_depth(
                                    child,
                                    top_ix,
                                    list_child_ix(ix, item_ix, ci),
                                    list_depth + 1,
                                    opts,
                                    theme,
                                    window,
                                    highlight,
                                );
                                if nested_list {
                                    div()
                                        .w_full()
                                        .min_w_0()
                                        .pt(px(LIST_NESTED_LIST_GAP - LIST_ITEM_CHILD_GAP))
                                        .child(rendered)
                                        .into_any_element()
                                } else {
                                    rendered
                                }
                            })),
                    )
                }))
                .into_any_element()
        }
        Block::Table {
            header,
            rows,
            align,
        } => render_table(header, rows, align, top_ix, ix, opts, theme, window),
        Block::Rule => div()
            .h(px(1.0))
            .w_full()
            // The plain border all but vanishes on dark backgrounds.
            .bg(theme.border_strong)
            .into_any_element(),
    }
}

fn nested_ordered_label(mut number: u64, depth: usize) -> String {
    if number == 0 {
        return "0.".into();
    }
    if depth % 3 == 1 {
        let mut letters = Vec::new();
        while number > 0 {
            number -= 1;
            letters.push(char::from(b'a' + (number % 26) as u8));
            number /= 26;
        }
        letters.into_iter().rev().chain(['.']).collect()
    } else if number <= 3999 {
        let mut label = String::new();
        for (value, digits) in [
            (1000, "m"),
            (900, "cm"),
            (500, "d"),
            (400, "cd"),
            (100, "c"),
            (90, "xc"),
            (50, "l"),
            (40, "xl"),
            (10, "x"),
            (9, "ix"),
            (5, "v"),
            (4, "iv"),
            (1, "i"),
        ] {
            while number >= value {
                label.push_str(digits);
                number -= value;
            }
        }
        label.push('.');
        label
    } else {
        format!("{number}.")
    }
}

fn alert_color(kind: AlertKind) -> Hsla {
    gpui::rgb(match kind {
        AlertKind::Note => 0x3b82f6,
        AlertKind::Tip => 0x10b981,
        AlertKind::Important => 0xa855f7,
        AlertKind::Warning => 0xf59e0b,
        AlertKind::Caution => 0xef4444,
    })
    .into()
}

fn alert_label_color(kind: AlertKind, appearance: Appearance) -> Hsla {
    match appearance {
        Appearance::Dark => alert_color(kind),
        Appearance::Light => gpui::rgb(match kind {
            AlertKind::Note => 0x1d4ed8,
            AlertKind::Tip => 0x047857,
            AlertKind::Important => 0x7e22ce,
            AlertKind::Warning => 0x92400e,
            AlertKind::Caution => 0xb91c1c,
        })
        .into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn container_children(
    children: &[Block],
    top_ix: usize,
    ix: usize,
    list_depth: usize,
    opts: &RenderOptions,
    theme: &Theme,
    window: &Window,
    highlight: HighlightLookup<'_, '_>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .children(children.iter().enumerate().map(|(child_ix, child)| {
            render_block_at_depth(
                child,
                top_ix,
                quote_child_ix(ix, child_ix),
                list_depth,
                opts,
                theme,
                window,
                highlight,
            )
        }))
}

#[derive(gpui::IntoElement)]
struct DetailsDisclosure {
    id: SharedString,
    label: AnyElement,
    body: Div,
    color: Hsla,
}

impl gpui::RenderOnce for DetailsDisclosure {
    fn render(self, window: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| false);
        let open = *state.read(cx);
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .id(self.id)
                    .cursor_pointer()
                    .role(gpui::Role::Button)
                    .tab_index(0)
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_color(self.color)
                    .child(if open { "▾" } else { "▸" })
                    .child(self.label)
                    .on_click(move |_, window, cx| {
                        state.update(cx, |open, cx| {
                            *open = !*open;
                            cx.notify();
                        });
                        window.refresh();
                    }),
            )
            .when(open, |block| block.child(self.body))
    }
}

/// Heading scale (size, line height) in px, after T3's chat markdown: h1 20,
/// h2 17, h3 15.5, then body size for h4-h6.
pub(crate) fn heading_metrics(level: u8) -> (f32, f32) {
    match level {
        1 => (20.0, 26.0),
        2 => (17.0, 24.0),
        3 => (15.5, 22.0),
        _ => (crate::typography::TEXT_MD, 22.0),
    }
}

/// Base weight and color for a heading: h1/h2 bold, the rest semibold, all in
/// the strong text color except h6, which reads muted.
fn heading_style(level: u8, theme: &Theme) -> TextStyle {
    TextStyle {
        weight: if level <= 2 {
            FontWeight::BOLD
        } else {
            FontWeight::SEMIBOLD
        },
        color: Some(if level >= 6 {
            theme.text_muted
        } else {
            theme.text_strong
        }),
    }
}

pub(crate) fn render_heading_with_metrics(
    runs: &[crate::markdown::parser::InlineRun],
    metrics: (f32, f32),
    opts: &RenderOptions,
    theme: &Theme,
) -> AnyElement {
    text_element(runs, metrics.0, metrics.1, true, 0, 0, opts, theme)
}
