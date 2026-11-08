#![allow(dead_code)]

use zuse_ui_block::{Component, Spec, SpecError, parse};

#[track_caller]
pub fn ok(spec: &str) -> Spec {
    match parse(spec) {
        Ok(parsed) => parsed,
        Err(error) => panic!("expected spec to parse:\n{spec}\n--- error ---\n{error}"),
    }
}

#[track_caller]
pub fn root(spec: &str) -> Component {
    ok(spec).root
}

#[track_caller]
pub fn err(spec: &str) -> SpecError {
    match parse(spec) {
        Ok(parsed) => panic!("expected spec to be rejected:\n{spec}\n--- tree ---\n{parsed:?}"),
        Err(error) => error,
    }
}

/// Rejected, and the error mentions `needle`.
#[track_caller]
pub fn rejects(spec: &str, needle: &str) -> SpecError {
    let error = err(spec);
    let text = error.to_string();
    assert!(
        text.contains(needle),
        "error for\n{spec}\nshould mention {needle:?} but was:\n{text}"
    );
    error
}

pub fn repeat(item: &str, count: usize) -> String {
    vec![item; count].join(", ")
}

pub fn text_of(len: usize) -> String {
    "a".repeat(len)
}
