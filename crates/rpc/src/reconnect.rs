//! Replacement sockets are isolated channels, never a queue of replayable actions.
use crate::{
    RpcError,
    client::{Incoming, Shared},
};
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub struct Channel {
    pub outgoing: mpsc::Sender<String>,
    pub incoming: mpsc::Receiver<String>,
    pub closed: CancellationToken,
    /// Host identity already delivered while the channel was being set up.
    pub engine_info: Option<serde_json::Value>,
}

#[derive(Clone)]
pub(super) struct Session {
    pub sender: mpsc::Sender<String>,
    pub retired: CancellationToken,
}
pub(super) struct State {
    current: Mutex<Option<Session>>,
    pending: Weak<Shared>,
    input: mpsc::Sender<Incoming>,
    stopped: CancellationToken,
    generation: std::sync::atomic::AtomicU64,
}
impl State {
    pub(super) fn new(pending: Weak<Shared>, input: mpsc::Sender<Incoming>) -> Arc<Self> {
        Arc::new(Self {
            current: Mutex::new(None),
            pending,
            input,
            stopped: CancellationToken::new(),
            generation: std::sync::atomic::AtomicU64::new(0),
        })
    }
    pub(super) fn sender(&self) -> Option<Session> {
        if self.stopped.is_cancelled() {
            return None;
        }
        self.current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub(super) fn close(&self) {
        self.stopped.cancel();
    }
}
#[derive(Clone)]
pub struct Control(Arc<State>);
impl Control {
    pub(super) fn new(state: Arc<State>) -> Self {
        Self(state)
    }
    pub fn generation(&self) -> u64 {
        self.0.generation.load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn stopped(&self) -> CancellationToken {
        self.0.stopped.clone()
    }
    pub fn close(&self) {
        self.0.close();
    }
    /// Install only a verified socket, after the previous lease has retired.
    /// The returned token fires after old sends are refused and pending calls end.
    pub fn attach(&self, mut channel: Channel) -> Result<CancellationToken, RpcError> {
        let state = self.0.clone();
        let mut current = state.current.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopped.is_cancelled() || current.is_some() {
            return Err(RpcError::Transport(
                "The connection is stopped or already attached".into(),
            ));
        }
        let (output, mut outgoing) = mpsc::channel::<String>(16);
        let retired = CancellationToken::new();
        *current = Some(Session {
            sender: output,
            retired: retired.clone(),
        });
        state
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        drop(current);
        let finished = CancellationToken::new();
        let done = finished.clone();
        tokio::spawn(async move {
            let exchange = async {
                loop {
                    tokio::select! {
                        frame=outgoing.recv()=>match frame {Some(frame)=>channel.outgoing.send(frame).await.map_err(|_|())?,None=>return Ok::<_,()>(())},
                        frame=channel.incoming.recv()=>match frame {Some(body)=>state.input.send(Incoming{body,retired:retired.clone()}).await.map_err(|_|())?,None=>return Ok(())},
                    }
                }
            };
            tokio::select! {biased;_=state.stopped.cancelled()=>{},_=channel.closed.cancelled()=>{},_=exchange=>{}}
            // Close the old receiver BEFORE draining pending requests. A caller
            // that captured this sender can only fail, never reach a new socket.
            outgoing.close();
            retired.cancel();
            let mut current = state.current.lock().unwrap_or_else(|e| e.into_inner());
            current.take();
            if let Some(pending) = state.pending.upgrade() {
                pending.interrupt(true);
            }
            drop(current);
            drop(channel);
            done.cancel();
        });
        Ok(finished)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RpcClient;
    use serde_json::{Value, json};
    use std::time::Duration;
    fn channel() -> (Channel, mpsc::Receiver<String>, mpsc::Sender<String>) {
        let (outgoing, requests) = mpsc::channel(16);
        let (replies, incoming) = mpsc::channel(16);
        (
            Channel {
                outgoing,
                incoming,
                closed: CancellationToken::new(),
                engine_info: None,
            },
            requests,
            replies,
        )
    }
    async fn frame(receiver: &mut mpsc::Receiver<String>) -> Value {
        serde_json::from_str(
            &tokio::time::timeout(Duration::from_secs(2), receiver.recv())
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap()
    }
    #[tokio::test]
    async fn lost_reply_is_not_replayed_and_offline_calls_never_enter_the_next_channel() {
        let (client, control) = RpcClient::reconnecting();
        let client = Arc::new(client);
        assert!(
            client
                .call("Write", json!({}))
                .await
                .unwrap_err()
                .to_string()
                .contains("not sent")
        );
        let (first, mut requests, _replies) = channel();
        let drop_link = first.closed.clone();
        let closed = control.attach(first).unwrap();
        let owner = client.clone();
        let write = tokio::spawn(async move { owner.call("Write", json!({"value":1})).await });
        let old = frame(&mut requests).await;
        assert_eq!(old["method"], "Write");
        drop_link.cancel();
        tokio::time::timeout(Duration::from_secs(2), closed.cancelled())
            .await
            .unwrap();
        assert!(
            write
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("may have completed")
        );
        assert!(
            client
                .call("Write", json!({"value":2}))
                .await
                .unwrap_err()
                .to_string()
                .contains("not sent")
        );
        let (second, mut requests, replies) = channel();
        control.attach(second).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), requests.recv())
                .await
                .is_err()
        );
        let owner = client.clone();
        let read = tokio::spawn(async move { owner.call("Read", json!({})).await });
        let next = frame(&mut requests).await;
        assert_eq!(next["method"], "Read");
        assert_ne!(next["id"], old["id"]);
        replies
            .send(json!({"id":old["id"],"ok":"stale"}).to_string())
            .await
            .unwrap();
        replies
            .send(json!({"id":next["id"],"ok":"fresh"}).to_string())
            .await
            .unwrap();
        assert_eq!(read.await.unwrap().unwrap(), "fresh");
        control.close();
    }
    #[tokio::test]
    async fn stalled_old_subscription_cannot_block_retirement_or_leak_buffered_frames() {
        let (client, control) = RpcClient::reconnecting();
        let client = Arc::new(client);
        let (first, mut requests, replies) = channel();
        let drop_link = first.closed.clone();
        let closed = control.attach(first).unwrap();
        let mut watch = client.subscribe_scoped("Watch", json!({})).await.unwrap();
        let first = frame(&mut requests).await;
        let progress = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sent = progress.clone();
        let producer = tokio::spawn(async move {
            for n in 0..400 {
                if replies
                    .send(json!({"id":first["id"],"item":n}).to_string())
                    .await
                    .is_err()
                {
                    return;
                }
                sent.store(n + 1, std::sync::atomic::Ordering::SeqCst);
            }
        });
        // More than the consumer's 256 entries proves the reader has reached
        // stream backpressure without relying on a wall-clock scheduling guess.
        tokio::time::timeout(Duration::from_secs(2), async {
            while progress.load(std::sync::atomic::Ordering::SeqCst) < 280 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop_link.cancel();
        tokio::time::timeout(Duration::from_secs(2), closed.cancelled())
            .await
            .unwrap();
        assert!(watch.recv().await.is_none());
        producer.abort();
        let (second, mut requests, replies) = channel();
        control.attach(second).unwrap();
        let mut next = client.subscribe_scoped("Watch", json!({})).await.unwrap();
        let request = frame(&mut requests).await;
        replies
            .send(json!({"id":request["id"],"item":"fresh"}).to_string())
            .await
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), next.recv())
                .await
                .unwrap()
                .unwrap(),
            "fresh"
        );
        control.close();
    }
}
