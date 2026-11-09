//! Reads must continue while a large response drains over a slow downlink.
use axum::extract::ws::Message;
use futures::{Sink, SinkExt, Stream, StreamExt};
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Compression {
    None,
    Gzip,
    DeflateStream,
}

struct Encoder {
    mode: Compression,
    stream: Option<flate2::write::ZlibEncoder<Vec<u8>>>,
}
impl Encoder {
    fn new(mode: Compression) -> Self {
        Self { mode, stream: None }
    }
    async fn encode(mut self, data: String) -> Result<(Self, Message), ()> {
        if self.mode == Compression::None || data.len() < 256 {
            return Ok((self, Message::Text(data.into())));
        }
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            if self.mode == Compression::DeflateStream {
                let length = u32::try_from(data.len()).map_err(|_| ())?;
                let stream = self.stream.get_or_insert_with(|| {
                    flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(3))
                });
                stream.write_all(data.as_bytes()).map_err(|_| ())?;
                // Sync flush exposes this frame immediately while preserving
                // the dictionary. A dictionary never crosses connections.
                stream.flush().map_err(|_| ())?;
                let mut bytes = Vec::with_capacity(6 + stream.get_ref().len());
                bytes.extend_from_slice(b"ZD");
                bytes.extend_from_slice(&length.to_le_bytes());
                bytes.append(stream.get_mut());
                Ok((self, Message::Binary(bytes.into())))
            } else {
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                encoder.write_all(data.as_bytes()).map_err(|_| ())?;
                let bytes = encoder.finish().map_err(|_| ())?;
                let message = if bytes.len() < data.len() {
                    Message::Binary(bytes.into())
                } else {
                    Message::Text(data.into())
                };
                Ok((self, message))
            }
        })
        .await
        .map_err(|_| ())?
    }
}

pub(crate) async fn exchange<W, R, E>(
    mut writer: W,
    mut reader: R,
    mut outgoing: mpsc::Receiver<zuse_rpc::OutputFrame>,
    incoming: mpsc::Sender<String>,
    compression: Compression,
) where
    W: Sink<Message> + Unpin,
    R: Stream<Item = Result<Message, E>> + Unpin,
{
    let (control, mut controls) = mpsc::channel(4);
    let write = async {
        let mut encoder = Encoder::new(compression);
        let mut pending = None;
        loop {
            let mut held = Vec::new();
            let message = tokio::select! { biased;
                control = controls.recv() => match control {
                    Some(message) => message,
                    None => return,
                },
                data = async { match pending.take() { Some(frame) => Some(frame), None => outgoing.recv().await } } => match data {
                    Some(mut frame) => {
                        let mut data = std::mem::take(&mut frame.text);
                        held.push(frame);
                        // Batch already-ready envelopes without delaying the
                        // first text or control response for a timer.
                        if compression != Compression::None {
                            while data.len() < 32 * 1024 {
                                let Ok(next) = outgoing.try_recv() else { break; };
                                if data.len() + next.len() + 1 > 64 * 1024 { pending = Some(next); break; }
                                data.push('\n');
                                data.push_str(&next);
                                held.push(next);
                            }
                        }
                        match encoder.encode(data).await {
                        Ok((next, message)) => { encoder = next; message },
                        Err(()) => return,
                        }
                    },
                    None => return,
                },
            };
            if writer.send(message).await.is_err() {
                return;
            }
            drop(held);
        }
    };
    let read = async {
        while let Some(Ok(message)) = reader.next().await {
            match message {
                Message::Text(data) => {
                    if incoming.send(data.to_string()).await.is_err() {
                        return;
                    }
                }
                Message::Ping(data) => {
                    if control.send(Message::Pong(data)).await.is_err() {
                        return;
                    }
                }
                Message::Close(_) => return,
                _ => {}
            }
        }
    };
    // Ending either half retires both. Never resume a partially written frame.
    tokio::select! { _ = write => {}, _ = read => {} }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture;
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for Fixture {
        async fn handle(
            &self,
            _: &str,
            _: serde_json::Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            zuse_rpc::RpcReply::value(&"large response".repeat(1000))
        }
    }

    #[tokio::test]
    async fn compression_is_opt_in_and_preserves_the_complete_utf8_envelope() {
        use std::io::Read;
        let data = serde_json::json!({"id": 7, "item": {"reset": [{"text": "本🦀 history ".repeat(4000)}]}}).to_string();
        let (_, Message::Text(legacy)) = Encoder::new(Compression::None)
            .encode(data.clone())
            .await
            .unwrap()
        else {
            panic!("legacy framing changed")
        };
        assert_eq!(legacy.as_str(), data);
        let (_, Message::Binary(encoded)) = Encoder::new(Compression::Gzip)
            .encode(data.clone())
            .await
            .unwrap()
        else {
            panic!("large frame was not compressed")
        };
        assert!(encoded.len() * 20 < data.len());
        let mut decoded = String::new();
        flate2::read::GzDecoder::new(encoded.as_ref())
            .read_to_string(&mut decoded)
            .unwrap();
        assert_eq!(decoded, data);
        assert!(matches!(
            Encoder::new(Compression::Gzip)
                .encode("small delta".into())
                .await
                .unwrap(),
            (_, Message::Text(_))
        ));
        let small_update =
            serde_json::json!({"id": 8, "item": {"text": "stream block ".repeat(30)}}).to_string();
        assert!(small_update.len() < 1024);
        assert!(matches!(
            Encoder::new(Compression::Gzip)
                .encode(small_update)
                .await
                .unwrap(),
            (_, Message::Binary(_))
        ));
    }

    #[tokio::test]
    async fn blocked_downlink_still_delivers_cancellation_and_closes_cleanly() {
        for compression in [
            Compression::None,
            Compression::Gzip,
            Compression::DeflateStream,
        ] {
            let (started, waiting) = tokio::sync::oneshot::channel();
            let sink = Box::pin(futures::sink::unfold(
                Some(started),
                |mut started, _: Message| async move {
                    if let Some(started) = started.take() {
                        let _ = started.send(());
                    }
                    futures::future::pending::<()>().await;
                    Ok::<_, ()>(started)
                },
            ));
            let (mut peer, stream) = futures::channel::mpsc::unbounded::<Result<Message, ()>>();
            let (out, outgoing) = mpsc::channel(1);
            let (requests, inbound) = mpsc::channel(1);
            let dispatcher = tokio::spawn(zuse_rpc::serve_connection_bounded(
                std::sync::Arc::new(Fixture),
                out,
                inbound,
                64 * 1024,
            ));
            let (incoming, mut received) = mpsc::channel(1);
            requests
                .send(r#"{"id":1,"method":"large"}"#.into())
                .await
                .unwrap();
            let task = tokio::spawn(exchange(sink, stream, outgoing, incoming, compression));
            waiting.await.unwrap();
            let cancel = r#"{"id":1,"cancel":true}"#;
            peer.send(Ok(Message::Text(cancel.into()))).await.unwrap();
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(1), received.recv())
                    .await
                    .unwrap()
                    .as_deref(),
                Some(cancel)
            );
            peer.send(Ok(Message::Close(None))).await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap();
            drop(requests);
            dispatcher.await.unwrap();
        }
    }
}
