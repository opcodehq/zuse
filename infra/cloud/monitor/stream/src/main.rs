//! Shared H.264 capture with bounded per-viewer lag and one access unit per packet.
use axum::{
    Router,
    body::Body,
    extract::{
        State,
        ws::{Message, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use bytes::Bytes;
use futures_util::{Stream, StreamExt, stream};
use std::{convert::Infallible, io, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    sync::{OwnedSemaphorePermit, Semaphore, broadcast},
};

#[derive(Clone)]
struct Display {
    key: Arc<String>,
    frames: broadcast::Sender<Bytes>,
    viewers: Arc<Semaphore>,
    screenshots: Arc<Semaphore>,
}

fn authorized(s: &Display, h: &HeaderMap) -> bool {
    h.get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        == Some(s.key.as_str())
}

fn grab() -> Command {
    let mut cmd = Command::new(std::env::var_os("ZUSE_FFMPEG").unwrap_or_else(|| "ffmpeg".into()));
    cmd.kill_on_drop(true).stdin(Stdio::null()).args([
        "-nostdin",
        "-loglevel",
        "error",
        "-probesize",
        "32",
        "-filter_threads",
        "2",
        "-f",
        "x11grab",
        "-framerate",
        "60",
        "-video_size",
        "1920x1080",
        "-i",
        ":91",
    ]);
    cmd
}

async fn capture(tx: &broadcast::Sender<Bytes>) -> io::Result<()> {
    let mut child = Command::new("zuse-capture")
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut pipe = child.stdout.take().unwrap();
    while tx.receiver_count() > 0 {
        let mut header = [0; 12];
        pipe.read_exact(&mut header).await?;
        let length = (u32::from_le_bytes(header[..4].try_into().unwrap()) & 0x3fff_ffff) as usize;
        if !(1..=8 * 1024 * 1024).contains(&length) {
            return Err(io::Error::other("invalid capture packet"));
        }
        let mut packet = vec![0; 12 + length];
        packet[..12].copy_from_slice(&header);
        pipe.read_exact(&mut packet[12..]).await?;
        let _ = tx.send(packet.into());
    }
    Ok(())
}

fn packets(
    frames: &broadcast::Sender<Bytes>,
    permit: OwnedSemaphorePermit,
) -> impl Stream<Item = Result<Bytes, Infallible>> + use<> {
    let state = (frames.subscribe(), true, permit);
    stream::unfold(state, |(mut rx, mut needs_keyframe, permit)| async move {
        loop {
            match rx.recv().await {
                Ok(frame) if !needs_keyframe || frame[3] & 0x80 != 0 => {
                    return Some((Ok::<_, Infallible>(frame), (rx, false, permit)));
                }
                Ok(_) => (),
                Err(broadcast::error::RecvError::Lagged(_)) => needs_keyframe = true,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
}

async fn websocket(State(s): State<Display>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    // Browser WebSockets cannot set Authorization. Offer the credential as an
    // additional protocol and echo only the constant application protocol.
    let protocols = headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let offered = protocols.split(',').map(str::trim);
    if !offered.clone().any(|p| p == "zuse.monitor.v1")
        || !offered
            .into_iter()
            .any(|p| p.strip_prefix("bearer.") == Some(s.key.as_str()))
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(permit) = s.viewers.try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    ws.protocols(["zuse.monitor.v1"])
        .max_message_size(1024)
        .max_frame_size(1024)
        .on_upgrade(move |mut socket| async move {
            let frames = packets(&s.frames, permit);
            tokio::pin!(frames);
            loop {
                tokio::select! {
                    incoming = socket.recv() => match incoming {
                        Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                        _ => break,
                    },
                    frame = frames.next() => {
                        let Some(Ok(frame)) = frame else { break };
                        let send = socket.send(Message::Binary(frame));
                        if !matches!(tokio::time::timeout(Duration::from_secs(4), send).await, Ok(Ok(()))) {
                            break;
                        }
                    }
                }
            }
        })
}

async fn video(State(s): State<Display>, headers: HeaderMap) -> Response {
    if !authorized(&s, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(permit) = s.viewers.try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let body = packets(&s.frames, permit);
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "no-store, no-transform"),
        ],
        Body::from_stream(body),
    )
        .into_response()
}

async fn screenshot(State(s): State<Display>, headers: HeaderMap) -> Response {
    if !authorized(&s, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(_permit) = s.screenshots.try_acquire() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        grab()
            .args([
                "-frames:v",
                "1",
                "-threads",
                "1",
                "-f",
                "image2pipe",
                "-c:v",
                "png",
                "pipe:1",
            ])
            .output(),
    )
    .await;
    match result {
        Ok(Ok(out)) if out.status.success() => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            out.stdout,
        )
            .into_response(),
        _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let key = std::env::var("ZUSE_DISPLAY_PASSWORD").expect("missing viewer credential");
    assert!(key.len() >= 32);
    let (frames, _) = broadcast::channel(8);
    let source = frames.clone();
    tokio::spawn(async move {
        loop {
            if source.receiver_count() > 0
                && let Err(e) = capture(&source).await
            {
                eprintln!("capture failed: {e}");
            }
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
    });
    let app = Router::new()
        .route(
            "/",
            get(|| async {
                (
                    [(header::REFERRER_POLICY, "no-referrer")],
                    Html(include_str!("viewer.html")),
                )
            }),
        )
        .route(
            "/ready",
            get(|State(s): State<Display>, headers: HeaderMap| async move {
                if authorized(&s, &headers) {
                    StatusCode::OK
                } else {
                    StatusCode::UNAUTHORIZED
                }
            }),
        )
        .route("/stream", get(video))
        .route("/live", get(websocket))
        .route("/screenshot", get(screenshot))
        .with_state(Display {
            key: Arc::new(key),
            frames,
            viewers: Arc::new(Semaphore::new(16)),
            screenshots: Arc::new(Semaphore::new(2)),
        });
    let port = std::env::var("ZUSE_DISPLAY_PORT").unwrap_or_else(|_| "6901".into());
    axum::serve(
        tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?,
        app,
    )
    .await
}
