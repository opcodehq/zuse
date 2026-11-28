//! Code fence layout, syntax runs, and interaction state.

use std::{collections::HashMap, rc::Rc};

use gpui::{
    AnyElement, Context, CursorStyle, Div, Hsla, Render, SharedString, TextRun, Window, div, font,
    prelude::*, px,
};
use zuse_syntax::{HighlightKind, HighlightSpan};

use crate::{
    markdown::veil::{apply_veil, slice_spans},
    theme::Theme,
    tooltip::TooltipExt as _,
};

use super::{
    CachedCode, CodeHighlight, DiagramView, RenderOptions, selection_paint::selectable_text_element,
};

/// Default code block metrics; the rendered size comes from the theme.
pub const CODE_TEXT_SIZE: f32 = crate::typography::CODE_FONT_SIZE_DEFAULT;
/// Line height as a multiple of the code size (T3 `leading-snug`), so a
/// user-chosen size keeps the default's row rhythm.
const CODE_LINE_HEIGHT_RATIO: f32 = 1.375;
pub const CODE_LINE_HEIGHT: f32 = CODE_TEXT_SIZE * CODE_LINE_HEIGHT_RATIO;
/// Fence body padding (T3 `pre`: 0.8rem / 0.9rem).
pub const CODE_PADDING_X: f32 = 14.4;
pub const CODE_PADDING_Y: f32 = 12.8;
/// Fence header: no divider or plate, 6/6/0/12 padding around the label row.
const CODE_HEADER_PAD_TOP: f32 = 6.0;
const CODE_HEADER_PAD_RIGHT: f32 = 6.0;
const CODE_HEADER_PAD_LEFT: f32 = 12.0;
/// Label row height when a header has no actions (an action row is taller).
const CODE_HEADER_LABEL_HEIGHT: f32 = 20.0;
const CODE_HEADER_TEXT_SIZE: f32 = 11.0;
const CODE_FRAME_RADIUS: f32 = 8.0;
/// Icon-only header actions (T3 `icon-xs`).
const CODE_ACTION_SIZE: f32 = 24.0;
const CODE_ACTION_ICON_SIZE: f32 = 12.0;
const CODE_SCROLLBAR_HIT_HEIGHT: f32 = 10.0;
type HoverHandler = Rc<dyn Fn(bool, &mut Window, &mut gpui::App)>;
type PointerHandler = Rc<dyn Fn(gpui::Pixels, &mut Window, &mut gpui::App)>;
type ReleaseHandler = Rc<dyn Fn(&mut Window, &mut gpui::App)>;

/// Transcript-owned interaction for the one fenced block represented by a
/// virtualized Markdown row. The renderer owns only layout/chrome; callbacks
/// keep durable preference and mutable scroll state in [`crate::transcript`].
#[derive(Clone)]
pub struct CodeUi {
    pub key: SharedString,
    pub fit_content: bool,
    pub scroll: gpui::ScrollHandle,
    pub scrollbar: Option<CodeScrollbarUi>,
    pub toggle_fit: Rc<dyn Fn(&mut Window, &mut gpui::App)>,
    pub viewport_hover: HoverHandler,
    pub drag_move: PointerHandler,
}

#[derive(Clone)]
pub struct CodeScrollbarUi {
    pub metrics: crate::popover::HorizontalScrollbarMetrics,
    pub active: bool,
    pub hover: HoverHandler,
    pub press: PointerHandler,
    pub release: ReleaseHandler,
}

#[derive(Default)]
pub struct CodeFenceRuntime {
    pub scroll: gpui::ScrollHandle,
    pub scrollbar: crate::popover::HorizontalScrollbarState,
}

/// Build the interaction state shared by chat and file-preview code fences.
/// The durable Fit choice is global; scroll and drag state stay with one fence.
pub fn code_ui_for<V, F>(
    key: SharedString,
    fit_content: bool,
    runtime: &mut CodeFenceRuntime,
    entity: gpui::WeakEntity<V>,
    runtimes: F,
) -> CodeUi
where
    V: 'static,
    F: Fn(&mut V) -> &mut HashMap<SharedString, CodeFenceRuntime> + Clone + 'static,
{
    let scroll = runtime.scroll.clone();
    let scrollbar = (!fit_content)
        .then(|| runtime.scrollbar.metrics(&scroll))
        .flatten()
        .filter(|_| runtime.scrollbar.visible())
        .map(|metrics| CodeScrollbarUi {
            metrics,
            active: runtime.scrollbar.active(),
            hover: {
                let entity = entity.clone();
                let key = key.clone();
                let runtimes = runtimes.clone();
                Rc::new(move |hovered, _, cx| {
                    let _ = entity.update(cx, |owner, cx| {
                        if runtimes(owner)
                            .get_mut(&key)
                            .is_some_and(|runtime| runtime.scrollbar.set_bar_hovered(hovered))
                        {
                            cx.notify();
                        }
                    });
                })
            },
            press: {
                let entity = entity.clone();
                let key = key.clone();
                let runtimes = runtimes.clone();
                Rc::new(move |pointer_x, _, cx| {
                    let _ = entity.update(cx, |owner, cx| {
                        let Some(runtime) = runtimes(owner).get_mut(&key) else {
                            return;
                        };
                        let scroll = runtime.scroll.clone();
                        if runtime.scrollbar.begin_press(&scroll, pointer_x) {
                            cx.stop_propagation();
                            cx.notify();
                        }
                    });
                })
            },
            release: {
                let entity = entity.clone();
                let key = key.clone();
                let runtimes = runtimes.clone();
                Rc::new(move |_, cx| {
                    let _ = entity.update(cx, |owner, cx| {
                        if let Some(runtime) = runtimes(owner).get_mut(&key) {
                            runtime.scrollbar.end_press();
                            cx.notify();
                        }
                    });
                })
            },
        });

    CodeUi {
        key: key.clone(),
        fit_content,
        scroll,
        scrollbar,
        toggle_fit: Rc::new(move |_, cx| {
            let fit = !crate::settings::current(cx).code_fences_fit_content;
            crate::settings::update(crate::settings::SavePolicy::Immediate, cx, |settings| {
                settings.code_fences_fit_content = fit
            });
            cx.refresh_windows();
        }),
        viewport_hover: {
            let entity = entity.clone();
            let key = key.clone();
            let runtimes = runtimes.clone();
            Rc::new(move |hovered, _, cx| {
                let _ = entity.update(cx, |owner, cx| {
                    if runtimes(owner)
                        .get_mut(&key)
                        .is_some_and(|runtime| runtime.scrollbar.set_viewport_hovered(hovered))
                    {
                        cx.notify();
                    }
                });
            })
        },
        drag_move: {
            Rc::new(move |pointer_x, _, cx| {
                let _ = entity.update(cx, |owner, cx| {
                    let Some(runtime) = runtimes(owner).get_mut(&key) else {
                        return;
                    };
                    let scroll = runtime.scroll.clone();
                    if runtime.scrollbar.drag_to(&scroll, pointer_x) {
                        cx.notify();
                    }
                });
            })
        },
    }
}

#[derive(Clone)]
struct CodeScrollbarDrag {
    key: SharedString,
}

struct CodeScrollbarDragGhost;

impl Render for CodeScrollbarDragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

struct CodeBlockTooltip(SharedString);

impl Render for CodeBlockTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::tooltip::chip(self.0.clone(), Theme::of(cx))
    }
}

fn code_line_selection_key(row_key: &str, code_ix: usize, line_ix: usize) -> std::sync::Arc<str> {
    format!("{row_key}-code{code_ix}-line{line_ix}").into()
}

/// What a fence's header names: its language (also the highlighting key)
/// and an optional `title=`/`filename=` that replaces the language label.
#[derive(Clone, Copy, Default)]
pub(crate) struct FenceLabel<'a> {
    pub language: Option<&'a str>,
    pub title: Option<&'a str>,
    pub closed: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_code_block(
    label: FenceLabel<'_>,
    code: &str,
    top_ix: usize,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
    highlight: CodeHighlight,
) -> AnyElement {
    let language = label.language;
    if language.is_some_and(|l| l.eq_ignore_ascii_case("mermaid")) {
        if let Some(handler) = opts.media.as_ref().and_then(|media| media.diagram.as_ref()) {
            let frame_id: SharedString = format!("{}-mermaid-{ix}", opts.row_key).into();
            let diagram = match handler(code, frame_id.clone(), theme) {
                DiagramView::Source => {
                    return render_code_block_source(
                        label, code, top_ix, ix, opts, theme, highlight,
                    );
                }
                DiagramView::Failed(reason) => {
                    let notice = code_notice(format!("{frame_id}-failure").into(), reason, theme);
                    return render_code_block_source_with_actions(
                        label,
                        code,
                        top_ix,
                        ix,
                        opts,
                        theme,
                        highlight,
                        vec![notice],
                    );
                }
                DiagramView::Diagram(diagram) => diagram,
            };
            let toggle = diagram.toggle_source.clone();
            let toggle_action = code_icon_action(
                format!("{frame_id}-source-toggle").into(),
                if diagram.show_source {
                    "Show diagram"
                } else {
                    "Show source"
                },
                if diagram.show_source {
                    crate::icons::EYE
                } else {
                    crate::icons::FILE_CODE
                },
                Rc::new(move |window, cx| toggle(window, cx)),
                theme,
            );
            if diagram.show_source {
                return render_code_block_source_with_actions(
                    label,
                    code,
                    top_ix,
                    ix,
                    opts,
                    theme,
                    highlight,
                    vec![toggle_action],
                );
            }
            let mut actions = vec![toggle_action];
            actions.extend(code_copy_button(code, ix, opts, theme));
            return code_block_frame(frame_id, label, actions, diagram.body, theme)
                .into_any_element();
        }
    }
    render_code_block_source(label, code, top_ix, ix, opts, theme, highlight)
}

fn code_icon_action(
    id: SharedString,
    label: &'static str,
    icon_path: &'static str,
    handler: Rc<dyn Fn(&mut Window, &mut gpui::App)>,
    theme: &Theme,
) -> AnyElement {
    code_action_shell(id, gpui::transparent_black(), theme)
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            handler(window, cx);
        })
        .anchored_tooltip(crate::tooltip::Placement::Below, move |_, cx| {
            cx.new(move |_| CodeBlockTooltip(label.into())).into()
        })
        .child(code_action_icon(icon_path, theme))
        .into_any_element()
}

/// The shared 24px icon-only button behind every fence header action: muted
/// glyph, the theme's hover plate fading in under the pointer.
fn code_action_shell(id: SharedString, base: Hsla, theme: &Theme) -> gpui::Stateful<Div> {
    let fade_key = id.to_string();
    div()
        .id(id)
        .size(px(CODE_ACTION_SIZE))
        .flex_none()
        .rounded(px(6.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .bg(crate::motion::hover_blend(
            &fade_key,
            base,
            theme.element_hover,
        ))
        .on_hover(crate::motion::hover_listener(fade_key))
}

fn code_action_icon(icon_path: &'static str, theme: &Theme) -> gpui::Svg {
    crate::icons::icon(icon_path)
        .size(px(CODE_ACTION_ICON_SIZE))
        .text_color(theme.text_muted)
}

/// A passive header marker whose tooltip explains why a fence stays source.
fn code_notice(id: SharedString, message: SharedString, theme: &Theme) -> AnyElement {
    div()
        .id(id)
        .size(px(CODE_ACTION_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .anchored_tooltip(crate::tooltip::Placement::Below, move |_, cx| {
            let message = message.clone();
            cx.new(move |_| CodeBlockTooltip(message)).into()
        })
        .child(
            crate::icons::icon(crate::icons::DANGER_TRIANGLE)
                .size(px(13.0))
                .text_color(theme.warning_muted),
        )
        .into_any_element()
}

fn code_copy_button(
    code: &str,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
) -> Option<AnyElement> {
    opts.copy.clone().map(|copy| {
        let copied = copy.copied_ix == Some(ix);
        let code_text: SharedString = code.to_string().into();
        let handler = copy.handler.clone();
        // The owner clears `copied_ix` after ~1.2s, turning the check back
        // into the copy glyph.
        code_action_shell(
            format!("{}-copy{ix}", opts.row_key).into(),
            gpui::transparent_black(),
            theme,
        )
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            handler(ix, code_text.clone(), window, cx);
        })
        .anchored_tooltip(crate::tooltip::Placement::Below, move |_, cx| {
            let label = if copied { "Copied" } else { "Copy code" };
            cx.new(move |_| CodeBlockTooltip(label.into())).into()
        })
        .child(code_action_icon(
            if copied {
                crate::icons::CHECK
            } else {
                crate::icons::COPY
            },
            theme,
        ))
        .into_any_element()
    })
}

/// Shell fence languages that may offer "Run in terminal".
fn is_shell_fence(language: Option<&str>) -> bool {
    language.is_some_and(|language| {
        ["bash", "sh", "zsh", "shell"]
            .iter()
            .any(|shell| language.eq_ignore_ascii_case(shell))
    })
}

/// Invisible format characters (bidi overrides, zero-width marks, tags)
/// that would make a rendered command differ from what a terminal receives.
fn is_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// The command a shell fence would run, when it is one plain line (T3's
/// rule): no control or invisible format characters, so what the block
/// shows is exactly what the terminal receives, and no trailing `\`
/// continuation.
pub(crate) fn runnable_shell_command<'a>(language: Option<&str>, code: &'a str) -> Option<&'a str> {
    if !is_shell_fence(language) {
        return None;
    }
    let command = code.trim();
    (!command.is_empty()
        && !command.ends_with('\\')
        && !command.chars().any(|c| c.is_control() || is_format_char(c)))
    .then_some(command)
}

fn code_run_button(
    language: Option<&str>,
    code: &str,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
) -> Option<AnyElement> {
    let run = opts.run.clone()?;
    let command: SharedString = runnable_shell_command(language, code)?.to_string().into();
    Some(
        code_action_shell(
            format!("{}-run{ix}", opts.row_key).into(),
            gpui::transparent_black(),
            theme,
        )
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            (run.handler)(command.clone(), window, cx);
        })
        .anchored_tooltip(crate::tooltip::Placement::Below, |_, cx| {
            cx.new(|_| CodeBlockTooltip("Run in terminal".into()))
                .into()
        })
        .child(code_action_icon(crate::icons::ACTION_PLAY, theme))
        .into_any_element(),
    )
}

/// The synthetic file name whose icon stands for a fence language: the
/// language as an extension, with common long names mapped to theirs.
fn fence_language_file_name(language: &str) -> String {
    let lower = language.to_ascii_lowercase();
    let ext = match lower.as_str() {
        "dockerfile" | "docker" => return "Dockerfile".into(),
        "make" | "makefile" => return "Makefile".into(),
        "rust" => "rs",
        "javascript" => "js",
        "typescript" => "ts",
        "python" | "python3" => "py",
        "golang" => "go",
        "bash" | "shell" | "zsh" | "console" | "shellscript" => "sh",
        "markdown" => "md",
        "ruby" => "rb",
        "kotlin" => "kt",
        "csharp" | "c#" => "cs",
        "c++" => "cpp",
        "yml" => "yaml",
        "text" | "txt" | "plain" | "plaintext" => "txt",
        other => other,
    };
    format!("file.{ext}")
}

/// The fence label's file-type icon: from the title's own name, or from the
/// language when the icon theme knows it (an unknown language stays bare).
fn fence_label_icon(label: FenceLabel<'_>) -> Option<String> {
    if let Some(title) = label.title {
        return Some(title.to_string());
    }
    let name = fence_language_file_name(label.language?);
    crate::file_icons::has_specific_file_icon(&name).then_some(name)
}

fn code_block_header(
    label: FenceLabel<'_>,
    actions: Vec<AnyElement>,
    theme: &Theme,
) -> Option<gpui::Div> {
    let text = label.title.or(label.language);
    if text.is_none() && actions.is_empty() {
        return None;
    }
    let icon = fence_label_icon(label);
    Some(
        div()
            .flex_none()
            .min_h(px(CODE_HEADER_PAD_TOP + CODE_HEADER_LABEL_HEIGHT))
            .pt(px(CODE_HEADER_PAD_TOP))
            .pr(px(CODE_HEADER_PAD_RIGHT))
            .pl(px(CODE_HEADER_PAD_LEFT))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .font_family(theme.font_mono.clone())
                    .text_size(px(CODE_HEADER_TEXT_SIZE))
                    .text_color(theme.text_muted)
                    .children(icon.map(|name| {
                        crate::file_icons::icon(
                            crate::file_icons::FileIconIdentity::file(&name),
                            theme.appearance,
                        )
                        .size(px(14.0))
                    }))
                    .children(text.map(|text| {
                        div()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(text.to_string()))
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    .children(actions),
            ),
    )
}

/// Shared visual shell for ordinary code and generated diagram fences.
fn code_block_frame(
    id: SharedString,
    label: FenceLabel<'_>,
    actions: Vec<AnyElement>,
    body: AnyElement,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    // T3: dark fences are a borderless ink plate; light ones a faint
    // secondary plate with the border at 70%.
    let (bg, border) = if theme.appearance.is_dark() {
        (theme.ink(0.05), gpui::transparent_black())
    } else {
        (theme.ink(0.04), theme.border.opacity(0.7))
    };
    div()
        .id(id)
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .rounded(px(CODE_FRAME_RADIUS))
        .bg(bg)
        .border_1()
        .border_color(border)
        .overflow_hidden()
        .relative()
        .children(code_block_header(label, actions, theme))
        .child(body)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_code_block_source(
    label: FenceLabel<'_>,
    code: &str,
    top_ix: usize,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
    highlight: CodeHighlight,
) -> AnyElement {
    render_code_block_source_with_actions(
        label,
        code,
        top_ix,
        ix,
        opts,
        theme,
        highlight,
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
fn render_code_block_source_with_actions(
    label: FenceLabel<'_>,
    code: &str,
    top_ix: usize,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
    highlight: CodeHighlight,
    mut extra_actions: Vec<AnyElement>,
) -> AnyElement {
    let mono = font(theme.font_mono.clone());
    // Per-line strings + runs through the cross-frame cache (validity: code
    // length + highlight slice identity — a fresh highlight Arc re-derives).
    let hl_key = highlight.map_or((0, 0), |h| (h.as_ptr() as usize, h.len()));
    let build = || {
        let lines: Vec<(SharedString, Vec<TextRun>)> = code
            .split('\n')
            .enumerate()
            .map(|(li, line)| {
                let spans = highlight
                    .and_then(|h| h.get(li))
                    .map(|t| &t[..])
                    .unwrap_or(&[]);
                (
                    SharedString::from(line.to_string()),
                    runs_for_syntax_line(line, spans, &mono, theme),
                )
            })
            .collect();
        Rc::new(CachedCode {
            code_len: code.len(),
            hl_key,
            lines,
        })
    };
    let cached: Rc<CachedCode> = match &opts.cache {
        Some(cache) => {
            let mut cache = cache.borrow_mut();
            cache.sync_style();
            let entry = cache
                .code
                .entry((opts.row_key.clone(), top_ix, ix))
                .or_insert_with(&build);
            if entry.code_len != code.len() || entry.hl_key != hl_key {
                *entry = build();
            }
            entry.clone()
        }
        None => build(),
    };
    // Streaming veil over appended code, tracked on the whole code text and
    // sliced per line below (paint-only run recolor — heights stay exact).
    let veil_spans = match &opts.veil {
        Some(veil) => veil.borrow_mut().advance(ix, code, opts.now),
        None => Vec::new(),
    };
    let scroll_id: SharedString = format!("{}-code{ix}", opts.row_key).into();
    let sel_wash = theme.selection;
    let code_ui = opts.code.as_ref().and_then(|code| code.get(&ix)).cloned();
    let fit_content = code_ui.as_ref().is_some_and(|ui| ui.fit_content);

    let fit_button = code_ui.as_ref().map(|ui| {
        let toggle = ui.toggle_fit.clone();
        // A pressed Fit keeps a resting plate so the toggle reads as on.
        let base = if fit_content {
            theme.element_hover.opacity(0.7)
        } else {
            gpui::transparent_black()
        };
        code_action_shell(format!("{}-fit{ix}", opts.row_key).into(), base, theme)
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                toggle(window, cx);
            })
            .anchored_tooltip(crate::tooltip::Placement::Below, move |_, cx| {
                cx.new(move |_| {
                    CodeBlockTooltip(
                        if fit_content {
                            "Use horizontal scrolling"
                        } else {
                            "Fit content"
                        }
                        .into(),
                    )
                })
                .into()
            })
            .child(code_action_icon(crate::icons::WRAP_TEXT, theme))
    });
    let mut actions = Vec::new();
    actions.extend(fit_button.map(IntoElement::into_any_element));
    actions.append(&mut extra_actions);
    if label.closed {
        actions.extend(code_run_button(label.language, code, ix, opts, theme));
    }
    actions.extend(code_copy_button(code, ix, opts, theme));

    let lines = div()
        .map(|el| {
            if fit_content {
                el.w_full().min_w_0()
            } else {
                el.min_w_full().flex_none()
            }
        })
        .px(px(CODE_PADDING_X))
        .py(px(CODE_PADDING_Y))
        .font_family(theme.font_mono.clone())
        .text_size(px(theme.code_font_size))
        .line_height(px(theme.code_font_size * CODE_LINE_HEIGHT_RATIO))
        .map(|el| {
            if fit_content {
                el.whitespace_normal()
            } else {
                el.whitespace_nowrap()
            }
        })
        .flex()
        .flex_col()
        .children((0..cached.lines.len()).scan(0usize, move |off, li| {
            let (line, runs) = &cached.lines[li];
            let start = *off;
            *off = start + line.len() + 1; // +1 for the '\n'
            let local = slice_spans(&veil_spans, start, start + line.len());
            let runs = apply_veil(runs.clone(), &local);
            let key = code_line_selection_key(&opts.row_key, ix, li);
            Some(
                div()
                    .map(|el| {
                        if fit_content {
                            el.w_full()
                                .min_w_0()
                                .min_h(px(theme.code_font_size * CODE_LINE_HEIGHT_RATIO))
                        } else {
                            el.h(px(theme.code_font_size * CODE_LINE_HEIGHT_RATIO))
                                .flex_none()
                        }
                    })
                    .child(selectable_text_element(key, line.clone(), runs, sel_wash)),
            )
        }));

    let body: AnyElement = if let Some(ui) = code_ui.as_ref() {
        if fit_content {
            div()
                .w_full()
                .min_w_0()
                .overflow_hidden()
                .child(lines)
                .into_any_element()
        } else {
            let mut scroller = div()
                .id(scroll_id)
                .w_full()
                .min_w_0()
                .flex()
                .overflow_x_scroll()
                .track_scroll(&ui.scroll)
                .child(lines);
            // A vertical wheel over code must keep bubbling to the transcript;
            // only a true horizontal gesture moves this local viewport.
            scroller.style().restrict_scroll_to_axis = Some(true);
            scroller.into_any_element()
        }
    } else {
        // Non-transcript previews keep their existing native scroll behavior.
        div()
            .id(scroll_id)
            .w_full()
            .overflow_x_scroll()
            .child(lines)
            .into_any_element()
    };

    let scrollbar = (!fit_content)
        .then(|| code_ui.as_ref())
        .flatten()
        .and_then(|ui| {
            let bar = ui.scrollbar.as_ref()?;
            let metrics = bar.metrics;
            let hover = bar.hover.clone();
            let press = bar.press.clone();
            let release_up = bar.release.clone();
            let release_out = bar.release.clone();
            let thumb_height = if bar.active {
                crate::popover::MENU_SCROLLBAR_HOVER_THUMB_WIDTH
            } else {
                crate::popover::MENU_SCROLLBAR_THUMB_WIDTH
            };
            Some(
                div()
                    .id(SharedString::from(format!("{}-scrollbar", ui.key)))
                    .absolute()
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .h(px(CODE_SCROLLBAR_HIT_HEIGHT))
                    .cursor(CursorStyle::Arrow)
                    .on_hover(move |hovered, window, cx| hover(*hovered, window, cx))
                    .on_mouse_down(gpui::MouseButton::Left, move |event, window, cx| {
                        press(event.position.x, window, cx);
                    })
                    .on_drag(
                        CodeScrollbarDrag {
                            key: ui.key.clone(),
                        },
                        |_, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| CodeScrollbarDragGhost)
                        },
                    )
                    .on_mouse_up(gpui::MouseButton::Left, move |_, window, cx| {
                        release_up(window, cx)
                    })
                    .on_mouse_up_out(gpui::MouseButton::Left, move |_, window, cx| {
                        release_out(window, cx)
                    })
                    .child(
                        div()
                            .absolute()
                            .left(px(
                                crate::popover::MENU_SCROLLBAR_TRACK_INSET + metrics.thumb_left
                            ))
                            .bottom(px(2.0))
                            .w(px(metrics.thumb_width))
                            .h(px(thumb_height))
                            .rounded(px(thumb_height / 2.0))
                            .bg(theme
                                .text_faint
                                .opacity(if bar.active { 0.68 } else { 0.5 })),
                    ),
            )
        });

    let frame_body = div().w_full().relative().child(body).children(scrollbar);
    let mut block = code_block_frame(
        format!("{}-code-frame{ix}", opts.row_key).into(),
        label,
        actions,
        frame_body.into_any_element(),
        theme,
    );
    if let Some(ui) = code_ui {
        let viewport_hover = ui.viewport_hover.clone();
        let drag_move = ui.drag_move.clone();
        let drag_key = ui.key;
        block = block
            .on_hover(move |hovered, window, cx| viewport_hover(*hovered, window, cx))
            .on_drag_move(
                move |event: &gpui::DragMoveEvent<CodeScrollbarDrag>, window, cx| {
                    let Some(drag) = event.dragged_item().downcast_ref::<CodeScrollbarDrag>()
                    else {
                        return;
                    };
                    if drag.key == drag_key {
                        drag_move(event.event.position.x, window, cx);
                    }
                },
            );
    }
    block.into_any_element()
}

/// Paint color for a token class — the soft syntax palette (round 9: the
/// original's mdTheme code blocks are monochrome `#e7e7e7`, but the user
/// asked for color; these are the diff pane's hues, now shared by both).
pub fn token_color(kind: HighlightKind, theme: &Theme) -> Hsla {
    theme.syntax.color(kind)
}

/// Build the exact-cover `TextRun` list for one code line from its tokens.
/// Same font family everywhere (comments italic) — recoloring never reflows.
/// Build paint-only runs from the neutral Tree-sitter contract.
pub fn runs_for_syntax_line(
    line: &str,
    spans: &[HighlightSpan],
    mono: &gpui::Font,
    theme: &Theme,
) -> Vec<TextRun> {
    runs_for_syntax_line_with_plain(line, spans, mono, theme.text, theme)
}

pub fn runs_for_syntax_line_with_plain(
    line: &str,
    spans: &[HighlightSpan],
    mono: &gpui::Font,
    plain_color: Hsla,
    theme: &Theme,
) -> Vec<TextRun> {
    let plain = |len: usize| TextRun {
        len,
        font: mono.clone(),
        color: plain_color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut runs = Vec::new();
    let mut at = 0usize;
    for span in spans {
        if span.range.start > at {
            runs.push(plain(span.range.start - at));
        }
        let mut run = plain(span.range.len());
        run.color = token_color(span.kind, theme);
        if span.kind == HighlightKind::Comment {
            // Mono italics keep the same advance, so this cannot reflow.
            run.font.style = gpui::FontStyle::Italic;
        }
        runs.push(run);
        at = span.range.end;
    }
    if at < line.len() {
        runs.push(plain(line.len() - at));
    }
    runs.retain(|run| run.len > 0);
    runs
}
