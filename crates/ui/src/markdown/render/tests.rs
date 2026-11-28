use super::*;
use super::{
    blocks::{list_child_ix, quote_child_ix},
    selection_paint::range_rects_with_positions,
    text::{
        flatten_runs_weighted, plain_file_reference_lines, sole_file_reference,
        sole_workspace_file_link,
    },
};
use crate::markdown::parser::{Block, InlineRun, InlineStyle, parse_full};
use gpui::{Bounds, FontStyle, FontWeight, font, point, px, size};
use std::ops::Range;
use zuse_syntax::HighlightKind;

#[test]
fn code_block_indices_include_nested_quotes_and_lists() {
    let quoted = parse_full("> ```rust\n> let x = 1;\n> ```\n");
    assert_eq!(
        code_block_indices(&quoted.blocks[0].block, 0),
        vec![quote_child_ix(0, 0)]
    );

    let listed = parse_full("- Result:\n\n  ```json\n  {\"ok\":true}\n  ```\n");
    assert_eq!(
        code_block_indices(&listed.blocks[0].block, 0),
        vec![list_child_ix(0, 0, 1)]
    );
    assert_ne!(quote_child_ix(0, 0), 0);
    assert_ne!(list_child_ix(0, 0, 10), list_child_ix(0, 1, 0));

    let top_level = parse_full("paragraph\n\n```text\nvalue\n```\n");
    assert_eq!(code_block_indices(&top_level.blocks[1].block, 1), vec![1]);
}

#[test]
fn sole_workspace_link_gets_a_file_path() {
    let runs = vec![InlineRun {
        text: "slides.ts".into(),
        style: InlineStyle {
            link: Some("src/slides.ts#L12".into()),
            ..Default::default()
        },
    }];
    assert_eq!(
        sole_workspace_file_link(&runs, "/work/zuse"),
        Some("src/slides.ts".into())
    );
}

/// The sole-file row follows the same grammar as a click: an encoded
/// path resolves decoded, and an absolute path no root owns is still the
/// linking chat's file (read-only, but not plain text).
#[test]
fn sole_file_row_covers_decoded_and_outside_links() {
    let decoded = vec![InlineRun {
        text: "it's here.txt".into(),
        style: InlineStyle {
            link: Some("2026-09-26/Some%20Folder/it's%20here.txt".into()),
            ..Default::default()
        },
    }];
    assert_eq!(
        sole_workspace_file_link(&decoded, "/work/zuse"),
        Some("2026-09-26/Some Folder/it's here.txt".into())
    );

    let outside = vec![InlineRun {
        text: "INFORME.md".into(),
        style: InlineStyle {
            link: Some("/tmp/elsewhere/INFORME.md".into()),
            ..Default::default()
        },
    }];
    assert_eq!(
        sole_workspace_file_link(&outside, "/work/zuse"),
        Some("/tmp/elsewhere/INFORME.md".into())
    );

    // A relative path no root owns stays plain text.
    let unresolved = vec![InlineRun {
        text: "go.md".into(),
        style: InlineStyle {
            link: Some("../go.md".into()),
            ..Default::default()
        },
    }];
    assert_eq!(sole_workspace_file_link(&unresolved, "/work/zuse"), None);
}

#[test]
fn a_file_label_shows_in_place_of_the_path_which_stays_the_copy_source() {
    let path = "2026-09-29/Some Long Folder Name/SOURCES.md";
    let linked = InlineRun {
        text: path.into(),
        style: InlineStyle {
            link: Some(path.into()),
            file_label: Some("SOURCES.md".into()),
            ..Default::default()
        },
    };
    // A whole line or list item still gets the file row's icon identity.
    assert_eq!(
        sole_file_reference(std::slice::from_ref(&linked), "/work/zuse"),
        Some(path.into())
    );

    let sentence = vec![
        InlineRun {
            text: "See ".into(),
            style: InlineStyle::default(),
        },
        linked,
        InlineRun {
            text: " next.".into(),
            style: InlineStyle::default(),
        },
    ];
    let flat = flatten_runs(&sentence, &Theme::dark(), false);
    assert_eq!(flat.text.as_ref(), "See SOURCES.md next.");
    assert_eq!(flat.links, vec![(4..14, path.to_owned())]);
    assert_eq!(
        flat.runs.iter().map(|run| run.len).sum::<usize>(),
        flat.text.len()
    );
    let original = flat.original.expect("the path is kept for copy");
    assert_eq!(original.text.as_ref(), format!("See {path} next."));
    // Selecting the whole label copies the whole path; the prose around
    // it maps one to one.
    let offsets = &original.offsets;
    assert_eq!(offsets.original(4), 4);
    assert_eq!(offsets.original(14), 4 + path.len());
    assert_eq!(offsets.original(flat.text.len()), original.text.len());
    assert_eq!(offsets.displayed(4 + path.len() + 1), 15);
}

#[test]
fn an_authored_file_link_label_is_never_replaced_by_the_file_name() {
    let tree = parse_full("[docs/a.md](docs/a.md)");
    let Block::Paragraph { runs } = &tree.blocks[0].block else {
        panic!("expected a paragraph");
    };
    let flat = flatten_runs(runs, &Theme::dark(), false);
    assert_eq!(flat.text.as_ref(), "docs/a.md");
    assert!(flat.original.is_none(), "the authored label is the source");
}

#[test]
fn direct_file_decoration_excludes_mixed_and_external_links() {
    let linked = InlineRun {
        text: "slides.ts".into(),
        style: InlineStyle {
            link: Some("src/slides.ts".into()),
            ..Default::default()
        },
    };
    let mixed = vec![
        InlineRun {
            text: "See ".into(),
            style: InlineStyle::default(),
        },
        linked,
    ];
    assert_eq!(sole_workspace_file_link(&mixed, "/work/zuse"), None);

    let external = vec![InlineRun {
        text: "website".into(),
        style: InlineStyle {
            link: Some("https://example.com/slides.ts".into()),
            ..Default::default()
        },
    }];
    assert_eq!(sole_workspace_file_link(&external, "/work/zuse"), None);
}

#[test]
fn standalone_recognized_filename_gets_a_decorative_identity() {
    let plain = vec![InlineRun {
        text: "AudienceView.tsx".into(),
        style: InlineStyle::default(),
    }];
    assert_eq!(
        sole_file_reference(&plain, "/work/zuse"),
        Some("AudienceView.tsx".into())
    );

    let linked_unknown = vec![InlineRun {
        text: "artifact.unknown".into(),
        style: InlineStyle {
            link: Some("build/artifact.unknown".into()),
            ..Default::default()
        },
    }];
    assert_eq!(
        sole_file_reference(&linked_unknown, "/work/zuse"),
        Some("build/artifact.unknown".into())
    );
}

#[test]
fn plain_file_decoration_rejects_prose_code_and_unknown_dotted_tokens() {
    let plain = |text: &str| {
        vec![InlineRun {
            text: text.into(),
            style: InlineStyle::default(),
        }]
    };
    assert_eq!(
        sole_file_reference(&plain("version 1.2"), "/work/zuse"),
        None
    );
    assert_eq!(
        sole_file_reference(&plain("example.invalid"), "/work/zuse"),
        None
    );

    let code = vec![InlineRun {
        text: "slides.ts".into(),
        style: InlineStyle {
            code: true,
            ..Default::default()
        },
    }];
    assert_eq!(sole_file_reference(&code, "/work/zuse"), None);
}

#[test]
fn hard_break_file_list_resolves_every_recognized_line() {
    let runs = vec![InlineRun {
        text: "slides.ts\ncourseDecks.ts\nAudienceView.tsx\nglobals.css".into(),
        style: InlineStyle::default(),
    }];
    assert_eq!(
        plain_file_reference_lines(&runs, "/work/zuse"),
        Some(vec![
            "slides.ts".into(),
            "courseDecks.ts".into(),
            "AudienceView.tsx".into(),
            "globals.css".into(),
        ])
    );

    let mixed = vec![InlineRun {
        text: "slides.ts\nnot a file".into(),
        style: InlineStyle::default(),
    }];
    assert_eq!(plain_file_reference_lines(&mixed, "/work/zuse"), None);
}

/// Model GPUI's upstream affinity at a soft-wrap boundary: byte 5 is
/// reported at the end of row 0, while byte 6 is after the first glyph on
/// row 1.
fn wrapped_position(ix: usize) -> Option<gpui::Point<gpui::Pixels>> {
    (ix <= 9).then(|| {
        if ix <= 5 {
            point(px(ix as f32 * 10.0), px(0.0))
        } else {
            point(px((ix - 5) as f32 * 10.0), px(22.0))
        }
    })
}

fn wrapped_range_rects(range: Range<usize>) -> Vec<Bounds<gpui::Pixels>> {
    range_rects_with_positions(
        Bounds::new(point(px(0.0), px(0.0)), size(px(50.0), px(44.0))),
        px(22.0),
        &range,
        0.0,
        0.0,
        wrapped_position,
    )
}

#[test]
fn range_starting_at_soft_wrap_includes_first_glyph() {
    let rects = wrapped_range_rects(5..9);
    assert_eq!(rects.len(), 1);
    assert_eq!(rects[0].origin, point(px(0.0), px(22.0)));
    assert_eq!(rects[0].size, size(px(40.0), px(22.0)));
}

#[test]
fn range_crossing_soft_wrap_includes_first_continuation_glyph() {
    let rects = wrapped_range_rects(2..9);
    assert_eq!(rects.len(), 2);
    assert_eq!(rects[0].origin, point(px(20.0), px(0.0)));
    assert_eq!(rects[0].size, size(px(30.0), px(22.0)));
    assert_eq!(rects[1].origin, point(px(0.0), px(22.0)));
    assert_eq!(rects[1].size, size(px(40.0), px(22.0)));
}

#[test]
fn code_line_runs_cover_exactly() {
    let theme = Theme::dark();
    let mono = font(theme.font_mono.clone());
    let line = r#"let x = "hi"; // done"#;
    let document = zuse_syntax::highlight(zuse_syntax::HighlightRequest {
        source: line,
        path: None,
        fence_tag: Some("rust"),
    })
    .unwrap();
    let runs = runs_for_syntax_line(line, &document.lines[0], &mono, &theme);
    let total: usize = runs.iter().map(|r| r.len).sum();
    assert_eq!(total, line.len());
    assert!(
        runs.iter()
            .all(|r| r.font.family == mono.family && r.font.weight == mono.weight),
        "highlight keeps the code family and weight"
    );
    assert!(
        runs.iter()
            .any(|r| r.font.style == FontStyle::Italic && r.color == theme.syntax.comment)
    );
    // At least one non-plain color made it through.
    assert!(runs.iter().any(|r| r.color != theme.text));
}

#[test]
fn tree_sitter_runs_are_rich_and_paint_only() {
    let theme = Theme::dark();
    let mono = font(theme.font_mono.clone());
    let line = "let widget = build!(42);";
    let document = zuse_syntax::highlight(zuse_syntax::HighlightRequest {
        source: line,
        path: None,
        fence_tag: Some("rust"),
    })
    .unwrap();
    let runs = runs_for_syntax_line(line, &document.lines[0], &mono, &theme);
    assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), line.len());
    assert!(runs.iter().all(|run| run.font == mono));
    let colors = runs.iter().map(|run| run.color).collect::<Vec<_>>();
    assert!(colors.contains(&theme.syntax.keyword));
    assert!(colors.contains(&theme.syntax.macro_name));
    assert!(colors.contains(&theme.syntax.number));
}

#[test]
fn affected_language_roles_flow_through_markdown_paint_only() {
    let cases: &[(&str, &str, &[HighlightKind])] = &[
        (
            "typescript",
            "export function derive(name: string) { return call(name); }",
            &[
                HighlightKind::Keyword,
                HighlightKind::Function,
                HighlightKind::Parameter,
                HighlightKind::TypeBuiltin,
            ],
        ),
        (
            "tsx",
            "function card(props: Props): JSX.Element { return <main id={props.id} />; }",
            &[
                HighlightKind::Tag,
                HighlightKind::Attribute,
                HighlightKind::Type,
            ],
        ),
        (
            "kotlin",
            "fun greet(name: String) = println(name)",
            &[
                HighlightKind::Keyword,
                HighlightKind::Function,
                HighlightKind::Parameter,
                HighlightKind::TypeBuiltin,
            ],
        ),
        (
            "dockerfile",
            "RUN echo \"hello\"",
            &[
                HighlightKind::Keyword,
                HighlightKind::Function,
                HighlightKind::String,
            ],
        ),
    ];
    for &(fence_tag, line, required) in cases {
        let document = zuse_syntax::highlight(zuse_syntax::HighlightRequest {
            source: line,
            path: None,
            fence_tag: Some(fence_tag),
        })
        .unwrap();
        let kinds = document.lines[0]
            .iter()
            .map(|span| span.kind)
            .collect::<Vec<_>>();
        for &kind in required {
            assert!(kinds.contains(&kind), "missing {kind:?} for {fence_tag}");
        }
        for theme in [Theme::dark(), Theme::light()] {
            let mono = font(theme.font_mono.clone());
            let runs = runs_for_syntax_line(line, &document.lines[0], &mono, &theme);
            assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), line.len());
            assert!(runs.iter().all(|run| run.font == mono));
            let colors = runs.iter().map(|run| run.color).collect::<Vec<_>>();
            for &kind in required {
                assert!(
                    colors.contains(&token_color(kind, &theme)),
                    "missing {kind:?} color for {fence_tag}"
                );
            }
        }
    }
}

#[test]
fn code_line_runs_with_no_tokens_are_one_plain_run() {
    let theme = Theme::dark();
    let mono = font(theme.font_mono.clone());
    let runs = runs_for_syntax_line("plain text", &[], &mono, &theme);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].len, 10);
}

#[test]
fn flatten_collects_and_merges_inline_code_ranges() {
    let theme = Theme::dark();
    let code = |text: &str| InlineRun {
        text: text.into(),
        style: InlineStyle {
            code: true,
            ..Default::default()
        },
    };
    let plain = |text: &str| InlineRun {
        text: text.into(),
        style: InlineStyle::default(),
    };
    let flat = flatten_runs(
        &[
            plain("use "),
            code("foo"),
            code("()"),
            plain(" and "),
            code("bar"),
        ],
        &theme,
        false,
    );
    // Adjacent code runs merge into ONE wash box; separated ones don't.
    let ranges: Vec<_> = flat
        .code_ranges
        .iter()
        .map(|range| &flat.text[range.clone()])
        .collect();
    assert_eq!(ranges, ["\u{202f}foo()\u{202f}", "\u{202f}bar\u{202f}"]);
    assert_eq!(
        flat.original.as_ref().unwrap().text.as_ref(),
        "use foo() and bar"
    );
    // Code text is the violet tint; the square run background is gone
    // (the rounded wash is painted by the canvas underlay instead).
    assert_eq!(flat.runs[1].color, inline_code_text(&theme));
    assert_eq!(flat.runs[1].background_color, None);
    assert_eq!(flat.runs[0].color, theme.text);
}

#[test]
fn flatten_runs_maps_links_and_styles() {
    let theme = Theme::dark();
    let runs = vec![
        InlineRun {
            text: "go ".into(),
            style: InlineStyle::default(),
        },
        InlineRun {
            text: "here".into(),
            style: InlineStyle {
                link: Some("https://x.dev".into()),
                ..Default::default()
            },
        },
        InlineRun {
            text: " now".into(),
            style: InlineStyle {
                bold: true,
                ..Default::default()
            },
        },
    ];
    let flat = flatten_runs(&runs, &theme, false);
    assert_eq!(flat.text, "go here now");
    assert_eq!(flat.links, vec![(3..7, "https://x.dev".to_string())]);
    let total: usize = flat.runs.iter().map(|r| r.len).sum();
    assert_eq!(total, flat.text.len());
    // Web links read info blue at medium weight, with a soft underline.
    assert_eq!(flat.runs[1].color, crate::site_links::link_color(&theme));
    assert_eq!(flat.runs[1].font.weight, FontWeight::MEDIUM);
    assert!(flat.runs[1].underline.is_some());
    // Strong prose is bold (700).
    assert_eq!(flat.runs[2].font.weight, FontWeight::BOLD);
}

#[test]
fn table_columns_floor_and_padding() {
    // A short column keeps its content width (floored at MIN_COLUMN_CONTENT
    // + padding); a wide one may wrap but no narrower than minColumnWidth.
    let geo = table_columns(&[10.0, 200.0]);
    assert_eq!(geo.naturals, vec![72.0, 224.0]); // 48+24, 200+24
    assert_eq!(geo.minimums, vec![72.0, 96.0]);
    assert_eq!(geo.min_table_width, 168.0);
}

#[test]
fn table_columns_make_room_for_heading_words_within_a_limit() {
    let geo = super::table::table_columns_with_headings(
        &[130.0, 180.0, 240.0, 100.0],
        &[82.0, 48.0, 240.0],
    );
    assert_eq!(geo.minimums, [106.0, 96.0, 160.0, 96.0]);
    assert_eq!(geo.min_table_width, 458.0);
}

#[test]
fn table_columns_are_content_proportional_not_equal() {
    let geo = table_columns(&[300.0, 60.0, 60.0]);
    // Flex grow factors are the naturals — a prose column gets a larger
    // share than short ones (not equal thirds).
    assert!(geo.naturals[0] > 3.0 * geo.naturals[1] * 0.9);
    assert_eq!(geo.naturals[1], geo.naturals[2]);
}

#[test]
fn table_header_preserves_semibold_weight() {
    let theme = Theme::dark();
    let runs = vec![InlineRun {
        text: "Header".into(),
        style: InlineStyle::default(),
    }];
    let flat = flatten_runs_weighted(&runs, &theme, TABLE_HEADER_WEIGHT, None, false);
    assert_eq!(flat.runs[0].font.weight, FontWeight::SEMIBOLD);
    // Strong runs retain the same header weight.
    let bold_runs = vec![InlineRun {
        text: "Strong".into(),
        style: InlineStyle {
            bold: true,
            ..Default::default()
        },
    }];
    let flat = flatten_runs_weighted(&bold_runs, &theme, TABLE_HEADER_WEIGHT, None, false);
    assert_eq!(flat.runs[0].font.weight, FontWeight::SEMIBOLD);
}

#[test]
fn dark_transcript_softens_body_text_and_retains_emphasis() {
    use crate::theme::{contrast_ratio, flatten};

    let theme = Theme::dark();
    let tree = super::super::parser::parse_full("Body and **emphasis**.");
    let Block::Paragraph { runs } = &tree.blocks[0].block else {
        panic!("expected a paragraph");
    };
    let flat = flatten_runs_weighted(runs, &theme, FontWeight::NORMAL, None, true);
    let body = flat.runs[0].color;
    let bubble = flatten(theme.user_bubble_fill(), theme.bg);
    assert_eq!(flat.runs[0].font.weight, FontWeight::NORMAL);
    assert!(contrast_ratio(body, theme.bg) < contrast_ratio(theme.text, theme.bg));
    assert!(contrast_ratio(body, bubble) >= 4.5);
    let emphasis = contrast_ratio(flat.runs[1].color, theme.bg);
    assert!(emphasis > contrast_ratio(body, theme.bg));
    assert!(emphasis < contrast_ratio(theme.text_strong, theme.bg));
    assert_eq!(flat.runs[1].font.weight, FontWeight::BOLD);
}

#[test]
fn adjacent_same_link_runs_merge_into_one_range() {
    let theme = Theme::dark();
    let style = InlineStyle {
        link: Some("https://x.dev".into()),
        ..Default::default()
    };
    let runs = vec![
        InlineRun {
            text: "bold".into(),
            style: InlineStyle {
                bold: true,
                ..style.clone()
            },
        },
        InlineRun {
            text: " tail".into(),
            style,
        },
    ];
    let flat = flatten_runs(&runs, &theme, false);
    assert_eq!(flat.links, vec![(0..9, "https://x.dev".to_string())]);
}

#[test]
fn viewport_cache_releases_offscreen_paint_data() {
    let mut cache = RenderCache::default();
    for row in ["visible", "offscreen"] {
        cache.flats.insert(
            (row.into(), 0, 0),
            Rc::new(FlatText {
                original: None,
                text: "text".into(),
                runs: Vec::new(),
                links: Vec::new(),
                code_ranges: Vec::new(),
                user_literals: Vec::new(),
                file_glyphs: Vec::new(),
                favicons: Vec::new(),
            }),
        );
        cache.code.insert(
            (row.into(), 0, 0),
            Rc::new(CachedCode {
                code_len: 0,
                hl_key: (0, 0),
                lines: Vec::new(),
            }),
        );
    }
    cache.retain_rows(&std::collections::HashSet::from(["visible".into()]));
    assert_eq!(cache.flats.len(), 1);
    assert_eq!(cache.code.len(), 1);
    assert!(cache.flats.keys().all(|(row, _, _)| row == "visible"));
    cache.retain_rows(&std::collections::HashSet::new());
    assert!(cache.flats.is_empty());
    assert!(cache.code.is_empty());
}

#[test]
fn style_generation_change_invalidates_cached_runs() {
    let mut cache = RenderCache {
        generation: 10,
        ..Default::default()
    };
    cache.flats.insert(
        ("row".into(), 0, 0),
        Rc::new(FlatText {
            original: None,
            text: "cached".into(),
            runs: Vec::new(),
            links: Vec::new(),
            code_ranges: Vec::new(),
            user_literals: Vec::new(),
            file_glyphs: Vec::new(),
            favicons: Vec::new(),
        }),
    );
    cache.code.insert(
        ("row".into(), 0, 0),
        Rc::new(super::CachedCode {
            code_len: 6,
            hl_key: (0, 0),
            lines: vec![("cached".into(), Vec::new())],
        }),
    );
    cache.sync_generation(10);
    assert_eq!(cache.flats.len(), 1, "same style is idempotent");
    assert_eq!(cache.code.len(), 1);
    cache.sync_generation(11);
    assert!(
        cache.flats.is_empty(),
        "font or color changes invalidate runs"
    );
    assert!(cache.code.is_empty());
}
