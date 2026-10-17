use super::HostToken;
use crate::BrowserServer;
use axum::extract::ws::Message;
use futures::{SinkExt, StreamExt};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::task::JoinSet;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{self, client::IntoClientRequest, protocol::WebSocketConfig},
};
use zuse_remote::{MAX_RECORD_BYTES, PairId, authority::Credentials};
/// Maintain outbound rendezvous sockets for saved grants. RPC and Noise remain
/// authorized by the execution host, including revocation and profile scope.
/// This future owns all workers; cancellation or drop cannot detach them.
pub async fn serve_host(
    server: BrowserServer,
    origin: &str,
    token: HostToken,
) -> anyhow::Result<()> {
    let origin = zuse_remote::normalized_origin(origin)?;
    let (authority, device) = server
        .pairings
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Pairing authority is unavailable"))?;
    let mut running = HashSet::new();
    let mut tasks = JoinSet::new();
    let mut scan = tokio::time::interval(Duration::from_secs(1));
    scan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {biased;
            _=server.shutdown.cancelled()=>break,
            done=tasks.join_next(),if !tasks.is_empty()=>{if let Some(Ok(id))=done{running.remove(&id);}},
            _=scan.tick()=>{
                let Ok(peers)=authority.list()else{break;};
                for peer in peers {
                    let Ok(id)=PairId::parse(&peer.id)else{continue;};
                    // Refresh expiry even for existing workers, cancelling stale invitations.
                    let Ok(credentials)=authority.credentials(&id)else{continue;};
                    if !running.insert(peer.id.clone()){continue;}
                    let server=server.clone();let token=token.clone();let origin=origin.clone();let authority=authority.clone();let device=device.clone();
                    tasks.spawn(async move{
                        let credentials=Arc::new(credentials);
                        let workers=(0..4).map(|_|worker(server.clone(),authority.clone(),device.clone(),credentials.clone(),origin.clone(),token.clone()));
                        futures::future::join_all(workers).await;peer.id
                    });
                }
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}
async fn worker(
    server: BrowserServer,
    authority: zuse_remote::authority::Authority,
    device: String,
    credentials: Arc<Credentials>,
    origin: String,
    token: HostToken,
) {
    let scheme = if origin.starts_with("https:") {
        "wss"
    } else {
        "ws"
    };
    let url = format!(
        "{scheme}{}/api/paired-host/{}",
        &origin[origin.find(':').unwrap()..],
        credentials.id().as_str()
    );
    let mut delay = Duration::from_millis(250);
    loop {
        if !credentials.is_live() || server.shutdown.is_cancelled() {
            return;
        }
        let attempt = async {
            let mut request = url.clone().into_client_request().map_err(|_| ())?;
            request.headers_mut().insert(
                tungstenite::http::header::AUTHORIZATION,
                token.header().parse().map_err(|_| ())?,
            );
            let config = WebSocketConfig {
                max_message_size: Some(MAX_RECORD_BYTES),
                max_frame_size: Some(MAX_RECORD_BYTES),
                ..Default::default()
            };
            let (mut socket, _) = tokio::time::timeout(
                Duration::from_secs(10),
                connect_async_with_config(request, Some(config), false),
            )
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
            let mut ping = tokio::time::interval(Duration::from_secs(15));
            let mut seen = tokio::time::Instant::now();
            loop {
                tokio::select! {
                    _=ping.tick()=>{
                        if seen.elapsed()>Duration::from_secs(45){return Err(());}
                        tokio::time::timeout(Duration::from_secs(5),socket.send(tungstenite::Message::Ping(vec![]))).await.map_err(|_|())?.map_err(|_|())?;
                    },
                    frame=socket.next()=>{seen=tokio::time::Instant::now();match frame {
                        Some(Ok(tungstenite::Message::Text(text)))if text=="connected"=>break,
                        Some(Ok(tungstenite::Message::Ping(data)))=>{tokio::time::timeout(Duration::from_secs(5),socket.send(tungstenite::Message::Pong(data))).await.map_err(|_|())?.map_err(|_|())?;},
                        Some(Ok(tungstenite::Message::Pong(_)))=>{},_=>return Err(())
                    }}
                }
            }
            let _permit = server
                .paired_slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| ())?;
            // Translate framing only; the same host session performs all crypto,
            // proof confirmation and canonical authorization as direct access.
            let socket = socket
                .with(|message: Message| {
                    futures::future::ready(Ok::<_, tungstenite::Error>(match message {
                        Message::Binary(data) => tungstenite::Message::Binary(data.to_vec()),
                        Message::Ping(data) => tungstenite::Message::Ping(data.to_vec()),
                        Message::Pong(data) => tungstenite::Message::Pong(data.to_vec()),
                        _ => tungstenite::Message::Close(None),
                    }))
                })
                .map(|frame| {
                    frame.map(|message| match message {
                        tungstenite::Message::Binary(data) => Message::Binary(data.into()),
                        tungstenite::Message::Ping(data) => Message::Ping(data.into()),
                        tungstenite::Message::Pong(data) => Message::Pong(data.into()),
                        _ => Message::Close(None),
                    })
                });
            crate::paired::session(
                server.clone(),
                authority.clone(),
                device.clone(),
                credentials.clone(),
                socket,
            )
            .await;
            Ok::<_, ()>(())
        };
        let success = tokio::select! {biased;
            _=server.shutdown.cancelled()=>return,
            _=credentials.revoked()=>return,
            result=attempt=>result.is_ok(),
        };
        delay = if success {
            Duration::from_millis(250)
        } else {
            (delay * 2).min(Duration::from_secs(10))
        };
        tokio::select! {_=server.shutdown.cancelled()=>return,_=credentials.revoked()=>return,_=tokio::time::sleep(delay)=>{}}
    }
}
