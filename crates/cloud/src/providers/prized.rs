//! Prized sandboxes: Firecracker microVMs (us-west-2) with memory-keeping pause.
//!
//! Prized has no custom images. A managed base is a prepared, paused parent
//! sandbox, and each computer is a fork of it (the documented pattern); a
//! built-in template id also works for unprepared probes. Commands run as the
//! user `prized` without sudo, and uploads must stay under `/home/prized`.
//! A sandbox still running when `maxRuntimeMin` ends deletes itself, so busy
//! computers have their limit pushed back through [`Provider::keep_running`].
use super::{
    ExecResult, Machine, MachineState, ProviderError,
    contract::*,
    transport::{Rest, read, text},
};
use async_trait::async_trait;
use reqwest::{Method, Url};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use zuse_protocol::cloud_computers::ProviderId;

/// The label carrying our allocation name; Prized has no name field.
const NAME_LABEL: &str = "zuse-allocation-name";
const OPERATION_LABEL: &str = "zuse-allocation-operation";
/// Uploads outside the sandbox user's home are refused by Prized.
const HOME: &str = "/home/prized/";
/// Paid plans allow at most seven days of running time per resume.
const MAX_RUNTIME_MIN: u32 = 7 * 24 * 60;

pub struct Prized {
    pub(super) rest: Rest,
    edges: Mutex<HashMap<String, (Url, Instant)>>,
    pub(super) edge_origins: Vec<String>,
}
impl Prized {
    pub fn new(api_key: String) -> Result<Self, ProviderError> {
        Ok(Self {
            rest: Rest::new(
                "https://api.prized.dev/api/v1",
                api_key,
                "Authorization",
                true,
            )?,
            edges: Mutex::new(HashMap::new()),
            edge_origins: vec!["https://term.prized.dev".into()],
        })
    }
    fn machine(value: &Value) -> Result<Machine, ProviderError> {
        let id = text(value, "id")?;
        Ok(Machine {
            id: id.into(),
            name: value["labels"][NAME_LABEL]
                .as_str()
                .unwrap_or(id)
                .to_owned(),
            state: match text(value, "state")? {
                "running" => MachineState::Running,
                "paused" => MachineState::Paused,
                "starting" | "pausing" | "resizing" => MachineState::Starting,
                other => MachineState::Unknown(other.into()),
            },
            vcpu: value["resources"]["vcpu"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok()),
            memory_bytes: value["resources"]["memoryMb"]
                .as_u64()
                .and_then(|n| n.checked_mul(1024 * 1024)),
        })
    }
    fn url(&self, segments: &[&str], query: &[(&str, &str)]) -> Result<Url, ProviderError> {
        let mut url = self.rest.route(segments)?;
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        Ok(url)
    }
    async fn call(
        &self,
        method: Method,
        url: Url,
        body: Option<&Value>,
        idempotency: Option<&str>,
    ) -> Result<Value, ProviderError> {
        self.rest
            .request(method, url, body, idempotency, None)
            .await
    }
    /// Cache a sandbox's edge URL after checking it belongs to Prized's edge.
    fn remember(&self, value: &Value) -> Result<(), ProviderError> {
        let (Some(id), Some(base)) = (value["id"].as_str(), value["toolboxBase"].as_str()) else {
            return Ok(());
        };
        let parsed = Url::parse(base).map_err(|_| ProviderError::InvalidResponse)?;
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(ProviderError::InvalidResponse);
        }
        let _ = self
            .rest
            .trusted_request(Method::GET, parsed.clone(), &self.origins())?;
        let mut cache = self.edges.lock().map_err(|_| ProviderError::Unavailable)?;
        if cache.len() >= 256 {
            cache.clear();
        }
        cache.insert(id.into(), (parsed, Instant::now()));
        Ok(())
    }
    fn origins(&self) -> Vec<&str> {
        self.edge_origins.iter().map(String::as_str).collect()
    }
    fn forget(&self, id: &str) -> Result<(), ProviderError> {
        self.edges
            .lock()
            .map_err(|_| ProviderError::Unavailable)?
            .remove(id);
        Ok(())
    }
    async fn edge(&self, id: &str, segments: &[&str]) -> Result<Url, ProviderError> {
        let cached = self
            .edges
            .lock()
            .map_err(|_| ProviderError::Unavailable)?
            .get(id)
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(60))
            .map(|(url, _)| url.clone());
        let mut url = match cached {
            Some(url) => url,
            None => {
                let value = self
                    .call(Method::GET, self.url(&["sandboxes", id], &[])?, None, None)
                    .await?;
                self.remember(&value["sandbox"])?;
                let base = value["sandbox"]["toolboxBase"]
                    .as_str()
                    .ok_or(ProviderError::InvalidResponse)?;
                Url::parse(base).map_err(|_| ProviderError::InvalidResponse)?
            }
        };
        url.path_segments_mut()
            .map_err(|_| ProviderError::Rejected)?
            .pop_if_empty()
            .extend(segments);
        Ok(url)
    }
    fn operation_receipt(id: &str, desired: DesiredState) -> ProviderOperation {
        ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
            desired,
        }
    }
}

#[async_trait]
impl Provider for Prized {
    fn id(&self) -> ProviderId {
        ProviderId::Prized
    }
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        match self
            .call(Method::GET, self.url(&["sandboxes", id], &[])?, None, None)
            .await
        {
            Ok(value) => {
                let sandbox = &value["sandbox"];
                if text(sandbox, "id")? != id {
                    return Err(ProviderError::InvalidResponse);
                }
                if matches!(sandbox["state"].as_str(), Some("deleting" | "deleted")) {
                    return Ok(None);
                }
                self.remember(sandbox)?;
                Ok(Some(Self::machine(sandbox)?))
            }
            Err(ProviderError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
    async fn find_allocation(&self, name: &str) -> Result<Option<Machine>, ProviderError> {
        let label = format!("{NAME_LABEL}={name}");
        let value = self
            .call(
                Method::GET,
                self.url(&["sandboxes"], &[("label", &label)])?,
                None,
                None,
            )
            .await?;
        let live = value["sandboxes"]
            .as_array()
            .ok_or(ProviderError::InvalidResponse)?
            .iter()
            .filter(|sandbox| sandbox["labels"][NAME_LABEL] == name)
            .filter(|sandbox| !matches!(sandbox["state"].as_str(), Some("deleting" | "deleted")))
            .collect::<Vec<_>>();
        match live.as_slice() {
            [] => Ok(None),
            [sandbox] => Ok(Some(Self::machine(sandbox)?)),
            _ => Err(ProviderError::Conflict),
        }
    }
    async fn verify_artifact(
        &self,
        artifact: &str,
        binding: Option<&super::artifact::ArtifactBinding>,
    ) -> Result<(), ProviderError> {
        if binding.is_some() {
            return Err(ProviderError::Rejected);
        }
        // A prepared parent must exist and be paused, so a fork copies a quiet base.
        if artifact.starts_with("sbx_") {
            let parent = self
                .inspect(artifact)
                .await?
                .ok_or(ProviderError::NotFound)?;
            if parent.state != MachineState::Paused {
                return Err(ProviderError::Conflict);
            }
        }
        Ok(())
    }
    async fn allocate(&self, request: &Allocation<'_>) -> Result<Machine, ProviderError> {
        request.validate()?;
        self.verify_artifact(request.artifact, request.artifact_binding)
            .await?;
        if request.size != "default" {
            return Err(ProviderError::Rejected);
        }
        let labels = json!({NAME_LABEL:request.name,OPERATION_LABEL:request.operation_id});
        let runtime = request.lifetime_seconds.div_ceil(60).min(MAX_RUNTIME_MIN);
        let value = if request.artifact.starts_with("sbx_") {
            let value = self
                .call(
                    Method::POST,
                    self.url(
                        &["sandboxes", request.artifact, "fork"],
                        &[("wait", "running")],
                    )?,
                    Some(&json!({"count":1,"labels":labels,"maxRuntimeMin":runtime})),
                    Some(request.operation_id),
                )
                .await?;
            value["sandboxes"][0].clone()
        } else {
            let value = self
                .call(
                    Method::POST,
                    self.url(&["sandboxes"], &[("wait", "running")])?,
                    Some(&json!({"template":request.artifact,"labels":labels,"maxRuntimeMin":runtime})),
                    Some(request.operation_id),
                )
                .await?;
            value["sandbox"].clone()
        };
        let machine = Self::machine(&value).map_err(|_| ProviderError::OutcomeUnknown)?;
        if machine.name != request.name {
            return Err(ProviderError::OutcomeUnknown);
        }
        self.remember(&value)
            .map_err(|_| ProviderError::OutcomeUnknown)?;
        Ok(machine)
    }
    async fn sleep(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.call(
            Method::POST,
            self.url(&["sandboxes", id, "pause"], &[])?,
            None,
            None,
        )
        .await?;
        self.forget(id)?;
        Ok(Self::operation_receipt(id, DesiredState::Sleeping))
    }
    async fn resume(
        &self,
        id: &str,
        lifetime_seconds: u32,
    ) -> Result<ProviderOperation, ProviderError> {
        if !(60..=86400).contains(&lifetime_seconds) {
            return Err(ProviderError::Rejected);
        }
        self.forget(id)?;
        self.call(
            Method::POST,
            self.url(&["sandboxes", id, "resume"], &[])?,
            None,
            None,
        )
        .await?;
        Ok(Self::operation_receipt(id, DesiredState::Running))
    }
    async fn delete(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        match self
            .call(
                Method::DELETE,
                self.url(&["sandboxes", id], &[])?,
                None,
                None,
            )
            .await
        {
            Ok(_) | Err(ProviderError::NotFound) => {}
            // 410 sandbox_deleted: already gone.
            Err(ProviderError::Rejected) if self.inspect(id).await?.is_none() => {}
            Err(error) => return Err(error),
        }
        self.forget(id)?;
        Ok(Self::operation_receipt(id, DesiredState::Deleted))
    }
    async fn operation(
        &self,
        receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        if receipt.operation_id.is_some() {
            return Err(ProviderError::InvalidResponse);
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
        let url = self.edge(id, &["process", "execute"]).await?;
        let response = self
            .rest
            .trusted_request(Method::POST, url, &self.origins())?
            .json(&json!({"command":command,"timeout":60}))
            .timeout(Duration::from_secs(70))
            .send()
            .await
            .map_err(|_| ProviderError::OutcomeUnknown)?;
        if response.status().as_u16() == 409 || response.status().as_u16() == 503 {
            return Err(ProviderError::Unavailable);
        }
        if !response.status().is_success() {
            return Err(ProviderError::OutcomeUnknown);
        }
        let bytes = read(response, 8 * 1024 * 1024)
            .await
            .map_err(|_| ProviderError::OutcomeUnknown)?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| ProviderError::OutcomeUnknown)?;
        if value["timedOut"] == true {
            return Err(ProviderError::OutcomeUnknown);
        }
        let exit_code = value["exitCode"]
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or(ProviderError::OutcomeUnknown)?;
        Ok(ExecResult {
            exit_code,
            stdout: value["stdout"].as_str().unwrap_or("").as_bytes().to_vec(),
            stderr: value["stderr"].as_str().unwrap_or("").as_bytes().to_vec(),
        })
    }
    async fn upload(&self, id: &str, path: &str, contents: &[u8]) -> Result<(), ProviderError> {
        if contents.len() > 1024 * 1024
            || !path.starts_with(HOME)
            || path.contains("/../")
            || path.chars().any(char::is_control)
        {
            return Err(ProviderError::Rejected);
        }
        let mut url = self.edge(id, &["files", "upload"]).await?;
        url.query_pairs_mut().append_pair("path", path);
        let response = self
            .rest
            .trusted_request(Method::POST, url, &self.origins())?
            .header("Content-Type", "application/octet-stream")
            .body(contents.to_vec())
            .send()
            .await
            .map_err(|_| ProviderError::OutcomeUnknown)?;
        if !response.status().is_success() {
            return Err(ProviderError::OutcomeUnknown);
        }
        read(response, 65536)
            .await
            .map_err(|_| ProviderError::OutcomeUnknown)?;
        Ok(())
    }
    /// Push the running limit back (a running sandbox past it deletes itself)
    /// and count as activity for any auto-pause.
    async fn keep_running(&self, id: &str) -> Result<(), ProviderError> {
        self.call(
            Method::POST,
            self.url(&["sandboxes", id, "timeout"], &[])?,
            Some(&json!({"inMin":24 * 60})),
            None,
        )
        .await
        .map(|_| ())
    }
}

#[cfg(test)]
#[path = "prized_tests.rs"]
mod tests;
