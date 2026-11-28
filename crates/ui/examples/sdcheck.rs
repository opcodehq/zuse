//! Smoke check for projecting a sent URL chip beside Markdown prose.

fn main() {
    let (display, spans) =
        zuse_ui::composer::sent_mention_display("check https://example.com and **bold**")
            .expect("expected a sent link chip");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].path.as_ref(), "https://example.com");
    assert!(display.ends_with(" and **bold**"));
    println!("Sent link chip projection passed.");
}
