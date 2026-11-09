//! Validation errors. Every problem carries the 1-based source line it was
//! found on (when it has one) and a message that says what to change, so an
//! agent can fix the spec and retry without guessing.

use std::fmt;

/// At most this many problems are listed; the rest are summarised as a count.
const MAX_LISTED: usize = 20;

/// One problem found in a spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// 1-based line in the submitted source, when the problem has a location.
    pub line: Option<usize>,
    pub message: String,
}

impl Issue {
    pub(crate) fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line: Some(line),
            message: message.into(),
        }
    }

    pub(crate) fn general(message: impl Into<String>) -> Self {
        Self {
            line: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// A spec was rejected. Holds every problem found in the failing phase
/// (syntax, structure, or component validation), deduplicated and in order.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", render(.issues))]
pub struct SpecError {
    issues: Vec<Issue>,
}

impl SpecError {
    pub(crate) fn new(mut issues: Vec<Issue>) -> Self {
        let mut seen = Vec::with_capacity(issues.len());
        issues.retain(|issue| {
            if seen.contains(issue) {
                false
            } else {
                seen.push(issue.clone());
                true
            }
        });
        debug_assert!(!issues.is_empty());
        Self { issues }
    }

    pub(crate) fn single(issue: Issue) -> Self {
        Self {
            issues: vec![issue],
        }
    }

    /// Every problem found, in source order within each phase.
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }
}

fn render(issues: &[Issue]) -> String {
    if let [only] = issues {
        return only.to_string();
    }
    let mut out = format!("UI spec has {} problems:", issues.len());
    for issue in issues.iter().take(MAX_LISTED) {
        out.push_str("\n- ");
        out.push_str(&issue.to_string());
    }
    if issues.len() > MAX_LISTED {
        out.push_str(&format!(
            "\n- …and {} more; fix the ones above and retry.",
            issues.len() - MAX_LISTED
        ));
    }
    out
}
