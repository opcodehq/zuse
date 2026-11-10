//! Form defaults and the submitted answer text.

use crate::common::root;
use zuse_ui_block::{
    Component, FieldValue, default_field_values, default_value, form_fields, form_submission_text,
};

const DEPLOY: &str = r#"root = Form([env, region, migrate, rollout, notes], "Deploy", "Deploy settings")
env = RadioGroup("env", "Environment", ["staging", "production"], "staging")
region = Select("region", "Region", ["us-east-1", "eu-west-1"], "eu-west-1")
migrate = Checkbox("migrate", "Run migrations", true)
rollout = Slider("rollout", "Rollout", 0, 100, 25, 5)
notes = TextArea("notes", "Notes", "Optional")"#;

fn text(value: &str) -> FieldValue {
    FieldValue::Text(value.into())
}

#[test]
fn defaults_come_from_each_field() {
    let form = root(DEPLOY);
    assert_eq!(
        default_field_values(&form),
        vec![
            ("env".to_string(), text("staging")),
            ("region".to_string(), text("eu-west-1")),
            ("migrate".to_string(), FieldValue::Bool(true)),
            ("rollout".to_string(), FieldValue::Number(25.0)),
            ("notes".to_string(), text("")),
        ]
    );
    let slider = root(r#"root = Form([Slider("s", "S", -3, 3)])"#);
    assert_eq!(
        default_field_values(&slider),
        vec![("s".to_string(), FieldValue::Number(-3.0))]
    );
    assert_eq!(default_value(&Component::Text { text: "x".into() }), None);
}

#[test]
fn submission_text_with_defaults() {
    let form = root(DEPLOY);
    assert_eq!(
        form_submission_text(&form, &[]),
        "Deploy settings\n- Environment: staging\n- Region: eu-west-1\n- Run migrations: Yes\n- Rollout: 25\n- Notes: —"
    );
}

#[test]
fn submission_text_with_answers() {
    let form = root(DEPLOY);
    let values = vec![
        ("env".to_string(), text("production")),
        ("migrate".to_string(), FieldValue::Bool(false)),
        ("rollout".to_string(), FieldValue::Number(30.5)),
        ("notes".to_string(), text("  ship it\n")),
        ("unknown".to_string(), text("ignored")),
    ];
    assert_eq!(
        form_submission_text(&form, &values),
        "Deploy settings\n- Environment: production\n- Region: eu-west-1\n- Run migrations: No\n- Rollout: 30.5\n- Notes: ship it"
    );
}

#[test]
fn labels_keep_their_punctuation_and_headings_fall_back() {
    let form = root(
        r#"root = Form([Checkbox("ready", "Ready?"), Input("why", "Reason:"), Input("who", "Owner")], "Send")"#,
    );
    assert_eq!(
        form_submission_text(&form, &[("who".into(), text("   "))]),
        "Send\n- Ready? No\n- Reason: —\n- Owner: —"
    );
    let untitled = root(r#"root = Form([Input("a", "A", null, "x")])"#);
    assert_eq!(form_submission_text(&untitled, &[]), "- A: x");
    let empty_title = root(r#"root = Form([Input("a", "A")], "Send", "")"#);
    assert_eq!(form_submission_text(&empty_title, &[]), "- A: —");
}

#[test]
fn fields_are_collected_through_layout_in_order() {
    let form = root(
        r#"root = Form([Text("Intro"), Grid([Input("a", "A"), Card([Checkbox("b", "B")])]), Tabs([{label: "One", content: Select("c", "C", ["x"])}, {label: "Two", content: Slider("d", "D", 1, 2)}])])"#,
    );
    let names: Vec<&str> = form_fields(&form).iter().map(|field| field.name).collect();
    assert_eq!(names, vec!["a", "b", "c", "d"]);
    assert_eq!(
        form_submission_text(&form, &[("c".into(), text("x"))]),
        "- A: —\n- B: No\n- C: x\n- D: 1"
    );
}

#[test]
fn field_values_serialize_untagged() {
    let values = vec![text("a"), FieldValue::Bool(true), FieldValue::Number(2.5)];
    let json = serde_json::to_string(&values).unwrap();
    assert_eq!(json, r#"["a",true,2.5]"#);
    let back: Vec<FieldValue> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, values);
}
