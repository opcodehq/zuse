//! Flatten inline Markdown into shaped text, then paint code washes, chips, and links.

use std::{ops::Range, rc::Rc};

use gpui::{
    AnyElement, BorderStyle, FontStyle, FontWeight, Hsla, SharedString, StyledText, TextRun,
    UnderlineStyle, canvas, div, font, prelude::*, px, quad,
};

use crate::{
    markdown::{parser::InlineRun, veil::apply_veil},
    theme::Theme,
};

use super::{
    LinkTarget, RenderOptions,
    selection_paint::{paint_text_selection_with_wash, range_rects, selectable_text_wrap},
};

/// Base weight plus an optional color for every plain (non-code, non-link)
/// run of a text block. `None` keeps the body and emphasis colors.
#[derive(Clone, Copy)]
pub(super) struct TextStyle {
    pub(super) weight: FontWeight,
    pub(super) color: Option<Hsla>,
}

impl TextStyle {
    pub(super) fn body(bold_default: bool) -> Self {
        Self {
            weight: if bold_default {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            },
            color: None,
        }
    }
}

/// Flattened inline runs: one string + gpui `TextRun`s + clickable link ranges
/// + inline-code ranges (their rounded washes are painted by a canvas UNDER
/// the text — `TextRun::background_color` can only paint square boxes).
/// `text` is a `SharedString` so cached reuse across frames is an Arc clone.
#[derive(Clone)]
pub struct FlatText {
    pub original: Option<crate::markdown::link_presentation::OriginalText>,
    pub text: SharedString,
    pub runs: Vec<TextRun>,
    pub links: Vec<(Range<usize>, String)>,
    pub code_ranges: Vec<Range<usize>>,
    pub(crate) user_literals: Vec<(usize, Range<usize>)>,
    /// Reserved slots before resolved file links, with each link's path —
    /// displayed-text coordinates. Each slot paints the file-type icon.
    pub file_glyphs: Vec<(Range<usize>, String)>,
    /// Reserved slots before web links, with each link's URL — displayed-
    /// text coordinates. Each slot paints the site's favicon.
    pub favicons: Vec<(Range<usize>, String)>,
}

/// An `http(s)` destination: a web link, which gets a favicon and the link
/// colour (files and internal links keep the monochrome treatment).
pub(crate) fn is_web_link(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && crate::site_links::host(url).is_some()
}

/// Inline-code ink: neutral strong text over a faint plate.
pub fn inline_code_text(theme: &Theme) -> Hsla {
    theme.code_text
}
pub fn inline_code_wash(theme: &Theme) -> Hsla {
    theme.code_wash
}
/// Inline code inside a user bubble: the bubble is itself a wash, so the
/// plate steps up to stay distinct from it.
pub fn user_inline_code_wash(theme: &Theme) -> Hsla {
    theme.wash(0.12)
}
/// Rounded-wash geometry: small radius on a slightly inset box (paint-only —
/// x extends 1px past the padding glyphs, y insets 2px from the 22px line box).
pub const INLINE_CODE_RADIUS: f32 = 4.0;
pub const INLINE_CODE_PAD_X: f32 = 1.0;
pub const INLINE_CODE_INSET_Y: f32 = 2.0;
/// Shown (never copied) inside each end of an inline-code wash so the plate
/// has real horizontal padding. NARROW NO-BREAK SPACE rather than a thin
/// space: the line wrapper treats it as glue, so a pad never wraps away from
/// its code.
pub(crate) const INLINE_CODE_PAD: &str = "\u{202F}";

/// A padding glyph's run: the code run's paint in the prose font, whose
/// narrow space is proportional (a mono font may draw it a full cell wide).
fn inline_code_pad_run(code_run: &TextRun, theme: &Theme) -> TextRun {
    let mut run = code_run.clone();
    run.len = INLINE_CODE_PAD.len();
    run.font.family = theme.font_sans.clone();
    run.strikethrough = None;
    run
}

/// Flatten inline runs into shaped-text inputs. Pure given a theme.
pub fn flatten_runs(runs: &[InlineRun], theme: &Theme, bold_default: bool) -> FlatText {
    flatten_runs_weighted(
        runs,
        theme,
        TextStyle::body(bold_default).weight,
        None,
        false,
    )
}

/// [`flatten_runs`] with an explicit base weight (table headers are 700 per
/// zuse's `table.headerWeight`; strong runs in prose are bold, and never drop
/// below a semibold base). `plain_color` overrides the color of every run that
/// is neither code nor a link (headings).
pub(super) fn flatten_runs_weighted(
    runs: &[InlineRun],
    theme: &Theme,
    base_weight: FontWeight,
    plain_color: Option<Hsla>,
    transcript: bool,
) -> FlatText {
    let body_color = if transcript {
        theme.transcript_text()
    } else {
        theme.text
    };
    let emphasis_color = if body_color == theme.text {
        theme.text_strong
    } else {
        crate::theme::mix(body_color, theme.text_strong, 0.55)
    };
    let mut text = String::new();
    let mut out: Vec<TextRun> = Vec::with_capacity(runs.len());
    let mut links: Vec<(Range<usize>, String)> = Vec::new();
    let mut code_ranges: Vec<Range<usize>> = Vec::new();
    let mut user_literals = Vec::new();
    // A run may stand for a longer source text than it shows (a file link
    // labeled by its file name). The source is only kept when some run does.
    // A bare link to a known site shows its short label (`owner/repo#12`),
    // as the composer and sent messages do; the URL stays the copy source.
    let site_label = |run: &InlineRun| {
        run.style
            .link
            .as_deref()
            .filter(|url| *url == run.text.as_str() && is_web_link(url))
            .and_then(crate::site_links::known_site_link)
            .map(|(_, label)| label)
    };
    let site_labels: Vec<Option<String>> = runs.iter().map(site_label).collect();
    // Display-only runs (a sent message's indentation) show without a
    // source, so copy and selection skip them.
    let relabeled = runs.iter().any(|run| {
        run.style.file_label.is_some()
            || run.style.display_only
            || (run.text.is_empty() && run.style.image.is_some())
    }) || site_labels.iter().any(Option::is_some)
        || runs
            .iter()
            .any(|run| run.style.code && !run.text.is_empty());
    let mut source = String::new();
    let mut omissions: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    // Inline-code padding: the last run of the open code span, whose style
    // its trailing pad takes.
    let mut open_code: Option<TextRun> = None;
    for (run, site_label) in runs.iter().zip(&site_labels) {
        if run.text.is_empty() && run.style.image.is_none() {
            continue;
        }
        if !run.style.code
            && let Some(code_run) = open_code.take()
        {
            close_inline_code(&mut text, &source, &mut omissions, &mut code_ranges);
            out.push(inline_code_pad_run(&code_run, theme));
        }
        let pad_start = (run.style.code && open_code.is_none()).then(|| {
            open_inline_code(&mut text, &source, &mut omissions);
            text.len() - INLINE_CODE_PAD.len()
        });
        let shown = run
            .style
            .file_label
            .as_deref()
            .or(site_label.as_deref())
            .unwrap_or_else(|| {
                if run.text.is_empty() && run.style.image.is_some() {
                    "Image"
                } else {
                    &run.text
                }
            });
        let start = text.len();
        text.push_str(shown);
        if let Some(index) = run.style.user_literal {
            user_literals.push((index, start..text.len()));
        }
        if relabeled {
            let source_start = source.len();
            if !run.style.display_only {
                source.push_str(&run.text);
            }
            if run.style.display_only || shown != run.text.as_str() {
                omissions.push((source_start..source.len(), start..text.len()));
            }
        }
        let mut f = if run.style.code {
            font(theme.font_mono.clone())
        } else {
            font(theme.font_sans.clone())
        };
        f.weight = if run.style.bold && base_weight.0 < FontWeight::SEMIBOLD.0 {
            FontWeight::BOLD
        } else {
            base_weight
        };
        f.style = if run.style.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        // Web links read info blue at medium weight with a soft underline
        // and lead with their favicon; other links (files, internal) stay
        // monochrome — foreground with an underline.
        let is_link = run.style.link.is_some();
        let web = run.style.link.as_deref().is_some_and(is_web_link);
        if run.style.footnote {
            f.weight = FontWeight::SEMIBOLD;
        } else if web && !transcript && !run.style.code && f.weight.0 < FontWeight::MEDIUM.0 {
            f.weight = FontWeight::MEDIUM;
        }
        // Keep inline code, links, and emphasis clear against the prose.
        let color = if run.style.code {
            inline_code_text(theme)
        } else if web {
            crate::site_links::link_color(theme)
        } else if is_link {
            theme.text
        } else if run.style.footnote || run.style.strikethrough {
            // Struck text recedes along with its line.
            theme.text_muted
        } else if let Some(color) = plain_color {
            color
        } else if transcript && f.weight.0 >= FontWeight::SEMIBOLD.0 {
            emphasis_color
        } else {
            body_color
        };
        if run.style.code {
            // Merge adjacent code runs into one wash box (like links below);
            // a new span's box starts at its leading pad.
            match (pad_start, code_ranges.last_mut()) {
                (None, Some(range)) => range.end = text.len(),
                _ => code_ranges.push(pad_start.unwrap_or(start)..text.len()),
            }
        }
        if let Some(url) = &run.style.link {
            // A still-streaming link (mend.rs sentinel) keeps link styling —
            // so the URL's completion changes nothing visually — but is not
            // clickable until the real destination exists.
            if url != crate::markdown::mend::PENDING_LINK_URL {
                // Merge adjacent runs of the same link into one clickable range.
                // A code pad between two runs of one link stays inside it.
                match links.last_mut() {
                    Some((range, last_url))
                        if last_url == url
                            && (range.end == start
                                || text.get(range.end..start) == Some(INLINE_CODE_PAD)) =>
                    {
                        range.end = text.len();
                    }
                    _ => links.push((start..text.len(), url.clone())),
                }
            }
        }
        let text_run = TextRun {
            len: shown.len(),
            font: f,
            color,
            // Inline code's wash is painted as ROUNDED quads by the canvas
            // underlay (`code_wash_underlay`) — a run background here could
            // only be a square box.
            background_color: None,
            underline: (is_link && !transcript).then_some(UnderlineStyle {
                color: Some(if web {
                    crate::site_links::link_underline(theme)
                } else {
                    theme.text_muted
                }),
                thickness: px(1.0),
                wavy: false,
            }),
            strikethrough: run.style.strikethrough.then_some(gpui::StrikethroughStyle {
                thickness: px(1.0),
                color: Some(theme.text_muted),
            }),
        };
        if run.style.code {
            if pad_start.is_some() {
                out.push(inline_code_pad_run(&text_run, theme));
            }
            open_code = Some(text_run.clone());
        }
        out.push(text_run);
    }
    if let Some(code_run) = open_code.take() {
        close_inline_code(&mut text, &source, &mut omissions, &mut code_ranges);
        out.push(inline_code_pad_run(&code_run, theme));
    }
    FlatText {
        // The source survives for copy and selection only where a label
        // actually replaced text.
        original: (!omissions.is_empty()).then(|| {
            crate::markdown::link_presentation::OriginalText {
                text: source.into(),
                offsets: crate::markdown::link_presentation::OffsetMap {
                    omissions,
                    prior: None,
                },
            }
        }),
        text: text.into(),
        runs: out,
        links,
        code_ranges,
        user_literals,
        file_glyphs: Vec::new(),
        favicons: Vec::new(),
    }
}

/// Show a leading inline-code pad: displayed only, so copy and selection
/// read the source without it.
fn open_inline_code(
    text: &mut String,
    source: &str,
    omissions: &mut Vec<(Range<usize>, Range<usize>)>,
) {
    let shown = text.len();
    text.push_str(INLINE_CODE_PAD);
    omissions.push((source.len()..source.len(), shown..text.len()));
}

/// Show the trailing pad of the open code span and extend its wash over it.
fn close_inline_code(
    text: &mut String,
    source: &str,
    omissions: &mut Vec<(Range<usize>, Range<usize>)>,
    code_ranges: &mut [Range<usize>],
) {
    open_inline_code(text, source, omissions);
    if let Some(range) = code_ranges.last_mut() {
        range.end = text.len();
    }
}

/// Flatten through the cross-frame cache when one is wired: settled blocks
/// reuse text + runs untouched (O(1) per block per frame); only blocks the
/// incremental parser invalidated rebuild.
pub(super) fn flatten_cached(
    runs: &[InlineRun],
    style: TextStyle,
    top_ix: usize,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
) -> Rc<FlatText> {
    let flatten = || {
        Rc::new(flatten_runs_weighted(
            runs,
            theme,
            style.weight,
            style.color,
            opts.transcript,
        ))
    };
    match &opts.cache {
        Some(cache) => {
            let mut cache = cache.borrow_mut();
            cache.sync_style();
            cache
                .flats
                .entry((opts.row_key.clone(), top_ix, ix))
                .or_insert_with(flatten)
                .clone()
        }
        None => flatten(),
    }
}

/// Veiled, clickable text for a flattened block (no sizing wrapper).
pub(super) fn flat_text_element(
    flat: &Rc<FlatText>,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
) -> AnyElement {
    if opts
        .link
        .as_ref()
        .is_some_and(|ui| ui.source_session.is_some())
        && !flat.links.is_empty()
    {
        return crate::markdown::link_presentation::ResponsiveText {
            flat: flat.clone(),
            ix,
            opts: opts.clone(),
            theme: theme.clone(),
        }
        .into_any_element();
    }
    flat_text_presented_element(flat, ix, opts, theme)
}

pub(in crate::markdown) fn flat_text_presented_element(
    flat: &FlatText,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
) -> AnyElement {
    // Streaming veil: opacity-only recolor of the runs covering newly appended
    // chunks. Same text, same fonts, same lengths — layout is untouched.
    // Settled elements return no spans and reuse the cached runs unsplit.
    let text_runs = match &opts.veil {
        Some(veil) => {
            let original = flat
                .original
                .as_ref()
                .map_or(&flat.text, |original| &original.text);
            let spans = veil.borrow_mut().advance(ix, original, opts.now);
            let spans = if let Some(original) = &flat.original {
                spans
                    .into_iter()
                    .filter_map(|(range, opacity)| {
                        let range = original.offsets.displayed(range.start)
                            ..original.offsets.displayed(range.end);
                        (!range.is_empty()).then_some((range, opacity))
                    })
                    .collect()
            } else {
                spans
            };
            apply_veil(flat.runs.clone(), &spans)
        }
        None => flat.runs.clone(),
    };
    let chips = opts
        .user_chips
        .as_ref()
        .and_then(|chips| chips.for_ranges(&flat.user_literals));
    let text_runs = match &chips {
        Some(chips) => chips.style_runs(&flat.text, text_runs, theme),
        None => text_runs,
    };
    let styled = StyledText::new(flat.text.clone()).with_runs(text_runs);
    let layout = styled.layout().clone();
    let text_el = styled.into_any_element();
    let link_layout = layout.clone();
    // Underlay canvas: inline-code washes + the selection wash, painted
    // BEFORE the text (earlier sibling ⇒ underneath), reading glyph geometry
    // from the text's own layout handle. Pure paint — never in layout. The
    // same paint pass re-registers the frame-scoped window mouse listeners
    // that drive text selection (round 18; see markdown/selection.rs).
    let sel_key: std::sync::Arc<str> = format!("{}:{ix}", opts.row_key).into();
    let code_ranges = flat.code_ranges.clone();
    let flat_text = flat
        .original
        .as_ref()
        .map_or_else(|| flat.text.clone(), |original| original.text.clone());
    let offsets = flat
        .original
        .as_ref()
        .map(|original| original.offsets.clone());
    let wash = if opts.user {
        user_inline_code_wash(theme)
    } else {
        inline_code_wash(theme)
    };
    let sel_wash = theme.selection;
    let underlay = canvas(
        |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            for range in &code_ranges {
                for rect in range_rects(&layout, range, INLINE_CODE_PAD_X, INLINE_CODE_INSET_Y) {
                    window.paint_quad(quad(
                        rect,
                        px(INLINE_CODE_RADIUS),
                        wash,
                        px(0.0),
                        gpui::transparent_black(),
                        BorderStyle::default(),
                    ));
                }
            }
            paint_text_selection_with_wash(
                window,
                hitbox,
                &sel_key,
                &flat_text,
                &layout,
                sel_wash,
                offsets.as_ref(),
            );
        },
    )
    .absolute()
    .size_full();
    let child = selectable_text_wrap()
        .children(
            chips
                .as_ref()
                .map(|chips| chips.underlay(link_layout.clone(), theme.clone())),
        )
        .child(underlay)
        .child(text_el)
        .into_any_element();
    let child = match chips {
        Some(chips) => chips.wrap(
            format!("{}-chips-{ix}", opts.row_key).into(),
            child,
            link_layout.clone(),
            theme,
        ),
        None => child,
    };
    if flat.links.is_empty() {
        return child;
    }
    // A link's leading favicon or file icon slot activates the link too.
    let favicon_start = |range: &std::ops::Range<usize>| {
        flat.favicons
            .iter()
            .chain(&flat.file_glyphs)
            .find(|(slot, _)| slot.end == range.start)
            .map_or(range.start, |(slot, _)| slot.start)
    };
    let child = crate::markdown::link_interaction::LinkRanges {
        id: format!(
            "{}-{}-t{ix}",
            opts.row_key,
            opts.link
                .as_ref()
                .and_then(|ui| ui.source_session.as_deref())
                .unwrap_or_default()
        )
        .into(),
        child,
        layout: link_layout.clone(),
        links: flat
            .links
            .iter()
            .map(|(range, url)| {
                (
                    favicon_start(range)..range.end,
                    LinkTarget::new(
                        flat.original
                            .as_ref()
                            .map_or(&flat.text[range.clone()], |original| {
                                &original.text[original.offsets.original(range.start)
                                    ..original.offsets.original(range.end)]
                            }),
                        url,
                    ),
                )
            })
            .collect(),
        ui: opts.link.clone(),
        underline_on_hover: opts.transcript,
    }
    .into_any_element();
    // Leading file icons of resolved file links and favicons of web links
    // paint over the shaped text.
    if flat.file_glyphs.is_empty() && flat.favicons.is_empty() {
        return child;
    }
    crate::markdown::link_presentation::FileLinkGlyphs {
        id: format!("{}-file-glyphs-{ix}", opts.row_key).into(),
        child,
        layout: link_layout,
        slots: flat.file_glyphs.clone(),
        favicons: flat.favicons.clone(),
    }
    .into_any_element()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn text_element(
    runs: &[InlineRun],
    size: f32,
    line_height: f32,
    bold_default: bool,
    top_ix: usize,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
) -> AnyElement {
    text_element_styled(
        runs,
        size,
        line_height,
        TextStyle::body(bold_default),
        top_ix,
        ix,
        opts,
        theme,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn text_element_styled(
    runs: &[InlineRun],
    size: f32,
    line_height: f32,
    style: TextStyle,
    top_ix: usize,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
) -> AnyElement {
    if let Some(image_ui) = opts.media.as_ref().and_then(|media| media.image.as_ref())
        && let Some(image) = runs.iter().find_map(|run| run.style.image.as_ref())
        && runs
            .iter()
            .filter(|run| !run.text.trim().is_empty() || run.style.image.is_some())
            .count()
            == 1
    {
        return image_ui(image, format!("{}-image-{ix}", opts.row_key).into(), theme);
    }
    if let Some(lines) = opts
        .workspace_root
        .as_deref()
        .and_then(|root| plain_file_reference_lines(runs, root))
    {
        let run_style = runs[0].style.clone();
        return div()
            .flex()
            .flex_col()
            .children(lines.into_iter().enumerate().map(|(line_ix, text)| {
                text_element_styled(
                    &[InlineRun {
                        text,
                        style: run_style.clone(),
                    }],
                    size,
                    line_height,
                    style,
                    top_ix,
                    ix.wrapping_mul(4099).wrapping_add(line_ix + 2000),
                    opts,
                    theme,
                )
            }))
            .into_any_element();
    }
    let flat = flatten_cached(runs, style, top_ix, ix, opts, theme);
    let inner = flat_text_element(&flat, ix, opts, theme);
    let direct_file = opts
        .workspace_root
        .as_deref()
        .and_then(|root| sole_file_reference(runs, root))
        .or_else(|| sole_image_file_link(runs));
    let content = if let Some(path) = direct_file {
        div()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(file_icon_well(&path, theme))
            .child(div().min_w_0().flex_1().child(inner))
            .into_any_element()
    } else {
        inner
    };
    div()
        .text_size(crate::typography::ui_rems(size))
        .line_height(crate::typography::ui_rems(line_height))
        .child(content)
        .into_any_element()
}

/// The file-type icon in its small rounded well, as it leads a paragraph that
/// is one file link (a photo for an image). Inline images use the same lead.
pub(crate) fn file_icon_well(path: &str, theme: &Theme) -> AnyElement {
    div()
        .size(px(20.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(crate::file_icons::well_bg(theme))
        .child(
            crate::file_icons::icon(
                crate::file_icons::FileIconIdentity::file(path),
                theme.appearance,
            )
            .size(px(14.0)),
        )
        .into_any_element()
}

/// A file icon belongs beside a link only when the whole visible paragraph is
/// one safe workspace-file target. Mixed prose and external links retain the
/// ordinary inline Markdown layout.
pub(super) fn sole_workspace_file_link(runs: &[InlineRun], workspace_root: &str) -> Option<String> {
    let mut target = None;
    for run in runs.iter().filter(|run| !run.text.is_empty()) {
        if run.style.image.is_some() || run.style.task.is_some() {
            return None;
        }
        let link = run.style.link.as_deref()?;
        if link == crate::markdown::mend::PENDING_LINK_URL {
            return None;
        }
        match target {
            Some(previous) if previous != link => return None,
            None => target = Some(link),
            _ => {}
        }
    }
    crate::workspace_links::resolve_workspace_file_link(target?, workspace_root)
        .map(|link| link.path)
}

/// A paragraph that is one link to an image file keeps its photo lead even
/// where no workspace root is known yet (a cloud chat whose rows are still
/// loading, or a file outside every checkout), so image links always read
/// the same.
pub(super) fn sole_image_file_link(runs: &[InlineRun]) -> Option<String> {
    let mut target = None;
    for run in runs.iter().filter(|run| !run.text.is_empty()) {
        if run.style.image.is_some() || run.style.task.is_some() {
            return None;
        }
        let link = run.style.link.as_deref()?;
        match target {
            Some(previous) if previous != link => return None,
            None => target = Some(link),
            _ => {}
        }
    }
    let target = target?;
    if target == crate::markdown::mend::PENDING_LINK_URL
        || is_web_link(target)
        || target.contains("://")
    {
        return None;
    }
    let path = target.split(['#', '?']).next().unwrap_or(target);
    crate::attachments::is_image_path(path).then(|| path.to_owned())
}

/// A model sometimes emits a bare filename after being asked for a file list
/// instead of preserving the workspace link it would normally author. Treat a
/// whole paragraph as a decorative file identity only when the token resolves
/// safely within the workspace and the icon theme recognizes it. The stricter
/// mapping check keeps version numbers, domains, and unknown dotted prose from
/// acquiring file affordances; unlike a real link, this stays non-interactive.
pub(super) fn sole_plain_file_reference(
    runs: &[InlineRun],
    workspace_root: &str,
) -> Option<String> {
    let mut text = String::new();
    for run in runs.iter().filter(|run| !run.text.is_empty()) {
        if run.style.link.is_some()
            || run.style.image.is_some()
            || run.style.task.is_some()
            || run.style.code
        {
            return None;
        }
        text.push_str(&run.text);
    }
    let candidate = text.trim();
    if candidate.is_empty() || candidate.chars().any(char::is_whitespace) {
        return None;
    }
    let path = crate::workspace_links::resolve_workspace_file_link(candidate, workspace_root)?.path;
    crate::file_icons::has_specific_file_icon(&path).then_some(path)
}

/// Preserve hard-break file lists as one compact paragraph while giving each
/// line its own icon. Limiting this path to one uniformly styled run avoids
/// rewriting mixed inline formatting or ordinary wrapped prose.
pub(super) fn plain_file_reference_lines(
    runs: &[InlineRun],
    workspace_root: &str,
) -> Option<Vec<String>> {
    let [run] = runs else { return None };
    if run.style.link.is_some()
        || run.style.image.is_some()
        || run.style.task.is_some()
        || run.style.code
        || !run.text.contains('\n')
    {
        return None;
    }
    let lines = run
        .text
        .lines()
        .map(str::trim)
        .map(|candidate| {
            if candidate.is_empty() || candidate.chars().any(char::is_whitespace) {
                return None;
            }
            let path =
                crate::workspace_links::resolve_workspace_file_link(candidate, workspace_root)?
                    .path;
            crate::file_icons::has_specific_file_icon(&path).then(|| candidate.to_owned())
        })
        .collect::<Option<Vec<_>>>()?;
    (lines.len() > 1).then_some(lines)
}

pub(super) fn sole_file_reference(runs: &[InlineRun], workspace_root: &str) -> Option<String> {
    sole_workspace_file_link(runs, workspace_root)
        .or_else(|| sole_plain_file_reference(runs, workspace_root))
}
