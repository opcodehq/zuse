use super::*;
// Appended inside the exact source-bound hosted module by the private test harness.
#[cfg(test)]
mod hosted_session_tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use std::sync::atomic::AtomicUsize;
    use tower::ServiceExt;

    struct Service;
    #[async_trait::async_trait]
    impl RpcService for Service {
        async fn handle(
            &self,
            _: &str,
            _: serde_json::Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            Ok(zuse_rpc::RpcReply::Value(serde_json::Value::Null))
        }
    }
    struct Sessions(Arc<AtomicUsize>);
    impl Factory for Sessions {
        fn create(&self, public_controller_id: &str) -> anyhow::Result<SessionService> {
            // The actual browser draft bridge uses UUID::parse_str(device).
            anyhow::ensure!(
                uuid::Uuid::parse_str(public_controller_id).is_ok(),
                "browser draft storage requires a UUID controller identity"
            );
            let retired = self.0.clone();
            Ok(SessionService {
                rpc: Arc::new(Service),
                retire: Arc::new(move || {
                    retired.fetch_add(1, Ordering::SeqCst);
                }),
                authenticated: Arc::new(|| false),
                closed: CancellationToken::new(),
            })
        }
    }
    fn server(count: Arc<AtomicUsize>, max: usize) -> HostedBrowser {
        HostedBrowser::new(
            "https://cloud.example",
            Arc::new(Sessions(count)),
            max,
            Duration::from_secs(60),
        )
        .unwrap()
    }
    fn headers(cookie: Option<&str>) -> HeaderMap {
        let mut result = HeaderMap::new();
        result.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://cloud.example"),
        );
        result.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        if let Some(cookie) = cookie {
            result.insert(header::COOKIE, HeaderValue::from_str(cookie).unwrap());
        }
        result
    }
    async fn call(
        server: &HostedBrowser,
        method: &str,
        path: &str,
        headers: HeaderMap,
    ) -> Response {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        *request.headers_mut() = headers;
        server.clone().router().oneshot(request).await.unwrap()
    }
    fn cookie(response: &Response) -> String {
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }
    #[tokio::test]
    async fn anonymous_bootstrap_is_scoped_and_csrf_fails_before_creation() {
        let count = Arc::new(AtomicUsize::new(0));
        let server = server(count.clone(), 2);
        assert_eq!(
            call(&server, "POST", "/api/session/check", HeaderMap::new())
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let mut foreign = headers(None);
        foreign.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://foreign.example"),
        );
        assert_eq!(
            call(&server, "POST", "/api/session/check", foreign)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let mut cross_site = headers(None);
        cross_site.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
        assert_eq!(
            call(&server, "POST", "/api/session/check", cross_site)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert!(server.sessions.lock().unwrap().is_empty());
        let a = call(&server, "POST", "/api/session/check", headers(None)).await;
        assert_eq!(a.status(), StatusCode::OK);
        let ca = cookie(&a);
        let b = call(&server, "POST", "/api/session/check", headers(None)).await;
        let cb = cookie(&b);
        assert_ne!(ca, cb);
        assert_eq!(server.sessions.lock().unwrap().len(), 2);
        let ea = server.lookup(&headers(Some(&ca))).unwrap();
        let eb = server.lookup(&headers(Some(&cb))).unwrap();
        assert!(!Arc::ptr_eq(&ea, &eb));
        assert!(!Arc::ptr_eq(&ea.service.rpc, &eb.service.rpc));
        assert_eq!(
            call(&server, "POST", "/api/session/check", headers(Some(&ca)))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            call(&server, "POST", "/api/session/check", headers(None))
                .await
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert!(
            a.headers()[header::SET_COOKIE]
                .to_str()
                .unwrap()
                .contains("HttpOnly; Secure; SameSite=Strict")
        );
        assert_eq!(a.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
    #[tokio::test(start_paused = true)]
    async fn logout_and_expiry_retire_exactly_once_without_retiring_peer() {
        let count = Arc::new(AtomicUsize::new(0));
        let server = server(count.clone(), 2);
        let ca = cookie(&call(&server, "POST", "/api/session/check", headers(None)).await);
        let cb = cookie(&call(&server, "POST", "/api/session/check", headers(None)).await);
        let a = server.lookup(&headers(Some(&ca))).unwrap();
        let b = server.lookup(&headers(Some(&cb))).unwrap();
        assert_eq!(
            call(&server, "DELETE", "/api/session", headers(Some(&ca)))
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
        assert!(a.closed.is_cancelled());
        assert!(!b.closed.is_cancelled());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        drop(a);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(61)).await;
        assert!(server.lookup(&headers(Some(&cb))).is_none());
        assert!(b.closed.is_cancelled());
        drop(b);
        assert_eq!(count.load(Ordering::SeqCst), 2);
        let fresh = call(&server, "POST", "/api/session/check", headers(Some(&cb))).await;
        assert_eq!(fresh.status(), StatusCode::OK);
        assert_eq!(fresh.headers()[CHANGED], "1");
        assert_ne!(cookie(&fresh), cb);
    }
    #[tokio::test]
    async fn duplicate_cookie_cannot_select_an_actor_and_socket_slots_are_nonwaiting() {
        let server = server(Arc::new(AtomicUsize::new(0)), 2);
        let ca = cookie(&call(&server, "POST", "/api/session/check", headers(None)).await);
        assert!(
            server
                .lookup(&headers(Some(&format!("{ca}; {ca}"))))
                .is_none()
        );
        let entry = server.lookup(&headers(Some(&ca))).unwrap();
        let first = entry.sockets.clone().try_acquire_owned().unwrap();
        let second = entry.sockets.clone().try_acquire_owned().unwrap();
        assert!(entry.sockets.clone().try_acquire_owned().is_err());
        drop(first);
        assert!(entry.sockets.clone().try_acquire_owned().is_ok());
        drop(second);
        let mut global = Vec::new();
        for _ in 0..8 {
            global.push(server.slots.clone().try_acquire_owned().unwrap());
        }
        assert!(server.slots.clone().try_acquire_owned().is_err());
        drop(global);
        assert!(server.slots.clone().try_acquire_owned().is_ok());
    }
    #[tokio::test]
    async fn hosted_assets_have_app_base_correct_mime_and_no_rest_spa_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let assets = directory.path();
        std::fs::write(
            assets.join("index.html"),
            "<script type=module src='/client.js'></script>",
        )
        .unwrap();
        assert!(
            server(Arc::new(AtomicUsize::new(0)), 1)
                .router_with_assets(assets.into())
                .is_err()
        );
        std::fs::write(
            assets.join("index.html"),
            "<script type=module src='/app/client.js'></script>",
        )
        .unwrap();
        std::fs::write(assets.join("client.js"), "export const ready=true;").unwrap();
        std::fs::write(assets.join("client.wasm"), b"\0asm").unwrap();
        let router = server(Arc::new(AtomicUsize::new(0)), 1)
            .router_with_assets(assets.into())
            .unwrap();
        let call = |uri: &str| {
            router
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        };
        let redirect = call("/app").await.unwrap();
        assert_eq!(redirect.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(redirect.headers()[header::LOCATION], "/app/");
        let page = call("/app/").await.unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!(
            page.headers()["cross-origin-embedder-policy"],
            "require-corp"
        );
        let js = call("/app/client.js").await.unwrap();
        assert_eq!(js.status(), StatusCode::OK);
        assert!(
            js.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .contains("javascript")
        );
        let wasm = call("/app/client.wasm").await.unwrap();
        assert_eq!(wasm.status(), StatusCode::OK);
        assert_eq!(wasm.headers()[header::CONTENT_TYPE], "application/wasm");
        assert_eq!(
            call("/v1/cloud/workspaces/missing").await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call("/api/not-registered").await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }
    #[tokio::test]
    async fn cookie_absent_replacement_is_authoritative_and_socket_pressure_is_retryable() {
        let server = server(Arc::new(AtomicUsize::new(0)), 2);
        let first = call(&server, "POST", "/api/session/check", headers(None)).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(
            first.headers().get(CHANGED).and_then(|v| v.to_str().ok()),
            Some("1"),
            "new anonymous controller must revoke previous SPA authority even when old cookie expired"
        );
        let cookie = cookie(&first);
        let entry = server.lookup(&headers(Some(&cookie))).unwrap();
        let first_slot = entry.sockets.clone().try_acquire_owned().unwrap();
        let second_slot = entry.sockets.clone().try_acquire_owned().unwrap();
        let busy = call(
            &server,
            "POST",
            "/api/session/check",
            headers(Some(&cookie)),
        )
        .await;
        assert_eq!(busy.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(busy.headers()[header::RETRY_AFTER], "3");
        assert!(!busy.headers().contains_key(CHANGED));
        assert!(!entry.closed.is_cancelled());
        drop(first_slot);
        drop(second_slot);
        let retry = call(
            &server,
            "POST",
            "/api/session/check",
            headers(Some(&cookie)),
        )
        .await;
        assert_eq!(retry.status(), StatusCode::OK);
        assert!(!retry.headers().contains_key(CHANGED));
    }
    #[tokio::test]
    async fn full_socket_budget_does_not_allocate_unreachable_anonymous_sessions() {
        let server = server(Arc::new(AtomicUsize::new(0)), 2);
        let mut slots = Vec::new();
        for _ in 0..8 {
            slots.push(server.slots.clone().try_acquire_owned().unwrap());
        }
        let response = call(&server, "POST", "/api/session/check", headers(None)).await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(server.sessions.lock().unwrap().is_empty());
        drop(slots);
        assert_eq!(
            call(&server, "POST", "/api/session/check", headers(None))
                .await
                .status(),
            StatusCode::OK
        );
    }
}
