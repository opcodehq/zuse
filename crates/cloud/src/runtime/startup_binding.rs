//! Capability-gated initial storage preparation shares the bootstrap transaction.
use super::*;
pub(super) fn prepare(
    config: &mut Value,
    row: &Value,
    generation: i64,
    now: i64,
) -> Result<(), ApiError> {
    if config["startupBindingVersion"] == 2 && !negotiated(config, row, generation) {
        // A restore/recreation can carry old configuration but must perform its
        // own storage enrollment rather than reuse an initial binding marker.
        config
            .as_object_mut()
            .ok_or_else(rejected)?
            .remove("startupBindingVersion");
    }
    if config["rustFastStartVersion"] != 1
        || !receipt_fast_start(config)
        || !config["runtimeBootstrapReceipt"]["capabilities"]
            .as_array()
            .is_some_and(|caps| {
                caps.iter()
                    .any(|cap| cap == zuse_protocol::cloud::STARTUP_BIND_CAPABILITY)
            })
        || config["sessionHeadVersion"].is_number()
        || !config["rustRecoveryRestore"].is_null()
        || !config["rustStorageEnrollment"].is_null()
    {
        return Ok(());
    }
    let source = required(row, "provider_sandbox_id")?;
    let disk = config["rustRuntimeStorage"]["storageIncarnationId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::conflict("runtime_storage_enrollment_required"))?;
    let credential = config["runtimeCredentialKeyThumbprint"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(rejected)?;
    let signing = config["runtimeSigningKeyThumbprint"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(rejected)?;
    config["rustStorageEnrollment"] = json!({"enrollmentId":uuid::Uuid::new_v4().to_string(),"phase":"prepared",
        "storageIncarnationId":disk,"credentialKeyThumbprint":credential,"signingKeyThumbprint":signing,
        "sourceSandboxId":source,"generation":generation,"preparedAt":now});
    config["startupBindingVersion"] = json!(2);
    Ok(())
}

pub(super) fn negotiated(config: &Value, row: &Value, generation: i64) -> bool {
    let record = &config["rustStorageEnrollment"];
    config["startupBindingVersion"] == 2
        && receipt_fast_start(config)
        && config["runtimeBootstrapReceipt"]["capabilities"]
            .as_array()
            .is_some_and(|caps| {
                caps.iter()
                    .any(|cap| cap == zuse_protocol::cloud::STARTUP_BIND_CAPABILITY)
            })
        && record["generation"].as_i64() == Some(generation)
        && matches!(record["phase"].as_str(), Some("prepared" | "committed"))
        && record["sourceSandboxId"].is_string()
        && record["sourceSandboxId"] == row["provider_sandbox_id"]
        && record["storageIncarnationId"].is_string()
        && record["storageIncarnationId"] == config["rustRuntimeStorage"]["storageIncarnationId"]
        && record["credentialKeyThumbprint"].is_string()
        && record["credentialKeyThumbprint"] == config["runtimeCredentialKeyThumbprint"]
        && record["signingKeyThumbprint"].is_string()
        && record["signingKeyThumbprint"] == config["runtimeSigningKeyThumbprint"]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initial_storage_negotiation_does_not_survive_a_different_restore_binding() {
        let row = json!({"provider_sandbox_id":"machine"});
        let config = json!({"rustFastStartVersion":1,"runtimeBootstrapReceipt":{"capabilities":["rust-fast-start-v1",zuse_protocol::cloud::STARTUP_BIND_CAPABILITY]},"rustRuntimeStorage":{"storageIncarnationId":"disk"},"runtimeCredentialKeyThumbprint":"credential","runtimeSigningKeyThumbprint":"signing"});
        let mut initial = config.clone();
        prepare(&mut initial, &row, 1, 100).unwrap();
        assert!(negotiated(&initial, &row, 1));
        let original = initial["rustStorageEnrollment"]["enrollmentId"].clone();
        prepare(&mut initial, &row, 1, 101).unwrap();
        assert_eq!(initial["rustStorageEnrollment"]["enrollmentId"], original);
        for (field, value) in [
            ("runtimeCredentialKeyThumbprint", json!("new-key")),
            (
                "rustRuntimeStorage",
                json!({"storageIncarnationId":"new-disk"}),
            ),
        ] {
            let mut changed = initial.clone();
            changed[field] = value;
            changed["rustRecoveryRestore"] = json!({"operationId":"restore"});
            prepare(&mut changed, &row, 1, 102).unwrap();
            assert!(changed["startupBindingVersion"].is_null());
            assert!(!negotiated(&changed, &row, 1));
        }
        let mut restored = initial.clone();
        restored["rustRecoveryRestore"] = json!({"operationId":"restore"});
        prepare(&mut restored, &row, 2, 102).unwrap();
        assert!(restored["startupBindingVersion"].is_null());
        assert_eq!(restored["rustStorageEnrollment"]["enrollmentId"], original);
        assert!(!negotiated(
            &initial,
            &json!({"provider_sandbox_id":"new-machine"}),
            1
        ));
    }
}
