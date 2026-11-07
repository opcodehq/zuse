//! Every catalogue limit at its boundary: the largest accepted value passes
//! and one past it fails.

use crate::common::{err, ok, rejects, repeat, text_of};
use zuse_ui_block::{Component, MAX_SOURCE_CHARS};

fn strings(count: usize) -> String {
    format!("[{}]", repeat("\"x\"", count))
}

fn points(count: usize, value: &str) -> String {
    format!(
        "[{}]",
        (0..count)
            .map(|i| format!(r#"{{label: "{i}", value: {value}}}"#))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn form(field: &str) -> String {
    format!("root = Form([{field}])")
}

#[test]
fn source_length_cap() {
    let prefix = "root = Text(\"";
    let suffix = "\")";
    let body = MAX_SOURCE_CHARS - prefix.len() - suffix.len();
    let exact = format!("{prefix}{}{suffix}", text_of(body));
    assert_eq!(exact.len(), MAX_SOURCE_CHARS);
    ok(&exact);
    rejects(
        &format!("{prefix}{}{suffix}", text_of(body + 1)),
        "The spec is 32769 characters; the limit is 32768",
    );
    // Measured in UTF-16 units: one astral character counts as two.
    let astral = format!("{prefix}{}😀{suffix}", text_of(body - 1));
    err(&astral);
}

#[test]
fn containers_hold_at_most_64_children() {
    for container in ["Card", "Grid"] {
        ok(&format!(
            "root = {container}([{}])",
            repeat(r#"Text("x")"#, 64)
        ));
        rejects(
            &format!("root = {container}([{}])", repeat(r#"Text("x")"#, 65)),
            &format!("{container} children: allows at most 64 items (got 65)"),
        );
    }
    let fields = (0..64)
        .map(|i| format!(r#"Checkbox("f{i}", "F")"#))
        .collect::<Vec<_>>()
        .join(", ");
    ok(&format!("root = Form([{fields}])"));
    rejects(
        &format!("root = Form([{fields}, Text(\"x\")])"),
        "Form children: allows at most 64 items",
    );
    rejects("root = Form([])", "Form children: needs at least 1 item");
    ok("root = Grid([])");
    ok("root = Card([])");
    rejects(
        "root = Grid()",
        "Grid required argument `children` is missing",
    );
}

#[test]
fn key_value_and_list_hold_at_most_100_entries() {
    let pairs = |n| repeat(r#"{key: "k", value: "v"}"#, n);
    ok(&format!("root = KeyValue([{}])", pairs(100)));
    ok("root = KeyValue([])");
    rejects(
        &format!("root = KeyValue([{}])", pairs(101)),
        "KeyValue pairs: allows at most 100 items",
    );
    ok(&format!("root = List({})", strings(100)));
    rejects(
        &format!("root = List({})", strings(101)),
        "List items: allows at most 100 items",
    );
}

#[test]
fn table_columns_rows_and_cells() {
    ok(&format!("root = Table({}, [{}])", strings(20), strings(20)));
    rejects(
        &format!("root = Table({}, [])", strings(21)),
        "Table columns: allows at most 20 items",
    );
    rejects(
        "root = Table([], [])",
        "Table columns: needs at least 1 item",
    );
    ok(&format!(
        "root = Table([\"A\"], [{}])",
        repeat("[\"x\"]", 200)
    ));
    rejects(
        &format!("root = Table([\"A\"], [{}])", repeat("[\"x\"]", 201)),
        "Table rows: allows at most 200 items",
    );
    rejects(
        &format!("root = Table({}, [{}])", strings(20), strings(21)),
        "Table rows[0]: allows at most 20 items",
    );
    let error = rejects(
        "root = Table([\"A\", \"B\"], [\n  [\"1\", \"2\"],\n  [\"3\"]\n])",
        "rows[1] has 1 cell but there are 2 columns",
    );
    assert_eq!(error.issues()[0].line, Some(3));
}

#[test]
fn progress_percent_is_0_to_100() {
    for percent in ["0", "100", "42.5"] {
        ok(&format!(r#"root = Progress("p", {percent})"#));
    }
    for percent in ["-1", "100.5", "101", "\"50\""] {
        err(&format!(r#"root = Progress("p", {percent})"#));
    }
}

#[test]
fn grid_columns_are_whole_numbers_1_to_4() {
    for columns in ["1", "4", "2.0"] {
        ok(&format!(r#"root = Grid([Text("x")], {columns})"#));
    }
    rejects(r#"root = Grid([Text("x")], 0)"#, "must be between 1 and 4");
    rejects(r#"root = Grid([Text("x")], 5)"#, "must be between 1 and 4");
    rejects(r#"root = Grid([Text("x")], 2.5)"#, "must be a whole number");
}

#[test]
fn bar_and_line_charts_take_1_to_100_finite_points() {
    for chart in ["BarChart", "LineChart"] {
        ok(&format!(r#"root = {chart}("c", {})"#, points(1, "-2")));
        ok(&format!(r#"root = {chart}("c", {})"#, points(100, "1.5")));
        rejects(
            &format!(r#"root = {chart}("c", [])"#),
            "points: needs at least 1 item",
        );
        rejects(
            &format!(r#"root = {chart}("c", {})"#, points(101, "1")),
            "points: allows at most 100 items",
        );
        rejects(
            &format!(r#"root = {chart}("c", {})"#, points(1, "1e400")),
            "must be a finite number",
        );
    }
}

#[test]
fn donut_takes_1_to_12_nonnegative_points() {
    ok(&format!(r#"root = DonutChart("d", {})"#, points(12, "1")));
    ok(&format!(r#"root = DonutChart("d", {})"#, points(1, "0")));
    rejects(
        &format!(r#"root = DonutChart("d", {})"#, points(13, "1")),
        "allows at most 12 items",
    );
    rejects(r#"root = DonutChart("d", [])"#, "needs at least 1 item");
    rejects(
        &format!(r#"root = DonutChart("d", {})"#, points(1, "-0.5")),
        "points[0].value: must be at least 0 (got -0.5)",
    );
}

#[test]
fn steps_take_1_to_20_items_with_known_statuses() {
    let steps = |n: usize, status: &str| {
        format!(
            "root = Steps([{}])",
            repeat(&format!(r#"{{label: "s", status: "{status}"}}"#), n)
        )
    };
    for status in ["done", "active", "pending", "failed"] {
        ok(&steps(1, status));
    }
    ok(&steps(20, "done"));
    rejects(&steps(21, "done"), "allows at most 20 items");
    rejects("root = Steps([])", "needs at least 1 item");
    rejects(
        &steps(1, "skipped"),
        r#"Steps items[0].status: expected one of "done", "active", "pending", "failed""#,
    );
}

#[test]
fn flow_nodes_labels_details_and_caption() {
    let flow = |n: usize, label: &str, detail: &str, caption: &str| {
        format!(
            r#"root = Flow([{}], "{caption}")"#,
            repeat(&format!(r#"{{label: "{label}", detail: "{detail}"}}"#), n)
        )
    };
    ok(&flow(2, &text_of(60), &text_of(200), &text_of(200)));
    ok(&flow(8, "a", "", ""));
    rejects(&flow(1, "a", "", ""), "Flow nodes: needs at least 2 items");
    rejects(&flow(9, "a", "", ""), "Flow nodes: allows at most 8 items");
    rejects(
        &flow(2, "", "", ""),
        "Flow nodes[0].label: must not be empty",
    );
    rejects(
        &flow(2, &text_of(61), "", ""),
        "Flow nodes[0].label: must be at most 60 characters (got 61)",
    );
    rejects(
        &flow(2, "a", &text_of(201), ""),
        "Flow nodes[0].detail: must be at most 200 characters",
    );
    rejects(
        &flow(2, "a", "", &text_of(201)),
        "Flow caption: must be at most 200 characters",
    );
    rejects(
        r#"root = Flow([{label: "a", highlight: "yes"}, {label: "b"}])"#,
        "Flow nodes[0].highlight: expected true or false",
    );
}

#[test]
fn tabs_take_2_to_6_labeled_components() {
    let tabs = |n: usize, label: &str| {
        format!(
            "root = Tabs([{}])",
            repeat(&format!(r#"{{label: "{label}", content: Text("x")}}"#), n)
        )
    };
    ok(&tabs(2, &text_of(40)));
    ok(&tabs(6, "t"));
    rejects(&tabs(1, "t"), "Tabs tabs: needs at least 2 items");
    rejects(&tabs(7, "t"), "Tabs tabs: allows at most 6 items");
    rejects(&tabs(2, ""), "Tabs tabs[0].label: must not be empty");
    rejects(
        &tabs(2, &text_of(41)),
        "Tabs tabs[0].label: must be at most 40 characters",
    );
    rejects(
        r#"root = Tabs([{label: "a"}, {label: "b", content: Text("x")}])"#,
        "missing required key `content`",
    );
}

#[test]
fn follow_ups_take_1_to_6_items() {
    let items = |n: usize, label: &str, prompt: &str| {
        format!(
            "root = FollowUps([{}])",
            repeat(&format!(r#"{{label: "{label}", prompt: "{prompt}"}}"#), n)
        )
    };
    ok(&items(1, &text_of(80), &text_of(2000)));
    ok(&items(6, "a", "b"));
    rejects(&items(7, "a", "b"), "allows at most 6 items");
    rejects(&items(1, &text_of(81), "b"), "label: must be at most 80");
    rejects(
        &items(1, "a", &text_of(2001)),
        "prompt: must be at most 2000",
    );
    rejects(&items(1, "a", ""), "prompt: must not be empty");
}

#[test]
fn field_names_follow_the_identifier_pattern() {
    for name in ["a", "Z9", "a-b_c", &format!("a{}", text_of(39))] {
        ok(&form(&format!(r#"Input("{name}", "L")"#)));
    }
    for name in [
        "",
        "1a",
        "-a",
        "_a",
        "a b",
        "a.b",
        "é",
        &format!("a{}", text_of(40)),
    ] {
        rejects(
            &form(&format!(r#"Input("{name}", "L")"#)),
            "is not a valid field name",
        );
    }
}

#[test]
fn field_labels_and_values() {
    rejects(&form(r#"Input("a", "")"#), "Input label: must not be empty");
    ok(&form(&format!(
        r#"Input("a", "A", "placeholder", "{}")"#,
        text_of(2000)
    )));
    rejects(
        &form(&format!(r#"Input("a", "A", null, "{}")"#, text_of(2001))),
        "Input value: must be at most 2000 characters",
    );
    ok(&form(&format!(
        r#"TextArea("a", "A", null, "{}")"#,
        text_of(8000)
    )));
    rejects(
        &form(&format!(r#"TextArea("a", "A", null, "{}")"#, text_of(8001))),
        "TextArea value: must be at most 8000 characters",
    );
    ok(&format!(
        r#"root = Form([Input("a", "A")], "{}")"#,
        text_of(40)
    ));
    rejects(
        &format!(r#"root = Form([Input("a", "A")], "{}")"#, text_of(41)),
        "Form submitLabel: must be at most 40 characters",
    );
    rejects(
        r#"root = Form([Input("a", "A")], "")"#,
        "Form submitLabel: must not be empty",
    );
}

#[test]
fn select_options() {
    let select =
        |options: &str, value: &str| form(&format!(r#"Select("s", "S", {options}{value})"#));
    let distinct = |n: usize| {
        format!(
            "[{}]",
            (0..n)
                .map(|i| format!("\"o{i}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    ok(&select(&distinct(1), ""));
    ok(&select(&distinct(50), ", \"o49\""));
    rejects(&select("[]", ""), "Select options: needs at least 1 item");
    rejects(
        &select(&distinct(51), ""),
        "Select options: allows at most 50 items",
    );
    ok(&select(&format!("[\"{}\"]", text_of(120)), ""));
    rejects(
        &select(&format!("[\"{}\"]", text_of(121)), ""),
        "Select options[0]: must be at most 120 characters",
    );
    rejects(
        &select("[\"\"]", ""),
        "Select options[0]: must not be empty",
    );
    rejects(
        &select(&distinct(2), ", \"nope\""),
        r#"Select s: value must be one of the options (got "nope"; options: "o0", "o1")"#,
    );
}

#[test]
fn radio_group_takes_2_to_8_options() {
    let radio = |n: usize, value: &str| {
        form(&format!(
            r#"RadioGroup("r", "R", [{}]{value})"#,
            (0..n)
                .map(|i| format!("\"o{i}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    };
    ok(&radio(2, ", \"o1\""));
    ok(&radio(8, ""));
    rejects(&radio(1, ""), "RadioGroup options: needs at least 2 items");
    rejects(&radio(9, ""), "RadioGroup options: allows at most 8 items");
    rejects(
        &radio(2, ", \"o5\""),
        "RadioGroup r: value must be one of the options",
    );
}

#[test]
fn slider_range_value_and_step() {
    let slider = |args: &str| form(&format!(r#"Slider("s", "S", {args})"#));
    for args in [
        "0, 10",
        "0, 10, 0",
        "0, 10, 10",
        "-5, 5, -5, 0.5",
        "0, 1, null, 0.01",
    ] {
        ok(&slider(args));
    }
    rejects(
        &slider("5, 5"),
        "Slider s: min must be below max (min 5, max 5)",
    );
    rejects(
        &slider("0, 10, -1"),
        "Slider s: value must be within min and max",
    );
    rejects(
        &slider("0, 10, 11"),
        "Slider s: value must be within min and max",
    );
    rejects(
        &slider("0, 10, 5, 0"),
        "Slider step: must be greater than 0 (got 0)",
    );
    rejects(
        &slider("0, 10, 5, -1"),
        "Slider step: must be greater than 0",
    );
    rejects(&slider("0, 1e400"), "Slider max: must be a finite number");
}

#[test]
fn checkbox_checked_is_boolean() {
    assert_eq!(
        crate::common::root(&form(r#"Checkbox("c", "C")"#)),
        Component::Form {
            children: vec![Component::Checkbox {
                name: "c".into(),
                label: "C".into(),
                checked: false
            }],
            submit_label: None,
            title: None,
        }
    );
    rejects(
        &form(r#"Checkbox("c", "C", "yes")"#),
        "expected true or false",
    );
}

#[test]
fn tones_are_neutral_good_warn_or_bad() {
    for tone in ["neutral", "good", "warn", "bad"] {
        ok(&format!(r#"root = Badge("b", "{tone}")"#));
        ok(&format!(r#"root = Callout("c", "{tone}")"#));
    }
    rejects(
        r#"root = Callout("c", "info")"#,
        "Callout tone: expected one of",
    );
}

// ─── Expansion bounds ───────────────────────────────────────────────────────

/// `k` is a 302-node KeyValue; `root` references it 12 times and adds
/// `stats` inline Stat calls. Visits: root 2 + 12 × 303 + stats nodes, plus
/// `k` itself (302).
fn expansion_spec(stats: &[&str]) -> String {
    let refs = vec!["k"; 12];
    let mut children: Vec<&str> = refs;
    children.extend_from_slice(stats);
    format!(
        "root = Card([{}])\nk = KeyValue([{}])",
        children.join(", "),
        repeat(r#"{key: "a", value: "b"}"#, 100)
    )
}

#[test]
fn expansion_allows_exactly_4096_visited_nodes() {
    let four = vec![r#"Stat("a", "b", "c")"#; 39];
    ok(&expansion_spec(&four));
    let mut over = vec![r#"Stat("a", "b", "c")"#; 38];
    over.push(r#"Stat("a", "b")"#);
    over.push(r#"Text("x")"#);
    rejects(
        &expansion_spec(&over),
        "expanding references visits 4097 values (limit 4096)",
    );
}

#[test]
fn expansion_allows_depth_48_but_not_49() {
    // 22 Cards (2 levels each) around Tabs whose deepest string sits at +4.
    let tabs = r#"Tabs([{label: "a", content: Text("x")}, {label: "b", content: Text("y")}])"#;
    let mut deep = tabs.to_string();
    for _ in 0..22 {
        deep = format!("Card([{deep}])");
    }
    ok(&format!("root = {deep}"));

    let mut too_deep = r#"Text("x")"#.to_string();
    for _ in 0..24 {
        too_deep = format!("Card([{too_deep}])");
    }
    rejects(&format!("root = {too_deep}"), "too complex");
}

#[test]
fn reference_chains_count_toward_depth() {
    // Each hop is Card → [ → ref → target: three levels.
    let chain = |hops: usize| {
        let mut parts = vec![r#"n0 = Text("x")"#.to_string()];
        for i in 1..=hops {
            parts.push(format!("n{i} = Card([n{}])", i - 1));
        }
        parts.push(format!("root = n{hops}"));
        parts.join("\n")
    };
    // root → n15 adds one hop; 15 Card hops × 3 + Text string → 47.
    ok(&chain(15));
    rejects(&chain(16), "nest more than 48 levels deep");
    // Hostile chains stay cheap and still report the bound, in either order.
    let long = chain(1000);
    rejects(&long, "too complex");
    let reversed: Vec<&str> = long.lines().rev().collect();
    rejects(&reversed.join("\n"), "too complex");
}
