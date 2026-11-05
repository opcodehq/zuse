use super::*;
#[test]
fn secret_redaction_carries_pipe_chunk_boundaries_and_masks_truncated_prefixes() {
    let mut secrets = PrivateEnvironment::new();
    secrets.insert(
        "PROJECT_TOKEN".into(),
        zeroize::Zeroizing::new("private-secret".into()),
    );
    let mut redactor = Redactor::new(&secrets);
    let mut output = redactor.push(b"before private-", false);
    output.extend(redactor.push(b"secret after", true));
    assert_eq!(output, b"before [redacted] after");
    assert_eq!(
        redact_output(b"before private-", &secrets),
        b"before [redacted]"
    );
}
#[tokio::test]
async fn secret_values_reach_setup_without_entering_receipts_or_setup_logs() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let checkout = root.path().join("repo");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    let mut recipe = EnvironmentRecipe::default();
    recipe.secret_bindings = vec!["secret_fixture".into()];
    recipe.scripts.initialize =
        Some("test -n \"$PROJECT_REGISTRY_TOKEN\"; printf '%s' \"$PROJECT_REGISTRY_TOKEN\"".into());
    let mut secrets = PrivateEnvironment::new();
    secrets.insert(
        "PROJECT_REGISTRY_TOKEN".into(),
        zeroize::Zeroizing::new("private-environment-fixture-secret".into()),
    );
    prepare_with_environment(&data, &checkout, "task", "incarnation", &recipe, &secrets)
        .await
        .unwrap();
    let receipt = std::fs::read_to_string(data.join("environment-ready.json")).unwrap();
    assert!(!receipt.contains("private-environment-fixture-secret"));
    assert!(receipt.contains("secret_fixture"));
    let log = std::fs::read_to_string(data.join("environment-initialize.json")).unwrap();
    assert!(log.contains("[redacted]"));
    assert!(!log.contains("private-environment-fixture-secret"));
}
