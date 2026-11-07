//! Width-dependent presentation; selection offsets always refer to original text.
use super::render::FlatText;
use gpui::{SharedString, TextRun};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// Maps offsets between a stage's input text and its shown text. `omissions`
/// pairs `(input range, shown range)` in order; the shown span can be SHORTER
/// (link truncation) or LONGER (a reserved glyph slot). Stages chain
/// through `prior`: a slot insertion map sits under a truncation map so
/// displayed coordinates resolve all the way back to the source text.
#[derive(Clone, Debug, Default)]
pub struct OffsetMap {
    pub omissions: Vec<(Range<usize>, Range<usize>)>,
    /// The previous stage's map (closer to the source text), if any.
    pub prior: Option<Box<OffsetMap>>,
}
impl OffsetMap {
    /// This stage only: input-text offset for a shown offset.
    fn local_original(&self, displayed: usize) -> usize {
        // The LAST overlapping-preceding omission's end delta already carries
        // every earlier omission's effect (the coordinate systems meet
        // there), so assigning — not summing — is the transitive shift.
        let mut shift: isize = 0;
        for (original, shown) in &self.omissions {
            if displayed < shown.start {
                break;
            }
            if displayed < shown.end {
                return original.start;
            }
            shift = original.end as isize - shown.end as isize;
        }
        displayed.saturating_add_signed(shift)
    }
    /// This stage only: shown offset for an input-text offset.
    fn local_displayed(&self, original: usize) -> usize {
        let mut shift: isize = 0;
        for (source, shown) in &self.omissions {
            if original < source.start {
                break;
            }
            if original < source.end {
                return shown.start;
            }
            shift = source.end as isize - shown.end as isize;
        }
        original.saturating_add_signed(-shift)
    }
    /// This stage only: shown range for an input range — an input range that
    /// overlaps a replaced span expands to cover the replacement.
    pub(crate) fn local_displayed_range(&self, range: Range<usize>) -> Range<usize> {
        let mut result = self.local_displayed(range.start)..self.local_displayed(range.end);
        for (source, shown) in &self.omissions {
            if range.start < source.end && range.end > source.start {
                result.start = result.start.min(shown.start);
                result.end = result.end.max(shown.end);
            }
        }
        result
    }
    /// Shown offset → source-text offset (all stages).
    pub fn original(&self, displayed: usize) -> usize {
        let local = self.local_original(displayed);
        self.prior
            .as_ref()
            .map_or(local, |prior| prior.original(local))
    }
    /// Source-text offset → shown offset (all stages).
    pub fn displayed(&self, original: usize) -> usize {
        let mid = self
            .prior
            .as_ref()
            .map_or(original, |prior| prior.displayed(original));
        self.local_displayed(mid)
    }
    /// Source-text range → shown range (all stages).
    pub fn displayed_range(&self, range: Range<usize>) -> Range<usize> {
        let mid = match &self.prior {
            Some(prior) => prior.displayed_range(range.clone()),
            None => range,
        };
        self.local_displayed_range(mid)
    }
}
#[derive(Clone)]
pub struct OriginalText {
    pub text: SharedString,
    pub offsets: OffsetMap,
}

pub(crate) fn slice_runs(runs: &[TextRun], range: Range<usize>) -> Vec<TextRun> {
    let mut at = 0;
    runs.iter()
        .filter_map(|run| {
            let start = at;
            at += run.len;
            let len = at.min(range.end).saturating_sub(start.max(range.start));
            (len > 0).then(|| {
                let mut run = run.clone();
                run.len = len;
                run
            })
        })
        .collect()
}

pub fn truncate(
    flat: &FlatText,
    width: f32,
    measure: impl Fn(&str, &[TextRun]) -> f32,
) -> FlatText {
    let mut omissions = Vec::new();
    for (range, url) in &flat.links {
        if super::links::LinkTarget::new("", url).navigation.is_err() {
            continue;
        }
        let label = &flat.text[range.clone()];
        let runs = slice_runs(&flat.runs, range.clone());
        // A link's favicon or file icon slot shares the row with its label.
        let width = flat
            .favicons
            .iter()
            .chain(&flat.file_glyphs)
            .find(|(slot, _)| slot.end == range.start)
            .map_or(width, |(slot, _)| {
                width
                    - measure(
                        &flat.text[slot.clone()],
                        &slice_runs(&flat.runs, slot.clone()),
                    )
            });
        if measure(label, &runs) <= width {
            continue;
        }
        let boundaries: Vec<_> = label.grapheme_indices(true).map(|(i, _)| i).collect();
        let candidate = |end: usize| {
            let text = format!("{}…", &label[..end]);
            let mut runs = slice_runs(&runs, 0..end);
            let mut ellipsis = slice_runs(&flat.runs, range.start + end..range.end).remove(0);
            ellipsis.len = '…'.len_utf8();
            runs.push(ellipsis);
            (text, runs)
        };
        let mut low = 0;
        let mut high = boundaries.len();
        while low < high {
            let middle = (low + high) / 2;
            let (text, runs) = candidate(boundaries[middle]);
            if measure(&text, &runs) <= width {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let prefix = boundaries[low.saturating_sub(1)];
        // Never make a short label longer just to show an ellipsis.
        if range.end - (range.start + prefix) > '…'.len_utf8() {
            omissions.push(range.start + prefix..range.end);
        }
    }
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut offsets = OffsetMap::default();
    let mut at = 0;
    for omitted in omissions {
        text.push_str(&flat.text[at..omitted.start]);
        runs.extend(slice_runs(&flat.runs, at..omitted.start));
        let start = text.len();
        text.push('…');
        let mut style = slice_runs(&flat.runs, omitted.clone()).remove(0);
        style.len = '…'.len_utf8();
        runs.push(style);
        at = omitted.end;
        offsets.omissions.push((omitted, start..text.len()));
    }
    text.push_str(&flat.text[at..]);
    runs.extend(slice_runs(&flat.runs, at..flat.text.len()));
    // This stage's ranges are in `flat.text` coordinates — map them locally;
    // `prior` then chains through any earlier substitution to the source.
    offsets.prior = flat
        .original
        .as_ref()
        .map(|original| Box::new(original.offsets.clone()));
    let shown = |range: &Range<usize>| offsets.local_displayed_range(range.clone());
    FlatText {
        text: text.into(),
        runs,
        links: flat
            .links
            .iter()
            .map(|(r, url)| (shown(r), url.clone()))
            .collect(),
        code_ranges: flat.code_ranges.iter().map(shown).collect(),
        file_glyphs: flat
            .file_glyphs
            .iter()
            .map(|(range, path)| (shown(range), path.clone()))
            .collect(),
        favicons: flat
            .favicons
            .iter()
            .map(|(range, url)| (shown(range), url.clone()))
            .collect(),
        original: Some(OriginalText {
            text: flat
                .original
                .as_ref()
                .map_or_else(|| flat.text.clone(), |original| original.text.clone()),
            offsets,
        }),
    }
}

use super::render::{INLINE_CODE_INSET_Y, RenderOptions, range_rects};
use crate::theme::Theme;
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, Pixels, Size, TextLayout, Window, point, prelude::*, px, size,
};
use std::rc::Rc;

pub struct ResponsiveText {
    pub flat: Rc<FlatText>,
    pub opts: RenderOptions,
    pub theme: Theme,
    pub ix: usize,
}
pub(super) fn present(
    flat: &FlatText,
    width: Pixels,
    font_size: Pixels,
    window: &Window,
    opts: &RenderOptions,
) -> FlatText {
    let with_glyphs = with_file_link_glyphs(flat, opts);
    truncate(&with_glyphs, f32::from(width), |text, runs| {
        window
            .text_system()
            .shape_text(text.to_owned().into(), font_size, runs, None, None)
            .map(|lines| {
                lines
                    .iter()
                    .map(|line| f32::from(line.size(font_size).width))
                    .fold(0., f32::max)
            })
            .unwrap_or(f32::INFINITY)
    })
}

/// The slot reserved before a resolved file link for its file-type icon:
/// five NBSPs of text advance, holding a 14px icon — a reserved slot the
/// shaped text never notices. Copy and selection map the slot back to the
/// link start, so the raw text survives and nothing reflows on hover.
const FILE_GLYPH_SLOT: &str = "\u{00A0}\u{00A0}\u{00A0}\u{00A0}\u{00A0}";
pub(crate) const FILE_GLYPH_SIZE: f32 = 14.0;

/// The slot reserved before a web link for its favicon: five NBSPs, holding
/// the 1em icon with the reference's `.3em` gap before the label. Copy and
/// selection map the slot back to the link start.
const FAVICON_SLOT: &str = "\u{00A0}\u{00A0}\u{00A0}\u{00A0}\u{00A0}";
pub(crate) const FAVICON_SIZE: f32 = 14.0;

/// Reserve a slot before every file link the surface's roots resolve for its
/// file-type icon, and one before every web link for its favicon. Web links that no root owns, and
/// targets no known root owns, keep their shape apart from the favicon, as
/// does a paragraph that is nothing but one file link — that renders as the
/// sole file row with its own icon tile.
/// What a link's leading slot paints.
enum LeadIcon {
    Favicon(String),
    File(String),
}

fn with_file_link_glyphs(flat: &FlatText, opts: &RenderOptions) -> FlatText {
    if flat.links.is_empty() {
        return flat.clone();
    }
    // `(at, slot, icon)`: both kinds of slot open their link.
    let mut insertions: Vec<(usize, &str, LeadIcon)> = flat
        .links
        .iter()
        .filter(|(_, target)| super::render::is_web_link(target))
        .map(|(range, url)| (range.start, FAVICON_SLOT, LeadIcon::Favicon(url.clone())))
        .collect();
    if let Some(ui) = opts.link.as_ref().filter(|ui| ui.file_roots.is_some())
        && !paragraph_is_sole_file_link(flat, opts.workspace_root.as_deref())
    {
        insertions.extend(flat.links.iter().filter_map(|(range, target)| {
            let link = ui.file_link(target)?;
            Some((range.start, FILE_GLYPH_SLOT, LeadIcon::File(link.path)))
        }));
    }
    if insertions.is_empty() {
        return flat.clone();
    }
    insertions.sort_by_key(|(at, _, _)| *at);
    // Positions after an insertion slide by the slot length; an insertion
    // point itself resolves to its "before" side so a range ending on it
    // never swallows its own slot.
    let shift_before = |offset: usize| -> usize {
        insertions
            .iter()
            .filter(|(at, _, _)| *at < offset)
            .map(|(_, slot, _)| slot.len())
            .sum()
    };
    let shift_through = |offset: usize| -> usize {
        insertions
            .iter()
            .filter(|(at, _, _)| *at <= offset)
            .map(|(_, slot, _)| slot.len())
            .sum()
    };
    let map_range = |range: &Range<usize>| {
        range.start + shift_through(range.start)..range.end + shift_before(range.end)
    };
    let slot_run = |at: usize, len: usize| {
        let mut run = slice_runs(&flat.runs, at..flat.text.len())
            .into_iter()
            .next()
            .or_else(|| flat.runs.first().cloned())?;
        run.len = len;
        run.underline = None;
        run.strikethrough = None;
        Some(run)
    };
    let Some(slot_runs): Option<Vec<TextRun>> = insertions
        .iter()
        .map(|(at, slot, _)| slot_run(*at, slot.len()))
        .collect()
    else {
        return flat.clone();
    };

    let mut text = String::with_capacity(
        flat.text.len()
            + insertions
                .iter()
                .map(|(_, slot, _)| slot.len())
                .sum::<usize>(),
    );
    let mut runs: Vec<TextRun> = Vec::new();
    let mut offsets = OffsetMap::default();
    let mut glyphs = Vec::new();
    let mut favicons = Vec::new();
    let mut at = 0;
    for (ix, (insert_at, slot, icon)) in insertions.iter().enumerate() {
        text.push_str(&flat.text[at..*insert_at]);
        runs.extend(slice_runs(&flat.runs, at..*insert_at));
        let start = text.len();
        text.push_str(slot);
        runs.push(slot_runs[ix].clone());
        offsets
            .omissions
            .push((*insert_at..*insert_at, start..text.len()));
        match icon {
            LeadIcon::Favicon(url) => favicons.push((start..text.len(), url.clone())),
            LeadIcon::File(path) => glyphs.push((start..text.len(), path.clone())),
        }
        at = *insert_at;
    }
    text.push_str(&flat.text[at..]);
    runs.extend(slice_runs(&flat.runs, at..flat.text.len()));
    offsets.prior = flat
        .original
        .as_ref()
        .map(|original| Box::new(original.offsets.clone()));
    FlatText {
        text: text.into(),
        runs,
        links: flat
            .links
            .iter()
            .map(|(range, url)| (map_range(range), url.clone()))
            .collect(),
        code_ranges: flat.code_ranges.iter().map(&map_range).collect(),
        file_glyphs: glyphs,
        favicons,
        original: Some(OriginalText {
            text: flat
                .original
                .as_ref()
                .map_or_else(|| flat.text.clone(), |original| original.text.clone()),
            offsets,
        }),
    }
}

/// Whether the whole paragraph is one workspace file link: it renders as the
/// sole-file row (file icon tile), which carries no trailing glyph.
fn paragraph_is_sole_file_link(flat: &FlatText, workspace_root: Option<&str>) -> bool {
    let Some(root) = workspace_root else {
        return false;
    };
    let [(range, target)] = flat.links.as_slice() else {
        return false;
    };
    flat.text[..range.start].trim().is_empty()
        && flat.text[range.end..].trim().is_empty()
        && crate::workspace_links::resolve_workspace_file_link(target, root).is_some()
}
impl IntoElement for ResponsiveText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for ResponsiveText {
    type RequestLayoutState = ();
    type PrepaintState = AnyElement;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, ()) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.pixel_snap(
            style
                .line_height
                .to_pixels(font_size.into(), window.rem_size()),
        );
        let flat = self.flat.clone();
        let opts = self.opts.clone();
        let id = window.request_measured_layout(
            Default::default(),
            move |known, available, window, _| {
                let width = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let shown = width.map(|width| present(&flat, width, font_size, window, &opts));
                let flat = shown.as_ref().unwrap_or(&flat);
                let lines = window
                    .text_system()
                    .shape_text(flat.text.clone(), font_size, &flat.runs, width, None)
                    .unwrap_or_default();
                let mut result: Size<Pixels> = Size::default();
                for line in lines.iter() {
                    let size = line.size(line_height);
                    result.width = result.width.max(size.width).ceil();
                    result.height += size.height;
                }
                // Keep the final width equal to the one that decided truncation.
                if let Some(width) = width {
                    result.width = width;
                }
                result
            },
        );
        (id, ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let flat = present(&self.flat, bounds.size.width, font_size, window, &self.opts);
        let mut child =
            super::render::flat_text_presented_element(&flat, self.ix, &self.opts, &self.theme);
        child.prepaint_as_root(
            bounds.origin,
            bounds.size.map(AvailableSpace::Definite),
            window,
            cx,
        );
        child
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        child: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        child.paint(window, cx);
    }
}

/// Paints the leading file-type icon of every resolved file link over the
/// shaped text — the same reserved-slot overlay pattern, so the NBSP slots
/// keep layout, selection and copy stable.
pub(crate) struct FileLinkGlyphs {
    pub id: SharedString,
    pub child: AnyElement,
    pub layout: TextLayout,
    /// Leading file icon slots of resolved file links, with their paths.
    pub slots: Vec<(Range<usize>, String)>,
    /// Leading favicon slots of web links, with their URLs.
    pub favicons: Vec<(Range<usize>, String)>,
}

impl IntoElement for FileLinkGlyphs {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for FileLinkGlyphs {
    type RequestLayoutState = ();
    type PrepaintState = Vec<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone().into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        self.child.prepaint(window, cx);
        let theme = Theme::of(cx).clone();
        let mut overlays = Vec::new();
        for (slot, path) in &self.slots {
            let Some(rect) = range_rects(&self.layout, slot, 0.0, INLINE_CODE_INSET_Y)
                .first()
                .copied()
            else {
                continue;
            };
            let side = px(FILE_GLYPH_SIZE).min(rect.size.height);
            let mut icon_el = crate::file_icons::icon(
                crate::file_icons::FileIconIdentity::file(path),
                theme.appearance,
            )
            .size(side)
            .into_any_element();
            icon_el.prepaint_as_root(
                point(
                    rect.origin.x + px(1.0),
                    rect.origin.y + (rect.size.height - side) / 2.0,
                ),
                size(side, side).map(AvailableSpace::Definite),
                window,
                cx,
            );
            overlays.push(icon_el);
        }
        let tint = crate::site_links::link_color(&theme);
        for (slot, url) in &self.favicons {
            let Some(rect) = range_rects(&self.layout, slot, 0.0, INLINE_CODE_INSET_Y)
                .first()
                .copied()
            else {
                continue;
            };
            let side = px(FAVICON_SIZE).min(rect.size.height);
            let origin = point(
                rect.origin.x + px(1.0),
                rect.origin.y + (rect.size.height - side) / 2.0,
            );
            let url = url.clone();
            let mut favicon = gpui::canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    crate::site_links::paint_favicon(window, bounds, &url, tint, cx)
                },
            )
            .size(side)
            .into_any_element();
            favicon.prepaint_as_root(
                origin,
                size(side, side).map(AvailableSpace::Definite),
                window,
                cx,
            );
            overlays.push(favicon);
        }
        overlays
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        overlays: &mut Vec<AnyElement>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
        for overlay in overlays {
            overlay.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        parser::{InlineRun, InlineStyle},
        render::flatten_runs,
    };
    use super::*;
    fn link(text: &str) -> InlineRun {
        InlineRun {
            text: text.into(),
            style: InlineStyle {
                link: Some("https://example.com/destination".into()),
                ..Default::default()
            },
        }
    }
    fn measured(text: &str, _: &[TextRun]) -> f32 {
        text.graphemes(true).count() as f32
    }

    #[test]
    fn file_link_glyphs_reserve_a_slot_only_for_resolved_files() {
        use super::super::render::{LinkOutcome, LinkUi};
        let run = |text: &str, link: Option<&str>| InlineRun {
            text: text.into(),
            style: InlineStyle {
                link: link.map(str::to_owned),
                ..Default::default()
            },
        };
        let flat = flatten_runs(
            &[
                run("see ", None),
                run("src/lib.rs", Some("src/lib.rs")),
                run(" and ", None),
                run("docs", Some("https://example.com/x")),
                run(" and ", None),
                run("elsewhere.rs", Some("../elsewhere.rs")),
            ],
            &Theme::dark(),
            false,
        );
        let mut opts = RenderOptions::settled("row".into());
        opts.workspace_root = Some("/repo".into());
        opts.link = Some(LinkUi {
            source_session: Some("chat".into()),
            source_local: true,
            file_roots: Some(Rc::new(vec![crate::workspace_links::FileLinkRoot {
                chat: Some("chat".into()),
                root: "/repo".into(),
                local: true,
            }])),
            handler: Rc::new(|_, _, _| LinkOutcome::Rejected),
        });
        let shown = with_file_link_glyphs(&flat, &opts);
        // Only the resolved file link grows a slot: the web link and the
        // escaped target keep their shape.
        assert_eq!(shown.file_glyphs.len(), 1);
        assert_eq!(&shown.text[shown.file_glyphs[0].0.clone()], FILE_GLYPH_SLOT);
        assert_eq!(shown.links.len(), 3);
        // The reserved slot sits right before the link and never reaches
        // copy or selection — both icon ends map back to the link's start.
        let slot = &shown.file_glyphs[0].0;
        assert_eq!(slot.end, shown.links[0].0.start);
        let map = &shown.original.as_ref().unwrap().offsets;
        assert_eq!(map.original(slot.start), map.original(slot.end));

        // A paragraph that is nothing but one file link stays the sole-file
        // row (icon tile), never the inline icon.
        let sole = flatten_runs(
            &[run("src/lib.rs", Some("src/lib.rs"))],
            &Theme::dark(),
            false,
        );
        assert!(with_file_link_glyphs(&sole, &opts).file_glyphs.is_empty());

        // Without a root snapshot no link grows a glyph (previews, web-only
        // surfaces).
        let mut no_roots = opts.clone();
        no_roots.link = Some(LinkUi {
            source_session: Some("chat".into()),
            source_local: false,
            file_roots: None,
            handler: Rc::new(|_, _, _| LinkOutcome::Rejected),
        });
        assert!(
            with_file_link_glyphs(&flat, &no_roots)
                .file_glyphs
                .is_empty()
        );
    }
    #[test]
    fn truncation_preserves_graphemes_destinations_and_original_selection() {
        let label = "https://example.com/á🙂👨‍👩‍👧‍👦界/very/long/path";
        let flat = flatten_runs(&[link(label)], &Theme::dark(), false);
        for width in [1., 12., 25., 32.] {
            let shown = truncate(&flat, width, measured);
            assert!(measured(&shown.text, &[]) <= width);
            let original = shown.original.as_ref().unwrap();
            assert_eq!(original.text.as_ref(), label);
            assert_eq!(original.offsets.original(shown.text.len()), label.len());
            assert_eq!(shown.links[0].1, flat.links[0].1);
            assert_eq!(
                shown.runs.iter().map(|r| r.len).sum::<usize>(),
                shown.text.len()
            );
            for (i, _) in shown.text.char_indices() {
                assert!(label.is_char_boundary(original.offsets.original(i)));
            }
        }
        let shown = truncate(&flat, 25., measured);
        assert!(shown.text.starts_with("https://example.com/"));
    }
    #[test]
    fn partial_and_cross_block_copies_map_back_to_source() {
        let source = "https://example.com/abcdefghijklmnopqrstuvwxyz";
        let flat = flatten_runs(
            &[
                link(source),
                InlineRun {
                    text: " between ".into(),
                    style: Default::default(),
                },
                link("https://second.example/long/path/to/resource"),
            ],
            &Theme::dark(),
            false,
        );
        let shown = truncate(&flat, 24., measured);
        let map = &shown.original.as_ref().unwrap().offsets;
        assert_eq!(map.omissions.len(), 2);
        let first = &shown.links[0].0;
        assert_eq!(
            &flat.text[map.original(first.start)..map.original(first.end)],
            source
        );
        assert_eq!(&flat.text[map.original(0)..map.original(5)], "https");
        let end = map.original(shown.text.len());
        let spans = super::super::selection::resolve_spans(
            &[("a", &flat.text), ("b", "next block")],
            (0, 0),
            (1, 4),
        );
        let copied = spans
            .iter()
            .map(|span| &span.text[span.range.clone()])
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(copied, format!("{}\nnext", &flat.text[..end]));
        let omission = &map.omissions[0];
        assert_eq!(
            map.displayed_range(omission.0.start + 1..omission.0.end - 1),
            omission.1
        );
    }
    #[test]
    fn widening_and_streaming_recompute_presentation_without_stale_offsets() {
        let flat = flatten_runs(
            &[link("https://example.com/a/long/link")],
            &Theme::dark(),
            false,
        );
        let narrow = truncate(&flat, 15., measured);
        let wide = truncate(&flat, 1000., measured);
        assert!(narrow.text.ends_with('…'));
        assert_eq!(wide.text, flat.text);
        assert!(wide.original.as_ref().unwrap().offsets.omissions.is_empty());
        let growing = flatten_runs(
            &[link("https://example.com/a/long/link/streaming-tail")],
            &Theme::dark(),
            false,
        );
        let next = truncate(&growing, 15., measured);
        assert_eq!(
            next.original.unwrap().offsets.original(next.text.len()),
            growing.text.len()
        );
    }
    #[test]
    fn styled_link_runs_are_trimmed_without_corrupting_offsets() {
        let mut bold = link("very-long-bold-suffix");
        bold.style.bold = true;
        let flat = flatten_runs(&[link("https://example.com/"), bold], &Theme::dark(), false);
        let shown = truncate(&flat, 24., measured);
        assert_eq!(shown.links.len(), 1);
        assert!(shown.runs.last().unwrap().font.weight.0 >= gpui::FontWeight::SEMIBOLD.0);
        assert_eq!(
            shown.runs.iter().map(|r| r.len).sum::<usize>(),
            shown.text.len()
        );
    }
}
