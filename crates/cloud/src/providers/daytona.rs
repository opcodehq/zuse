//! Daytona v0 API, pinned snapshot class and persistent lifecycle policy.
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

#[derive(Clone, Copy)]
pub enum SandboxClass {
    Container,
    LinuxVm,
}
impl SandboxClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::LinuxVm => "linux-vm",
        }
    }
}
pub struct Daytona {
    pub(super) rest: Rest,
    class: SandboxClass,
    target: Option<String>,
    toolbox: Mutex<HashMap<String, (Url, Instant)>>,
    pub(super) toolbox_origins: Vec<String>,
}
impl Daytona {
    pub fn new(
        api_key: String,
        class: SandboxClass,
        target: Option<String>,
    ) -> Result<Self, ProviderError> {
        if target.as_ref().is_some_and(|s| {
            s.is_empty()
                || s.len() > 100
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        }) {
            return Err(ProviderError::Rejected);
        }
        Ok(Self {
            rest: Rest::new("https://app.daytona.io/api", api_key, "Authorization", true)?,
            class,
            target,
            toolbox: Mutex::new(HashMap::new()),
            toolbox_origins: vec![
                "https://proxy.app.daytona.io".into(),
                "https://toolbox.daytona.com".into(),
                "https://toolbox.daytona.io".into(),
            ],
        })
    }
    fn machine(&self, value: &Value) -> Result<Machine, ProviderError> {
        if value["sandboxClass"] != self.class.as_str() {
            return Err(ProviderError::Rejected);
        }
        if value["autoDeleteInterval"].as_i64().is_none_or(|n| n >= 0)
            || value["ttlMinutes"].as_i64().is_some_and(|n| n > 0)
        {
            return Err(ProviderError::Rejected);
        }
        Ok(Machine {
            id: text(value, "id")?.into(),
            name: text(value, "name")?.into(),
            state: match text(value, "state")? {
                "started" => MachineState::Running,
                "stopped" | "archived" => MachineState::Stopped,
                "paused" => MachineState::Paused,
                "creating" | "restoring" | "starting" | "stopping" | "pending_build"
                | "building_snapshot" | "pulling_snapshot" | "archiving" | "pausing"
                | "resuming" => MachineState::Starting,
                other => MachineState::Unknown(other.into()),
            },
            vcpu: value["cpu"].as_u64().and_then(|n| u32::try_from(n).ok()),
            memory_bytes: value["memory"]
                .as_u64()
                .and_then(|n| n.checked_mul(1024 * 1024 * 1024)),
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
    fn remember(&self, id: &str, url: &str) -> Result<Url, ProviderError> {
        let parsed = Url::parse(url).map_err(|_| ProviderError::InvalidResponse)?;
        if parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path().trim_end_matches('/') != "/toolbox"
        {
            return Err(ProviderError::InvalidResponse);
        }
        // Validate before caching, and again before sending infrastructure credentials.
        let _ = self.rest.trusted_request(
            Method::GET,
            parsed.clone(),
            &self
                .toolbox_origins
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        )?;
        let mut cache = self
            .toolbox
            .lock()
            .map_err(|_| ProviderError::Unavailable)?;
        if cache.len() >= 256 {
            cache.clear();
        }
        cache.insert(id.into(), (parsed.clone(), Instant::now()));
        Ok(parsed)
    }
    async fn toolbox(&self, id: &str, segments: &[&str]) -> Result<Url, ProviderError> {
        let cached = self
            .toolbox
            .lock()
            .map_err(|_| ProviderError::Unavailable)?
            .get(id)
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(60))
            .map(|(url, _)| url.clone());
        let mut url = if let Some(url) = cached {
            url
        } else {
            let value = self
                .call(Method::GET, &["sandbox", id, "toolbox-proxy-url"], None)
                .await?;
            self.remember(id, text(&value, "url")?)?
        };
        if id.is_empty()
            || id.len() > 200
            || id.chars().any(char::is_control)
            || id == "."
            || id == ".."
        {
            return Err(ProviderError::Rejected);
        }
        url.path_segments_mut()
            .map_err(|_| ProviderError::Rejected)?
            .pop_if_empty()
            .push(id)
            .extend(segments);
        Ok(url)
    }
    /// Called only for a stopped, credential-free preparation computer.
    pub async fn snapshot_prepared(&self, id: &str, name: &str) -> Result<String, ProviderError> {
        let machine = self.inspect(id).await?.ok_or(ProviderError::NotFound)?;
        if machine.state != MachineState::Stopped {
            return Err(ProviderError::Conflict);
        }
        self.call(
            Method::POST,
            &["sandbox", id, "snapshot"],
            Some(&json!({"name":name})),
        )
        .await?;
        Ok(name.into())
    }
}
#[async_trait]
impl Provider for Daytona {
    fn id(&self) -> ProviderId {
        ProviderId::Daytona
    }
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        match self.call(Method::GET, &["sandbox", id], None).await {
            Ok(value) => {
                if text(&value, "id")? != id {
                    return Err(ProviderError::InvalidResponse);
                }
                if value["state"] == "destroyed" {
                    return Ok(None);
                }
                let machine = self.machine(&value)?;
                if let Some(url) = value["toolboxProxyUrl"].as_str() {
                    self.remember(id, url)?;
                }
                Ok(Some(machine))
            }
            Err(ProviderError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
    async fn find_allocation(&self, name: &str) -> Result<Option<Machine>, ProviderError> {
        // The exact name endpoint avoids the eventually consistent listing API.
        match self.call(Method::GET, &["sandbox", name], None).await {
            Ok(value) => {
                if text(&value, "name")? != name {
                    return Err(ProviderError::InvalidResponse);
                }
                if value["state"] == "destroyed" {
                    return Ok(None);
                }
                Ok(Some(self.machine(&value)?))
            }
            Err(ProviderError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
    async fn allocate(&self, request: &Allocation<'_>) -> Result<Machine, ProviderError> {
        request.validate()?;
        self.verify_artifact(request.artifact, request.artifact_binding)
            .await?;
        if request.size != "default" {
            return Err(ProviderError::Rejected);
        }
        let mut body = json!({"name":request.name,"snapshot":request.artifact,"user":"root","env":{},"secrets":[],
            "labels":{"zuse-label":request.name,"zuse-operation-id":request.operation_id},"public":false,
            "autoStopInterval":request.lifetime_seconds.div_ceil(60),"autoPauseInterval":0,"autoDeleteInterval":-1,"ttlMinutes":0});
        if let Some(target) = &self.target {
            body["target"] = json!(target);
        }
        let value = self.call(Method::POST, &["sandbox"], Some(&body)).await?;
        let machine = self
            .machine(&value)
            .map_err(|_| ProviderError::OutcomeUnknown)?;
        if value["autoDeleteInterval"] != -1 || value["ttlMinutes"].as_i64().is_some_and(|n| n > 0)
        {
            return Err(ProviderError::OutcomeUnknown);
        }
        if let Some(url) = value["toolboxProxyUrl"].as_str() {
            self.remember(&machine.id, url)
                .map_err(|_| ProviderError::OutcomeUnknown)?;
        }
        Ok(machine)
    }
    async fn sleep(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.call(Method::POST, &["sandbox", id, "stop"], None)
            .await?;
        self.toolbox
            .lock()
            .map_err(|_| ProviderError::Unavailable)?
            .remove(id);
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
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
        // The retained sandbox keeps its original lifecycle policy and filesystem.
        self.toolbox
            .lock()
            .map_err(|_| ProviderError::Unavailable)?
            .remove(id);
        self.call(Method::POST, &["sandbox", id, "start"], None)
            .await?;
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
            desired: DesiredState::Running,
        })
    }
    async fn delete(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        match self.call(Method::DELETE, &["sandbox", id], None).await {
            Ok(_) | Err(ProviderError::NotFound) => {}
            Err(error) => return Err(error),
        }
        self.toolbox
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
        let url = self.toolbox(id, &["process", "execute"]).await?;
        let response = self
            .rest
            .trusted_request(
                Method::POST,
                url,
                &self
                    .toolbox_origins
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            )?
            .json(&json!({"command":command,"timeout":60}))
            .timeout(Duration::from_secs(70))
            .send()
            .await
            .map_err(|_| ProviderError::OutcomeUnknown)?;
        if !response.status().is_success() {
            return Err(ProviderError::OutcomeUnknown);
        }
        let bytes = read(response, 8 * 1024 * 1024)
            .await
            .map_err(|_| ProviderError::OutcomeUnknown)?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| ProviderError::OutcomeUnknown)?;
        let exit_code = value
            .get("exitCode")
            .filter(|value| !value.is_null())
            .or_else(|| value.get("code"))
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok())
            .ok_or(ProviderError::OutcomeUnknown)?;
        Ok(ExecResult {
            exit_code,
            stdout: value["result"].as_str().unwrap_or("").as_bytes().to_vec(),
            stderr: vec![],
        })
    }
    async fn upload(&self, id: &str, path: &str, contents: &[u8]) -> Result<(), ProviderError> {
        if contents.len() > 1024 * 1024
            || !path.starts_with('/')
            || path.chars().any(char::is_control)
            || path.contains('"')
        {
            return Err(ProviderError::Rejected);
        }
        let url = self.toolbox(id, &["files", "bulk-upload"]).await?;
        let boundary = format!("zuse-{}", uuid::Uuid::new_v4().simple());
        let mut body=format!("--{boundary}\r\nContent-Disposition: form-data; name=\"files[0].path\"\r\n\r\n{path}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"files[0].file\"; filename=\"upload\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
        body.extend_from_slice(contents);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let response = self
            .rest
            .trusted_request(
                Method::POST,
                url,
                &self
                    .toolbox_origins
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            )?
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        body::Bytes,
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::{get, post},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    #[derive(Clone)]
    struct Fixture {
        endpoint: String,
        malicious: Arc<AtomicBool>,
        uploads: Arc<AtomicUsize>,
        commands: Arc<AtomicUsize>,
    }
    fn auth(headers: &HeaderMap) {
        assert_eq!(headers["authorization"], "Bearer infrastructure-key");
    }
    fn detail(fixture: &Fixture) -> Value {
        json!({"id":"machine-one","name":"task-one","state":"started","sandboxClass":"container","autoDeleteInterval":-1,"ttlMinutes":0,"cpu":2,"memory":4,
        "toolboxProxyUrl":if fixture.malicious.load(Ordering::SeqCst){"https://attacker.invalid/toolbox".into()}else{format!("{}/toolbox",fixture.endpoint)}})
    }
    async fn create(
        State(fixture): State<Fixture>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        auth(&headers);
        assert_eq!(body["snapshot"], "managed-base");
        assert_eq!(body["autoDeleteInterval"], -1);
        assert_eq!(body["ttlMinutes"], 0);
        assert_eq!(body["secrets"], json!([]));
        Json(detail(&fixture))
    }
    async fn inspect(State(fixture): State<Fixture>, headers: HeaderMap) -> Json<Value> {
        auth(&headers);
        Json(detail(&fixture))
    }
    async fn execute(
        State(fixture): State<Fixture>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        auth(&headers);
        fixture.commands.fetch_add(1, Ordering::SeqCst);
        assert_eq!(body["command"], "printf ready");
        Json(json!({"exitCode":0,"result":"ready"}))
    }
    async fn upload(State(fixture): State<Fixture>, headers: HeaderMap, body: Bytes) -> StatusCode {
        auth(&headers);
        fixture.uploads.fetch_add(1, Ordering::SeqCst);
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("files[0].path"));
        assert!(body.contains("/tmp/bootstrap"));
        assert!(body.contains("private-bootstrap-fixture"));
        StatusCode::NO_CONTENT
    }
    #[tokio::test]
    async fn persistent_class_and_trusted_toolbox_are_required_and_secrets_use_file_uploads() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let fixture = Fixture {
            endpoint: endpoint.clone(),
            malicious: Arc::new(AtomicBool::new(false)),
            uploads: Arc::new(AtomicUsize::new(0)),
            commands: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route("/sandbox", post(create))
            .route("/sandbox/machine-one", get(inspect))
            .route("/sandbox/task-one", get(inspect))
            .route("/toolbox/machine-one/process/execute", post(execute))
            .route("/toolbox/machine-one/files/bulk-upload", post(upload))
            .with_state(fixture.clone());
        let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        let mut provider =
            Daytona::new("infrastructure-key".into(), SandboxClass::Container, None).unwrap();
        provider.rest.test_endpoint(endpoint.clone());
        provider.toolbox_origins = vec![endpoint.clone()];
        let request = Allocation {
            artifact: "managed-base",
            artifact_binding: None,
            name: "task-one",
            operation_id: "operation-one",
            size: "default",
            lifetime_seconds: 600,
        };
        let machine = provider.allocate(&request).await.unwrap();
        assert_eq!(machine.id, "machine-one");
        assert_eq!(
            provider
                .find_allocation("task-one")
                .await
                .unwrap()
                .unwrap()
                .id,
            "machine-one"
        );
        provider
            .upload(&machine.id, "/tmp/bootstrap", b"private-bootstrap-fixture")
            .await
            .unwrap();
        assert_eq!(fixture.commands.load(Ordering::SeqCst), 0);
        assert_eq!(
            provider
                .execute(&machine.id, "printf ready")
                .await
                .unwrap()
                .stdout,
            b"ready"
        );
        assert_eq!(fixture.uploads.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.commands.load(Ordering::SeqCst), 1);
        fixture.malicious.store(true, Ordering::SeqCst);
        assert_eq!(
            provider.allocate(&request).await.unwrap_err(),
            ProviderError::OutcomeUnknown
        );
        let mut wrong_class =
            Daytona::new("infrastructure-key".into(), SandboxClass::LinuxVm, None).unwrap();
        wrong_class.rest.test_endpoint(endpoint);
        assert_eq!(
            wrong_class.allocate(&request).await.unwrap_err(),
            ProviderError::OutcomeUnknown
        );
        drop(server);
    }
}
