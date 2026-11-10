//! The diff lines an inline review comment is anchored to.
use crate::theme::Theme;
use gpui::{AnyElement, div, prelude::*, px};

/// The most lines a comment shows; a hunk can run to hundreds.
const MAX_VISIBLE: usize = 8;
/// Unchanged or neighboring lines kept above the commented ones.
const CONTEXT: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Added,
    Removed,
    Context,
}

#[derive(Debug, PartialEq, Eq)]
struct Row {
    number: Option<u64>,
    kind: Kind,
    text: String,
}

/// The two starting line numbers from `@@ -old,n +new,n @@`.
fn header_lines(header: &str) -> Option<(u64, u64)> {
    let mut parts = header.strip_prefix("@@ ")?.split_whitespace();
    let start = |part: Option<&str>, sign: char| -> Option<u64> {
        part?.strip_prefix(sign)?.split(',').next()?.parse().ok()
    };
    Some((start(parts.next(), '-')?, start(parts.next(), '+')?))
}

fn parse(hunk: &str) -> Vec<Row> {
    let mut lines = hunk.lines();
    let Some((mut old, mut new)) = lines.next().and_then(header_lines) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for line in lines {
        let (kind, number) = match line.chars().next() {
            Some('+') => {
                new += 1;
                (Kind::Added, new - 1)
            }
            Some('-') => {
                old += 1;
                (Kind::Removed, old - 1)
            }
            // "\ No newline at end of file" is not a line of the file.
            Some('\\') => continue,
            _ => {
                old += 1;
                new += 1;
                (Kind::Context, new - 1)
            }
        };
        rows.push(Row {
            number: Some(number),
            kind,
            text: line.get(1..).unwrap_or_default().replace('\t', "    "),
        });
    }
    rows
}

/// GitHub ends a comment's hunk at the commented line, so `commented` lines
/// count back from the end.
pub(crate) fn review_hunk(hunk: &str, commented: usize, theme: &Theme) -> Option<AnyElement> {
    let rows = parse(hunk);
    if rows.is_empty() {
        return None;
    }
    let commented = commented.clamp(1, MAX_VISIBLE).min(rows.len());
    let visible = (commented + CONTEXT).min(MAX_VISIBLE).min(rows.len());
    let first_commented = rows.len() - commented;
    let start = rows.len() - visible;
    Some(
        div()
            .debug_selector(|| "pr-review-hunk".into())
            .py(px(2.0))
            .flex()
            .flex_col()
            .font_family(theme.font_mono.clone())
            .text_size(px(11.0))
            .line_height(px(18.0))
            .children(rows.into_iter().enumerate().skip(start).map(|(ix, row)| {
                let (tint, sign) = match row.kind {
                    Kind::Added => (Some(theme.diff_add), "+"),
                    Kind::Removed => (Some(theme.diff_del), "-"),
                    Kind::Context => (None, " "),
                };
                let bar = if ix >= first_commented {
                    theme.accent
                } else {
                    gpui::transparent_black()
                };
                div()
                    .min_w_0()
                    .flex()
                    .border_l_2()
                    .border_color(bar)
                    .when_some(tint, |el, tint| el.bg(tint.opacity(0.12)))
                    .child(
                        div()
                            .flex_none()
                            .w(px(34.0))
                            .pr(px(6.0))
                            .flex()
                            .justify_end()
                            .text_color(theme.text_faint)
                            .child(row.number.map(|n| n.to_string()).unwrap_or_default()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(12.0))
                            .text_color(tint.unwrap_or(theme.text_faint))
                            .child(sign),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.text)
                            .child(row.text),
                    )
            }))
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_follow_each_side_of_the_hunk() {
        let rows =
            parse("@@ -10,3 +20,3 @@ fn a()\n keep\n-old\n+new\n\\ No newline at end of file");
        assert_eq!(
            rows.iter().map(|r| (r.number, r.kind)).collect::<Vec<_>>(),
            [
                (Some(20), Kind::Context),
                (Some(11), Kind::Removed),
                (Some(21), Kind::Added)
            ]
        );
    }

    #[test]
    fn missing_or_malformed_hunks_show_nothing() {
        assert!(parse("").is_empty());
        assert!(parse("not a hunk\n+x").is_empty());
    }
}
