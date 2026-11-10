//! The control SDK shares transport limits, never browser-cookie authority.
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_tungstenite::tungstenite::Message;
use zuse_rpc::{RpcError, RpcReply, RpcService};

struct Service(AtomicUsize);
#[async_trait::async_trait]
impl RpcService for Service {
    async fn handle(&self, method: &str, _: Value) -> Result<RpcReply, RpcError> {
        assert_eq!(
            method, "ControlCall",
            "raw engine operations must never reach the service"
        );
        self.0.fetch_add(1, Ordering::SeqCst);
        RpcReply::value(&json!({"allowed":true}))
    }
}
#[tokio::test]
async fn control_socket_preserves_operation_boundary_and_closes_on_host_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let service = Arc::new(Service(AtomicUsize::new(0)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let stop = tokio_util::sync::CancellationToken::new();
    let server = zuse_serve::BrowserServer::new(service.clone(), "x".repeat(64), None).unwrap();
    let task = tokio::spawn(server.serve_with_shutdown(
        listener,
        root.path().into(),
        None,
        stop.clone().cancelled_owned(),
    ));
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/api/control"))
        .await
        .unwrap();
    socket
        .send(Message::Text(
            json!({"id":1,"method":"Mutate","params":{}}).to_string(),
        ))
        .await
        .unwrap();
    let Message::Text(reply) = socket.next().await.unwrap().unwrap() else {
        panic!("missing rejection");
    };
    let rejected: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(rejected["id"], 1);
    assert!(rejected.get("err").is_some());
    assert_eq!(service.0.load(Ordering::SeqCst), 0);
    socket
        .send(Message::Text(
            json!({"id":2,"method":"ControlCall","params":{}}).to_string(),
        ))
        .await
        .unwrap();
    let Message::Text(reply) = socket.next().await.unwrap().unwrap() else {
        panic!("missing control reply");
    };
    assert_eq!(
        serde_json::from_str::<Value>(&reply).unwrap()["ok"]["allowed"],
        true
    );
    assert_eq!(service.0.load(Ordering::SeqCst), 1);
    stop.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let closed = tokio::time::timeout(std::time::Duration::from_secs(1), socket.next())
        .await
        .expect("control socket must not outlive its host");
    assert!(matches!(
        closed,
        None | Some(Err(_)) | Some(Ok(Message::Close(_)))
    ));
}
