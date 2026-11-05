//! Contract fixture uses trusted TLS and the hosted API's versioned scope routes.
use super::*;
use serde_json::{Value, json};
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivatePkcs8KeyDer},
    },
};

struct Credentials(Mutex<String>);
#[async_trait]
impl PluginCredentials for Credentials {
    async fn current(&self) -> Result<PluginCredential, PluginError> {
        Ok(PluginCredential {
            subject: self.0.lock().unwrap().clone(),
            access_token: "fixture-token".into(),
        })
    }
}
struct Server {
    requests: Mutex<Vec<(String, String, Value)>>,
    barrier: tokio::sync::Barrier,
    held: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
struct Fixture {
    client: PluginClient,
    credentials: Arc<Credentials>,
    server: Arc<Server>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.cancel();
        self.task.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let signed = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert = CertificateDer::from(signed.cert.der().to_vec());
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert],
            PrivatePkcs8KeyDer::from(signed.signing_key.serialize_der()).into(),
        )
        .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let credentials = Arc::new(Credentials(Mutex::new("user".into())));
        let mut client = PluginClient::new(
            &format!("https://localhost:{}", address.port()),
            credentials.clone(),
        )
        .await
        .unwrap();
        client.http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .add_root_certificate(
                reqwest::Certificate::from_pem(signed.cert.pem().as_bytes()).unwrap(),
            )
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let server = Arc::new(Server {
            requests: Mutex::new(vec![]),
            barrier: tokio::sync::Barrier::new(6),
            held: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let copy = server.clone();
        let stop = CancellationToken::new();
        let cancel = stop.clone();
        let task = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! { biased;
                    _=cancel.cancelled()=>break,
                    accepted=listener.accept()=>{
                        let Ok((socket,_))=accepted else {break;};
                        let acceptor=acceptor.clone();let state=copy.clone();
                        tasks.spawn(async move {
                            let Ok(mut stream)=acceptor.accept(socket).await else {return;};
                            let mut bytes=vec![];let mut buf=[0u8;4096];
                            let (head,body)=loop {
                                let n=stream.read(&mut buf).await.unwrap_or(0);if n==0{return;}bytes.extend_from_slice(&buf[..n]);
                                if bytes.len()>16384{return;}
                                let Some(end)=bytes.windows(4).position(|w|w==b"\r\n\r\n")else{continue;};
                                let head=String::from_utf8_lossy(&bytes[..end]).to_string();
                                let length=head.lines().filter_map(|l|l.split_once(':')).find(|(k,_)|k.eq_ignore_ascii_case("content-length")).and_then(|(_,v)|v.trim().parse::<usize>().ok()).unwrap_or(0);
                                if bytes.len()<end+4+length {continue;}
                                break (head,serde_json::from_slice::<Value>(&bytes[end+4..end+4+length]).unwrap_or(Value::Null));
                            };
                            let route=head.lines().next().unwrap().split_whitespace().nth(1).unwrap().to_string();
                            let header=|name:&str|head.lines().filter_map(|l|l.split_once(':')).find(|(k,_)|k.eq_ignore_ascii_case(name)).map(|(_,v)|v.trim().to_owned()).unwrap_or_default();
                            assert_eq!(header("authorization"),"Bearer fixture-token");
                            let workspace=header("x-zuse-workspace");
                            state.requests.lock().unwrap().push((route.clone(),workspace.clone(),body.clone()));
                            if body["address"]=="hold" {state.held.notify_one();state.release.notified().await;}
                            if body["address"]=="concurrent" {state.barrier.wait().await;}
                            let result=if route=="/v1/organizations" {json!([{"id":"team","name":"Team","role":"admin"},{"id":"billing","name":"Billing","role":"billing"}])}
                            else if route.ends_with("/plugins/tools") {json!({"workspace":workspace,"arguments":body["arguments"]})}
                            else if body["action"]=="list" {json!({"kind":"snapshot","tenantId":if workspace=="organization:wrong"{"personal:user"}else{body["tenantId"].as_str().unwrap_or("personal:user")},"tenants":[],"endpoint":"unused","catalog":[],"connections":[]})}
                            else {json!({"kind":"ok"})};
                            let (status,json)=match body["address"].as_str() {
                                Some("unreachable")=>("400 Bad Request",json!({"error":"plugin_unreachable"}).to_string()),
                                Some("odd-code")=>("400 Bad Request",json!({"error":"Not A Code"}).to_string()),
                                Some("gateway")=>("502 Bad Gateway","<html>gateway</html>".to_owned()),
                                _=>("200 OK",serde_json::to_string(&result).unwrap()),
                            };
                            let response=format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",json.len(),json);
                            let _=stream.write_all(response.as_bytes()).await;let _=stream.shutdown().await;
                        });
                    },
                    _=tasks.join_next(),if !tasks.is_empty()=>{}
                }
            }
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        });
        Self {
            client,
            credentials,
            server,
            stop,
            task,
        }
    }
}
#[tokio::test]
async fn concurrent_calls_and_management_use_exact_workspace_and_versioned_routes() {
    let f = Fixture::new().await;
    let cancel = CancellationToken::new();
    let requests = (0..6).map(|id| {
        let client = f.client.clone();
        let cancel = cancel.clone();
        async move {
            let tenant = if id % 2 == 0 {
                "personal:user"
            } else {
                "organization:team"
            };
            let request = PluginToolRequest::Call {
                address: "concurrent".into(),
                arguments: serde_json::from_value(json!({"id":id})).unwrap(),
            };
            let value = client
                .tools_in_tenant(tenant, &request, true, &cancel)
                .await
                .unwrap();
            assert_eq!(
                value["workspace"],
                if id % 2 == 0 {
                    "personal"
                } else {
                    "organization:team"
                }
            );
            assert_eq!(value["arguments"]["id"], id);
        }
    });
    tokio::time::timeout(Duration::from_secs(5), futures::future::join_all(requests))
        .await
        .unwrap();
    let choices = f.client.workspaces(&cancel).await.unwrap();
    assert_eq!(
        choices.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["personal:user", "organization:team"]
    );
    f.client
        .request(
            &PluginRequest::List {
                tenant_id: Some("organization:team".into()),
            },
            &cancel,
        )
        .await
        .unwrap();
    f.client
        .request(
            &PluginRequest::Complete {
                tenant_id: "organization:team".into(),
                ticket: "one-use".into(),
            },
            &cancel,
        )
        .await
        .unwrap();
    let rows = f.server.requests.lock().unwrap();
    assert_eq!(
        rows.iter()
            .filter(|r| r.0 == "/v1/organization-workspaces/team/v1/plugins/tools")
            .count(),
        3
    );
    assert_eq!(
        rows.iter().filter(|r| r.0 == "/v1/plugins/tools").count(),
        3
    );
    assert_eq!(
        rows.iter()
            .filter(|r| r.0 == "/v1/organization-workspaces/team/v1/plugins")
            .count(),
        2
    );
}
#[tokio::test]
async fn invalid_scope_permission_and_wrong_workspace_responses_do_not_fall_back() {
    let f = Fixture::new().await;
    let cancel = CancellationToken::new();
    for tenant in [
        "personal:other",
        "organization:",
        "organization:a/b",
        "organization:a\r\nx-header: value",
    ] {
        assert!(matches!(
            f.client
                .tools_in_tenant(tenant, &PluginToolRequest::List, true, &cancel)
                .await,
            Err(PluginError::InvalidWorkspace)
        ));
    }
    let call = PluginToolRequest::Call {
        address: "send".into(),
        arguments: Default::default(),
    };
    assert!(matches!(
        f.client
            .tools_in_tenant("organization:team", &call, false, &cancel)
            .await,
        Err(PluginError::PermissionRequired)
    ));
    assert!(f.server.requests.lock().unwrap().is_empty());
    assert!(matches!(
        f.client
            .request(
                &PluginRequest::List {
                    tenant_id: Some("organization:wrong".into())
                },
                &cancel
            )
            .await,
        Err(PluginError::InvalidResponse)
    ));
    assert_eq!(f.server.requests.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn in_flight_account_switch_and_cancellation_never_publish_or_replay() {
    let f = Fixture::new().await;
    let request = PluginToolRequest::Call {
        address: "hold".into(),
        arguments: Default::default(),
    };
    let client = f.client.clone();
    let r = request.clone();
    let cancel = CancellationToken::new();
    let run = tokio::spawn(async move {
        client
            .tools_in_tenant("organization:team", &r, true, &cancel)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), f.server.held.notified())
        .await
        .unwrap();
    *f.credentials.0.lock().unwrap() = "other".into();
    f.server.release.notify_one();
    assert!(matches!(
        run.await.unwrap(),
        Err(PluginError::AccountChanged)
    ));
    *f.credentials.0.lock().unwrap() = "user".into();
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    let client = f.client.clone();
    let run = tokio::spawn(async move {
        client
            .tools_in_tenant("organization:team", &request, true, &signal)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), f.server.held.notified())
        .await
        .unwrap();
    cancel.cancel();
    assert!(matches!(run.await.unwrap(), Err(PluginError::Cancelled)));
    f.server.release.notify_one();
    assert_eq!(f.server.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn rejected_requests_keep_the_api_error_code() {
    let f = Fixture::new().await;
    let cancel = CancellationToken::new();
    for (address, expected) in [
        ("unreachable", "plugin_unreachable"),
        ("odd-code", "http_400"),
        ("gateway", "http_502"),
    ] {
        let call = PluginToolRequest::Call {
            address: address.into(),
            arguments: Default::default(),
        };
        let error = f
            .client
            .tools_in_tenant("personal:user", &call, true, &cancel)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, PluginError::Rejected { code, .. } if code == expected),
            "{address}: {error:?}"
        );
        assert_eq!(
            error.to_string(),
            format!(
                "Plugin request failed ({expected}). Check Plugins in the workspace this chat belongs to."
            )
        );
    }
}

#[test]
fn rejection_code_accepts_only_short_snake_case_codes() {
    assert_eq!(
        rejection_code(400, br#"{"error":"plugin_unreachable"}"#),
        "plugin_unreachable"
    );
    assert_eq!(rejection_code(400, br#"{"error":"Bad-Code"}"#), "http_400");
    assert_eq!(rejection_code(400, br#"{"error":7}"#), "http_400");
    let long = format!(r#"{{"error":"{}"}}"#, "a".repeat(65));
    assert_eq!(rejection_code(403, long.as_bytes()), "http_403");
    assert_eq!(rejection_code(502, b"<html>gateway</html>"), "http_502");
    assert_eq!(rejection_code(500, b""), "http_500");
}
