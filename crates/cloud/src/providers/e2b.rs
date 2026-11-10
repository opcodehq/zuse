//! E2B control and envd transports. Inspection never connects or wakes a sandbox.
use super::{
    ExecResult, Machine, MachineState, ProviderError,
    contract::*,
    transport::{Rest, read, text},
};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{Method, Url};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;
use zuse_protocol::cloud_computers::ProviderId;

#[derive(Clone)]
struct Guest {
    url: Url,
    token: Zeroizing<String>,
    fetched: Instant,
}
pub struct E2b {
    pub(super) rest: Rest,
    guests: Mutex<HashMap<String, Guest>>,
    max_timeout_seconds: u32,
}
impl E2b {
    pub fn new(api_key: String) -> Result<Self, ProviderError> {
        Ok(Self {
            rest: Rest::new("https://api.e2b.app", api_key, "x-api-key", false)?,
            guests: Mutex::new(HashMap::new()),
            // The shared controller's 24-hour hint exceeds Hobby limits.
            // Active timeouts pause retained sandboxes; this is not a keepalive.
            max_timeout_seconds: 3600,
        })
    }
    pub fn with_max_timeout(mut self, seconds: u32) -> Result<Self, ProviderError> {
        if !(60..=86400).contains(&seconds) {
            return Err(ProviderError::Rejected);
        }
        self.max_timeout_seconds = seconds;
        Ok(self)
    }
    fn machine(value: &Value) -> Result<Machine, ProviderError> {
        Ok(Machine {
            id: text(value, "sandboxID")?.into(),
            name: value["metadata"]["zuse-label"]
                .as_str()
                .unwrap_or("")
                .into(),
            state: match value["state"].as_str().unwrap_or("running") {
                "running" => MachineState::Running,
                "paused" => MachineState::Paused,
                "pausing" | "resuming" | "creating" => MachineState::Starting,
                other => MachineState::Unknown(other.into()),
            },
            vcpu: None,
            memory_bytes: None,
        })
    }
    fn remember(&self, value: &Value) -> Result<(), ProviderError> {
        let Some(token) = value["envdAccessToken"].as_str() else {
            return Ok(());
        };
        let id = text(value, "sandboxID")?;
        if id.len() > 128
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || token.is_empty()
            || token.len() > 16384
        {
            return Err(ProviderError::InvalidResponse);
        }
        let domain = value["domain"].as_str().unwrap_or("e2b.app");
        if domain != "e2b.app" && !domain.ends_with(".e2b.app") {
            return Err(ProviderError::InvalidResponse);
        }
        let url = Url::parse(&format!("https://49983-{id}.{domain}/"))
            .map_err(|_| ProviderError::InvalidResponse)?;
        if url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ProviderError::InvalidResponse);
        }
        let mut guests = self.guests.lock().map_err(|_| ProviderError::Unavailable)?;
        if guests.len() >= 256 {
            guests.clear();
        }
        guests.insert(
            id.into(),
            Guest {
                url,
                token: Zeroizing::new(token.into()),
                fetched: Instant::now(),
            },
        );
        Ok(())
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
    async fn guest(&self, id: &str) -> Result<Guest, ProviderError> {
        if let Some(guest) = self
            .guests
            .lock()
            .map_err(|_| ProviderError::Unavailable)?
            .get(id)
            .filter(|g| g.fetched.elapsed() < Duration::from_secs(60))
            .cloned()
        {
            return Ok(guest);
        }
        let value = self.call(Method::GET, &["sandboxes", id], None).await?;
        if text(&value, "sandboxID")? != id || value["state"] != "running" {
            return Err(ProviderError::Conflict);
        }
        self.remember(&value)?;
        self.guests
            .lock()
            .map_err(|_| ProviderError::Unavailable)?
            .get(id)
            .cloned()
            .ok_or(ProviderError::Unavailable)
    }
    fn guest_request(
        &self,
        id: &str,
        guest: &Guest,
        method: Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        let url = guest.url.join(path).map_err(|_| ProviderError::Rejected)?;
        if url.origin() != guest.url.origin() {
            return Err(ProviderError::Rejected);
        }
        Ok(self
            .rest
            .http
            .request(method, url)
            .header("X-Access-Token", guest.token.as_str())
            .header("E2b-Sandbox-Id", id)
            .header("E2b-Sandbox-Port", "49983")
            .header("Authorization", "Basic cm9vdDo="))
    }
    pub async fn snapshot(&self, id: &str, name: &str) -> Result<String, ProviderError> {
        let value = self
            .call(
                Method::POST,
                &["sandboxes", id, "snapshots"],
                Some(&json!({"name":name})),
            )
            .await?;
        text(&value, "snapshotID")
            .map(str::to_owned)
            .map_err(|_| ProviderError::OutcomeUnknown)
    }
}
#[async_trait]
impl Provider for E2b {
    fn id(&self) -> ProviderId {
        ProviderId::E2b
    }
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        match self.call(Method::GET, &["sandboxes", id], None).await {
            Ok(value) => {
                let machine = Self::machine(&value)?;
                if machine.id != id {
                    return Err(ProviderError::InvalidResponse);
                }
                self.remember(&value)?;
                Ok(Some(machine))
            }
            Err(ProviderError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
    async fn find_allocation(&self, name: &str) -> Result<Option<Machine>, ProviderError> {
        let filter = Url::parse_with_params("https://metadata.invalid", &[("zuse-label", name)])
            .map_err(|_| ProviderError::Rejected)?
            .query()
            .unwrap_or("")
            .to_owned();
        let mut url = self.rest.route(&["v2", "sandboxes"])?;
        url.query_pairs_mut()
            .append_pair("metadata", &filter)
            .append_pair("limit", "100")
            .append_pair("state", "running,paused");
        let value = self
            .rest
            .request(Method::GET, url, None, None, None)
            .await?;
        let list = value.as_array().ok_or(ProviderError::InvalidResponse)?;
        if list.len() >= 100 {
            return Err(ProviderError::Unavailable);
        }
        let mut found = None;
        for value in list.iter().filter(|v| v["metadata"]["zuse-label"] == name) {
            if found.is_some() {
                return Err(ProviderError::Conflict);
            }
            self.remember(value)?;
            found = Some(Self::machine(value)?);
        }
        Ok(found)
    }
    async fn verify_artifact(
        &self,
        artifact: &str,
        binding: Option<&super::artifact::ArtifactBinding>,
    ) -> Result<(), ProviderError> {
        let Some(binding) = binding else {
            return Ok(());
        };
        binding.validate(ProviderId::E2b, artifact)?;
        let mut url = self.rest.route(&["templates", artifact])?;
        url.query_pairs_mut().append_pair("limit", "2");
        let value = self
            .rest
            .request(Method::GET, url, None, None, None)
            .await?;
        if value["templateID"] != artifact
            || value["buildID"] != binding.artifact_version
            || value["buildCount"] != 1
            || value["buildStatus"] != "ready"
            || value["public"] != false
            || !value["names"]
                .as_array()
                .into_iter()
                .flatten()
                .chain(value["aliases"].as_array().into_iter().flatten())
                .filter_map(Value::as_str)
                .any(|name| {
                    name == binding.artifact_name
                        || name.ends_with(&format!("/{}", binding.artifact_name))
                })
        {
            return Err(ProviderError::Rejected);
        }
        Ok(())
    }
    async fn allocate(&self, request: &Allocation<'_>) -> Result<Machine, ProviderError> {
        request.validate()?;
        if request.size != "default" {
            return Err(ProviderError::Rejected);
        }
        self.verify_artifact(request.artifact, request.artifact_binding)
            .await?;
        let value = self
            .call(
                Method::POST,
                &["sandboxes"],
                Some(&json!({"templateID":request.artifact,
            "timeout":request.lifetime_seconds.min(self.max_timeout_seconds),"secure":true,"autoPause":true,"envVars":{},
            "metadata":{"zuse-label":request.name,"zuse-operation-id":request.operation_id}})),
            )
            .await?;
        self.verify_artifact(request.artifact, request.artifact_binding)
            .await
            .map_err(|_| ProviderError::OutcomeUnknown)?;
        self.remember(&value)
            .map_err(|_| ProviderError::OutcomeUnknown)?;
        // Creation returns connection metadata and may omit the submitted name.
        // Verify the original resource before returning it to the controller;
        // a missing/failed readback is an uncertain create, never a new create.
        let id = text(&value, "sandboxID").map_err(|_| ProviderError::OutcomeUnknown)?;
        let detail = self
            .call(Method::GET, &["sandboxes", id], None)
            .await
            .map_err(|_| ProviderError::OutcomeUnknown)?;
        if detail["sandboxID"] != id
            || detail["metadata"]["zuse-label"] != request.name
            || detail["metadata"]["zuse-operation-id"] != request.operation_id
        {
            return Err(ProviderError::OutcomeUnknown);
        }
        self.remember(&detail)
            .map_err(|_| ProviderError::OutcomeUnknown)?;
        Self::machine(&detail).map_err(|_| ProviderError::OutcomeUnknown)
    }
    async fn sleep(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.call(
            Method::POST,
            &["sandboxes", id, "pause"],
            Some(&json!({"memory":true})),
        )
        .await?;
        self.guests
            .lock()
            .map_err(|_| ProviderError::Unavailable)?
            .remove(id);
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
            desired: DesiredState::Sleeping,
        })
    }
    async fn sleep_memory(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.sleep(id).await
    }
    async fn memory_operation(
        &self,
        receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        if receipt.desired != DesiredState::Sleeping {
            return Err(ProviderError::InvalidResponse);
        }
        self.operation(receipt).await
    }
    async fn resume(
        &self,
        id: &str,
        lifetime_seconds: u32,
    ) -> Result<ProviderOperation, ProviderError> {
        if !(60..=86400).contains(&lifetime_seconds) {
            return Err(ProviderError::Rejected);
        }
        self.guests
            .lock()
            .map_err(|_| ProviderError::Unavailable)?
            .remove(id);
        let value = self
            .call(
                Method::POST,
                &["sandboxes", id, "connect"],
                Some(&json!({"timeout":lifetime_seconds.min(self.max_timeout_seconds),"autoPause":true})),
            )
            .await?;
        if text(&value, "sandboxID").map_err(|_| ProviderError::OutcomeUnknown)? != id {
            return Err(ProviderError::OutcomeUnknown);
        }
        self.remember(&value)
            .map_err(|_| ProviderError::OutcomeUnknown)?;
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
            desired: DesiredState::Running,
        })
    }
    /// E2B pauses a computer when its timeout ends, even mid-turn, and plans
    /// cap the timeout (one hour on the staging account). Resetting it while
    /// the computer is in use keeps long runs going.
    async fn keep_running(&self, id: &str) -> Result<(), ProviderError> {
        self.call(
            Method::POST,
            &["sandboxes", id, "timeout"],
            Some(&json!({"timeout":self.max_timeout_seconds})),
        )
        .await
        .map(|_| ())
    }
    async fn delete(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        match self.call(Method::DELETE, &["sandboxes", id], None).await {
            Ok(_) | Err(ProviderError::NotFound) => {}
            Err(error) => return Err(error),
        }
        self.guests
            .lock()
            .map_err(|_| ProviderError::Unavailable)?
            .remove(id);
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
        let guest = self.guest(id).await?;
        let payload=json!({"process":{"cmd":"/bin/bash","args":["--noprofile","--norc","-c",command],"envs":{}},"stdin":false}).to_string().into_bytes();
        let mut body = vec![0];
        body.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        body.extend(payload);
        let response = self
            .guest_request(id, &guest, Method::POST, "process.Process/Start")?
            .header("Connect-Protocol-Version", "1")
            .header("Content-Type", "application/connect+json")
            .timeout(Duration::from_secs(60))
            .body(body)
            .send()
            .await
            .map_err(|_| ProviderError::OutcomeUnknown)?;
        if !response.status().is_success() {
            return Err(ProviderError::OutcomeUnknown);
        }
        read_process(response).await
    }
    async fn upload(&self, id: &str, path: &str, contents: &[u8]) -> Result<(), ProviderError> {
        if contents.len() > 1024 * 1024
            || !path.starts_with('/')
            || path.contains(['\r', '\n', '\0'])
        {
            return Err(ProviderError::Rejected);
        }
        let guest = self.guest(id).await?;
        let boundary = format!("zuse-{}", uuid::Uuid::new_v4().simple());
        let mut body=format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"upload\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
        body.extend_from_slice(contents);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let response = self
            .guest_request(id, &guest, Method::POST, "files")?
            .query(&[("path", path), ("username", "root")])
            .header(
                "Content-Type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
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
}

#[derive(Default)]
struct ProcessDecoder {
    buffer: Vec<u8>,
    started: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
impl ProcessDecoder {
    fn push(&mut self, bytes: &[u8]) -> Result<Option<ExecResult>, ProviderError> {
        if self.buffer.len() + bytes.len() > 2 * 1024 * 1024 {
            return Err(ProviderError::OutcomeUnknown);
        }
        self.buffer.extend_from_slice(bytes);
        while self.buffer.len() >= 5 {
            let length = u32::from_be_bytes(self.buffer[1..5].try_into().unwrap()) as usize;
            if length > 1024 * 1024 || self.buffer[0] != 0 {
                return Err(ProviderError::OutcomeUnknown);
            }
            if self.buffer.len() < length + 5 {
                break;
            }
            let value: Value = serde_json::from_slice(&self.buffer[5..length + 5])
                .map_err(|_| ProviderError::OutcomeUnknown)?;
            self.buffer.drain(..length + 5);
            let event = &value["event"];
            if !self.started {
                if !event["start"]["pid"].as_u64().is_some_and(|pid| pid > 0) {
                    return Err(ProviderError::OutcomeUnknown);
                }
                self.started = true;
            }
            for (name, output) in [("stdout", &mut self.stdout), ("stderr", &mut self.stderr)] {
                if let Some(encoded) = event["data"][name].as_str() {
                    let part = STANDARD
                        .decode(encoded)
                        .map_err(|_| ProviderError::OutcomeUnknown)?;
                    if output.len() + part.len() > 1024 * 1024 {
                        return Err(ProviderError::OutcomeUnknown);
                    }
                    output.extend(part);
                }
            }
            if event.get("end").is_some() {
                if event["end"]["exited"] != true {
                    return Err(ProviderError::OutcomeUnknown);
                }
                // Connect/proto JSON omits a zero-valued exit code.
                let exit_code = match event["end"].get("exitCode") {
                    None | Some(Value::Null) => 0,
                    Some(value) => value
                        .as_i64()
                        .and_then(|v| i32::try_from(v).ok())
                        .ok_or(ProviderError::OutcomeUnknown)?,
                };
                return Ok(Some(ExecResult {
                    stdout: std::mem::take(&mut self.stdout),
                    stderr: std::mem::take(&mut self.stderr),
                    exit_code,
                }));
            }
        }
        Ok(None)
    }
}
async fn read_process(mut response: reqwest::Response) -> Result<ExecResult, ProviderError> {
    let mut decoder = ProcessDecoder::default();
    let mut total = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ProviderError::OutcomeUnknown)?
    {
        total += chunk.len();
        if total > 8 * 1024 * 1024 {
            return Err(ProviderError::OutcomeUnknown);
        }
        if let Some(result) = decoder.push(&chunk)? {
            return Ok(result);
        }
    }
    // A network disconnect is never an execution receipt.
    Err(ProviderError::OutcomeUnknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        body::{Body, Bytes},
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[tokio::test]
    async fn keeping_a_busy_computer_running_resets_its_timeout_to_the_plan_limit() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let app = Router::new().route(
            "/sandboxes/original-one/timeout",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let seen = seen.clone();
                async move {
                    assert_eq!(headers["x-api-key"], "infrastructure-key");
                    seen.lock().unwrap().push(body);
                    StatusCode::NO_CONTENT
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        for (configured, expected) in [(None, 3600), (Some(1200), 1200)] {
            let mut provider = E2b::new("infrastructure-key".into()).unwrap();
            if let Some(seconds) = configured {
                provider = provider.with_max_timeout(seconds).unwrap();
            }
            provider.rest.test_endpoint(endpoint.clone());
            provider.keep_running("original-one").await.unwrap();
            assert_eq!(
                requests.lock().unwrap().pop().unwrap(),
                json!({"timeout":expected})
            );
            assert_eq!(
                provider.keep_running("missing").await,
                Err(ProviderError::NotFound)
            );
        }
    }

    #[tokio::test]
    async fn allocation_and_every_resume_respect_the_deployment_timeout_limit() {
        for (configured, requested, expected) in [
            (None, 86400, 3600),
            (None, 600, 600),
            (Some(7200), 86400, 7200),
            (Some(1200), 3600, 1200),
        ] {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let create_requests = requests.clone();
            let resume_requests = requests.clone();
            let app = Router::new()
                .route("/sandboxes", post(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let requests = create_requests.clone();
                    async move {
                        assert_eq!(headers["x-api-key"], "infrastructure-key");
                        requests.lock().unwrap().push(body);
                        Json(json!({"sandboxID":"original-one","state":"running"}))
                    }
                }))
                .route("/sandboxes/original-one", get(|| async {
                    Json(json!({"sandboxID":"original-one","state":"running","metadata":{"zuse-label":"task-one","zuse-operation-id":"operation-one"}}))
                }))
                .route("/sandboxes/original-one/connect", post(move |Json(body): Json<Value>| {
                    let requests = resume_requests.clone();
                    async move {
                        requests.lock().unwrap().push(body);
                        Json(json!({"sandboxID":"original-one","state":"running"}))
                    }
                }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            }));
            let mut provider = E2b::new("infrastructure-key".into()).unwrap();
            if let Some(seconds) = configured {
                provider = provider.with_max_timeout(seconds).unwrap();
            }
            provider.rest.test_endpoint(endpoint);
            let allocated = provider
                .allocate(&Allocation {
                    artifact: "template-one",
                    artifact_binding: None,
                    name: "task-one",
                    operation_id: "operation-one",
                    size: "default",
                    lifetime_seconds: requested,
                })
                .await
                .unwrap();
            assert_eq!(allocated.id, "original-one");
            provider.resume(&allocated.id, requested).await.unwrap();
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            for request in requests.iter() {
                assert_eq!(request["timeout"], expected);
                assert_eq!(request["autoPause"], true);
            }
        }
    }
    #[tokio::test]
    async fn operational_version_fence_checks_before_and_after_the_only_create() {
        for scenario in [
            "success",
            "prechange",
            "postchange",
            "public",
            "name",
            "build-count",
            "not-ready",
            "different-resource",
            "different-name",
            "different-operation",
            "missing-detail",
            "detail-error",
        ] {
            let reads = Arc::new(AtomicUsize::new(0));
            let creates = Arc::new(AtomicUsize::new(0));
            let get_count = reads.clone();
            let create_count = creates.clone();
            let app=Router::new().route("/templates/template-one",get(move||{let n=get_count.fetch_add(1,Ordering::SeqCst);async move{Json(json!({"templateID":"template-one","buildID":if scenario=="prechange"||(scenario=="postchange"&&n>0){"changed"}else{"build-one"},"buildCount":if scenario=="build-count"{2}else{1},"buildStatus":if scenario=="not-ready"{"building"}else{"ready"},"public":scenario=="public","names":[if scenario=="name"{"other"}else{"team/zuse-e2b-stage-one"}]}))}}))
                .route("/sandboxes",post(move||{create_count.fetch_add(1,Ordering::SeqCst);async{Json(json!({"sandboxID":"sandbox-one","templateID":"template-one"}))}}))
                .route("/sandboxes/sandbox-one",get(move||async move{
                    if scenario=="missing-detail" { return StatusCode::NOT_FOUND.into_response(); }
                    if scenario=="detail-error" { return StatusCode::SERVICE_UNAVAILABLE.into_response(); }
                    Json(json!({"sandboxID":if scenario=="different-resource"{"other"}else{"sandbox-one"},"state":"running","metadata":{"zuse-label":if scenario=="different-name"{"other"}else{"task-one"},"zuse-operation-id":if scenario=="different-operation"{"other"}else{"operation-one"}}})).into_response()
                }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap()
            }));
            let mut provider = E2b::new("infrastructure-only".into()).unwrap();
            provider.rest.test_endpoint(endpoint);
            let binding = super::super::artifact::ArtifactBinding {
                version: 1,
                mode: "private-single-build".into(),
                artifact_id: "template-one".into(),
                artifact_version: "build-one".into(),
                artifact_name: "zuse-e2b-stage-one".into(),
                runtime_sha256: "a".repeat(64),
                manifest_sha256: "b".repeat(64),
            };
            let result = provider
                .allocate(&Allocation {
                    artifact: "template-one",
                    artifact_binding: Some(&binding),
                    name: "task-one",
                    operation_id: "operation-one",
                    size: "default",
                    lifetime_seconds: 600,
                })
                .await;
            if scenario == "success" {
                assert!(result.is_ok());
                assert_eq!(creates.load(Ordering::SeqCst), 1);
                assert_eq!(reads.load(Ordering::SeqCst), 2)
            } else if matches!(
                scenario,
                "postchange"
                    | "different-resource"
                    | "different-name"
                    | "different-operation"
                    | "missing-detail"
                    | "detail-error"
            ) {
                assert_eq!(result.unwrap_err(), ProviderError::OutcomeUnknown);
                assert_eq!(creates.load(Ordering::SeqCst), 1)
            } else {
                assert_eq!(result.unwrap_err(), ProviderError::Rejected);
                assert_eq!(creates.load(Ordering::SeqCst), 0)
            }
        }
    }
    fn frame(value: Value) -> Vec<u8> {
        let bytes = serde_json::to_vec(&value).unwrap();
        let mut frame = vec![0];
        frame.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        frame.extend(bytes);
        frame
    }
    #[test]
    fn fragmented_process_receipts_preserve_stderr_and_do_not_require_a_stream_close() {
        let mut bytes = frame(json!({"event":{"start":{"pid":42}}}));
        bytes.extend(frame(json!({"event":{"data":{"stdout":STANDARD.encode(b"output"),"stderr":STANDARD.encode(b"diagnostic")}}})));
        bytes.extend(frame(json!({"event":{"end":{"exited":true}}})));
        let mut decoder = ProcessDecoder::default();
        let mut result = None;
        for chunk in bytes.chunks(3) {
            if let Some(done) = decoder.push(chunk).unwrap() {
                result = Some(done);
            }
        }
        let result = result.unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, b"output");
        assert_eq!(result.stderr, b"diagnostic");
        assert!(
            ProcessDecoder::default()
                .push(&frame(json!({"event":{"end":{"exited":true}}})))
                .is_err()
        );
        assert!(ProcessDecoder::default().push(&[0, 0, 32, 0, 1]).is_err());
    }
    #[tokio::test]
    async fn guest_commands_use_scoped_envd_tokens_and_paused_inspection_never_wakes() {
        let connects = Arc::new(AtomicUsize::new(0));
        let count = connects.clone();
        let detail = json!({"sandboxID":"sandbox-one","state":"paused","envdAccessToken":"envd-only-token","domain":"e2b.app","metadata":{"zuse-label":"task-one"}});
        let app=Router::new().route("/sandboxes/sandbox-one",get(move |headers:HeaderMap|{let value=detail.clone();async move{
            assert_eq!(headers["x-api-key"],"infrastructure-key");Json(value)
        }})).route("/sandboxes/sandbox-one/connect",post(move||{count.fetch_add(1,Ordering::SeqCst);async{Json(json!({"sandboxID":"sandbox-one","envdAccessToken":"envd-only-token","domain":"e2b.app"}))}}))
        .route("/process.Process/Start",post(|headers:HeaderMap|async move{
            assert_eq!(headers["x-access-token"],"envd-only-token");assert_eq!(headers["authorization"],"Basic cm9vdDo=");assert!(!headers.contains_key("x-api-key"));
            let mut bytes=frame(json!({"event":{"start":{"pid":42}}}));bytes.extend(frame(json!({"event":{"end":{"exited":true,"exitCode":0}}})));
            use futures::StreamExt;
            // The exit receipt must be consumed immediately even if a proxy
            // keeps the HTTP response open indefinitely after that frame.
            let stream=futures::stream::iter(bytes.chunks(3).map(|c|Ok::<_,std::io::Error>(Bytes::copy_from_slice(c))).collect::<Vec<_>>()).chain(futures::stream::pending());
            Response::builder().header("content-type","application/connect+json").body(Body::from_stream(stream)).unwrap()
        })).route("/files",post(|headers:HeaderMap,body:Bytes|async move{
            assert_eq!(headers["x-access-token"],"envd-only-token");assert!(!headers.contains_key("x-api-key"));
            assert!(body.windows(3).any(|b|b==[0,255,10]));StatusCode::NO_CONTENT
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        let mut provider = E2b::new("infrastructure-key".into()).unwrap();
        provider.rest.test_endpoint(endpoint.clone());
        assert_eq!(
            provider
                .inspect("sandbox-one")
                .await
                .unwrap()
                .unwrap()
                .state,
            MachineState::Paused
        );
        assert_eq!(connects.load(Ordering::SeqCst), 0);
        provider.resume("sandbox-one", 600).await.unwrap();
        assert_eq!(connects.load(Ordering::SeqCst), 1);
        // Test-only loopback envd endpoint; production derives/validates an E2B domain.
        provider.guests.lock().unwrap().insert(
            "sandbox-one".into(),
            Guest {
                url: Url::parse(&endpoint).unwrap(),
                token: Zeroizing::new("envd-only-token".into()),
                fetched: Instant::now(),
            },
        );
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            provider.execute("sandbox-one", "true"),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.exit_code, 0);
        provider
            .upload("sandbox-one", "/tmp/bootstrap", &[0, 255, 10])
            .await
            .unwrap();
        drop(server);
    }
}
