//! Form helpers shared by renderers: field discovery, default values, and
//! the plain-text answer a submitted Form sends as the user's next message.

use serde::{Deserialize, Serialize};

use crate::model::Component;

/// A field's current value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldValue {
    /// Input, TextArea, Select and RadioGroup (empty = nothing chosen).
    Text(String),
    /// Checkbox.
    Bool(bool),
    /// Slider.
    Number(f64),
}

/// One field inside a Form, in render order.
#[derive(Debug, Clone, PartialEq)]
pub struct FormField<'a> {
    pub name: &'a str,
    pub label: &'a str,
    pub component: &'a Component,
    /// The value the field starts with.
    pub default: FieldValue,
}

/// Starting value of a field component: `checked` for checkboxes, `value`
/// (else `min`) for sliders, `value` (else empty) for text and choices.
/// `None` for non-field components.
pub fn default_value(field: &Component) -> Option<FieldValue> {
    Some(match field {
        Component::Checkbox { checked, .. } => FieldValue::Bool(*checked),
        Component::Slider { min, value, .. } => FieldValue::Number(value.unwrap_or(*min)),
        Component::Input { value, .. }
        | Component::TextArea { value, .. }
        | Component::Select { value, .. }
        | Component::RadioGroup { value, .. } => {
            FieldValue::Text(value.clone().unwrap_or_default())
        }
        _ => return None,
    })
}

/// Every field inside `form` (through Cards, Grids and Tabs), in order.
pub fn form_fields(form: &Component) -> Vec<FormField<'_>> {
    let mut fields = Vec::new();
    match form {
        Component::Form { children, .. } => {
            for child in children {
                collect(child, &mut fields);
            }
        }
        other => collect(other, &mut fields),
    }
    fields
}

fn collect<'a>(node: &'a Component, out: &mut Vec<FormField<'a>>) {
    if let (Some((name, label)), Some(default)) = (node.field_info(), default_value(node)) {
        out.push(FormField {
            name,
            label,
            component: node,
            default,
        });
        return;
    }
    for child in node.children() {
        collect(child, out);
    }
}

/// `(field name, starting value)` for every field in `form`.
pub fn default_field_values(form: &Component) -> Vec<(String, FieldValue)> {
    form_fields(form)
        .into_iter()
        .map(|field| (field.name.to_string(), field.default))
        .collect()
}

/// The message a submitted Form sends: the Form title (else its submit
/// label) on the first line, then `- Label: value` per field. Labels already
/// ending in `?` or `:` keep their punctuation; booleans read Yes/No; blank
/// text reads "—". Fields missing from `values` use their defaults.
pub fn form_submission_text(form: &Component, values: &[(String, FieldValue)]) -> String {
    let heading = match form {
        Component::Form {
            title,
            submit_label,
            ..
        } => title.as_deref().or(submit_label.as_deref()),
        _ => None,
    };
    let mut lines: Vec<String> = Vec::new();
    if let Some(heading) = heading.filter(|heading| !heading.is_empty()) {
        lines.push(heading.to_string());
    }
    for field in form_fields(form) {
        let value = values
            .iter()
            .rev()
            .find(|(name, _)| name == field.name)
            .map_or(&field.default, |(_, value)| value);
        let label = if field.label.ends_with('?') || field.label.ends_with(':') {
            field.label.to_string()
        } else {
            format!("{}:", field.label)
        };
        lines.push(format!("- {label} {}", format_value(value)));
    }
    lines.join("\n")
}

fn format_value(value: &FieldValue) -> String {
    let text = match value {
        FieldValue::Bool(true) => return "Yes".into(),
        FieldValue::Bool(false) => return "No".into(),
        FieldValue::Number(number) => crate::js_number(*number),
        FieldValue::Text(text) => text.trim().to_string(),
    };
    if text.is_empty() { "—".into() } else { text }
}
