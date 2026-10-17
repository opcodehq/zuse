//! Native paired connections never downgrade to the plaintext RPC dialer.
use crate::{RpcClient, RpcError};
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use zuse_remote::{MAX_RECORD_BYTES, PairedTarget};

/// A fresh authenticated connection. No automatic replay of prior requests.
/// Dropping it closes its socket; a reconnect must start a new Noise handshake.
pub async fn connect_paired(target: &PairedTarget) -> Result<RpcClient, RpcError> {
    let authenticate = async {
        let config = WebSocketConfig {
            max_message_size: Some(MAX_RECORD_BYTES),
            max_frame_size: Some(MAX_RECORD_BYTES),
            ..Default::default()
        };
        let (mut socket, _) = connect_async_with_config(target.socket_url(), Some(config), false)
            .await
            .map_err(|_| RpcError::Transport("Could not reach paired engine".into()))?;
        let invitation = target.invitation();
        let (handshake, first) = zuse_remote::initiate(&invitation.id, &invitation.secret)
            .map_err(|_| RpcError::Transport("Could not start paired authentication".into()))?;
        socket
            .send(Message::Binary(first))
            .await
            .map_err(|_| RpcError::Closed)?;
        loop {
            match socket.next().await {
                Some(Ok(Message::Binary(reply))) => {
                    let channel = handshake.finish(&reply).map_err(|_| {
                        RpcError::Transport("Paired engine authentication failed".into())
                    })?;
                    return Ok((socket, channel));
                }
                Some(Ok(Message::Ping(payload))) => socket
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|_| RpcError::Closed)?,
                Some(Ok(Message::Pong(_))) => {}
                _ => {
                    return Err(RpcError::Transport(
                        "Paired engine authentication failed".into(),
                    ));
                }
            }
        }
    };
    let (socket, mut channel) = tokio::time::timeout(Duration::from_secs(10), authenticate)
        .await
        .map_err(|_| RpcError::Transport("Paired engine connection timed out".into()))??;
    let (mut writer, mut reader) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(16);
    let (in_tx, in_rx) = mpsc::channel::<String>(16);
    tokio::spawn(async move {
        let exchange = async {
            let mut ping = tokio::time::interval(Duration::from_secs(20));
            ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last_seen = tokio::time::Instant::now();
            loop {
                tokio::select! {
                    _ = in_tx.closed() => return Err(()),
                    _ = ping.tick() => {
                        if last_seen.elapsed() >= Duration::from_secs(60) { return Err(()); }
                        writer.send(Message::Ping(vec![])).await.map_err(|_| ())?;
                    },
                    outgoing = out_rx.recv() => {
                        let Some(outgoing) = outgoing else { return Ok::<_, ()>(()); };
                        for record in channel.seal(outgoing.as_bytes()).map_err(|_| ())? {
                            tokio::time::timeout(Duration::from_secs(30), writer.send(Message::Binary(record)))
                                .await.map_err(|_| ())?.map_err(|_| ())?;
                        }
                    },
                    incoming = reader.next() => {
                        last_seen = tokio::time::Instant::now();
                        match incoming {
                            Some(Ok(Message::Binary(record))) => {
                                if let Some(message) = channel.open(&record).map_err(|_| ())? {
                                    let message = String::from_utf8(message).map_err(|_| ())?;
                                    in_tx.send(message).await.map_err(|_| ())?;
                                }
                            },
                            Some(Ok(Message::Ping(payload))) => writer.send(Message::Pong(payload)).await.map_err(|_| ())?,
                            Some(Ok(Message::Pong(_))) => {},
                            _ => return Err(()),
                        }
                    }
                }
            }
        };
        tokio::select! {
            _ = in_tx.closed() => {},
            _ = exchange => {},
        }
        channel.close();
        // Drop both halves instead of waiting on an unresponsive peer's close ACK.
    });
    Ok(RpcClient::new(out_tx, in_rx))
}
