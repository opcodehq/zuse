//! Direct HTTPS using the same HTTP/RPC router as loopback serving.
use anyhow::Context;
use futures::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use std::{io::Read, net::SocketAddr, path::Path, sync::Arc, time::Duration};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
    server::TlsStream,
};

#[derive(Clone)]
pub struct TlsConfig(Arc<rustls::ServerConfig>);
impl TlsConfig {
    /// Read a certificate chain and matching PKCS#8, RSA or SEC1 private key.
    /// Explicit crypto selection avoids relying on process-global defaults.
    pub fn from_pem_files(certificate: &Path, private_key: &Path) -> anyhow::Result<Self> {
        fn read(path: &Path) -> anyhow::Result<Vec<u8>> {
            let file = std::fs::File::open(path)
                .with_context(|| format!("Could not open TLS file {}", path.display()))?;
            anyhow::ensure!(
                file.metadata()?.is_file(),
                "TLS input must be a regular file"
            );
            let mut bytes = vec![];
            file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
            anyhow::ensure!(bytes.len() <= 1024 * 1024, "TLS input exceeds 1 MiB");
            Ok(bytes)
        }
        Self::from_pem(&read(certificate)?, &read(private_key)?)
    }

    /// Parse PEM text already held in memory, such as a generated certificate.
    pub fn from_pem(certificate: &[u8], private_key: &[u8]) -> anyhow::Result<Self> {
        let certificates = CertificateDer::pem_slice_iter(certificate)
            .collect::<Result<Vec<_>, _>>()
            .context("Could not parse TLS certificate chain")?;
        anyhow::ensure!(!certificates.is_empty(), "TLS certificate chain is empty");
        let key = PrivateKeyDer::from_pem_slice(private_key)
            .context("Could not parse TLS private key")?;
        let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certificates, key)
        .context("TLS certificate and private key do not match")?;
        // HTTP/1.1 supports ordinary requests and browser WebSocket upgrades.
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Self(Arc::new(config)))
    }
}

type Handshake = BoxFuture<'static, Option<(TlsStream<TcpStream>, SocketAddr)>>;
pub(super) struct Listener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
    handshakes: FuturesUnordered<Handshake>,
}
impl Listener {
    pub fn new(tcp: TcpListener, config: TlsConfig) -> Self {
        Self {
            tcp,
            acceptor: TlsAcceptor::from(config.0),
            handshakes: FuturesUnordered::new(),
        }
    }
}
impl axum::serve::Listener for Listener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            tokio::select! {
                result = self.handshakes.next(), if !self.handshakes.is_empty() => {
                    if let Some(Some(connection)) = result { return connection; }
                },
                accepted = self.tcp.accept(), if self.handshakes.len() < 64 => {
                    match accepted {
                        Ok((stream, address)) => {
                            let _ = stream.set_nodelay(true);
                            let acceptor = self.acceptor.clone();
                            self.handshakes.push(async move {
                                match tokio::time::timeout(Duration::from_secs(10), acceptor.accept(stream)).await {
                                    Ok(Ok(stream)) => Some((stream, address)),
                                    _ => None,
                                }
                            }.boxed());
                        }
                        Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                    }
                },
            }
        }
    }
    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrowserServer;
    struct Service;
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for Service {
        async fn handle(
            &self,
            _: &str,
            params: serde_json::Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            zuse_rpc::RpcReply::value(&params)
        }
    }
    fn material(directory: &Path) -> (Vec<u8>, TlsConfig) {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
        std::fs::write(directory.join("certificate.pem"), cert.pem()).unwrap();
        std::fs::write(directory.join("key.pem"), signing_key.serialize_pem()).unwrap();
        let config = TlsConfig::from_pem_files(
            &directory.join("certificate.pem"),
            &directory.join("key.pem"),
        )
        .unwrap();
        (cert.der().to_vec(), config)
    }
    #[test]
    fn malformed_or_mismatched_tls_material_is_rejected_before_serving() {
        let directory = tempfile::tempdir().unwrap();
        material(directory.path());
        let other = rcgen::KeyPair::generate().unwrap();
        std::fs::write(directory.path().join("key.pem"), other.serialize_pem()).unwrap();
        assert!(
            TlsConfig::from_pem_files(
                &directory.path().join("certificate.pem"),
                &directory.path().join("key.pem")
            )
            .is_err()
        );
        std::fs::write(
            directory.path().join("certificate.pem"),
            "not a certificate",
        )
        .unwrap();
        assert!(
            TlsConfig::from_pem_files(
                &directory.path().join("certificate.pem"),
                &directory.path().join("key.pem")
            )
            .is_err()
        );
    }
    #[tokio::test]
    async fn paired_app_listener_serves_an_isolated_app_and_no_owner_access() {
        let directory = tempfile::tempdir().unwrap();
        let (certificate, config) = material(directory.path());
        std::fs::write(
            directory.path().join("index.html"),
            "<title>Zuse</title>".repeat(16),
        )
        .unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = tcp.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let token = "a".repeat(64);
        let server = BrowserServer::new(Arc::new(Service), token.clone(), None).unwrap();
        let task = tokio::spawn(server.serve_paired_app(
            tcp,
            directory.path().into(),
            Some(config),
            async {
                let _ = stopped.await;
            },
        ));
        let client = reqwest::Client::builder()
            .no_proxy()
            .add_root_certificate(reqwest::Certificate::from_der(&certificate).unwrap())
            .build()
            .unwrap();
        let base = format!("https://{address}");
        let page = client.get(format!("{base}/")).send().await.unwrap();
        assert_eq!(page.status(), reqwest::StatusCode::OK);
        assert_eq!(page.headers()["cross-origin-opener-policy"], "same-origin");
        assert_eq!(
            page.headers()["cross-origin-embedder-policy"],
            "require-corp"
        );
        assert!(page.text().await.unwrap().contains("Zuse"));
        // Compression and caching are covered by assets.rs; this checks the shared router is mounted.
        let script = client
            .get(format!("{base}/missing.js"))
            .send()
            .await
            .unwrap();
        assert_eq!(script.headers()["cache-control"], "no-cache");
        assert_eq!(
            script.headers()["cross-origin-embedder-policy"],
            "require-corp"
        );
        let session = client
            .post(format!("{base}/api/session/check"))
            .header("Origin", &base)
            .send()
            .await
            .unwrap();
        assert_eq!(session.status(), reqwest::StatusCode::UNAUTHORIZED);
        let unlock = client
            .post(format!("{base}/api/session"))
            .header("Origin", &base)
            .json(&serde_json::json!({ "token": token }))
            .send()
            .await
            .unwrap();
        assert!(!unlock.headers().contains_key("set-cookie"));
        let owner_socket = client
            .get(format!("{base}/api/rpc"))
            .header("Connection", "upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .unwrap();
        assert_ne!(
            owner_socket.status(),
            reqwest::StatusCode::SWITCHING_PROTOCOLS
        );
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    #[tokio::test]
    async fn stalled_tls_handshakes_do_not_block_trusted_clients_or_shutdown() {
        let directory = tempfile::tempdir().unwrap();
        let (certificate, config) = material(directory.path());
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = tcp.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = BrowserServer::new(Arc::new(Service), "a".repeat(64), None).unwrap();
        let task = tokio::spawn(server.serve_with_shutdown(
            tcp,
            directory.path().into(),
            Some(config),
            async {
                let _ = stopped.await;
            },
        ));
        let stalled = TcpStream::connect(address).await.unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .add_root_certificate(reqwest::Certificate::from_der(&certificate).unwrap())
            .build()
            .unwrap();
        let health = client.get(format!("https://{address}/health")).send();
        let response = tokio::time::timeout(Duration::from_secs(3), health)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let denied = client
            .post(format!("https://{address}/api/session/check"))
            .header("Origin", format!("https://{address}"))
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), reqwest::StatusCode::UNAUTHORIZED);
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(stalled);
    }
}
