//! The release pipeline records `bootstrap-capabilities` output as the
//! capabilities a computer will send; it must be exactly what bootstrap sends.
use std::process::{Command, Stdio};

#[test]
fn bootstrap_capabilities_prints_what_bootstrap_sends() {
    let output = Command::new(env!("CARGO_BIN_EXE_zuse-cloud-runtime"))
        .arg("bootstrap-capabilities")
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .expect("execute the actual guest binary");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 output");
    let expected = serde_json::to_string(&zuse_cloud_runtime::identity::bootstrap_capabilities())
        .expect("capabilities serialize");
    assert_eq!(stdout, format!("{expected}\n"));
    let parsed: Vec<String> = serde_json::from_str(&stdout).expect("compact JSON array");
    assert!(parsed.contains(&zuse_protocol::cloud_runtime_updates::CAPABILITY.to_owned()));
}
