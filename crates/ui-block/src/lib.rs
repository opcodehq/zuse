//! UI block specs: small, display-only component programs agents send to be
//! rendered inline in chat.
//!
//! A spec is one statement per line, `name = Expr`, where an expression is a
//! positional component call `Stat("Builds", "42")`, a string (JSON escapes),
//! a number, `true`/`false`/`null`, an `[array]`, a `{key: value}` object, or
//! a bare reference to another statement. `root` is required and every other
//! statement must be reachable from it.
//!
//! [`parse`] tokenizes, normalizes common formatting variants, resolves
//! references (rejecting duplicates, unknown names, cycles and unused
//! statements), bounds reference expansion before materializing anything,
//! and validates every component against the catalogue into a typed
//! [`Component`] tree. Errors carry line numbers and say what to change.
//!
//! The catalogue that validates specs also generates the prompt lines
//! ([`component_lines`]), so the two can never drift apart.

mod build;
mod catalogue;
mod error;
mod form;
mod lexer;
mod model;
mod prompt;
mod resolve;
mod syntax;

use serde::{Deserialize, Serialize};

pub use catalogue::component_lines;
pub use error::{Issue, SpecError};
pub use form::{
    FieldValue, FormField, default_field_values, default_value, form_fields, form_submission_text,
};
pub use model::{
    ChartPoint, Component, FlowNode, FollowUp, KeyValuePair, Step, StepStatus, Tab, Tone,
};
pub use prompt::{EXAMPLES, RESPONSE_INSTRUCTIONS, tool_description};

/// Persisted spec content version; bump when the payload shape changes.
pub const SPEC_VERSION: u32 = 1;

/// Input cap applied before parsing, in UTF-16 code units (the unit the
/// reference implementation measures in).
pub const MAX_SOURCE_CHARS: usize = 32 * 1024;

/// Maximum nodes visited when every reference is expanded inline.
pub const MAX_EXPANDED_NODES: usize = resolve::MAX_VISITED as usize;

/// Maximum nesting depth of the expanded tree.
pub const MAX_EXPANDED_DEPTH: usize = syntax::MAX_DEPTH;

/// A validated spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spec {
    pub root: Component,
    /// Source to persist: the submitted text unchanged, or its canonical
    /// re-print when normalization rewrote a formatting variant (for
    /// example `CardHeader("x")` → `Text("x")`). Parsing it again yields the
    /// same tree and the same source.
    pub source: String,
}

/// Parse and fully validate a spec.
pub fn parse(source: &str) -> Result<Spec, SpecError> {
    if source.trim().is_empty() {
        return Err(SpecError::single(Issue::general(
            "The spec is empty. Start with `root = Component(...)`.",
        )));
    }
    check_length(source)?;
    let tokens = lexer::tokenize(source).map_err(SpecError::new)?;
    let mut statements = syntax::parse_statements(&tokens).map_err(SpecError::new)?;
    let normalized = resolve::normalize(&mut statements);
    let canonical = normalized.then(|| resolve::print(&statements));
    // The canonical re-print is what gets stored and parsed again, and it can
    // run longer than the submitted text (expanded pairs, escapes).
    if let Some(canonical) = &canonical {
        check_length(canonical)?;
    }
    let program = resolve::resolve(statements).map_err(SpecError::new)?;
    let root = build::build(&program).map_err(SpecError::new)?;
    Ok(Spec {
        root,
        source: canonical.unwrap_or_else(|| source.to_string()),
    })
}

fn check_length(source: &str) -> Result<(), SpecError> {
    let length = source.encode_utf16().count();
    if length > MAX_SOURCE_CHARS {
        return Err(SpecError::single(Issue::general(format!(
            "The spec is {length} characters; the limit is {MAX_SOURCE_CHARS}. Show less data or split it into several blocks."
        ))));
    }
    Ok(())
}

/// Validate a spec without keeping the tree.
pub fn validate(source: &str) -> Result<(), SpecError> {
    parse(source).map(|_| ())
}

/// Format a number the way JavaScript's `String(number)` does, so canonical
/// sources and form answers match the reference byte for byte.
pub(crate) fn js_number(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    if !value.is_finite() {
        return if value.is_nan() {
            "NaN".into()
        } else if value > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    let magnitude = value.abs();
    if (1e-6..1e21).contains(&magnitude) {
        return format!("{value}");
    }
    let formatted = format!("{value:e}");
    match formatted.split_once('e') {
        Some((mantissa, exponent)) if !exponent.starts_with('-') => {
            format!("{mantissa}e+{exponent}")
        }
        _ => formatted,
    }
}

#[cfg(test)]
mod tests {
    use super::js_number;

    #[test]
    fn numbers_print_like_javascript() {
        for (value, expected) in [
            (0.0, "0"),
            (-0.0, "0"),
            (25.0, "25"),
            (-3.0, "-3"),
            (2.5, "2.5"),
            (0.1, "0.1"),
            (1e21, "1e+21"),
            (1.5e300, "1.5e+300"),
            (1e-7, "1e-7"),
            (123456789012.0, "123456789012"),
        ] {
            assert_eq!(js_number(value), expected, "{value}");
        }
    }
}
