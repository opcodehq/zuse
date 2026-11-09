use super::*;
#[tokio::test]
async fn private_host_environment_is_not_serialized_and_retained_ptys_keep_their_values() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().to_str().unwrap();
    let terminals = Terminals::new();
    let values = |value: &str| {
        Arc::new(std::collections::BTreeMap::from([(
            "PROJECT_TOKEN".into(),
            zeroize::Zeroizing::new(value.into()),
        )]))
    };
    terminals.set_host_environment(values("private-first-fixture"));
    let first = terminals
        .open_with_shell(cwd, 80, 24, Some("/bin/sh"))
        .unwrap();
    terminals.set_host_environment(values("private-next-fixture"));
    let next = terminals
        .open_with_shell(cwd, 80, 24, Some("/bin/sh"))
        .unwrap();
    for (session, expected) in [
        (first, "private-first-fixture"),
        (next, "private-next-fixture"),
    ] {
        assert!(
            !serde_json::to_string(&session)
                .unwrap()
                .contains("private-")
        );
        let mut events = terminals.subscribe(&session.id, None).unwrap();
        terminals
            .write_bytes(
                &session.id,
                b"printf 'HOST_VALUE=%s\\n' \"$PROJECT_TOKEN\"; exit\n",
            )
            .unwrap();
        let mut output = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = events.recv().await {
                match event {
                    TerminalEvent::Data { data, .. } => output.extend(BASE64.decode(data).unwrap()),
                    TerminalEvent::Exit { .. } => break,
                }
            }
        })
        .await
        .unwrap();
        assert!(String::from_utf8_lossy(&output).contains(&format!("HOST_VALUE={expected}")));
        terminals.close(&session.id).unwrap();
    }
}
