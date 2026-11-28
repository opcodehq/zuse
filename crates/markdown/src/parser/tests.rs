use super::*;

fn shown(runs: &[InlineRun]) -> String {
    runs.iter().map(|run| run.text.as_str()).collect()
}

fn copied(runs: &[InlineRun]) -> String {
    runs.iter()
        .filter(|run| !run.style.display_only)
        .map(|run| run.text.as_str())
        .collect()
}

fn paragraph(block: &Block) -> &[InlineRun] {
    match block {
        Block::Paragraph { runs } => runs,
        other => panic!("expected a paragraph, got {other:?}"),
    }
}

#[test]
fn user_parse_keeps_a_single_newline_as_a_line_break() {
    let tree = parse_user("first line\nsecond **line**");
    assert_eq!(tree.len(), 1);
    let runs = paragraph(&tree.blocks[0].block);
    assert_eq!(shown(runs), "first line\nsecond line");
    assert!(runs.iter().any(|run| run.style.bold && run.text == "line"));
    // The assistant's parse still folds a soft break into a space.
    let tree = parse_full("first line\nsecond **line**");
    assert_eq!(
        shown(paragraph(&tree.blocks[0].block)),
        "first line second line"
    );
}

#[test]
fn user_parse_shows_leading_indentation_without_copying_it() {
    let tree = parse_user("  two\n\tthree\nnone");
    let runs = paragraph(&tree.blocks[0].block);
    assert_eq!(shown(runs), "  two\n    three\nnone");
    assert_eq!(copied(runs), "two\nthree\nnone");
    // Lists and quotes keep their own structure.
    let tree = parse_user("- a\n  - b");
    assert!(matches!(tree.blocks[0].block, Block::List { .. }));
    assert!(parse_full("  two").blocks.iter().all(|top| {
            matches!(&top.block, Block::Paragraph { runs } if runs.iter().all(|run| !run.style.display_only))
        }));
}

#[test]
fn user_parse_never_makes_indented_code() {
    let source = "Look:\n\n    let x = **1**;\n      deeper\n\n```\n    fenced\n```";
    let tree = parse_user(source);
    assert_eq!(tree.len(), 3, "{tree:?}");
    let runs = paragraph(&tree.blocks[1].block);
    assert_eq!(shown(runs), "    let x = 1;\n      deeper");
    assert_eq!(copied(runs), "let x = 1;\ndeeper");
    assert!(runs.iter().any(|run| run.style.bold && run.text == "1"));
    assert_eq!(
        source[tree.blocks[1].range.clone()].trim_end(),
        "let x = **1**;\n      deeper"
    );
    assert_eq!(
        tree.blocks[2].block,
        Block::CodeBlock {
            language: None,
            title: None,
            code: "    fenced".into()
        }
    );
    // The assistant still gets its indented code block.
    assert!(matches!(
        parse_full(source).blocks[1].block,
        Block::CodeBlock { .. }
    ));
}

#[test]
fn task_ranges_survive_nested_lists_unicode_crlf_and_streaming() {
    let source = "Título 🦀\r\n\r\nIntro\r\n\r\n- [ ] repetida\r\n  - [X] repetida\r\n\r\n> - [x] citada\r\n\r\n```md\r\n- [ ] literal\r\n```\r\n";
    fn collect(block: &Block, tasks: &mut Vec<TaskMarker>) {
        match block {
            Block::Paragraph { runs } => {
                tasks.extend(runs.iter().filter_map(|run| run.style.task.clone()))
            }
            Block::List { items, .. } => items
                .iter()
                .flatten()
                .for_each(|block| collect(block, tasks)),
            Block::BlockQuote { children, .. } => {
                children.iter().for_each(|block| collect(block, tasks))
            }
            _ => {}
        }
    }
    let tree = parse_full(source);
    let mut tasks = Vec::new();
    for top in &tree.blocks {
        collect(&top.block, &mut tasks);
    }
    assert_eq!(tasks.len(), 3);
    assert_eq!(
        tasks
            .iter()
            .map(|task| &source[task.range.clone()])
            .collect::<Vec<_>>(),
        ["[ ]", "[X]", "[x]"]
    );
    let mut stream = IncrementalParser::new();
    for ch in source.chars() {
        stream.append(&ch.to_string());
        assert_eq!(stream.tree(), &parse_full(stream.source()));
    }
}

#[test]
fn display_snapshots_share_stable_blocks_without_mutating_old_frames() {
    let mut parser = IncrementalParser::new();
    parser.set_text("first **bold** paragraph\n\nsecond paragraph\n\nlast **open");
    let before = parser.display_tree();
    let frozen = format!("{before:?}");
    assert!(Arc::ptr_eq(&before.blocks[0], &parser.tree().blocks[0]));
    parser.append(" tail**\n\nnext paragraph");
    let after = parser.display_tree();
    assert!(Arc::ptr_eq(&before.blocks[0], &after.blocks[0]));
    assert_eq!(
        format!("{before:?}"),
        frozen,
        "old paint snapshot is immutable"
    );
    assert_eq!(parser.tree(), &parse_full(parser.source()));
    parser.reset("replacement");
    assert_eq!(
        format!("{before:?}"),
        frozen,
        "reset cannot alter an old frame"
    );
    assert_eq!(parser.tree(), &parse_full("replacement"));
}

fn stream(chunks: usize, text: &str) -> IncrementalParser {
    let mut p = IncrementalParser::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    while start < bytes.len() {
        let mut end = (start + chunks).min(bytes.len());
        while end < bytes.len() && !text.is_char_boundary(end) {
            end += 1;
        }
        p.append(&text[start..end]);
        start = end;
    }
    p
}

const CORPORA: &[&str] = &[
    "# Title\n\nHello **bold** and *italic* and `code` and ~~gone~~.\n",
    "Paragraph one\nlazy continuation\n\nParagraph two with a [link](https://x.dev).\n",
    "- item one\n- item two\n  - nested a\n  - nested b\n- item three\n\ntail\n",
    "1. first\n2. second\n\n   loose paragraph in item\n\n3. third\n",
    "```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\nafter code\n",
    "intro\n\n```\nunclosed fence streaming",
    "> quoted line\n> more quote\n>\n> - a list in a quote\n\nplain\n",
    "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\ndone\n",
    "setext candidate\n===\n\nnext para\n---\n",
    "***\n\ntext between rules\n\n---\n",
    "- [x] done task\n- [ ] open task\n",
    "    indented code line one\n    line two\n\npara\n",
    "see [it's here](/tmp/2026/Some Folder/it's here.txt) and `[raw](/tmp/a b.md)`\n",
    "text\n\n```\n[x](/tmp/a b.md)\n```\n\nafter [y](file:///tmp/c d.md)\n",
    "para with <span>inline html</span> inside\n\n<div>\nblock html\n</div>\n",
    "###### deep heading\n\n#### h4\n",
];

#[test]
fn extended_constructs_render_as_structured_blocks_while_streaming() {
    let source = "> [!NOTE]
> **Keep** this.

<!-- hidden -->
Use <kbd>Enter</kbd><br>or $x+y$.[^note]

<details><summary>Build log</summary>

```sh title=build.sh
make
```

</details>

[^note]: From the output.
";
    let full = parse_full(source);
    for chunk in [1usize, 3, 7, 16] {
        assert_eq!(stream(chunk, source).tree(), &full, "chunk {chunk}");
    }

    assert_eq!(full.blocks.len(), 4);
    assert!(matches!(
        full.blocks[0].block,
        Block::BlockQuote {
            kind: Some(AlertKind::Note),
            ..
        }
    ));
    let runs = paragraph(&full.blocks[1].block);
    assert_eq!(shown(runs), "Use Enter\nor x+y.[note]");
    assert!(runs.iter().any(|run| run.style.code && run.text == "Enter"));
    assert!(runs.iter().any(|run| run.style.code && run.text == "x+y"));
    assert!(
        runs.iter()
            .any(|run| run.style.footnote && run.text == "[note]")
    );
    let Block::Details { summary, children } = &full.blocks[2].block else {
        panic!("expected disclosure");
    };
    assert_eq!(shown(summary), "Build log");
    assert!(
        matches!(&children[0], Block::CodeBlock { title: Some(title), .. } if title == "build.sh")
    );
    assert!(matches!(
        &full.blocks[3].block,
        Block::FootnoteDefinition {
            section_start: true,
            ..
        }
    ));
}

#[test]
fn incremental_matches_full_on_streamed_corpora() {
    for (ci, corpus) in CORPORA.iter().enumerate() {
        let full = parse_full(corpus);
        for chunk in [1usize, 2, 3, 7, 16, 64] {
            let inc = stream(chunk, corpus);
            assert_eq!(
                inc.tree(),
                &full,
                "corpus {ci} diverged at chunk size {chunk}:\n{corpus}"
            );
        }
    }
}

#[test]
fn appends_keep_committed_blocks_identical() {
    // Streaming stability invariant: blocks before the reparse boundary
    // (everything but the last two top-level blocks) must be reused
    // as-is across appends — same index, same value — so row/element keys
    // never re-mount and earlier blocks can never visibly reflow.
    for corpus in CORPORA {
        let mut p = IncrementalParser::new();
        let mut prev = p.tree().clone();
        let bytes = corpus.as_bytes();
        let mut start = 0;
        while start < bytes.len() {
            let mut end = (start + 3).min(bytes.len());
            while end < bytes.len() && !corpus.is_char_boundary(end) {
                end += 1;
            }
            p.append(&corpus[start..end]);
            start = end;

            let cur = p.tree();
            let committed = prev.blocks.len().saturating_sub(2);
            assert!(
                cur.blocks.len() >= committed,
                "committed blocks disappeared:\n{corpus}"
            );
            for i in 0..committed {
                assert_eq!(
                    cur.blocks[i], prev.blocks[i],
                    "block {i} changed across an append:\n{corpus}"
                );
            }
            prev = cur.clone();
        }
    }
}

#[test]
fn incremental_matches_full_with_link_definitions() {
    // Definitions act at a distance → parser falls back to full reparses,
    // so parity must still hold.
    let corpus = "See [docs] for more.\n\nMore text.\n\n[docs]: https://example.com\n";
    let full = parse_full(corpus);
    for chunk in [1usize, 3, 9] {
        assert_eq!(stream(chunk, corpus).tree(), &full, "chunk {chunk}");
    }
    // The reference actually resolved into a link.
    let has_link = full.blocks.iter().any(|b| match &b.block {
        Block::Paragraph { runs } => runs.iter().any(|r| r.style.link.is_some()),
        _ => false,
    });
    assert!(has_link, "expected [docs] to resolve to a link");
}

#[test]
fn set_text_appends_or_resets() {
    let mut p = IncrementalParser::new();
    p.set_text("hello");
    p.set_text("hello world");
    assert_eq!(p.tree(), &parse_full("hello world"));
    // Non-append rewrites reset cleanly.
    p.set_text("different");
    assert_eq!(p.tree(), &parse_full("different"));
    assert_eq!(p.source(), "different");
}

#[test]
fn block_structure_basics() {
    let tree = parse_full("## Head\n\npara **b _bi_** text\n\n```ts\nlet x = 1;\n```\n");
    assert_eq!(tree.len(), 3);
    match &tree.blocks[0].block {
        Block::Heading { level, runs } => {
            assert_eq!(*level, 2);
            assert_eq!(runs[0].text, "Head");
        }
        other => panic!("unexpected {other:?}"),
    }
    match &tree.blocks[1].block {
        Block::Paragraph { runs } => {
            assert_eq!(runs.len(), 4); // "para ", "b ", "bi" (bold+italic), " text"
            assert!(runs[1].style.bold && !runs[1].style.italic);
            assert!(runs[2].style.bold && runs[2].style.italic);
        }
        other => panic!("unexpected {other:?}"),
    }
    match &tree.blocks[2].block {
        Block::CodeBlock { language, code, .. } => {
            assert_eq!(language.as_deref(), Some("ts"));
            assert_eq!(code, "let x = 1;");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn nested_lists_and_tight_items() {
    let tree = parse_full("- a\n  - a1\n  - a2\n- b\n");
    let Block::List {
        ordered_start,
        items,
    } = &tree.blocks[0].block
    else {
        panic!("expected list");
    };
    assert_eq!(*ordered_start, None);
    assert_eq!(items.len(), 2);
    // Tight item text became an implicit paragraph, nested list follows.
    assert!(matches!(items[0][0], Block::Paragraph { .. }));
    assert!(matches!(items[0][1], Block::List { .. }));
}

#[test]
fn tables_parse_header_and_rows() {
    let tree = parse_full("| a | b |\n|---|---|\n| 1 | 2 |\n");
    let Block::Table {
        header,
        rows,
        align,
    } = &tree.blocks[0].block
    else {
        panic!("expected table");
    };
    assert_eq!(header.len(), 2);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][1][0].text, "2");
    assert_eq!(align, &vec![TableAlign::Left, TableAlign::Left]);
}

#[test]
fn tables_parse_column_alignment() {
    let tree = parse_full("| a | b | c |\n|:--|:-:|--:|\n| 1 | 2 | 3 |\n");
    let Block::Table { align, .. } = &tree.blocks[0].block else {
        panic!("expected table");
    };
    assert_eq!(
        align,
        &vec![TableAlign::Left, TableAlign::Center, TableAlign::Right]
    );
}

#[test]
fn links_carry_urls() {
    let tree = parse_full("go to [zed](https://zed.dev) now\n");
    let Block::Paragraph { runs } = &tree.blocks[0].block else {
        panic!()
    };
    let link = runs
        .iter()
        .find(|r| r.style.link.is_some())
        .expect("link run");
    assert_eq!(link.text, "zed");
    assert_eq!(link.style.link.as_deref(), Some("https://zed.dev"));
}

/// The paragraph's single link run: (text, url).
fn only_link(source: &str) -> Option<(String, String)> {
    let tree = parse_full(source);
    let Block::Paragraph { runs } = &tree.blocks[0].block else {
        panic!()
    };
    let links: Vec<_> = runs
        .iter()
        .filter_map(|r| Some((r.text.clone(), r.style.link.clone()?)))
        .collect();
    assert!(links.len() <= 1, "expected at most one link: {links:?}");
    links.into_iter().next()
}

/// Bare URLs autolink (the GFM extension pulldown-cmark lacks): the URL
/// becomes a clickable run, trailing sentence punctuation stays text.
#[test]
fn bare_urls_autolink() {
    assert_eq!(
        only_link("PR is updated: https://github.com/opcodehq/zuse/pull/31\n"),
        Some((
            "https://github.com/opcodehq/zuse/pull/31".into(),
            "https://github.com/opcodehq/zuse/pull/31".into()
        ))
    );
    assert_eq!(
        only_link("see https://x.dev/a, then rest.\n").map(|l| l.1),
        Some("https://x.dev/a".into())
    );
    // A wrapping paren is shed; one balanced by an opener in the path stays.
    assert_eq!(
        only_link("(docs: https://x.dev/Foo_(bar))\n").map(|l| l.1),
        Some("https://x.dev/Foo_(bar)".into())
    );
    // Bold text still autolinks, and the run keeps the emphasis.
    let tree = parse_full("**see https://x.dev now**\n");
    let Block::Paragraph { runs } = &tree.blocks[0].block else {
        panic!()
    };
    let link = runs.iter().find(|r| r.style.link.is_some()).unwrap();
    assert!(link.style.bold);
    assert_eq!(link.style.link.as_deref(), Some("https://x.dev"));
}

/// Non-links stay text: glued schemes, bare schemes, code spans, and the
/// destination text of a real markdown link.
#[test]
fn autolink_leaves_non_urls_alone() {
    assert_eq!(only_link("foohttps://x.dev is glued\n"), None);
    assert_eq!(only_link("the https:// scheme alone\n"), None);
    assert_eq!(only_link("`https://x.dev` in code\n"), None);
    // A markdown link whose TEXT is a URL keeps the written destination.
    assert_eq!(
        only_link("[https://shown.dev](https://real.dev)\n"),
        Some(("https://shown.dev".into(), "https://real.dev".into()))
    );
}

#[test]
fn top_level_ranges_are_stable_anchors() {
    let src = "first\n\nsecond\n\nthird";
    let tree = parse_full(src);
    assert_eq!(tree.len(), 3);
    assert!(
        tree.blocks
            .windows(2)
            .all(|w| w[0].range.start < w[1].range.start)
    );
    assert_eq!(&src[tree.blocks[1].range.clone()], "second\n");
}

/// Concatenated visible text of a block (what the user reads).
fn flat(block: &Block) -> String {
    fn walk(b: &Block, out: &mut String) {
        match b {
            Block::Paragraph { runs } | Block::Heading { runs, .. } => {
                for r in runs {
                    out.push_str(&r.text);
                }
            }
            Block::BlockQuote { children, .. } => children.iter().for_each(|c| walk(c, out)),
            Block::List { items, .. } => items.iter().flatten().for_each(|c| walk(c, out)),
            _ => {}
        }
    }
    let mut s = String::new();
    walk(block, &mut s);
    s
}

#[test]
fn display_tree_styles_hanging_bold_immediately() {
    let mut p = IncrementalParser::new();
    p.set_text("intro **bo");
    let display = p.display_tree();
    let Block::Paragraph { runs } = &display.blocks[0].block else {
        panic!("expected paragraph");
    };
    let bold: Vec<_> = runs.iter().filter(|r| r.style.bold).collect();
    assert_eq!(bold.len(), 1);
    assert_eq!(bold[0].text, "bo");
    assert!(!flat(&display.blocks[0].block).contains("**"));
    // The canonical tree stays honest: literal markers until truly closed.
    assert!(flat(&p.tree().blocks[0].block).contains("**"));
}

#[test]
fn display_tree_converges_to_canonical_when_balanced() {
    let corpus = "a **b** *c* `d` [e](https://x.dev) ~~f~~";
    let mut p = IncrementalParser::new();
    p.set_text(corpus);
    assert_eq!(p.display_tree(), *p.tree());
    assert_eq!(p.tree(), &parse_full(corpus));
}

#[test]
fn display_tree_never_leaks_streaming_urls() {
    let full = "read [docs](https://example.com/long/path) now";
    let mut p = IncrementalParser::new();
    for i in 1..=full.len() {
        if !full.is_char_boundary(i) {
            continue;
        }
        p.set_text(&full[..i]);
        let text = flat(&p.display_tree().blocks[0].block);
        assert!(!text.contains("http"), "url leaked at {i}: {text:?}");
    }
    // Mid-URL the link text carries the pending sentinel destination.
    let mut p = IncrementalParser::new();
    p.set_text("read [docs](https://exa");
    let Block::Paragraph { runs } = &p.display_tree().blocks[0].block else {
        panic!("expected paragraph");
    };
    let link = runs
        .iter()
        .find(|r| r.style.link.is_some())
        .expect("link run");
    assert_eq!(link.text, "docs");
    assert_eq!(
        link.style.link.as_deref(),
        Some(crate::mend::PENDING_LINK_URL)
    );
}

#[test]
fn display_tree_leaves_code_blocks_alone() {
    let mut p = IncrementalParser::new();
    p.set_text("intro\n\n```\nunclosed **fence");
    assert_eq!(p.display_tree(), *p.tree());
}

#[test]
fn display_tree_suppresses_setext_flicker() {
    // "para" + "\n-" parses as an H2 for exactly one chunk before the
    // list item's text arrives; the display tree keeps it a paragraph.
    let mut p = IncrementalParser::new();
    p.set_text("para\n-");
    let display = p.display_tree();
    assert!(
        matches!(
            display.blocks.last().unwrap().block,
            Block::Paragraph { .. }
        ),
        "expected paragraph, got {display:?}"
    );
}

#[test]
fn display_tree_prefix_matches_canonical_across_streams() {
    // Mending swaps only the last block: everything before it must be
    // byte-identical to the canonical tree so render caches and row keys
    // survive.
    for corpus in CORPORA {
        let mut p = IncrementalParser::new();
        let bytes = corpus.as_bytes();
        let mut start = 0;
        while start < bytes.len() {
            let mut end = (start + 3).min(bytes.len());
            while end < bytes.len() && !corpus.is_char_boundary(end) {
                end += 1;
            }
            p.append(&corpus[start..end]);
            start = end;

            let display = p.display_tree();
            let canonical = p.tree();
            for i in 0..canonical.blocks.len().saturating_sub(1) {
                assert_eq!(
                    display.blocks[i], canonical.blocks[i],
                    "display prefix diverged:\n{corpus}"
                );
            }
        }
    }
}

#[test]
fn empty_and_whitespace_sources() {
    assert!(parse_full("").is_empty());
    assert!(parse_full("\n\n  \n").is_empty());
    let mut p = IncrementalParser::new();
    p.append("");
    assert!(p.tree().is_empty());
}

#[test]
fn fence_info_titles() {
    assert_eq!(parse_fence_info(""), (None, None));
    assert_eq!(parse_fence_info("rust"), (Some("rust".into()), None));
    assert_eq!(
        parse_fence_info(r#"ts title="src/app config.ts""#),
        (Some("ts".into()), Some("src/app config.ts".into()))
    );
    assert_eq!(
        parse_fence_info("py title=main.py extra"),
        (Some("py".into()), Some("main.py".into()))
    );
    assert_eq!(
        parse_fence_info("bash filename=run.sh"),
        (Some("bash".into()), Some("run.sh".into()))
    );
    // `title` wins over `filename`, whatever the order.
    assert_eq!(
        parse_fence_info("js filename=a.js title='B'"),
        (Some("js".into()), Some("B".into()))
    );
    // An attribute in first position is not a language.
    assert_eq!(parse_fence_info("title=x.rs"), (None, Some("x.rs".into())));
    // An unterminated quote takes the rest; empty values are ignored.
    assert_eq!(
        parse_fence_info(r#"go title="a b"#),
        (Some("go".into()), Some("a b".into()))
    );
    assert_eq!(
        parse_fence_info(r#"go title="""#),
        (Some("go".into()), None)
    );
}

#[test]
fn code_block_keeps_fence_title() {
    let tree = parse_full("```rust title=\"lib.rs\"\nfn x() {}\n```\n");
    let Block::CodeBlock {
        language,
        title,
        code,
    } = &tree.blocks[0].block
    else {
        panic!("expected code block");
    };
    assert_eq!(language.as_deref(), Some("rust"));
    assert_eq!(title.as_deref(), Some("lib.rs"));
    assert_eq!(code, "fn x() {}");
}

#[cfg(test)]
mod closing_quote_blocks {
    use super::*;

    const STORY: &str = "\"How do we negotiate with machines that won't speak?\" someone asked.\n\nYuki almost laughed. \"You don't. You listen to the silence. And you finally understand what it means to be powerless.\"";

    #[test]
    fn full_parse_keeps_trailing_quote_in_block() {
        let tree = parse_full(STORY);
        for b in &tree.blocks {
            eprintln!("block {:?} => {:?}", b.range, &STORY[b.range.clone()]);
        }
        assert_eq!(tree.blocks.len(), 2, "two paragraphs expected");
        let last = &tree.blocks[1];
        assert!(STORY[last.range.clone()].ends_with("powerless.\""));
    }

    #[test]
    fn streamed_boundary_at_quote_adds_no_block() {
        // Stream with a commit boundary exactly between `powerless.` and `"`.
        let split = STORY.len() - 1;
        let mut p = IncrementalParser::new();
        p.set_text(&STORY[..split]);
        p.set_text(STORY);
        let tree = p.tree();
        for b in &tree.blocks {
            eprintln!("block {:?} => {:?}", b.range, &STORY[b.range.clone()]);
        }
        assert_eq!(tree.blocks.len(), 2, "streamed split must not add blocks");
        assert!(STORY[tree.blocks[1].range.clone()].ends_with("powerless.\""));
    }

    #[test]
    fn streamed_small_chunks_match_full_parse() {
        let mut p = IncrementalParser::new();
        let mut fed = String::new();
        for chunk in STORY.as_bytes().chunks(7) {
            fed.push_str(std::str::from_utf8(chunk).unwrap());
            p.set_text(&fed);
        }
        let full = parse_full(STORY);
        assert_eq!(p.tree().blocks.len(), full.blocks.len());
        for (a, b) in p.tree().blocks.iter().zip(full.blocks.iter()) {
            assert_eq!(a.range, b.range);
        }
    }
}

#[cfg(test)]
mod image_model_tests {
    use super::*;
    #[test]
    fn images_preserve_alt_title_position_links_and_empty_alt() {
        let tree =
            parse_full("Before [![**alt**](a.png \"Title\")](next.md) after ![](b.png) ![](b.png)");
        let Block::Paragraph { runs } = &tree.blocks[0].block else {
            panic!("paragraph");
        };
        let images: Vec<_> = runs.iter().filter_map(|r| r.style.image.as_ref()).collect();
        assert_eq!(images.len(), 3);
        assert_eq!(images[0].alt, "alt");
        assert_eq!(images[0].title, "Title");
        assert_eq!(images[0].link.as_deref(), Some("next.md"));
        assert_eq!(images[1].source, "b.png");
        assert!(images[1].alt.is_empty());
        assert_eq!(runs.first().unwrap().text, "Before ");
    }
}
