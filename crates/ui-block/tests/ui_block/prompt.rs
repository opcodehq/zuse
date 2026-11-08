//! Prompt text generated from the catalogue.

use crate::common::ok;
use zuse_ui_block::{EXAMPLES, RESPONSE_INSTRUCTIONS, component_lines, tool_description};

const EXPECTED_SIGNATURES: &[&str] = &[
    "Card(children?: any[], title?: string)",
    "Text(text: string)",
    "Stat(label: string, value: string, hint?: string)",
    "KeyValue(pairs: { key: string, value: string }[])",
    "Table(columns: string[], rows: string[][])",
    "Progress(label: string, percent: number)",
    r#"Badge(label: string, tone?: "neutral" | "good" | "warn" | "bad")"#,
    "List(items: string[])",
    "Grid(children: any[], columns?: number)",
    "BarChart(title: string, points: { label: string, value: number }[], unit?: string)",
    "LineChart(title: string, points: { label: string, value: number }[], unit?: string)",
    "DonutChart(title: string, points: { label: string, value: number }[], unit?: string)",
    r#"Callout(text: string, tone?: "neutral" | "good" | "warn" | "bad", title?: string)"#,
    r#"Steps(items: { label: string, status: "done" | "active" | "pending" | "failed", detail: string }[])"#,
    "Flow(nodes: { label: string, detail: string, highlight: boolean }[], caption?: string)",
    "Tabs(tabs: { label: string, content: any }[])",
    "Form(children: any[], submitLabel?: string, title?: string)",
    "Input(name: string, label: string, placeholder?: string, value?: string)",
    "TextArea(name: string, label: string, placeholder?: string, value?: string)",
    "Select(name: string, label: string, options: string[], value?: string)",
    "RadioGroup(name: string, label: string, options: string[], value?: string)",
    "Checkbox(name: string, label: string, checked?: boolean)",
    "Slider(name: string, label: string, min: number, max: number, value?: number, step?: number)",
];

#[test]
fn renders_positional_signatures_in_catalogue_order() {
    let lines = component_lines();
    let signatures: Vec<&str> = lines
        .lines()
        .map(|line| line.split_once(" — ").expect("signature — description").0)
        .collect();
    assert_eq!(signatures, EXPECTED_SIGNATURES);
    assert!(!lines.contains("FollowUps"));
    assert!(lines.contains(
        "Stat(label: string, value: string, hint?: string) — Single prominent metric — large value with a small label and optional hint."
    ));
    assert!(lines.contains("Slider(name: string, label: string, min: number, max: number, value?: number, step?: number) — Numeric range inside a Form; value defaults to min."));
}

#[test]
fn tool_description_examples_all_parse() {
    let description = tool_description();
    for (title, spec) in EXAMPLES {
        assert!(
            description.contains(&format!("Example — {title}:\n{spec}")),
            "{title}"
        );
        ok(spec);
    }
    assert!(description.contains(&component_lines()));
    assert!(description.contains(RESPONSE_INSTRUCTIONS));
    assert!(description.contains("- Pass null for an optional arg you want to skip"));
}

#[test]
fn prompt_text_uses_this_hosts_tool_names() {
    let description = tool_description();
    for text in [RESPONSE_INSTRUCTIONS, description.as_str()] {
        assert!(text.contains("show_ui"));
        assert!(!text.contains("emit_ui"));
        assert!(!text.contains("html_preview") && !text.contains("html_render"));
        assert!(text.contains("UI block"));
    }
    assert!(RESPONSE_INSTRUCTIONS.contains(
        "use `show_visual` for custom drawing, 3D scenes, shaders, or live simulations the components cannot express."
    ));
}
