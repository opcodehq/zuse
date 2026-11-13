//! A managed executable source binds each new CLI process to the path it
//! returns at that moment: a cloud computer that adopts a new harness bundle
//! changes what the next process runs without touching running ones.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use zuse_providers::{AcpHarness, ClaudeHarness, CodexHarness, ExecutableSource, Harness};

fn source(current: &Arc<Mutex<Option<PathBuf>>>) -> ExecutableSource {
    let current = current.clone();
    Arc::new(move || current.lock().unwrap().clone())
}

#[test]
fn every_managed_agent_resolves_through_the_current_binding() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("v1/cli");
    let second = dir.path().join("v2/cli");
    for path in [&first, &second] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "#!/bin/sh\n").unwrap();
    }
    let current = Arc::new(Mutex::new(Some(first.clone())));
    let harnesses: Vec<Box<dyn Harness>> = vec![
        Box::new(ClaudeHarness::new().with_executable_source(source(&current))),
        Box::new(CodexHarness::new().with_executable_source(source(&current))),
        Box::new(AcpHarness::grok().with_executable_source(source(&current))),
    ];
    for harness in &harnesses {
        assert_eq!(harness.executable_path(), Some(first.clone()));
        assert!(harness.installed());
    }
    *current.lock().unwrap() = Some(second.clone());
    for harness in &harnesses {
        assert_eq!(harness.executable_path(), Some(second.clone()));
    }
    // A binding whose files vanished is never launched.
    *current.lock().unwrap() = Some(dir.path().join("missing/cli"));
    for harness in &harnesses {
        assert_ne!(
            harness.executable_path(),
            Some(dir.path().join("missing/cli"))
        );
    }
}
