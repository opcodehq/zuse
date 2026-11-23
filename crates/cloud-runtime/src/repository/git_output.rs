//! What git printed when a repository step failed, safe to keep and show.
//!
//! A failed fetch used to leave only `runtime_repository_fetch_failed`. The
//! failure now carries a short tail of git's stderr with anything that can
//! carry a credential removed: URL user info, authorization headers, GitHub
//! token shapes and other long opaque tokens. Commit and object ids stay.

/// Most of git's error is in its last lines; keep at most this much of it.
pub const MAX_TAIL_BYTES: usize = 2048;
/// Bytes read from a git child's stderr; the rest is discarded as it arrives.
pub const MAX_CAPTURE_BYTES: usize = 64 * 1024;

/// A repository step failed. Displays as its fixed code, so existing callers
/// that match the code are unchanged; the stderr tail travels alongside.
#[derive(Debug)]
pub struct GitFailure {
    pub code: &'static str,
    pub stderr: String,
}
impl std::fmt::Display for GitFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code)
    }
}
impl std::error::Error for GitFailure {}

impl GitFailure {
    pub fn new(code: &'static str, stderr: &[u8], secrets: &[&str]) -> Self {
        Self {
            code,
            stderr: scrub(stderr, secrets),
        }
    }
}

/// The scrubbed stderr tail of the first repository failure in `error`'s chain.
pub fn failure_stderr(error: &anyhow::Error) -> Option<&str> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<GitFailure>())
        .map(|failure| failure.stderr.as_str())
        .filter(|stderr| !stderr.is_empty())
}

/// Credential-free, printable, bounded tail of git output.
pub fn scrub(raw: &[u8], secrets: &[&str]) -> String {
    let mut text = String::from_utf8_lossy(raw).into_owned();
    for secret in secrets.iter().filter(|s| s.len() >= 4) {
        text = text.replace(secret, "[redacted]");
    }
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            let line: String = line
                .chars()
                .map(|c| if c == '\t' { ' ' } else { c })
                .filter(|c| !c.is_control())
                .collect();
            redact_line(&line)
        })
        .filter(|line| !line.trim().is_empty())
        .collect();
    let joined = lines.join("\n");
    if joined.len() <= MAX_TAIL_BYTES {
        return joined;
    }
    let mut start = joined.len() - MAX_TAIL_BYTES;
    while !joined.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &joined[start..])
}

fn redact_line(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    // Everything after the earliest header- or assignment-style marker goes;
    // the kept prefix is scrubbed word by word like any other line.
    let earliest = [
        "authorization:",
        "proxy-authorization:",
        "bearer ",
        "token=",
    ]
    .iter()
    .filter_map(|marker| lower.find(marker).map(|at| (at, marker.len())))
    .min();
    match earliest {
        Some((at, len)) => format!(
            "{}{} [redacted]",
            redact_words(&line[..at]),
            line[at..at + len].trim_end()
        ),
        None => redact_words(line),
    }
}

fn redact_words(text: &str) -> String {
    text.split(' ')
        .map(redact_word)
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_word(word: &str) -> String {
    // https://user:secret@host/... keeps the scheme and host only.
    if let Some(scheme) = word.find("://") {
        let rest = &word[scheme + 3..];
        let end = rest.find('/').unwrap_or(rest.len());
        if let Some(at) = rest[..end].rfind('@') {
            return format!(
                "{}[redacted]@{}",
                &word[..scheme + 3],
                redact_word(&rest[at + 1..])
            );
        }
        return word.to_owned();
    }
    let core = word.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    let lower = core.to_ascii_lowercase();
    let token_prefix = [
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "glpat-",
        "sk-",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix) && core.len() > prefix.len() + 8);
    let opaque = core.len() >= 32
        && core
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        && !core.bytes().all(|b| b.is_ascii_hexdigit())
        && core.bytes().any(|b| b.is_ascii_digit())
        && core.bytes().any(|b| b.is_ascii_alphabetic());
    if token_prefix || opaque {
        word.replace(core, "[redacted]")
    } else {
        word.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_cause_and_removes_anything_credential_shaped() {
        let raw = b"remote: Repository not found.\n\
fatal: unable to access 'https://x-access-token:ghs_abcdefghijklmnopqrstuvwxyz0123@github.com/o/r.git/': The requested URL returned error: 403\n\
Authorization: Basic eC1hY2Nlc3MtdG9rZW46c2VjcmV0\n\
error: object 1f2e3d4c5b6a79881f2e3d4c5b6a79881f2e3d4c is missing\n\
using github_pat_11ABCDEFG0123456789_abcdefghijklmnop and runtime-credential-value\n\
\x1b[31mcolour\x07\n";
        let text = scrub(raw, &["runtime-credential-value"]);
        assert!(text.contains("remote: Repository not found."));
        assert!(text.contains("https://[redacted]@github.com/o/r.git/"));
        assert!(text.contains("returned error: 403"));
        assert!(text.contains("Authorization: [redacted]"));
        // Object ids are what diagnoses a broken checkout: never redacted.
        assert!(text.contains("1f2e3d4c5b6a79881f2e3d4c5b6a79881f2e3d4c"));
        for secret in [
            "ghs_abcdefghijklmnopqrstuvwxyz0123",
            "eC1hY2Nlc3MtdG9rZW46c2VjcmV0",
            "github_pat_11ABCDEFG0123456789_abcdefghijklmnop",
            "runtime-credential-value",
            "x-access-token",
            "\x1b",
            "\x07",
        ] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
    }

    #[test]
    fn a_credential_before_a_header_marker_is_still_redacted() {
        let text = scrub(
            b"fatal: https://x-access-token:ghs_abcdefghijklmnopqrstuvwxyz0123@github.com/o/r.git Bearer abc Authorization: Basic xyz\n",
            &[],
        );
        assert_eq!(
            text,
            "fatal: https://[redacted]@github.com/o/r.git Bearer [redacted]"
        );
    }

    #[test]
    fn keeps_only_a_bounded_tail() {
        let mut raw = Vec::new();
        for i in 0..2000 {
            raw.extend_from_slice(format!("line {i} of git progress\n").as_bytes());
        }
        raw.extend_from_slice("fatal: the real cause é\n".as_bytes());
        let text = scrub(&raw, &[]);
        assert!(text.len() <= MAX_TAIL_BYTES + "…".len());
        assert!(text.ends_with("fatal: the real cause é"));
        assert!(text.starts_with('…'));
    }

    #[test]
    fn the_failure_displays_its_code_and_the_tail_is_found_through_context() {
        let failure = GitFailure::new("runtime_repository_fetch_failed", b"fatal: boom\n", &[]);
        let error = anyhow::Error::new(failure).context("repository_preparation_failed");
        assert_eq!(failure_stderr(&error), Some("fatal: boom"));
        assert_eq!(
            anyhow::Error::new(GitFailure::new("runtime_repository_fetch_failed", b"", &[]))
                .to_string(),
            "runtime_repository_fetch_failed"
        );
        assert_eq!(failure_stderr(&anyhow::anyhow!("other")), None);
    }
}
