//! Render Markdown trees as GPUI elements. Submodules own block layout,
//! tables, inline text, code fences, and text selection.

use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc, time::Instant};

use gpui::{AnyElement, SharedString, TextRun, Window};
use zuse_syntax::HighlightSpan;

use crate::theme::Theme;

use super::veil::RowVeil;

mod blocks;
mod code_block;
mod selection_paint;
mod table;
mod text;

#[cfg(test)]
mod selection_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use blocks::TRANSCRIPT_GAP_AFTER_HEADING;
pub use blocks::{
    HighlightLookup, MD_BLOCK_GAP, MD_LINE_HEIGHT, MD_TEXT_SIZE, no_highlight, render_block,
    render_tree,
};
pub(crate) use blocks::{
    TRANSCRIPT_GAP_DEFAULT, TRANSCRIPT_LINE_HEIGHT, TRANSCRIPT_TEXT_SIZE, block_gap,
    code_block_indices, code_blocks, render_heading_with_metrics,
};
pub use code_block::{
    CODE_LINE_HEIGHT, CODE_PADDING_X, CODE_PADDING_Y, CODE_TEXT_SIZE, CodeFenceRuntime,
    CodeScrollbarUi, CodeUi, code_ui_for, runs_for_syntax_line, runs_for_syntax_line_with_plain,
    token_color,
};
pub(crate) use selection_paint::{
    clear_selection_surface, range_rects, selectable_text_element, update_drag_at,
};
#[cfg(test)]
pub(crate) use selection_paint::{
    paint_text_selection, selectable_text_wrap, selection_test_bounds, selection_test_entry_count,
    selection_test_snapshot,
};
pub use selection_paint::{
    selection_frame_reset, selection_frame_reset_for, selection_surface_reset,
};
pub use table::{
    TABLE_CELL_PADDING, TABLE_HEADER_WEIGHT, TABLE_MIN_COLUMN_CONTENT, TABLE_MIN_COLUMN_WIDTH,
    TableColumns, table_columns, table_hairline,
};
pub(super) use text::flat_text_presented_element;
pub use text::{
    FlatText, INLINE_CODE_INSET_Y, INLINE_CODE_PAD_X, INLINE_CODE_RADIUS, flatten_runs,
    inline_code_text, inline_code_wash, user_inline_code_wash,
};
pub(crate) use text::{file_icon_well, is_web_link};

/// Options for one rendered tree (a transcript row or a whole live message).
#[derive(Clone)]
pub struct RenderOptions {
    /// Use the conversation's prose spacing and text hierarchy.
    pub(crate) transcript: bool,
    pub tasks: Option<TaskUi>,
    pub media: Option<MediaUi>,
    /// Stable row key — prefixes element ids (scroll state, animations).
    pub row_key: SharedString,
    /// Streaming veil state for a live row: newly appended text fades in via
    /// paint-only run opacity, keyed per (element, chunk offset) so each chunk
    /// fades exactly once. `None` renders without fades (completed rows).
    pub veil: Option<Rc<RefCell<RowVeil>>>,
    /// Flatten/shape input cache (see [`RenderCache`]): settled blocks reuse
    /// their flat text + runs across frames instead of rebuilding them — the
    /// per-frame cost of a fading live row stays O(tail block), flat in the
    /// total reply length. `None` rebuilds every pass.
    pub cache: Option<Rc<RefCell<RenderCache>>>,
    /// Frame timestamp driving veil opacities (one clock per render pass).
    pub now: Instant,
    /// Code-block copy-button plumbing (round 9): `None` renders no button
    /// (previews outside the transcript).
    pub copy: Option<CopyUi>,
    /// Optional owner-provided routing for links that belong inside the app.
    /// Routing is explicit: rejected links never reach the external opener.
    pub link: Option<LinkUi>,
    /// Root used to distinguish a direct workspace-file link from an ordinary
    /// web link. Transcript surfaces provide it; generic Markdown previews do
    /// not acquire workspace-specific decoration.
    pub workspace_root: Option<SharedString>,
    /// Agent-transcript-only fence layout controls and tracked horizontal
    /// scroll state, keyed by the same element discriminator passed to
    /// [`render_block`]. `None` keeps non-chat Markdown surfaces unchanged.
    pub code: Option<HashMap<usize, CodeUi>>,
    /// "Run in terminal" for one-line shell fences. Owners set it only for
    /// settled blocks (never a fence that may still be streaming) on
    /// surfaces that can reach a terminal.
    pub run: Option<RunUi>,
    /// A sent user message inside its bubble: inline code takes a stronger
    /// wash so it reads against the bubble fill, and ordered lists number
    /// `1.` at every depth (no letters or numerals for nested levels).
    pub(crate) user: bool,
    pub(crate) user_chips: Option<Rc<crate::transcript::SentChips>>,
}

/// Sends a shell fence's command to the owner's terminal.
#[derive(Clone)]
pub struct RunUi {
    pub handler: Rc<dyn Fn(SharedString, &mut Window, &mut gpui::App)>,
}

#[derive(Clone)]
pub struct TaskUi {
    pub toggle: Option<Rc<dyn Fn(&super::parser::TaskMarker, &mut Window, &mut gpui::App)>>,
}

#[derive(Clone)]
pub struct MediaUi {
    pub diagram: Option<Rc<dyn Fn(&str, SharedString, &Theme) -> DiagramView>>,
    /// `None` keeps inline images in their ordinary text rendering.
    pub image: Option<Rc<dyn Fn(&super::parser::InlineImage, SharedString, &Theme) -> AnyElement>>,
}

/// How a Mermaid fence presents itself on the owning surface.
pub enum DiagramView {
    /// The ordinary source fence, unchanged (no diagram is available yet).
    Source,
    /// The source fence, flagged with the reason it cannot be drawn.
    Failed(SharedString),
    Diagram(DiagramUi),
}

pub struct DiagramUi {
    pub body: AnyElement,
    pub show_source: bool,
    pub toggle_source: Rc<dyn Fn(&mut Window, &mut gpui::App)>,
}

/// Copy-button wiring for one row's code blocks: the handler writes the code
/// to the clipboard and flips a transient per-row "Copied" state owned by the
/// transcript entity; `copied_ix` is the block currently showing feedback.
#[derive(Clone)]
pub struct CopyUi {
    pub handler: Rc<dyn Fn(usize, SharedString, &mut Window, &mut gpui::App)>,
    pub copied_ix: Option<usize>,
}

pub use super::links::{LinkAction, LinkActivation, LinkOutcome, LinkTarget};

#[derive(Clone)]
pub struct LinkUi {
    pub source_session: Option<String>,
    /// The linking chat lives on this device — outside links get their
    /// system-level menu rows (default app, file manager).
    pub source_local: bool,
    /// The ordered checkouts a file link may resolve against on this surface,
    /// the linking chat's own first. `None` renders no trailing open glyph
    /// and offers no file paths — previews and web-only surfaces.
    pub(crate) file_roots: Option<Rc<Vec<crate::workspace_links::FileLinkRoot>>>,
    pub handler: Rc<dyn Fn(&LinkActivation, &mut Window, &mut gpui::App) -> LinkOutcome>,
}

impl LinkUi {
    /// The workspace file `target` resolves to under this surface's roots,
    /// when the link is one at all. Drives the trailing open glyph and the
    /// menu's file rows; the click path re-resolves with owners. Owned links
    /// get their absolute path from the resolved root joined with the link's
    /// workspace-relative path; an outside link's absolute path is the
    /// decoded target itself.
    pub(crate) fn file_link(&self, target: &str) -> Option<crate::workspace_links::FileLink> {
        let roots = self.file_roots.as_deref()?;
        match crate::workspace_links::first_root_owning(
            target,
            roots.iter().map(|root| root.root.as_str()),
        )? {
            crate::workspace_links::FileLinkResolution::Owned { root: ix, link } => {
                let root = &roots[ix];
                let absolute = root.absolute(&link);
                // A project root past the linking chat's own opens by
                // absolute path (see `Shell::open_workspace_file_link`), so
                // its hover card shows that path too.
                let path = if ix > 0 && root.chat.is_none() {
                    absolute.to_string_lossy().into_owned()
                } else {
                    link.path
                };
                Some(crate::workspace_links::FileLink {
                    absolute,
                    path,
                    local: root.local,
                })
            }
            crate::workspace_links::FileLinkResolution::Outside(link) => {
                Some(crate::workspace_links::FileLink {
                    absolute: PathBuf::from(&link.path),
                    path: link.path,
                    local: self.source_local,
                })
            }
        }
    }
}

pub fn activate_link(
    target: LinkTarget,
    action: LinkAction,
    ui: Option<&LinkUi>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    if action == LinkAction::Copy {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(target.original.clone()));
        return;
    }
    let activation = LinkActivation {
        target,
        action,
        source_session: ui.and_then(|ui| ui.source_session.clone()),
    };
    let outcome = ui.map_or_else(
        || activation.web_outcome(false),
        |ui| (ui.handler)(&activation, window, cx),
    );
    if let LinkOutcome::External(url) = outcome {
        cx.open_url(&url);
    }
}

impl RenderOptions {
    /// Options for a completed (non-streaming) row — no veil, no cache.
    pub fn settled(row_key: SharedString) -> Self {
        Self {
            transcript: false,
            row_key,
            media: None,
            tasks: None,
            veil: None,
            cache: None,
            now: Instant::now(),
            copy: None,
            link: None,
            workspace_root: None,
            code: None,
            run: None,
            user: false,
            user_chips: None,
        }
    }

    fn prose_text_size(&self) -> f32 {
        if self.transcript {
            TRANSCRIPT_TEXT_SIZE
        } else {
            MD_TEXT_SIZE
        }
    }

    fn prose_line_height(&self) -> f32 {
        if self.transcript {
            TRANSCRIPT_LINE_HEIGHT
        } else {
            MD_LINE_HEIGHT
        }
    }
}

/// Cross-frame cache of flatten results, keyed by
/// `(row key, top-level block ix, element discriminator)`.
///
/// During a streaming fade the live row re-renders every frame; without the
/// cache each frame re-derives every block's flat `String` + `TextRun`s —
/// O(reply length) per frame, growing through long replies. The incremental
/// parser only ever touches a suffix of the top-level blocks
/// ([`super::parser::IncrementalParser::stable_prefix_blocks`]), so everything
/// below that boundary is byte-identical and its flatten result (and, via
/// gpui's line-layout cache keyed on identical text+runs, its shaping) can be
/// reused as-is. `SharedString`/`Rc` make the reuse O(1) per block.
/// Cached runs carry a resolved [`gpui::Hsla`] per span, so an entry is only
/// valid for the palette that produced it — content-only keys silently serve
/// dark-mode text onto a light background after an appearance switch.
/// [`RenderCache::sync_style`] drops everything when color or typography moves.
#[derive(Default)]
pub struct RenderCache {
    flats: HashMap<(SharedString, usize, usize), Rc<FlatText>>,
    code: HashMap<(SharedString, usize, usize), Rc<CachedCode>>,
    /// The [`crate::theme::style_generation`] these entries were shaped under.
    generation: u32,
}

/// Cached per-line code runs (validity: code length + highlight identity).
pub struct CachedCode {
    code_len: usize,
    /// Slice-pointer identity + len of the highlight Arc that produced this.
    hl_key: (usize, usize),
    lines: Vec<(SharedString, Vec<TextRun>)>,
}

impl RenderCache {
    /// Keep rows laid out in the previous viewport, including GPUI overdraw.
    /// Scrolling back regenerates these derived strings and code-line runs.
    pub(crate) fn retain_rows(&mut self, rows: &std::collections::HashSet<SharedString>) {
        self.flats.retain(|(row, _, _), _| rows.contains(row));
        self.code.retain(|(row, _, _), _| rows.contains(row));
    }

    /// Drop every cached entry for `row`.
    pub fn invalidate_row(&mut self, row: &str) {
        self.flats.retain(|(r, _, _), _| r.as_ref() != row);
        self.code.retain(|(r, _, _), _| r.as_ref() != row);
    }

    pub fn clear(&mut self) {
        self.flats.clear();
        self.code.clear();
    }

    /// Drop every entry if the resolved text style changed since shaping. Cheap
    /// enough (one relaxed atomic load) to call on every cache access.
    fn sync_style(&mut self) {
        let generation = crate::theme::style_generation();
        self.sync_generation(generation);
    }

    fn sync_generation(&mut self, generation: u32) {
        if self.generation != generation {
            self.clear();
            self.generation = generation;
        }
    }
}

/// Per-line highlight tokens for a code block, or `None` while pending.
pub type CodeHighlight<'a> = Option<&'a [Vec<HighlightSpan>]>;

/// Native fixture access to actual shaped link ranges; absent in shipped builds.
#[cfg(feature = "browser-fixture")]
pub fn fixture_link(target: &str) -> Option<(gpui::Point<gpui::Pixels>, gpui::FocusHandle)> {
    super::link_interaction::fixture_link(target)
}
