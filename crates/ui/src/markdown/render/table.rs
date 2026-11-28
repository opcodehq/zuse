//! Measure and render GFM table columns, cells, and overflow.

use std::rc::Rc;

use gpui::{AnyElement, FontWeight, Hsla, Pixels, SharedString, Window, div, prelude::*, px};
use gpui_base::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow};

use crate::{
    markdown::parser::{InlineRun, TableAlign},
    theme::Theme,
};

use super::{
    MD_LINE_HEIGHT, RenderOptions,
    blocks::nested_ix,
    text::{FlatText, TextStyle, flat_text_element, flatten_cached, text_element},
};

// Shared table metrics. Markdown cells keep the transcript's body type scale.
/// Horizontal cell padding in px.
pub const TABLE_CELL_PADDING: f32 = 12.0;
/// Vertical cell padding in px.
const TABLE_CELL_PADDING_Y: f32 = 8.0;
const TABLE_CORNER_RADIUS: f32 = 10.0;
/// Header row font weight.
pub const TABLE_HEADER_WEIGHT: FontWeight = FontWeight::SEMIBOLD;
/// Floor for a column's max-content share, so a short column ("1k") beside a
/// prose column keeps a readable width.
pub const TABLE_MIN_COLUMN_CONTENT: f32 = 48.0;
/// Minimum rendered column width in px, padding included (zuse
/// `table.minColumnWidth`). Naturally narrower columns keep their content
/// width; wider ones wrap down to this floor, then the table scrolls.
pub const TABLE_MIN_COLUMN_WIDTH: f32 = 96.0;
/// Limit the extra space a single long heading can reserve.
const TABLE_MAX_HEADING_FLOOR: f32 = 160.0;
/// Hairline tone (zuse md theme `table.borderColor`: rgba(255,255,255,0.1)).
pub fn table_hairline() -> Hsla {
    crate::theme::hairline(0.10)
}

/// Shared per-column table geometry.
pub struct TableColumns {
    /// Per-column max-content width, padding included.
    pub naturals: Vec<f32>,
    /// Per-column minimum width, padding included.
    pub minimums: Vec<f32>,
    /// Σ minimums — the width below which the table stops shrinking and scrolls.
    pub min_table_width: f32,
}

/// Resolve column geometry from measured per-column max-content widths
/// (content only; horizontal padding is added here).
pub fn table_columns(content_widths: &[f32]) -> TableColumns {
    table_columns_with_headings(content_widths, &[])
}

pub(super) fn table_columns_with_headings(
    content_widths: &[f32],
    heading_words: &[f32],
) -> TableColumns {
    let naturals: Vec<f32> = content_widths
        .iter()
        .map(|w| w.max(TABLE_MIN_COLUMN_CONTENT) + 2.0 * TABLE_CELL_PADDING)
        .collect();
    let minimums: Vec<f32> = naturals
        .iter()
        .enumerate()
        .map(|(column, natural)| {
            let heading_floor =
                heading_words.get(column).copied().unwrap_or(0.0) + 2.0 * TABLE_CELL_PADDING;
            natural.min(heading_floor.clamp(TABLE_MIN_COLUMN_WIDTH, TABLE_MAX_HEADING_FLOOR))
        })
        .collect();
    let min_table_width = minimums.iter().sum();
    TableColumns {
        naturals,
        minimums,
        min_table_width,
    }
}

/// Measure the longest heading word using the same display text and styling as
/// the cell. GPUI uses ordinary spaces as word boundaries and keeps NBSP glued.
fn heading_word_width(flat: &FlatText, font_size: Pixels, window: &Window) -> f32 {
    let text_system = window.text_system();
    let mut word_start = 0;
    let mut run_start = 0;
    let mut runs = flat.runs.iter().peekable();
    let mut longest: f32 = 0.;
    let mut word_runs = Vec::new();
    for word in flat.text.split([' ', '\n', '\r']) {
        let word_end = word_start + word.len();
        if !word.is_empty() {
            word_runs.clear();
            while let Some(run) = runs.peek() {
                let run_end = run_start + run.len;
                let len = run_end
                    .min(word_end)
                    .saturating_sub(run_start.max(word_start));
                if len > 0 {
                    let mut run = (*run).clone();
                    run.len = len;
                    word_runs.push(run);
                }
                if run_end > word_end {
                    break;
                }
                run_start = run_end;
                runs.next();
            }
            let layout =
                text_system.shape_line(word.to_owned().into(), font_size, &word_runs, None);
            longest = longest.max(f32::from(layout.width()).ceil() + 1.0);
        }
        word_start = word_end + 1;
    }
    longest.min(TABLE_MAX_HEADING_FLOOR - 2.0 * TABLE_CELL_PADDING)
}

/// Element/cache discriminator for a table cell (row-major under the block ix).
pub(in crate::markdown) fn table_cell_ix(ix: usize, r: usize, c: usize) -> usize {
    nested_ix((ix, 2u8, r, c))
}

/// Apply identical geometry to semantic header and data cells. All rows use
/// the same measured column proportions and minimum widths.
fn style_table_cell<E: Styled>(cell: E, natural: f32, minimum: f32, opts: &RenderOptions) -> E {
    cell.flex_grow(natural)
        .flex_shrink(natural)
        .flex_basis(px(0.0))
        .min_w(px(minimum))
        .px(px(TABLE_CELL_PADDING))
        .py(px(if opts.transcript {
            6.0
        } else {
            TABLE_CELL_PADDING_Y
        }))
        .text_size(crate::typography::ui_rems(opts.prose_text_size()))
        .line_height(crate::typography::ui_rems(MD_LINE_HEIGHT))
}

/// A GFM table built from zkit's semantic table components. Measured columns
/// wrap to a readable floor, then scroll horizontally on narrow surfaces.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_table(
    header: &[Vec<InlineRun>],
    rows: &[Vec<Vec<InlineRun>>],
    align: &[TableAlign],
    top_ix: usize,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
    window: &Window,
) -> AnyElement {
    // Header row first; pad ragged rows to the shared column count.
    let all: Vec<&[Vec<InlineRun>]> = std::iter::once(header)
        .filter(|h| !h.is_empty())
        .map(|h| h as &[Vec<InlineRun>])
        .chain(rows.iter().map(|r| r.as_slice()))
        .collect();
    let cols = all.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return gpui::Empty.into_any_element();
    }
    let has_header = !header.is_empty();

    // Flatten every cell (cache-aware) and take per-column max-content widths.
    let text_system = window.text_system();
    let font_size = crate::typography::ui_rems(opts.prose_text_size()).to_pixels(window.rem_size());
    let mut flats: Vec<Vec<Option<Rc<FlatText>>>> = Vec::with_capacity(all.len());
    let mut content = vec![0.0f32; cols];
    let mut heading_words = vec![0.0; cols];
    for (r, row) in all.iter().enumerate() {
        let weight = if has_header && r == 0 {
            TABLE_HEADER_WEIGHT
        } else {
            FontWeight::NORMAL
        };
        let mut out: Vec<Option<Rc<FlatText>>> = Vec::with_capacity(cols);
        for (c, natural) in content.iter_mut().enumerate() {
            let Some(runs) = row.get(c) else {
                out.push(None);
                continue;
            };
            let style = TextStyle {
                weight,
                color: None,
            };
            let flat = flatten_cached(runs, style, top_ix, table_cell_ix(ix, r, c), opts, theme);
            if !flat.text.is_empty() {
                // Intrinsic table proportions use the same bounded link presentation;
                // each cell then resolves its exact width during measured layout.
                let measured = opts
                    .link
                    .as_ref()
                    .filter(|ui| ui.source_session.is_some())
                    .map(|_| {
                        crate::markdown::link_presentation::present(
                            &flat,
                            px(560.),
                            font_size,
                            window,
                            opts,
                        )
                    });
                let flat = measured.as_ref().unwrap_or(&flat);
                // Cell sources are single-line; guard anyway (same byte count,
                // so the runs still cover the text exactly).
                let line: SharedString = if flat.text.contains('\n') {
                    flat.text.replace('\n', " ").into()
                } else {
                    flat.text.clone()
                };
                let width = f32::from(
                    text_system
                        .shape_line(line, font_size, &flat.runs, None)
                        .width(),
                );
                // Leave a pixel for glyph rounding so a fitted word does not
                // wrap its final character at a column's natural width.
                let width = width.ceil() + 1.0;
                *natural = natural.max(width);
                if has_header && r == 0 && width + 2.0 * TABLE_CELL_PADDING > TABLE_MIN_COLUMN_WIDTH
                {
                    heading_words[c] = heading_word_width(flat, font_size, window);
                }
            }
            out.push(Some(flat));
        }
        flats.push(out);
    }
    let geo = table_columns_with_headings(&content, &heading_words);

    let table_id: SharedString = format!("{}-table{ix}", opts.row_key).into();
    let part_id = |part: &str| -> SharedString { format!("{table_id}-{part}").into() };
    let mut inner = Table::new(part_id("content"))
        .row_count(all.len())
        .column_count(cols)
        .flex()
        .flex_col()
        .w_full()
        .min_w(px(
            geo.min_table_width + if opts.transcript { 0. } else { 2. }
        ))
        .when(!opts.transcript, |table| {
            table
                .border_1()
                .border_color(theme.border)
                .rounded(px(TABLE_CORNER_RADIUS))
        })
        .overflow_hidden();
    let mut body = TableBody::new(part_id("body")).flex().flex_col();
    for (r, row) in flats.iter().enumerate() {
        let is_header = has_header && r == 0;
        let mut row_el = TableRow::new(part_id(&format!("row-{r}")), r + 1)
            .flex()
            .flex_row()
            .items_start()
            .when(r > 0, |row| {
                row.border_t_1().border_color(if opts.transcript {
                    theme.border.opacity(0.6)
                } else {
                    theme.border
                })
            })
            .when(is_header && !opts.transcript, |row| {
                row.bg(theme.glass_hover())
                    .rounded_t(px(TABLE_CORNER_RADIUS - 1.0))
            });
        for (c, cell_flat) in row.iter().enumerate() {
            let mut cell = div().w_full().min_w_0();
            cell = match align.get(c).copied().unwrap_or_default() {
                TableAlign::Left => cell,
                TableAlign::Center => cell.text_center(),
                TableAlign::Right => cell.text_right(),
            };
            if opts
                .media
                .as_ref()
                .is_some_and(|media| media.image.is_some())
                && all[r]
                    .get(c)
                    .is_some_and(|runs| runs.iter().any(|run| run.style.image.is_some()))
            {
                cell = cell.child(text_element(
                    &all[r][c],
                    opts.prose_text_size(),
                    MD_LINE_HEIGHT,
                    has_header && r == 0,
                    top_ix,
                    table_cell_ix(ix, r, c),
                    opts,
                    theme,
                ));
            } else if let Some(flat) = cell_flat {
                cell = cell.child(flat_text_element(
                    flat,
                    table_cell_ix(ix, r, c),
                    opts,
                    theme,
                ));
            }
            let cell_id = part_id(&format!("cell-{r}-{c}"));
            row_el = if is_header {
                row_el.child(style_table_cell(
                    TableHead::new(cell_id, c + 1).child(cell),
                    geo.naturals[c],
                    geo.minimums[c],
                    opts,
                ))
            } else {
                row_el.child(style_table_cell(
                    TableCell::new(cell_id, c + 1).child(cell),
                    geo.naturals[c],
                    geo.minimums[c],
                    opts,
                ))
            };
        }
        if is_header {
            inner = inner.child(TableHeader::new(part_id("header")).child(row_el));
        } else {
            body = body.child(row_el);
        }
    }

    // The horizontal scroller — when the floors exceed the viewport the inner
    // block keeps `min_table_width` and this viewport scrolls it.
    let mut viewport = div()
        .id(table_id)
        .w_full()
        .overflow_x_scroll()
        .child(inner.child(body));
    viewport.style().restrict_scroll_to_axis = Some(true);
    viewport.into_any_element()
}
