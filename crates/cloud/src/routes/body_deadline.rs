//! Request bodies have a total read deadline. A client that opens a request and
//! drips bytes cannot keep a handler, buffer, or admission permit indefinitely.
use super::*;
use futures::StreamExt;
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;

/// Generous for the largest JSON and chunked-upload bodies on the main listener.
pub(super) const BODY_DEADLINE: Duration = Duration::from_secs(30);
/// GitHub delivers from its own network; a full 25 MB payload arrives well inside this.
pub(super) const WEBHOOK_BODY_DEADLINE: Duration = Duration::from_secs(20);
/// Concurrent webhook deliveries buffered at once (memory bound: permits x 25 MB).
pub(crate) const WEBHOOK_PERMITS: usize = 4;
/// A busy receiver waits briefly for a slot instead of queueing without bound.
pub(super) const WEBHOOK_ADMISSION_WAIT: Duration = Duration::from_secs(2);

/// Wraps the request body so that reading fails once `limit` has elapsed since
/// the request reached the handler stack. Bodiless methods are left untouched,
/// which keeps WebSocket upgrades and long-lived response streams unaffected.
pub(super) fn with_body_deadline(
    request: axum::extract::Request,
    limit: Duration,
) -> axum::extract::Request {
    if matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) {
        return request;
    }
    let deadline = tokio::time::Instant::now() + limit;
    let (parts, body) = request.into_parts();
    let stream = futures::stream::unfold(Some(body.into_data_stream()), move |state| async move {
        let mut stream = state?;
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(bytes))) => Some((Ok(bytes), Some(stream))),
            Ok(Some(Err(error))) => Some((Err(std::io::Error::other(error)), None)),
            Ok(None) => None,
            Err(_) => Some((
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "request body deadline exceeded",
                )),
                None,
            )),
        }
    });
    axum::extract::Request::from_parts(parts, axum::body::Body::from_stream(stream))
}

pub(super) async fn body_deadline(
    request: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    next.run(with_body_deadline(request, BODY_DEADLINE)).await
}

pub(super) fn webhook_gate() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(WEBHOOK_PERMITS))
}

pub(super) async fn github_webhook_admission(
    State(gate): State<Arc<Semaphore>>,
    request: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    // Bound buffered memory with a small permit pool, but never let one slow
    // sender hold a permit past the body deadline or block others indefinitely.
    let Ok(Ok(_permit)) = tokio::time::timeout(WEBHOOK_ADMISSION_WAIT, gate.acquire_owned()).await
    else {
        return ApiError::new(StatusCode::TOO_MANY_REQUESTS, "github_webhook_busy").into_response();
    };
    next.run(with_body_deadline(request, WEBHOOK_BODY_DEADLINE))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;

    fn slow_body() -> Body {
        // One byte, then silence: the classic slow-body pattern.
        Body::from_stream(futures::stream::unfold(0u8, |step| async move {
            if step == 0 {
                Some((Ok::<_, std::io::Error>(Bytes::from_static(b"{")), 1))
            } else {
                std::future::pending::<()>().await;
                None
            }
        }))
    }

    #[tokio::test(start_paused = true)]
    async fn slow_body_is_cut_off_at_the_deadline() {
        let app = Router::new().route(
            "/upload",
            post(|body: Bytes| async move { body.len().to_string() })
                .layer(middleware::from_fn(body_deadline)),
        );
        let request = axum::http::Request::post("/upload")
            .body(slow_body())
            .unwrap();
        let started = tokio::time::Instant::now();
        let response = app.oneshot(request).await.unwrap();
        assert!(response.status().is_client_error());
        assert!(started.elapsed() >= BODY_DEADLINE);
        assert!(started.elapsed() < BODY_DEADLINE + Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn complete_bodies_and_bodiless_requests_pass_through() {
        let app = Router::new()
            .route(
                "/upload",
                post(|body: Bytes| async move { body.len().to_string() }),
            )
            .route("/stream", get(|| async { "ok" }))
            .layer(middleware::from_fn(body_deadline));
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::post("/upload")
                    .body(Body::from("hello"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 64)
            .await
            .unwrap();
        assert_eq!(&body[..], b"5");
        let response = app
            .oneshot(
                axum::http::Request::get("/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test(start_paused = true)]
    async fn slow_webhook_senders_cannot_starve_a_real_delivery() {
        let gate = webhook_gate();
        let app = Router::new().route(
            "/webhook",
            post(|body: Bytes| async move { body.len().to_string() })
                .layer(DefaultBodyLimit::max(1024))
                .layer(middleware::from_fn_with_state(
                    gate.clone(),
                    github_webhook_admission,
                )),
        );
        // Fill every permit with a slow sender.
        let mut slow = Vec::new();
        for _ in 0..WEBHOOK_PERMITS {
            let app = app.clone();
            slow.push(tokio::spawn(async move {
                app.oneshot(
                    axum::http::Request::post("/webhook")
                        .body(slow_body())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
            }));
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(gate.available_permits(), 0);
        // While saturated, a new delivery is rejected after a short wait, not queued.
        let started = tokio::time::Instant::now();
        let busy = app
            .clone()
            .oneshot(
                axum::http::Request::post("/webhook")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(busy.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(started.elapsed() <= WEBHOOK_ADMISSION_WAIT + Duration::from_millis(10));
        // Slow senders are cut off at the deadline and release their permits.
        for task in slow {
            assert!(task.await.unwrap().is_client_error());
        }
        assert_eq!(gate.available_permits(), WEBHOOK_PERMITS);
        let delivered = app
            .oneshot(
                axum::http::Request::post("/webhook")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delivered.status(), StatusCode::OK);
    }
}
