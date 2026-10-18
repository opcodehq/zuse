//! Native paired connections never downgrade to the plaintext RPC dialer.
use crate::{RpcClient, RpcError};
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use zuse_remote::{
    MAX_RECORD_BYTES, PairedTarget, SecureChannel,
    heartbeat::{Action, Heartbeat},
};

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
    let (mut socket, mut channel) = tokio::time::timeout(Duration::from_secs(10), authenticate)
        .await
        .map_err(|_| RpcError::Transport("Paired engine connection timed out".into()))??;
    let clock = tokio::time::Instant::now();
    let mut heartbeat = Heartbeat::new();
    let confirm = async {
        let Action::Send(probe) = heartbeat.poll(0).map_err(|_| ())? else {
            return Err(());
        };
        send_records(&mut socket, &mut channel, &probe).await?;
        loop {
            match socket.next().await {
                Some(Ok(Message::Binary(record))) => {
                    if let Some(payload) = channel.open(&record).map_err(|_| ())? {
                        if !heartbeat
                            .consume(&payload, clock.elapsed().as_millis() as u64)
                            .map_err(|_| ())?
                        {
                            return Err(());
                        }
                        return Ok::<_, ()>(());
                    }
                }
                Some(Ok(Message::Ping(payload))) => {
                    send(&mut socket, Message::Pong(payload)).await?
                }
                Some(Ok(Message::Pong(_))) => {}
                _ => return Err(()),
            }
        }
    };
    tokio::time::timeout(
        Duration::from_millis(zuse_remote::heartbeat::INITIAL_TIMEOUT_MS),
        confirm,
    )
    .await
    .map_err(|_| RpcError::Transport(zuse_remote::Error::Heartbeat.to_string()))?
    .map_err(|_| RpcError::Transport(zuse_remote::Error::Heartbeat.to_string()))?;
    let (mut writer, mut reader) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(16);
    let (in_tx, in_rx) = mpsc::channel::<String>(16);
    tokio::spawn(async move {
        let exchange = async {
            loop {
                let wait = match heartbeat
                    .poll(clock.elapsed().as_millis() as u64)
                    .map_err(|_| ())?
                {
                    Action::Send(probe) => {
                        send_records(&mut writer, &mut channel, &probe).await?;
                        continue;
                    }
                    Action::Wait(ms) => ms,
                    Action::Expired => return Err(()),
                };
                tokio::select! {
                    _ = in_tx.closed() => return Err(()),
                    _ = tokio::time::sleep(Duration::from_millis(wait)) => continue,
                    outgoing = out_rx.recv() => {
                        let Some(outgoing) = outgoing else { return Ok::<_, ()>(()); };
                        send_records(&mut writer, &mut channel, outgoing.as_bytes()).await?;
                    },
                    incoming = reader.next() => {
                        match incoming {
                            Some(Ok(Message::Binary(record))) => {
                                if let Some(message) = channel.open(&record).map_err(|_| ())? {
                                    if heartbeat.consume(&message, clock.elapsed().as_millis() as u64).map_err(|_|())? { continue; }
                                    let message = String::from_utf8(message).map_err(|_| ())?;
                                    tokio::time::timeout(Duration::from_secs(30),in_tx.send(message)).await.map_err(|_|())?.map_err(|_|())?;
                                }
                            },
                            Some(Ok(Message::Ping(payload))) => send(&mut writer, Message::Pong(payload)).await?,
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

async fn send<W: futures::Sink<Message> + Unpin>(
    writer: &mut W,
    message: Message,
) -> Result<(), ()> {
    tokio::time::timeout(Duration::from_secs(30), writer.send(message))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}
async fn send_records<W: futures::Sink<Message> + Unpin>(
    writer: &mut W,
    channel: &mut SecureChannel,
    payload: &[u8],
) -> Result<(), ()> {
    // Bound the entire message, including all fragments, rather than restarting
    // a separate deadline for each record while a liveness check is pending.
    tokio::time::timeout(Duration::from_secs(30), async {
        for record in channel.seal(payload).map_err(|_| ())? {
            writer.send(Message::Binary(record)).await.map_err(|_| ())?;
        }
        Ok(())
    })
    .await
    .map_err(|_| ())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn link_pongs_cannot_complete_encrypted_host_confirmation() {
        let invitation = zuse_remote::Invitation::generate().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = PairedTarget::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            invitation.clone(),
        )
        .unwrap();
        let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let Message::Binary(first) = socket.next().await.unwrap().unwrap() else {
                panic!("missing handshake")
            };
            let (_channel, reply) =
                zuse_remote::accept(&invitation.id, &invitation.secret, &first).unwrap();
            socket.send(Message::Binary(reply)).await.unwrap();
            let Message::Binary(_) = socket.next().await.unwrap().unwrap() else {
                panic!("missing encrypted challenge")
            };
            // Model an endpoint that maintains its link but never returns the
            // authenticated application challenge. Pongs are not host proof.
            socket.send(Message::Pong(vec![])).await.unwrap();
            let _ = seen_tx.send(());
            while socket.next().await.is_some() {}
        });
        let connection = tokio::spawn(async move { connect_paired(&target).await });
        seen_rx.await.unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_millis(
            zuse_remote::heartbeat::INITIAL_TIMEOUT_MS + 1,
        ))
        .await;
        let error = match connection.await.unwrap() {
            Ok(_) => panic!("pongs are not proof"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("did not confirm responsiveness"));
        peer.abort();
        let _ = peer.await;
    }
}
