//! Loopback OAuth confirmation. Tickets remain in the engine and are consumed once
//! by the hosted service under the account that opened the authorization flow.
use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use zuse_plugins::PluginClient;
use zuse_protocol::plugins::PluginRequest;

pub struct PluginCallback {
    pub port: u16,
    pub cancel: CancellationToken,
}
impl Drop for PluginCallback {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
#[derive(Clone)]
struct CallbackState {
    port: u16,
    client: PluginClient,
    cancel: CancellationToken,
}
impl PluginCallback {
    pub async fn start(client: PluginClient, cancel: CancellationToken) -> anyhow::Result<Self> {
        let cancel = cancel.child_token();
        for port in 8976..=8979 {
            let listener =
                match tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await {
                    Ok(listener) => listener,
                    Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => continue,
                    Err(error) => return Err(error.into()),
                };
            let shutdown = cancel.clone();
            let router = Router::new()
                .route("/plugins/callback", get(complete))
                .with_state(CallbackState {
                    port,
                    client,
                    cancel: cancel.clone(),
                });
            tokio::spawn(async move {
                let _ = axum::serve(listener, router)
                    .with_graceful_shutdown(shutdown.cancelled_owned())
                    .await;
            });
            return Ok(Self { port, cancel });
        }
        anyhow::bail!("Plugin sign-in ports are busy. Close other sign-in windows and retry.")
    }
}
#[derive(Deserialize)]
struct CallbackQuery {
    plugin_ticket: Option<String>,
    plugin_tenant: Option<String>,
}
async fn complete(
    State(state): State<CallbackState>,
    Query(query): Query<CallbackQuery>,
    headers: HeaderMap,
) -> Response {
    let expected = format!("127.0.0.1:{}", state.port);
    if headers.get(header::HOST).and_then(|v| v.to_str().ok()) != Some(expected.as_str()) {
        return page(StatusCode::BAD_REQUEST, "Invalid sign-in callback.");
    }
    let (Some(ticket), Some(tenant)) = (query.plugin_ticket, query.plugin_tenant) else {
        return page(
            StatusCode::BAD_REQUEST,
            "Plugin sign-in did not complete. Return to Zuse and retry.",
        );
    };
    if ticket.len() > 256 || ticket.is_empty() || tenant.len() > 512 || tenant.is_empty() {
        return page(StatusCode::BAD_REQUEST, "Invalid sign-in callback.");
    }
    match state
        .client
        .request(
            &PluginRequest::Complete {
                tenant_id: tenant,
                ticket,
            },
            &state.cancel,
        )
        .await
    {
        Ok(_) => page(
            StatusCode::OK,
            "Plugin authorization received. Return to Zuse to check the connection.",
        ),
        Err(_) => page(
            StatusCode::BAD_REQUEST,
            "Plugin sign-in could not be confirmed. Return to Zuse and retry.",
        ),
    }
}
fn page(status: StatusCode, message: &'static str) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; frame-ancestors 'none'",
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        format!(
            "<!doctype html><meta charset=utf-8><title>Zuse</title><h1>Zuse</h1><p>{message}</p>"
        ),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    struct Credentials;
    #[async_trait::async_trait]
    impl zuse_plugins::PluginCredentials for Credentials {
        async fn current(
            &self,
        ) -> Result<zuse_plugins::PluginCredential, zuse_plugins::PluginError> {
            Ok(zuse_plugins::PluginCredential {
                subject: "test-account".into(),
                access_token: "test-token".into(),
            })
        }
    }
    #[tokio::test]
    async fn loopback_rejects_foreign_hosts_and_missing_tickets_without_cancelling_account() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let client = PluginClient::new("https://example.test", Arc::new(Credentials))
            .await
            .unwrap();
        let account_cancel = CancellationToken::new();
        let callback = PluginCallback::start(client, account_cancel.clone())
            .await
            .unwrap();
        for host in [
            "foreign.example".to_owned(),
            format!("127.0.0.1:{}", callback.port),
        ] {
            let mut socket =
                tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, callback.port))
                    .await
                    .unwrap();
            socket.write_all(format!("GET /plugins/callback HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            let mut response = String::new();
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                socket.read_to_string(&mut response),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(response.starts_with("HTTP/1.1 400"));
            assert!(response.contains("no-store"));
            assert!(!response.contains("test-token"));
        }
        let child = callback.cancel.clone();
        drop(callback);
        assert!(child.is_cancelled());
        assert!(!account_cancel.is_cancelled());
    }
}
