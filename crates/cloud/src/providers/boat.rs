//! Boat v1 control plane. Stops are reconciled by their original operation ID.
use super::{
    ExecResult, Machine, MachineState, ProviderError,
    contract::*,
    transport::{Rest, text},
};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::Method;
use serde_json::{Value, json};
use zuse_protocol::cloud_computers::ProviderId;

/// The installed runtime runs: the restored disk serves the files a boot needs.
pub(crate) const RUNTIME_ANSWERS: &str = "/usr/local/bin/zuse-cloud-runtime --version";

pub struct Boat {
    pub(super) rest: Rest,
}
impl Boat {
    pub fn new(api_key: String) -> Result<Self, ProviderError> {
        Ok(Self {
            rest: Rest::new("https://boat.dev/api/v1", api_key, "Authorization", true)?,
        })
    }
    fn machine(value: &Value) -> Result<Machine, ProviderError> {
        let state = text(value, "state")?;
        Ok(Machine {
            id: text(value, "id")?.into(),
            name: value["name"].as_str().unwrap_or("").into(),
            state: match state {
                "ready" | "idle" | "running" => MachineState::Running,
                "archived" | "stopped" => MachineState::Stopped,
                "init" | "provisioning" | "provisioned" | "cloning" | "archiving" | "resuming" => {
                    MachineState::Starting
                }
                other => MachineState::Unknown(other.into()),
            },
            vcpu: None,
            memory_bytes: None,
        })
    }
    async fn call(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<&Value>,
    ) -> Result<Value, ProviderError> {
        self.rest
            .request(method, self.rest.route(segments)?, body, None, None)
            .await
    }
    /// Save a credential-free prepared computer only. Never capture a task's live credentials.
    pub async fn save_prepared_snapshot(
        &self,
        id: &str,
        name: &str,
    ) -> Result<String, ProviderError> {
        let value = self
            .call(
                Method::POST,
                &["saved-snapshots"],
                Some(&json!({"sandboxId":id,"name":name})),
            )
            .await?;
        text(&value["snapshot"], "name")
            .map(str::to_owned)
            .map_err(|_| ProviderError::OutcomeUnknown)
    }
    pub async fn snapshot_state(&self, name: &str) -> Result<Option<String>, ProviderError> {
        match self
            .call(Method::GET, &["saved-snapshots", name], None)
            .await
        {
            Ok(value) => match text(&value["snapshot"], "status")? {
                state @ ("saving" | "ready" | "failed") => Ok(Some(state.into())),
                _ => Err(ProviderError::InvalidResponse),
            },
            Err(ProviderError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
}
#[async_trait]
impl Provider for Boat {
    fn id(&self) -> ProviderId {
        ProviderId::Boat
    }
    async fn runtime_ready(&self, id: &str) -> Result<bool, ProviderError> {
        let value = self.call(Method::GET, &["sandboxes", id], None).await?;
        let sandbox = &value["sandbox"];
        let machine = Self::machine(sandbox)?;
        if machine.id != id {
            return Err(ProviderError::InvalidResponse);
        }
        let hydrated = sandbox["hydrated"]
            .as_bool()
            .ok_or(ProviderError::InvalidResponse)?;
        if machine.state != MachineState::Running {
            return Ok(false);
        }
        if hydrated {
            return Ok(true);
        }
        // Boat restores disks lazily: full hydration took 11–21 s after create
        // or cold resume, while the installed runtime answered after 2–9 s.
        // Reads block until their blocks arrive and earlier writes survive
        // hydration and a cold restore (verified live, November 10, 2026), so
        // a runtime that answers is enough to deliver the boot credential.
        Ok(matches!(
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                self.execute(id, RUNTIME_ANSWERS),
            )
            .await,
            Ok(Ok(result)) if result.exit_code == 0
        ))
    }
    async fn verify_initial_source(
        &self,
        id: &str,
        artifact: &str,
        name: &str,
    ) -> Result<(), ProviderError> {
        self.verify_initial_source_with_receipt(id, artifact, name, None)
            .await
    }
    async fn verify_initial_source_with_receipt(
        &self,
        id: &str,
        artifact: &str,
        name: &str,
        receipt: Option<&VerifiedCreateSource>,
    ) -> Result<(), ProviderError> {
        let snapshot = self
            .call(Method::GET, &["saved-snapshots", artifact], None)
            .await?;
        if snapshot["snapshot"]["name"] != artifact || snapshot["snapshot"]["status"] != "ready" {
            return Err(ProviderError::Rejected);
        }
        let value = self.call(Method::GET, &["sandboxes", id], None).await?;
        let sandbox = &value["sandbox"];
        let machine = Self::machine(sandbox)?;
        // These are original create-request labels, not immutable provider
        // attestation. The helper independently verifies the actual image SRI.
        if machine.id != id
            || machine.name != name
            || machine.state != MachineState::Running
            || !sandbox["hydrated"].is_boolean()
        {
            return Err(ProviderError::Rejected);
        }
        let labels = sandbox["labels"]
            .as_object()
            .ok_or(ProviderError::Rejected)?;
        let authoritative = receipt.is_some_and(|r| r.matches_boat_source(id, artifact, name));
        if let (Some(receipt), Some(value)) = (receipt, labels.get("zuse-allocation-operation")) {
            if !receipt.matches_operation_label(value) {
                return Err(ProviderError::Rejected);
            }
        }
        for (key, expected) in [
            ("zuse-source-artifact", artifact),
            ("zuse-allocation-name", name),
        ] {
            match labels.get(key) {
                Some(value) if value.as_str() == Some(expected) => {}
                None if authoritative => {}
                _ => return Err(ProviderError::Rejected),
            }
        }
        Ok(())
    }
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        match self.call(Method::GET, &["sandboxes", id], None).await {
            Ok(value) => {
                let machine = Self::machine(&value["sandbox"])?;
                if machine.id != id {
                    return Err(ProviderError::InvalidResponse);
                }
                Ok(Some(machine))
            }
            Err(ProviderError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
    async fn find_allocation(&self, name: &str) -> Result<Option<Machine>, ProviderError> {
        let mut cursor = None::<String>;
        let mut seen = std::collections::HashSet::new();
        let mut found = None;
        for _ in 0..100 {
            let mut url = self.rest.route(&["sandboxes"])?;
            url.query_pairs_mut().append_pair("limit", "100").append_pair("state", "init,provisioning,provisioned,cloning,ready,idle,running,archiving,archived,stopped,resuming");
            if let Some(cursor) = &cursor {
                url.query_pairs_mut().append_pair("cursor", cursor);
            }
            let value = self
                .rest
                .request(Method::GET, url, None, None, None)
                .await?;
            let list = value["sandboxes"]
                .as_array()
                .ok_or(ProviderError::InvalidResponse)?;
            for item in list.iter().filter(|v| v["name"] == name) {
                let machine = Self::machine(item)?;
                if found
                    .as_ref()
                    .is_some_and(|prior: &Machine| prior.id != machine.id)
                {
                    return Err(ProviderError::Conflict);
                }
                found = Some(machine);
            }
            match value["pageInfo"]["nextCursor"].as_str() {
                None => return Ok(found),
                Some(next) if next.is_empty() || !seen.insert(next.to_owned()) => {
                    return Err(ProviderError::InvalidResponse);
                }
                Some(next) => cursor = Some(next.into()),
            }
        }
        // An exhausted scan budget is not evidence of absence after an uncertain create.
        Err(ProviderError::Unavailable)
    }
    async fn allocate(&self, request: &Allocation<'_>) -> Result<Machine, ProviderError> {
        request.validate()?;
        self.verify_artifact(request.artifact, request.artifact_binding)
            .await?;
        if !matches!(request.size, "small" | "default" | "large") {
            return Err(ProviderError::Rejected);
        }
        let value = self
            .rest
            .request(
                Method::POST,
                self.rest.route(&["sandboxes"])?,
                Some(
                    &json!({"name":request.name,"from":request.artifact,"type":request.size,
                "ttlSeconds":request.lifetime_seconds,"noEnv":true,"visibility":"private",
                "labels":{"zuse-source-artifact":request.artifact,"zuse-allocation-name":request.name,"zuse-allocation-operation":request.operation_id}}),
                ),
                Some(request.operation_id),
                None,
            )
            .await?;
        Self::machine(&value["sandbox"]).map_err(|_| ProviderError::OutcomeUnknown)
    }
    async fn sleep(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        let value = self
            .call(Method::POST, &["sandboxes", id, "stop"], Some(&json!({})))
            .await?;
        let operation = value
            .get("stop")
            .filter(|v| !v.is_null())
            .or_else(|| value["sandbox"].get("stop"))
            .ok_or(ProviderError::OutcomeUnknown)?;
        let operation_id = text(operation, "id").map_err(|_| ProviderError::OutcomeUnknown)?;
        if !matches!(
            operation["status"].as_str(),
            Some("pending" | "failing" | "completed" | "superseded")
        ) {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: Some(operation_id.into()),
            desired: DesiredState::Sleeping,
        })
    }
    async fn resume(
        &self,
        id: &str,
        lifetime_seconds: u32,
    ) -> Result<ProviderOperation, ProviderError> {
        if !(60..=86400).contains(&lifetime_seconds) {
            return Err(ProviderError::Rejected);
        }
        self.call(
            Method::POST,
            &["sandboxes", id, "resume"],
            Some(&json!({"ttlSeconds":lifetime_seconds})),
        )
        .await?;
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
            desired: DesiredState::Running,
        })
    }
    async fn delete(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        match self
            .rest
            .request(
                Method::DELETE,
                self.rest.route(&["sandboxes", id])?,
                None,
                None,
                Some(id),
            )
            .await
        {
            Ok(_) | Err(ProviderError::NotFound) => {}
            Err(error) => return Err(error),
        }
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
            desired: DesiredState::Deleted,
        })
    }
    async fn operation(
        &self,
        receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        if receipt.desired == DesiredState::Sleeping {
            let operation = receipt
                .operation_id
                .as_deref()
                .ok_or(ProviderError::InvalidResponse)?;
            let value = self
                .call(
                    Method::GET,
                    &["sandboxes", &receipt.resource_id, "stops", operation],
                    None,
                )
                .await?;
            let stop = &value["stop"];
            if text(stop, "id")? != operation {
                return Err(ProviderError::InvalidResponse);
            }
            match text(stop, "status")? {
                "pending" => return Ok(OperationState::Pending),
                "failing" => return Ok(OperationState::Retrying),
                "superseded" => return Ok(OperationState::Superseded),
                "completed" => {}
                _ => return Err(ProviderError::InvalidResponse),
            }
        }
        Ok(super::contract::observed(
            receipt,
            self.inspect(&receipt.resource_id).await?.as_ref(),
        ))
    }
    async fn execute(&self, id: &str, command: &str) -> Result<ExecResult, ProviderError> {
        if command.len() > 131072 || command.contains('\0') {
            return Err(ProviderError::Rejected);
        }
        let value = self
            .call(
                Method::POST,
                &["sandboxes", id, "commands"],
                Some(&json!({"command":command,"timeoutSeconds":60})),
            )
            .await?;
        if value["timedOut"] == true {
            return Err(ProviderError::OutcomeUnknown);
        }
        let exit_code = value["exitCode"]
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or(ProviderError::OutcomeUnknown)?;
        let stdout = value["stdout"]
            .as_str()
            .ok_or(ProviderError::OutcomeUnknown)?
            .as_bytes()
            .to_vec();
        let stderr = value["stderr"]
            .as_str()
            .ok_or(ProviderError::OutcomeUnknown)?
            .as_bytes()
            .to_vec();
        Ok(ExecResult {
            stdout,
            stderr,
            exit_code,
        })
    }
    async fn upload(&self, id: &str, path: &str, contents: &[u8]) -> Result<(), ProviderError> {
        if contents.len() > 1024 * 1024
            || !path.starts_with('/')
            || path.contains(['\r', '\n', '\0'])
        {
            return Err(ProviderError::Rejected);
        }
        self.call(
            Method::PUT,
            &["sandboxes", id, "files"],
            Some(&json!({"path":path,"content":STANDARD.encode(contents),"encoding":"base64"})),
        )
        .await?;
        Ok(())
    }
}
