use super::*;
use serde_json::{Value, json};
fn pair() -> (RpcClient, mpsc::Receiver<String>, mpsc::Sender<String>) {
    let (requests, out) = mpsc::channel(16);
    let (input, replies) = mpsc::channel(16);
    (RpcClient::new(requests, replies), out, input)
}
async fn request(out: &mut mpsc::Receiver<String>) -> Value {
    serde_json::from_str(&out.recv().await.unwrap()).unwrap()
}
#[tokio::test]
async fn unary_null_and_wire_errors_keep_their_reply_kind() {
    let (client, mut out, input) = pair();
    for response in [
        json!({"ok":null}),
        json!({"err":"capability error: cloud.denied"}),
    ] {
        let mut forward = client.forward("Read", json!({})).await.unwrap();
        let call = request(&mut out).await;
        let mut response = response;
        response["id"] = call["id"].clone();
        input.send(response.to_string()).await.unwrap();
        let frame = forward.recv().await.unwrap();
        assert_eq!(serde_json::to_value(frame).unwrap(), response);
        assert!(forward.recv().await.is_none());
        drop(forward);
        assert!(out.try_recv().is_err());
    }
}
#[tokio::test]
async fn acknowledged_stream_preserves_ack_items_and_completion() {
    let (client, mut out, input) = pair();
    let mut forward = client
        .forward(crate::methods::WATCH_CHECKOUT_CHANGE_REQUEST, json!({}))
        .await
        .unwrap();
    let call = request(&mut out).await;
    for payload in [
        json!({"ok":{"stream":true}}),
        json!({"item":{"revision":1}}),
        json!({"done":true}),
    ] {
        let mut frame = payload;
        frame["id"] = call["id"].clone();
        input.send(frame.to_string()).await.unwrap();
        assert_eq!(
            serde_json::to_value(forward.recv().await.unwrap()).unwrap(),
            frame
        );
    }
    assert!(forward.recv().await.is_none());
}
#[tokio::test]
async fn dropping_one_forward_cancels_only_its_own_request() {
    let (client, mut out, input) = pair();
    let first = client.forward("WatchFirst", json!({})).await.unwrap();
    let a = request(&mut out).await;
    let mut second = client.forward("WatchSecond", json!({})).await.unwrap();
    let b = request(&mut out).await;
    drop(first);
    let cancel = request(&mut out).await;
    assert_eq!(cancel["id"], a["id"]);
    assert_eq!(cancel["cancel"], true);
    input
        .send(json!({"id":b["id"],"item":"still attached"}).to_string())
        .await
        .unwrap();
    assert_eq!(
        second.recv().await.unwrap().item,
        Some(json!("still attached"))
    );
}
