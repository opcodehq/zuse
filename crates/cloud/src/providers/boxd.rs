//! Direct Boxd gRPC-web adapter, compatible with SDK 0.2.14.
//! No mutation retries: allocation/command outcomes belong to the leased reconciler.
use super::{ExecResult, Machine, MachineState, ProviderError, boxd_wire as wire};
use futures::StreamExt;
use prost::Message;
use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

const SERVICE: &str = "boxd.api.v1.BoxdApi";
const MAX_RESPONSE: usize = 8 * 1024 * 1024;

struct Token {
    value: String,
    expires_at: u64,
}

/// Infrastructure credentials stay server-side and are deliberately not Debug/Serialize.
pub struct Boxd {
    http: reqwest::Client,
    api_key: String,
    organization: String,
    endpoint: String,
    exchange_url: String,
    token: Mutex<Option<Token>>,
}

#[derive(Clone, Debug)]
pub struct Usage {
    pub machine_id: String,
    pub vcpu: u32,
    pub memory_bytes: u64,
    pub cost_micros: i64,
    pub complete: bool,
}

impl Boxd {
    /// SDK-compatible cluster override; infrastructure secrets never follow redirects.
    pub fn with_base_url(mut self, base: &str) -> Result<Self, ProviderError> {
        let mut url = reqwest::Url::parse(base).map_err(|_| ProviderError::Rejected)?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ProviderError::Rejected);
        }
        self.endpoint = url.as_str().trim_end_matches('/').to_owned();
        let host = url.host_str().ok_or(ProviderError::Rejected)?.to_owned();
        let single = host == "localhost"
            || host.starts_with("app.")
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok();
        if !single {
            url.set_host(Some(&format!("app.{host}")))
                .map_err(|_| ProviderError::Rejected)?;
            url.set_port(None).map_err(|_| ProviderError::Rejected)?;
        }
        url.set_path("/api/v1/auth/token");
        self.exchange_url = url.into();
        Ok(self)
    }

    pub async fn organization_usage(
        &self,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<Vec<Usage>, ProviderError> {
        let (since, until) = usage_bounds(start_ms, end_ms)?;
        let report: wire::OrgUsageResponse = self
            .unary(
                "GetOrgUsage",
                wire::OrgUsageRequest {
                    org: self.organization.clone(),
                    since,
                    until,
                },
                false,
            )
            .await?;
        validate_usage_period(
            report.period_start,
            report.period_end,
            &report.currency,
            since,
            until,
        )?;
        if report.total_cost_micro < 0 {
            return Err(ProviderError::InvalidResponse);
        }
        report.vms.into_iter().map(usage).collect()
    }

    pub async fn machine_usage(
        &self,
        id: &str,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<Usage, ProviderError> {
        machine_id(id)?;
        let (since, until) = usage_bounds(start_ms, end_ms)?;
        let report: wire::VmUsageResponse = self
            .unary(
                "GetVmUsage",
                wire::VmUsageRequest {
                    vm_id: id.to_owned(),
                    org: self.organization.clone(),
                    since,
                    until,
                },
                false,
            )
            .await?;
        validate_usage_period(
            report.period_start,
            report.period_end,
            &report.currency,
            since,
            until,
        )?;
        let report = usage(report.usage.ok_or(ProviderError::InvalidResponse)?)?;
        if report.machine_id != id || !report.complete {
            return Err(ProviderError::InvalidResponse);
        }
        Ok(report)
    }

    #[cfg(test)]
    pub(crate) fn set_test_endpoint(&mut self, endpoint: String) {
        self.exchange_url = format!("{endpoint}/token");
        self.endpoint = endpoint;
    }

    pub fn new(api_key: String, organization: Option<String>) -> Result<Self, ProviderError> {
        if api_key.is_empty() || api_key.len() > 4096 {
            return Err(ProviderError::Rejected);
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| ProviderError::Unavailable)?,
            api_key,
            organization: organization.unwrap_or_default(),
            endpoint: "https://boxd.sh:9443".into(),
            exchange_url: "https://app.boxd.sh/api/v1/auth/token".into(),
            token: Mutex::new(None),
        })
    }

    async fn token(&self) -> Result<String, ProviderError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ProviderError::Unavailable)?
            .as_secs();
        let mut cached = self.token.lock().await;
        if let Some(token) = cached
            .as_ref()
            .filter(|token| token.expires_at > now.saturating_add(300))
        {
            return Ok(token.value.clone());
        }
        let response = self
            .http
            .post(&self.exchange_url)
            .timeout(Duration::from_secs(15))
            .json(&serde_json::json!({"api_key":self.api_key}))
            .send()
            .await
            .map_err(|_| ProviderError::Unavailable)?;
        match response.status().as_u16() {
            200 => {}
            401 | 403 => return Err(ProviderError::Rejected),
            429 => return Err(ProviderError::RateLimited),
            _ => return Err(ProviderError::Unavailable),
        }
        #[derive(Deserialize)]
        struct Exchange {
            token: String,
            expires_at: u64,
        }
        let body = read_body(response, 64 * 1024).await?;
        let token: Exchange =
            serde_json::from_slice(&body).map_err(|_| ProviderError::InvalidResponse)?;
        if token.token.is_empty() || token.token.len() > 16384 || token.expires_at <= now {
            return Err(ProviderError::InvalidResponse);
        }
        let value = token.token.clone();
        *cached = Some(Token {
            value: token.token,
            expires_at: token.expires_at,
        });
        Ok(value)
    }

    async fn call(
        &self,
        method: &str,
        message: impl Message,
        mutation: bool,
    ) -> Result<Vec<Vec<u8>>, ProviderError> {
        let payload = message.encode_to_vec();
        if payload.len() > MAX_RESPONSE {
            return Err(ProviderError::Rejected);
        }
        let token = self.token().await?;
        let response = self
            .http
            .post(format!("{}/{SERVICE}/{method}", self.endpoint))
            .header("content-type", "application/grpc-web+proto")
            .header("x-grpc-web", "1")
            .bearer_auth(token)
            .body(frame(&payload))
            .send()
            .await
            .map_err(|_| {
                if mutation {
                    ProviderError::OutcomeUnknown
                } else {
                    ProviderError::Unavailable
                }
            })?;
        if let Some(status) = response.headers().get("grpc-status") {
            let status = status
                .to_str()
                .map_err(|_| ProviderError::InvalidResponse)?;
            check_status(
                status.parse().map_err(|_| ProviderError::InvalidResponse)?,
                mutation,
            )?;
        }
        match response.status().as_u16() {
            200 => {}
            401 | 403 => return Err(ProviderError::Rejected),
            429 => return Err(ProviderError::RateLimited),
            _ => {
                return Err(if mutation {
                    ProviderError::OutcomeUnknown
                } else {
                    ProviderError::Unavailable
                });
            }
        }
        let header_status = response
            .headers()
            .get("grpc-status")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok());
        let body = read_body(response, MAX_RESPONSE).await.map_err(|error| {
            if mutation
                && matches!(
                    error,
                    ProviderError::Unavailable | ProviderError::InvalidResponse
                )
            {
                ProviderError::OutcomeUnknown
            } else {
                error
            }
        })?;
        parse_frames(&body, header_status, mutation).map_err(|error| {
            if mutation && error == ProviderError::InvalidResponse {
                ProviderError::OutcomeUnknown
            } else {
                error
            }
        })
    }

    async fn unary<T: Message + Default>(
        &self,
        method: &str,
        request: impl Message,
        mutation: bool,
    ) -> Result<T, ProviderError> {
        let mut messages = self.call(method, request, mutation).await?;
        if messages.len() != 1 {
            return Err(if mutation {
                ProviderError::OutcomeUnknown
            } else {
                ProviderError::InvalidResponse
            });
        }
        T::decode(messages.pop().unwrap().as_slice()).map_err(|_| {
            if mutation {
                ProviderError::OutcomeUnknown
            } else {
                ProviderError::InvalidResponse
            }
        })
    }

    pub async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        match self
            .unary::<wire::MachineDetails>("GetVm", machine_id(id)?, false)
            .await
        {
            Err(ProviderError::NotFound) => Ok(None),
            Ok(machine) if !machine.vm_id.is_empty() => Ok(Some(Machine {
                id: machine.vm_id,
                name: machine.name,
                state: machine_state(machine.status),
                vcpu: (machine.vcpu > 0).then_some(machine.vcpu),
                memory_bytes: (machine.memory_bytes > 0).then_some(machine.memory_bytes),
            })),
            Ok(_) => Err(ProviderError::InvalidResponse),
            Err(error) => Err(error),
        }
    }

    /// A deterministic name lets the reconciler inspect an ambiguous create response.
    /// Snapshot sizing is retained; explicit resize is a separate provider operation.
    pub async fn create_from_snapshot(
        &self,
        snapshot: &str,
        name: &str,
        isolated: bool,
    ) -> Result<Machine, ProviderError> {
        if snapshot.is_empty()
            || snapshot.len() > 512
            || name.is_empty()
            || name.len() > 63
            || !name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        {
            return Err(ProviderError::Rejected);
        }
        let created: wire::CreatedMachine = self
            .unary(
                "CreateVmFromSnapshot",
                wire::CreateFromSnapshot {
                    snapshot: snapshot.into(),
                    name: name.into(),
                    config: Some(wire::VmConfig {
                        srf: Some(wire::SrfConfig {
                            auto_suspend_timeout_secs: Some(0),
                            auto_destroy_timeout_secs: 0,
                        }),
                    }),
                    org: self.organization.clone(),
                    isolated,
                },
                true,
            )
            .await?;
        if created.vm_id.is_empty() {
            return Err(ProviderError::InvalidResponse);
        }
        Ok(Machine {
            id: created.vm_id,
            name: created.name,
            state: machine_state(created.status),
            vcpu: None,
            memory_bytes: None,
        })
    }

    async fn action(&self, method: &str, id: &str) -> Result<(), ProviderError> {
        self.unary::<wire::Empty>(method, machine_id(id)?, true)
            .await
            .map(|_| ())
    }
    pub async fn start(&self, id: &str) -> Result<(), ProviderError> {
        self.action("StartVm", id).await
    }
    pub async fn hibernate(&self, id: &str) -> Result<(), ProviderError> {
        self.action("HibernateVm", id).await
    }
    pub async fn wake(&self, id: &str) -> Result<(), ProviderError> {
        self.action("WakeVm", id).await
    }
    pub async fn destroy(&self, id: &str) -> Result<(), ProviderError> {
        self.action("DestroyVm", id).await
    }
    pub async fn resume(&self, id: &str) -> Result<u64, ProviderError> {
        Ok(self
            .unary::<wire::ResumeResult>("ResumeVm", machine_id(id)?, true)
            .await?
            .resume_us)
    }
    pub async fn set_auto_hibernate(&self, id: &str, seconds: u32) -> Result<(), ProviderError> {
        if seconds > 2592000 {
            return Err(ProviderError::Rejected);
        }
        self.unary::<wire::Empty>(
            "SetAutoHibernateTimeout",
            wire::HibernateTimeout {
                vm_id: machine_id(id)?.vm_id,
                timeout_secs: seconds,
            },
            true,
        )
        .await
        .map(|_| ())
    }

    /// Bounded bootstrap/maintenance command. Interactive PTYs use guest RPC.
    pub async fn exec(&self, id: &str, command: &str) -> Result<ExecResult, ProviderError> {
        if command.is_empty() || command.len() > 65536 {
            return Err(ProviderError::Rejected);
        }
        let messages = self
            .call(
                "Exec",
                wire::ExecChunk {
                    vm_id: machine_id(id)?.vm_id,
                    command: command.into(),
                    ..Default::default()
                },
                true,
            )
            .await?;
        let mut result = ExecResult {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: -1,
        };
        for message in messages {
            let chunk = wire::ExecChunk::decode(message.as_slice())
                .map_err(|_| ProviderError::InvalidResponse)?;
            if chunk.is_stderr {
                result.stderr.extend_from_slice(&chunk.data);
            } else {
                result.stdout.extend_from_slice(&chunk.data);
            }
            result.exit_code = chunk.exit_code;
        }
        Ok(result)
    }

    /// Bootstrap secrets/config only. Runtime file RPC handles user workspace files.
    pub async fn upload(&self, id: &str, path: &str, data: &[u8]) -> Result<(), ProviderError> {
        if !path.starts_with('/')
            || path.len() > 4096
            || path.contains('\0')
            || data.len() > 3 * 1024 * 1024
        {
            return Err(ProviderError::Rejected);
        }
        let result: wire::Uploaded = self
            .unary(
                "UploadFile",
                wire::UploadFile {
                    vm_id: machine_id(id)?.vm_id,
                    path: path.into(),
                    data: data.to_vec(),
                },
                true,
            )
            .await?;
        if result.bytes_written != data.len() as u64 {
            return Err(ProviderError::InvalidResponse);
        }
        Ok(())
    }
}

fn machine_id(id: &str) -> Result<wire::MachineId, ProviderError> {
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return Err(ProviderError::Rejected);
    }
    Ok(wire::MachineId { vm_id: id.into() })
}
fn machine_state(status: String) -> MachineState {
    match status.as_str() {
        "running" => MachineState::Running,
        "suspended" | "hibernated" => MachineState::Paused,
        "stopped" => MachineState::Stopped,
        "starting" | "creating" | "pending" => MachineState::Starting,
        _ => MachineState::Unknown(status),
    }
}

async fn read_body(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, ProviderError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ProviderError::Unavailable)?;
        if body.len() + chunk.len() > limit {
            return Err(ProviderError::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
fn frame(message: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(message.len() + 5);
    bytes.push(0);
    bytes.extend_from_slice(&(message.len() as u32).to_be_bytes());
    bytes.extend_from_slice(message);
    bytes
}
fn check_status(code: u32, mutation: bool) -> Result<(), ProviderError> {
    match code {
        0 => Ok(()),
        5 => Err(ProviderError::NotFound),
        3 | 7 | 16 => Err(ProviderError::Rejected),
        6 | 9 | 10 => Err(ProviderError::Conflict),
        8 => Err(ProviderError::RateLimited),
        _ => Err(if mutation {
            ProviderError::OutcomeUnknown
        } else {
            ProviderError::Unavailable
        }),
    }
}
fn parse_frames(
    mut body: &[u8],
    mut status: Option<u32>,
    mutation: bool,
) -> Result<Vec<Vec<u8>>, ProviderError> {
    let mut messages = Vec::new();
    let mut trailers = false;
    while !body.is_empty() {
        if body.len() < 5 || trailers {
            return Err(ProviderError::InvalidResponse);
        }
        let size = u32::from_be_bytes(body[1..5].try_into().unwrap()) as usize;
        if size > MAX_RESPONSE || body.len() - 5 < size {
            return Err(ProviderError::InvalidResponse);
        }
        let payload = &body[5..5 + size];
        match body[0] {
            0 => messages.push(payload.to_vec()),
            0x80 => {
                trailers = true;
                for line in std::str::from_utf8(payload)
                    .map_err(|_| ProviderError::InvalidResponse)?
                    .split("\r\n")
                {
                    if let Some((key, value)) = line.split_once(':')
                        && key.eq_ignore_ascii_case("grpc-status")
                    {
                        status = Some(
                            value
                                .trim()
                                .parse()
                                .map_err(|_| ProviderError::InvalidResponse)?,
                        );
                    }
                }
            }
            _ => return Err(ProviderError::InvalidResponse),
        }
        body = &body[5 + size..];
    }
    check_status(status.ok_or(ProviderError::InvalidResponse)?, mutation)?;
    Ok(messages)
}

fn usage_bounds(start: i64, end: i64) -> Result<(i64, i64), ProviderError> {
    if start <= 0 || end <= start || end > 9_007_199_254_740_000 || end - start > 366 * 86_400_000 {
        return Err(ProviderError::Rejected);
    }
    Ok(((start + 999) / 1000, (end + 999) / 1000))
}
fn validate_usage_period(
    start: i64,
    end: i64,
    currency: &str,
    since: i64,
    until: i64,
) -> Result<(), ProviderError> {
    if start != since || end != until || !currency.eq_ignore_ascii_case("usd") {
        return Err(ProviderError::InvalidResponse);
    }
    Ok(())
}
fn usage(report: wire::VmUsage) -> Result<Usage, ProviderError> {
    if report.vm_id.is_empty() || report.cost_micro < 0 || report.cost_micro > 9_007_199_254_740_991
    {
        return Err(ProviderError::InvalidResponse);
    }
    Ok(Usage {
        machine_id: report.vm_id,
        vcpu: report.vcpu,
        memory_bytes: report.memory_bytes,
        cost_micros: report.cost_micro,
        complete: report.complete,
    })
}

#[cfg(test)]
mod usage_configuration_tests {
    use super::*;
    #[test]
    fn cluster_exchange_matches_sdk_and_refuses_insecure_credentials() {
        for (base, exchange) in [
            (
                "https://boxd.sh:9443",
                "https://app.boxd.sh/api/v1/auth/token",
            ),
            (
                "https://cluster.example:9443/",
                "https://app.cluster.example/api/v1/auth/token",
            ),
            (
                "https://app.cluster.example:8443",
                "https://app.cluster.example:8443/api/v1/auth/token",
            ),
            (
                "https://127.0.0.1:9443",
                "https://127.0.0.1:9443/api/v1/auth/token",
            ),
        ] {
            let client = Boxd::new("key".into(), None)
                .unwrap()
                .with_base_url(base)
                .unwrap();
            assert_eq!(client.exchange_url, exchange);
        }
        for base in [
            "http://boxd.sh:9443",
            "https://user:secret@boxd.sh:9443",
            "https://boxd.sh?token=secret",
            "https://boxd.sh#secret",
        ] {
            assert!(
                Boxd::new("key".into(), None)
                    .unwrap()
                    .with_base_url(base)
                    .is_err()
            );
        }
    }
}
