use super::redaction::*;
use crate::*;
use std::sync::Arc;
#[test]
fn secrets_filter_split_rotated_escaped_and_operational_input_boundaries() {
    let secrets = Arc::new(Secrets::default());
    secrets.record("old-token").unwrap();
    secrets.record("new\n\"token").unwrap();
    secrets.record("x").unwrap();
    let mut text = Text::new(secrets.clone());
    let mut bytes = text.push(b"before old-");
    bytes.extend(text.push(b"token after"));
    bytes.extend(text.finish());
    assert_eq!(String::from_utf8(bytes).unwrap(), "before [redacted] after");
    assert!(!secrets.mask("new\\n\\\"token").contains("token"));
    let mut events = Events::new(secrets.clone());
    let mut out = events.push(AgentEvent::TextDelta {
        text: "old-".into(),
    });
    out.extend(events.push(AgentEvent::TextDelta {
        text: "token".into(),
    }));
    out.extend(events.finish());
    let rendered = serde_json::to_string(&out).unwrap();
    assert!(!rendered.contains("old-token"));
    assert!(rendered.contains("[redacted]"));
    let mut questions = vec![UserInputQuestion {
        id: "question".into(),
        multi_select: false,
        multiline: false,
        header: "old-token".into(),
        question: "safe".into(),
        options: vec!["answer".into()],
        prefill: None,
    }];
    assert!(secrets.questions(&mut questions));
    assert_eq!(questions[0].options[0], "answer");
    questions[0].options.push("old-token".into());
    assert!(!secrets.questions(&mut questions));
    assert_eq!(questions[0].options[1], "old-token");
}

#[test]
fn secret_filter_preserves_retry_and_catalog_error_classification() {
    let secrets = Arc::new(Secrets::default());
    secrets.record("credential").unwrap();
    let error = secrets.error(HarnessError::Io(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "credential",
    )));
    assert!(matches!(&error,HarnessError::Io(error) if error.kind()==std::io::ErrorKind::TimedOut));
    assert!(!error.to_string().contains("credential"));
    let error = secrets.error(HarnessError::Discovery(crate::CatalogFailure {
        code: crate::CatalogFailureCode::AuthRequired,
        message: "credential".into(),
    }));
    assert_eq!(
        crate::CatalogFailure::classify(&error),
        crate::CatalogFailureCode::AuthRequired
    );
    assert!(!error.to_string().contains("credential"));
}
