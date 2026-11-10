//! Parsing, resolution and validation fixtures, ported from the reference
//! test suites plus extra coverage for syntax errors and error quality.

use crate::common::{err, ok, rejects, repeat, root};
use zuse_ui_block::{
    ChartPoint, Component, FlowNode, KeyValuePair, MAX_SOURCE_CHARS, Spec, Step, StepStatus, Tab,
    Tone, parse, validate,
};

// ─── Normalization ──────────────────────────────────────────────────────────

#[test]
fn recovers_common_generated_summary_variants() {
    for spec in [
        "root = Card([title, checks])\ntitle = CardHeader(\"Health\")\nchecks = KeyValue([[\"Build\", \"Passed\"]])",
        "root = KeyValue([{label: \"Build\", value: \"Passed\"}])",
    ] {
        assert!(validate(spec).is_ok(), "{spec}");
        let json = serde_json::to_string(&ok(spec).root).unwrap();
        assert!(!json.contains("CardHeader"), "{json}");
        assert!(json.contains(r#""key":"Build""#), "{json}");
    }
}

#[test]
fn normalization_builds_the_expected_tree() {
    let tree = root(
        r#"root = Card([CardHeader("Health"), KeyValue([["Build", "Passed"], {label: "Types", value: "Passed"}])])"#,
    );
    let pair = |key: &str| KeyValuePair {
        key: key.into(),
        value: "Passed".into(),
    };
    assert_eq!(
        tree,
        Component::Card {
            children: vec![
                Component::Text {
                    text: "Health".into()
                },
                Component::KeyValue {
                    pairs: vec![pair("Build"), pair("Types")],
                },
            ],
            title: None,
        }
    );
}

#[test]
fn normalization_preserves_literal_content_and_is_idempotent() {
    let source =
        r#"root = Card([CardHeader("Health"), KeyValue([["Build", "CardHeader(\"quoted\")"]])])"#;
    let prepared = ok(source);
    assert_eq!(
        prepared.source,
        r#"root = Card([Text("Health"), KeyValue([{"key": "Build", "value": "CardHeader(\"quoted\")"}])])"#
    );
    assert_eq!(ok(&prepared.source), prepared);
    assert!(
        serde_json::to_string(&prepared.root)
            .unwrap()
            .contains("quoted")
    );
}

#[test]
fn leaves_canonical_specs_and_quoted_source_text_unchanged() {
    let spec = r#"root = Text("CardHeader and KeyValue are literal text")"#;
    assert_eq!(
        ok(spec),
        Spec {
            root: Component::Text {
                text: "CardHeader and KeyValue are literal text".into()
            },
            source: spec.into(),
        }
    );
}

#[test]
fn keeps_rejecting_unsafe_or_ambiguous_variants() {
    for spec in [
        r#"root = Card([CardHeader("Health")"#,
        r#"root = KeyValue([["Build"]])"#,
        r#"root = KeyValue([["Build", "Passed", "extra"]])"#,
        r#"root = KeyValue([{label: "Build", value: 42}])"#,
        r#"root = Card([CardHeader("Health"), Input("secret", "Secret")])"#,
        r#"root = Card([CardHeader("Health"), root])"#,
        r#"root = Card([CardHeader("Health"), Query("data")])"#,
        "root = CardHeader(\"A\")\nroot = CardHeader(\"B\")",
    ] {
        err(spec);
    }
}

#[test]
fn card_header_with_other_arity_is_explained() {
    rejects(
        r#"root = Card([CardHeader("A", "B")])"#,
        "CardHeader takes exactly one argument",
    );
}

#[test]
fn key_value_with_both_key_and_label_is_not_rewritten() {
    rejects(
        r#"root = KeyValue([{key: "A", label: "B"}])"#,
        "missing required key `value`",
    );
}

// ─── Reference fixtures ─────────────────────────────────────────────────────

#[test]
fn validates_part_to_whole_charts_including_zero_totals() {
    for (a, b) in [("80", "40"), ("0", "0")] {
        ok(&format!(
            r#"root = DonutChart("Spend", [{{label: "A", value: {a}}}, {{label: "B", value: {b}}}], "USD")"#
        ));
    }
    let thirteen = (0..13)
        .map(|i| format!(r#"{{"label": "{i}", "value": 1}}"#))
        .collect::<Vec<_>>()
        .join(",");
    for points in [
        r#"[{label: "A", value: -1}]"#.to_string(),
        r#"[{label: "A", value: 1e308}, {label: "B", value: 1e308}]"#.to_string(),
        "[]".to_string(),
        format!("[{thirteen}]"),
    ] {
        err(&format!(r#"root = DonutChart("Spend", {points})"#));
    }
    rejects(
        r#"root = DonutChart("Spend", [{label: "A", value: 1e308}, {label: "B", value: 1e308}])"#,
        "Total must be finite",
    );
}

#[test]
fn accepts_a_valid_spec() {
    let tree = root(
        "root = Card([s1, t1], \"Status\")\ns1 = Stat(\"Downloads\", \"12k\")\nt1 = Table([\"Name\", \"Result\"], [[\"build\", \"ok\"],[\"lint\", \"ok\"]])",
    );
    assert_eq!(
        tree,
        Component::Card {
            children: vec![
                Component::Stat {
                    label: "Downloads".into(),
                    value: "12k".into(),
                    hint: None
                },
                Component::Table {
                    columns: vec!["Name".into(), "Result".into()],
                    rows: vec![
                        vec!["build".into(), "ok".into()],
                        vec!["lint".into(), "ok".into()]
                    ],
                },
            ],
            title: Some("Status".into()),
        }
    );
}

#[test]
fn rejects_empty_and_oversized_specs() {
    rejects("", "empty");
    rejects("   \n\t", "empty");
    rejects(&"x".repeat(MAX_SOURCE_CHARS + 1), "the limit is 32768");
}

#[test]
fn rejects_specs_with_no_resolvable_root() {
    let error = rejects("not a spec at all !!!", "expected `=`");
    assert_eq!(error.issues()[0].line, Some(1));
    rejects(r#"main = Text("hi")"#, "Define root");
}

#[test]
fn rejects_unknown_components_with_a_fixable_error() {
    let error = rejects(r#"root = Confetti("yay")"#, "Confetti");
    let text = error.to_string();
    assert!(text.contains("Card"), "{text}");
    assert!(
        !text.contains("FollowUps"),
        "legacy component is not advertised"
    );
}

#[test]
fn rejects_unresolved_references() {
    rejects("root = Card([missing])", "missing");
}

#[test]
fn rejects_query_and_mutation_statements() {
    rejects("q = Query(\"list_issues\")\nroot = Text(\"hi\")", "Query()");
    rejects(
        "m = Mutation(\"delete\")\nroot = Text(\"Hi\")",
        "Mutation()",
    );
}

#[test]
fn accepts_responsive_dashboards_with_both_chart_types() {
    let tree = root(
        r#"root = Card([Grid([Stat("Builds", "42"), Progress("Passing", 100)], 2), trend, comparison], "CI health")
trend = LineChart("Duration", [{label: "Mon", value: 12}, {label: "Tue", value: 9}], "s")
comparison = BarChart("Change", [{label: "Build", value: -3}, {label: "Test", value: 2}])"#,
    );
    let Component::Card { children, title } = tree else {
        panic!("expected a Card")
    };
    assert_eq!(title.as_deref(), Some("CI health"));
    assert_eq!(
        children[0],
        Component::Grid {
            children: vec![
                Component::Stat {
                    label: "Builds".into(),
                    value: "42".into(),
                    hint: None
                },
                Component::Progress {
                    label: "Passing".into(),
                    percent: 100.0
                },
            ],
            columns: Some(2),
        }
    );
    assert_eq!(
        children[2],
        Component::BarChart {
            title: "Change".into(),
            points: vec![
                ChartPoint {
                    label: "Build".into(),
                    value: -3.0
                },
                ChartPoint {
                    label: "Test".into(),
                    value: 2.0
                },
            ],
            unit: None,
        }
    );
}

#[test]
fn rejects_invalid_or_dynamic_input() {
    let seven = repeat(r#"{label: "a", prompt: "b"}"#, 7);
    for spec in [
        r#"root = Progress("Done", 101)"#.to_string(),
        r#"root = Progress("Done", -1)"#.to_string(),
        r#"root = Grid([Text("Hi")], 2.5)"#.to_string(),
        r#"root = Grid([Text("Hi")], 5)"#.to_string(),
        r#"root = Table(["A", "B"], [["one"]])"#.to_string(),
        "root = Table([], [])".to_string(),
        r#"root = BarChart("Empty", [])"#.to_string(),
        r#"root = LineChart("Invalid", [{label: "x", value: "bad"}])"#.to_string(),
        r#"root = Card(["not a component"])"#.to_string(),
        r#"root = Badge("State", "unknown")"#.to_string(),
        "root = Text(\"Hello\")\nroot = Text(\"Goodbye\")".to_string(),
        "$value = \"Hi\"\nroot = Text($value)".to_string(),
        r#"root = Text(If(true, "yes", "no"))"#.to_string(),
        "m = Mutation(\"delete\")\nroot = Text(\"Hi\")".to_string(),
        "root = Card([root])".to_string(),
        r#"root = Card([Text("incomplete")"#.to_string(),
        "root = FollowUps([])".to_string(),
        r#"root = FollowUps([{label: "", prompt: "Go"}])"#.to_string(),
        r#"root = FollowUps([{label: "Go"}])"#.to_string(),
        format!("root = FollowUps([{seven}])"),
    ] {
        err(&spec);
    }
}

#[test]
fn rejects_exponential_reference_expansion_before_materializing_it() {
    let mut parts = vec![r#"leaf = Text("hi")"#.to_string()];
    for index in 0..30 {
        let previous = if index == 0 {
            "leaf".to_string()
        } else {
            format!("node{}", index - 1)
        };
        parts.push(format!("node{index} = Card([{previous}, {previous}])"));
    }
    parts.push("root = Card([node29])".into());
    rejects(&parts.join("\n"), "too complex");
}

#[test]
fn rejects_wide_fan_out_without_deep_nesting() {
    // 2^12 copies of a leaf, but only a dozen reference hops deep.
    let mut parts = vec![r#"n0 = Text("hi")"#.to_string()];
    for index in 1..=12 {
        parts.push(format!("n{index} = Grid([n{p}, n{p}])", p = index - 1));
    }
    parts.push("root = Card([n12])".into());
    rejects(&parts.join("\n"), "expanding references visits");
}

#[test]
fn rejects_excessive_chart_and_container_data() {
    let points = (0..101)
        .map(|i| format!(r#"{{"label": "{i}", "value": {i}}}"#))
        .collect::<Vec<_>>()
        .join(",");
    rejects(
        &format!(r#"root = LineChart("Too many", [{points}])"#),
        "at most 100 items",
    );
    rejects(
        &format!("root = Card([{}])", repeat(r#"Text("x")"#, 65)),
        "at most 64 items",
    );
}

#[test]
fn accepts_follow_up_prompts_alongside_a_dashboard() {
    let tree = root(
        r#"root = Card([Stat("Failing", "2"), next], "Tests")
next = FollowUps([{label: "Fix failures", prompt: "Fix the two failing tests."}, {label: "Explain", prompt: "Explain why they fail."}])"#,
    );
    let Component::Card { children, .. } = tree else {
        panic!("expected a Card")
    };
    assert!(matches!(&children[1], Component::FollowUps { items } if items.len() == 2));
}

#[test]
fn accepts_flow_diagrams_and_rejects_ones_without_a_connection() {
    let tree = root(
        r#"root = Flow([{label: "Desktop app"}, {label: "Zuse", detail: "Orchestrates", highlight: true}, {label: "Cloud computers"}], "Conceptual")"#,
    );
    let node = |label: &str, detail: Option<&str>, highlight| FlowNode {
        label: label.into(),
        detail: detail.map(Into::into),
        highlight,
    };
    assert_eq!(
        tree,
        Component::Flow {
            nodes: vec![
                node("Desktop app", None, false),
                node("Zuse", Some("Orchestrates"), true),
                node("Cloud computers", None, false),
            ],
            caption: Some("Conceptual".into()),
        }
    );
    rejects(r#"root = Flow([{label: "Alone"}])"#, "at least 2 items");
}

#[test]
fn treats_tool_looking_strings_as_inert_text() {
    let text = "Query() Mutation() $state <script>alert(1)</script> # not a comment // nor this";
    assert_eq!(
        root(&format!(
            "root = Text({})",
            serde_json::to_string(text).unwrap()
        )),
        Component::Text { text: text.into() }
    );
}

#[test]
fn rejects_unused_components_instead_of_silently_losing_output() {
    let error = rejects(
        "unused = Text(\"Lost output\")\nroot = Text(\"Hi\")",
        "Unused statements",
    );
    assert!(error.to_string().contains("`unused` (line 1)"), "{error}");
}

// ─── Forms and layout ───────────────────────────────────────────────────────

#[test]
fn accepts_forms_with_every_field_type_and_nested_layout() {
    ok(
        r#"root = Form([Grid([env, region], 2), migrate, rollout, notes, Input("owner", "Owner")], "Deploy", "Deploy settings")
env = RadioGroup("env", "Environment", ["staging", "production"], "staging")
region = Select("region", "Region", ["us-east-1", "eu-west-1"])
migrate = Checkbox("migrate", "Run migrations", true)
rollout = Slider("rollout", "Rollout", 0, 100, 25, 5)
notes = TextArea("notes", "Notes")"#,
    );
    let tree = root(
        r#"root = Card([plan, risk, views])
plan = Steps([{label: "Build", status: "done"}, {label: "Ship", status: "pending"}])
risk = Callout("Locks the users table.", "warn", "Heads up")
views = Tabs([{label: "Before", content: Text("a")}, {label: "After", content: Text("b")}])"#,
    );
    let step = |label: &str, status| Step {
        label: label.into(),
        status,
        detail: None,
    };
    let tab = |label: &str, text: &str| Tab {
        label: label.into(),
        content: Component::Text { text: text.into() },
    };
    assert_eq!(
        tree,
        Component::Card {
            children: vec![
                Component::Steps {
                    items: vec![
                        step("Build", StepStatus::Done),
                        step("Ship", StepStatus::Pending)
                    ]
                },
                Component::Callout {
                    text: "Locks the users table.".into(),
                    tone: Some(Tone::Warn),
                    title: Some("Heads up".into())
                },
                Component::Tabs {
                    tabs: vec![tab("Before", "a"), tab("After", "b")]
                },
            ],
            title: None,
        }
    );
}

#[test]
fn rejects_invalid_forms_and_layouts_with_reasons() {
    for (spec, reason) in [
        (r#"root = Card([Input("a", "A")])"#, "inside a Form"),
        (
            r#"root = Form([Input("a", "A"), TextArea("a", "B")])"#,
            "Duplicate field",
        ),
        (r#"root = Form([Text("hi")])"#, "at least one field"),
        (
            r#"root = Form([Form([Input("a", "A")])])"#,
            "cannot be nested",
        ),
        (
            r#"root = Form([Select("s", "S", ["x"], "y")])"#,
            "one of the options",
        ),
        (
            r#"root = Form([RadioGroup("r", "R", ["only"])])"#,
            "options",
        ),
        (
            r#"root = Form([Slider("s", "S", 10, 0)])"#,
            "min must be below max",
        ),
        (
            r#"root = Form([Slider("s", "S", 0, 10, 20)])"#,
            "within min and max",
        ),
        (r#"root = Form([Input("bad name", "A")])"#, "name"),
        (
            r#"root = Tabs([{label: "A", content: "x"}, {label: "B", content: Text("b")}])"#,
            "must be a component",
        ),
        (
            r#"root = Tabs([{label: "Only", content: Text("a")}])"#,
            "tabs",
        ),
        (
            r#"root = Steps([{label: "x", status: "skipped"}])"#,
            "status",
        ),
    ] {
        rejects(spec, reason);
    }
}

#[test]
fn fields_inside_tabs_and_grids_of_a_form_share_one_scope() {
    ok(
        r#"root = Form([Grid([Input("a", "A")]), Tabs([{label: "One", content: Input("b", "B")}, {label: "Two", content: Card([Checkbox("c", "C")])}])])"#,
    );
    rejects(
        r#"root = Form([Grid([Input("a", "A")]), Tabs([{label: "One", content: Input("a", "B")}, {label: "Two", content: Text("x")}])])"#,
        "Duplicate field name: `a`",
    );
    rejects(
        r#"root = Tabs([{label: "One", content: Input("b", "B")}, {label: "Two", content: Text("x")}])"#,
        "must be inside a Form",
    );
}

#[test]
fn a_field_statement_referenced_twice_is_a_duplicate() {
    rejects(
        "root = Form([f, f])\nf = Input(\"a\", \"A\")",
        "Duplicate field name",
    );
}

// ─── Native render tree ─────────────────────────────────────────────────────

#[test]
fn resolves_references_using_the_shared_component_props() {
    let tree = root("metric = Stat(\"Count\", \"12\")\nroot = Card([metric], \"Overview\")");
    assert_eq!(
        tree,
        Component::Card {
            children: vec![Component::Stat {
                label: "Count".into(),
                value: "12".into(),
                hint: None
            }],
            title: Some("Overview".into()),
        }
    );
}

#[test]
fn references_may_chain_and_alias_data() {
    assert_eq!(
        root("root = alias\nalias = real\nreal = Text(\"hi\")"),
        Component::Text { text: "hi".into() }
    );
    let tree = root("root = Table(cols, rows)\ncols = [\"A\"]\nrows = [row, row]\nrow = [\"x\"]");
    assert!(matches!(tree, Component::Table { rows, .. } if rows.len() == 2));
}

#[test]
fn rejects_executable_and_malformed_content_before_returning_a_tree() {
    rejects(r#"root = eval("secret")"#, "Unknown component eval");
    rejects(r#"root = Slider("x", "X", 10, 1)"#, "inside a Form");
    rejects(
        r#"root = Form([Slider("x", "X", 10, 1)])"#,
        "min must be below max",
    );
}

#[test]
fn renderer_fixtures_parse() {
    for spec in [
        r#"root = DonutChart("Spend", [{label: "Model A", value: 80}, {label: "Model B", value: 20}], "USD")"#,
        r#"root = DonutChart("Spend", [{label: "No requests", value: 0}])"#,
        r#"root = Card([Grid([Stat("Tests", "428", "All passing"), Progress("Complete", 80)], 2), Table(["File", "Status"], [["app.ts", "passed"]]), Badge("Ready", "good")], "Build health")"#,
        r#"root = BarChart("Change", [{label: "Build", value: -3}, {label: "Tests", value: 2}, {label: "Lint", value: 0}], "s")"#,
        r#"root = LineChart("Latency", [{label: "Mon", value: 12}, {label: "Tue", value: 9}], "ms")"#,
        r#"root = Text("<img src=x onerror=alert(1)>")"#,
        r#"root = FollowUps([{label: "Run tests", prompt: "Run the full test suite."}])"#,
        "root = Card([plan, risk, views])\nplan = Steps([{label: \"Build\", status: \"done\"}, {label: \"Test\", status: \"active\", detail: \"3 of 5\"}, {label: \"Ship\", status: \"pending\"}])\nrisk = Callout(\"Migrations lock the users table.\", \"warn\", \"Heads up\")\nviews = Tabs([{label: \"Before\", content: Text(\"Old flow\")}, {label: \"After\", content: Text(\"New flow\")}])",
        r#"root = Flow([{label: "Desktop app"}, {label: "Zuse", detail: "Orchestrates", highlight: true}, {label: "Cloud computers"}], "Conceptual integration")"#,
    ] {
        ok(spec);
    }
    rejects(r#"root = Missing("legacy")"#, "Unknown component Missing");
}

// ─── Syntax ─────────────────────────────────────────────────────────────────

#[test]
fn accepts_literal_syntax_variants() {
    let tree = root(
        "// leading comment\n# another comment\nroot = Card([\n  Text(\"a\\n\\\"b\\\" \\u00e9 \\ud83d\\ude00\"), // trailing comment\n  Stat('Single', 'quoted'),\n  Progress(\"p\", 12.5),\n  Callout(\"c\", null, \"t\"),\n], \"Title\")\n\n",
    );
    assert_eq!(
        tree,
        Component::Card {
            children: vec![
                Component::Text {
                    text: "a\n\"b\" é 😀".into()
                },
                Component::Stat {
                    label: "Single".into(),
                    value: "quoted".into(),
                    hint: None
                },
                Component::Progress {
                    label: "p".into(),
                    percent: 12.5
                },
                Component::Callout {
                    text: "c".into(),
                    tone: None,
                    title: Some("t".into())
                },
            ],
            title: Some("Title".into()),
        }
    );
}

#[test]
fn accepts_fenced_specs_and_string_keys() {
    let tree = root("```\nroot = KeyValue([{\"key\": \"a\", \"value\": \"b\"}])\n```");
    assert_eq!(
        tree,
        Component::KeyValue {
            pairs: vec![KeyValuePair {
                key: "a".into(),
                value: "b".into()
            }]
        }
    );
}

#[test]
fn negative_decimal_and_exponent_numbers() {
    let tree = root(
        r#"root = BarChart("x", [{label: "a", value: -1.5}, {label: "b", value: 2e3}, {label: "c", value: -0.25}])"#,
    );
    let Component::BarChart { points, .. } = tree else {
        panic!("expected a BarChart")
    };
    let values: Vec<f64> = points.iter().map(|point| point.value).collect();
    assert_eq!(values, vec![-1.5, 2000.0, -0.25]);
}

#[test]
fn null_skips_optional_arguments_but_not_required_ones() {
    assert_eq!(
        root(r#"root = Card([Text("a")], null)"#),
        Component::Card {
            children: vec![Component::Text { text: "a".into() }],
            title: None
        }
    );
    assert_eq!(
        root(r#"root = Card(null, "Only a title")"#),
        Component::Card {
            children: vec![],
            title: Some("Only a title".into())
        }
    );
    ok(r#"root = Form([Slider("s", "S", 0, 10, null, 2)])"#);
    ok(r#"root = Flow([{label: "a", detail: null}, {label: "b", highlight: null}])"#);
    rejects(r#"root = Stat("a", null)"#, "`value` cannot be null");
    rejects(r#"root = Stat("a")"#, "`value` is missing");
    rejects(
        r#"root = Steps([{label: "a", status: null}])"#,
        "missing required key `status`",
    );
}

#[test]
fn rejects_excess_positional_arguments_with_the_signature() {
    rejects(
        r#"root = Text("a", "b")"#,
        "Text takes 1 argument but got 2. Arguments are positional — signature: Text(text: string)",
    );
}

#[test]
fn rejects_reactive_state_operators_and_expressions() {
    for (spec, needle) in [
        (
            "$count = 1\nroot = Text(\"a\")",
            "`$count` is reactive state",
        ),
        ("root = Text($name)", "`$name` is reactive state"),
        (r#"root = Text("a" + "b")"#, "operators like `+`"),
        (r#"root = Progress("x", 1 * 2)"#, "operators like `*`"),
        (r#"root = Progress("x", -total)"#, "operators like `-`"),
        (
            "root = Text(data.title)\ndata = Text(\"x\")",
            "member access",
        ),
        (
            r#"root = Text(true ? "a" : "b")"#,
            "conditional expressions",
        ),
        (
            r#"root = Text(@Each(items, "x"))"#,
            "`@Each` is a runtime builtin",
        ),
        (
            r#"root = Card([Count(items)])"#,
            "Count() is a runtime builtin",
        ),
        (r#"root = Card([Action("go")])"#, "Action() is an action"),
        (r#"root = Text(("grouped"))"#, "grouping with parentheses"),
        (r#"root = Text("a") == x"#, "operators like `==`"),
        (r#"root = Text("a");"#, "unexpected character `;`"),
    ] {
        rejects(spec, needle);
    }
}

#[test]
fn rejects_truncated_input_with_the_opening_line() {
    let error = rejects(
        "root = Card([\n  Text(\"a\"),\n  Text(\"b\"",
        "looks truncated",
    );
    assert_eq!(error.issues()[0].line, Some(3));
    let error = rejects("root = Text(\"never closed)", "string is never closed");
    assert_eq!(error.issues()[0].line, Some(1));
    rejects("root = KeyValue([{key: ", "looks truncated");
    rejects("root =", "has no value");
}

#[test]
fn reports_mismatched_brackets_and_bad_keys() {
    rejects(
        r#"root = Card([Text("a")))"#,
        "expected `,` or `]` but found `)`",
    );
    rejects(
        r#"root = Card([Text("a") Text("b")])"#,
        "expected `,` or `]`",
    );
    rejects(
        r#"root = KeyValue([{key "a"}])"#,
        "expected `:` after object key `key`",
    );
    rejects(r#"root = KeyValue([{1: "a"}])"#, "expected an object key");
    rejects(
        r#"root = KeyValue([{key: "a", key: "b", value: "c"}])"#,
        "object key `key` appears twice",
    );
    rejects(r#"root = Text("bad \q escape")"#, "invalid escape `\\q`");
}

#[test]
fn rejects_duplicate_names_cycles_and_unknown_identifiers_with_lines() {
    let error = rejects(
        "root = Card([a])\na = Text(\"x\")\na = Text(\"y\")",
        "already defined on line 2",
    );
    assert_eq!(error.issues()[0].line, Some(3));

    let error = rejects(
        "root = Card([a])\na = Card([b])\nb = Card([a])",
        "Cyclic reference: a → b → a",
    );
    assert_eq!(error.issues()[0].line, Some(3));
    rejects("root = Card([root])", "Cyclic reference: root → root");

    let error = rejects(
        "root = Card([\n  Text(\"a\"),\n  ghost\n])",
        "`ghost` is not defined",
    );
    assert_eq!(error.issues()[0].line, Some(3));
}

#[test]
fn aggregates_independent_problems() {
    let error = err(
        "root = Card([a, b, c])\na = Progress(\"x\", 101)\nb = Badge(\"y\", \"loud\")\nc = Table([\"A\"], [[\"1\", \"2\"]])",
    );
    let lines: Vec<_> = error.issues().iter().map(|issue| issue.line).collect();
    assert_eq!(lines, vec![Some(2), Some(3), Some(4)], "{error}");
    let text = error.to_string();
    assert!(text.starts_with("UI spec has 3 problems:"), "{text}");
    assert!(
        text.contains("line 2: Progress percent: must be between 0 and 100 (got 101)."),
        "{text}"
    );
    assert!(
        text.contains(
            r#"line 3: Badge tone: expected one of "neutral", "good", "warn", "bad", got string "loud"."#
        ),
        "{text}"
    );
    assert!(
        text.contains("rows[0] has 2 cells but there are 1 column"),
        "{text}"
    );

    let error = err("root = Card([a b])\nx = = 1\nok = Text(\"x\")\ny Text(\"z\")");
    let lines: Vec<_> = error.issues().iter().map(|issue| issue.line).collect();
    assert_eq!(lines, vec![Some(1), Some(2), Some(4)], "{error}");
}

#[test]
fn type_mismatches_name_the_path() {
    rejects(
        r#"root = Stat("Downloads", 12)"#,
        "Stat value: expected string, got number 12.",
    );
    rejects(
        r#"root = Steps([{label: "a", status: "done", detail: 3}])"#,
        "Steps items[0].detail: expected string, got number 3.",
    );
    rejects(
        r#"root = Table(["A"], [["x"], "y"])"#,
        "Table rows[1]: expected an array (string[]), got string \"y\".",
    );
    rejects(
        r#"root = Card([Text("a")], Text("b"))"#,
        "Card title: expected string, got component Text.",
    );
    rejects(
        r#"root = Card([null])"#,
        "must be a component like Text(\"…\"), got null",
    );
    rejects(r#"root = "just text""#, "root must be a component");
}

// ─── Serde ──────────────────────────────────────────────────────────────────

#[test]
fn typed_tree_round_trips_through_json_tagged_by_component_name() {
    let spec = ok(
        r#"root = Form([Grid([Input("who", "Who")], 2), Checkbox("ok", "OK?", true), Slider("n", "N", 0, 10, 5)], "Send", "Ask")"#,
    );
    let json = serde_json::to_value(&spec).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "root": {
                "component": "Form",
                "children": [
                    {"component": "Grid", "children": [{"component": "Input", "name": "who", "label": "Who"}], "columns": 2},
                    {"component": "Checkbox", "name": "ok", "label": "OK?", "checked": true},
                    {"component": "Slider", "name": "n", "label": "N", "min": 0.0, "max": 10.0, "value": 5.0},
                ],
                "submitLabel": "Send",
                "title": "Ask",
            },
            "source": spec.source,
        })
    );
    let back: Spec = serde_json::from_value(json).unwrap();
    assert_eq!(back, spec);
}

#[test]
fn parse_error_is_a_std_error() {
    let error: Box<dyn std::error::Error> = Box::new(parse("root = Nope()").unwrap_err());
    assert!(error.to_string().contains("Unknown component Nope"));
}
