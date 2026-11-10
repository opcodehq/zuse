//! Diagnostics ingress proves a live Core without pretending project preparation
//! or the selected coding agent is ready. Only a current failed stage can repair.
use super::*;
pub(super) fn failed_preparation_ingress(row: &Value) -> bool {
    let config = &row["request_config"];
    let ingress = &config["rustPreparationIngress"];
    let Some(operation) = config["rustAllocationOperationId"]
        .as_str()
        .filter(|v| uuid::Uuid::parse_str(v).is_ok())
    else {
        return false;
    };
    let Some(connected) = ingress["connectedAt"].as_i64().filter(|v| *v > 0) else {
        return false;
    };
    row["state"] == "setup"
        && row["desired_state"] == "ready"
        && row["runtime_state"] == "connecting"
        && config["runtimeRpcProtocol"] == zuse_protocol::cloud_gateway::RUST_RPC
        && config["rustPreparationVersion"] == 1
        && storage_enrolled(row)
        && ingress["version"] == 1
        && ingress["operationId"] == operation
        && ingress["generation"] == config["runtimeGeneration"]
        && ingress["sourceMachineId"] == row["provider_sandbox_id"]
        && row["provider_sandbox_id"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
        && ingress["storageIncarnationId"] == config["rustRuntimeStorage"]["storageIncarnationId"]
        && [
            ("repository", "repository_preparation_failed"),
            ("environment", "environment_setup_failed"),
            ("credentials", "credential_preparation_failed"),
        ]
        .iter()
        .any(|(stage, code)| {
            let failed = &config["preparation"][*stage];
            row["status_code"] == *code
                && failed["state"] == "failed"
                && failed["operationId"] == operation
                && failed["errorCode"] == *code
                && failed["updatedAt"]
                    .as_i64()
                    .is_some_and(|at| at >= connected)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_failure_repair_requires_current_operation_generation_disk_and_ingress() {
        let operation = "0e68ef5b-2a62-4a78-8401-050ac941898b";
        let row = json!({
            "workspace_id":"ws", "provider_sandbox_id":"machine",
            "state":"setup", "desired_state":"ready", "runtime_state":"connecting",
            "status_code":"credential_preparation_failed",
            "request_config":{
                "runtimeGeneration":1, "gatewayEpoch":1,
                "runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,
                "runtimeBootstrapReceipt":{"workspaceId":"ws","generation":1,"gatewayEpoch":1},
                "rustRuntimeRepairVersion":1, "rustRuntimeSupervisorVersion":1,
                "runtimeCredentialKeyThumbprint":"key", "runtimeSigningKeyThumbprint":"signing",
                "rustRuntimeStorage":{"storageIncarnationId":"disk"},
                "rustStorageEnrollment":{"phase":"committed","generation":1,
                    "sourceSandboxId":"machine","storageIncarnationId":"disk",
                    "credentialKeyThumbprint":"key","signingKeyThumbprint":"signing"},
                "rustPreparationVersion":1, "rustAllocationOperationId":operation,
                "rustPreparationIngress":{"version":1,"operationId":operation,"generation":1,
                    "sourceMachineId":"machine","storageIncarnationId":"disk","connectedAt":10},
                "preparation":{"credentials":{"operationId":operation,"state":"failed",
                    "errorCode":"credential_preparation_failed","startedAt":10,"updatedAt":20}}
            }
        });
        assert!(failed_preparation_ingress(&row));
        assert_eq!(workspace_metadata(&row)["runtimeRepairAvailable"], true);
        for (pointer, value) in [
            (
                "/request_config/rustPreparationIngress/operationId",
                json!(uuid::Uuid::new_v4().to_string()),
            ),
            (
                "/request_config/rustPreparationIngress/generation",
                json!(2),
            ),
            (
                "/request_config/rustPreparationIngress/sourceMachineId",
                json!("other"),
            ),
            (
                "/request_config/rustPreparationIngress/storageIncarnationId",
                json!("other"),
            ),
            (
                "/request_config/rustPreparationIngress/connectedAt",
                json!(21),
            ),
            (
                "/request_config/rustStorageEnrollment/phase",
                json!("pending"),
            ),
            (
                "/request_config/preparation/credentials/operationId",
                json!("other"),
            ),
            (
                "/request_config/preparation/credentials/errorCode",
                json!("vendor-body"),
            ),
            (
                "/request_config/preparation/credentials/state",
                json!("running"),
            ),
            ("/state", json!("deleted")),
            ("/desired_state", json!("paused")),
        ] {
            let mut changed = row.clone();
            *changed.pointer_mut(pointer).unwrap() = value;
            assert!(!failed_preparation_ingress(&changed), "fence {pointer}");
            assert_eq!(
                workspace_metadata(&changed)["runtimeRepairAvailable"],
                false,
                "metadata fence {pointer}"
            );
        }
    }
}
