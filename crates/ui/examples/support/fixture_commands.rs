/// Capture consumes the rest of the command line as a single path.
pub fn capture_path(line: &str) -> Option<&str> {
    let (command, path) = line.trim().split_once(char::is_whitespace)?;
    (command == "capture").then(|| path.trim())
}

#[cfg(test)]
mod tests {
    use super::capture_path;

    #[test]
    fn capture_preserves_spaces_and_unicode_in_the_path() {
        assert_eq!(
            capture_path("capture /tmp/review shots/sidekick.png"),
            Some("/tmp/review shots/sidekick.png")
        );
        assert_eq!(
            capture_path("  capture\t/tmp/review shots/图.png  "),
            Some("/tmp/review shots/图.png")
        );
    }

    #[test]
    fn other_commands_do_not_enter_the_capture_handler() {
        assert_eq!(capture_path("click 10 20"), None);
        assert_eq!(capture_path("capture-other /tmp/shot.png"), None);
    }
}
