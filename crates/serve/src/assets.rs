//! Browser delivery shared by the engine and relay. Keep compression and
//! caching off authenticated APIs and long-lived WebSocket transports.
use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::Response,
};
use std::path::PathBuf;
use tower_http::{
    compression::CompressionLayer,
    services::{ServeDir, ServeFile},
};

pub(crate) fn router(directory: PathBuf) -> Router {
    Router::new()
        .fallback_service(
            ServeDir::new(&directory)
                .precompressed_br()
                .precompressed_gzip()
                .not_found_service(ServeFile::new(directory.join("index.html"))),
        )
        // Streaming gzip also covers builds without precompressed sidecars.
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn(cache_policy))
}

async fn cache_policy(request: Request, next: Next) -> Response {
    let fingerprinted = fingerprinted_asset(request.uri().path());
    let mut response = next.run(request).await;
    let html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));
    // SPA fallbacks and entry HTML must always discover the current build.
    // Only content-addressed files can safely survive a deployment in cache.
    let policy = if (response.status().is_success()
        || response.status() == StatusCode::NOT_MODIFIED)
        && fingerprinted
        && !html
    {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(policy));
    // Precompressed ServeDir responses also vary by the negotiated encoding.
    response
        .headers_mut()
        .append(header::VARY, HeaderValue::from_static("accept-encoding"));
    response
}

fn fingerprinted_asset(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or_default();
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    if !matches!(extension, "wasm" | "js" | "css" | "woff" | "woff2") {
        return false;
    }
    // Trunk names Rust output zuse-web-<hex>_bg.wasm and zuse-web-<hex>.js.
    let stem = stem.strip_suffix("_bg").unwrap_or(stem);
    let hash = |value: &str| value.len() >= 8 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    stem.rsplit_once('-').is_some_and(|(_, value)| hash(value))
        || stem.split_once('-').is_some_and(|(value, _)| hash(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use std::io::{Read, Write};
    use tower::ServiceExt;

    fn servers(directory: &std::path::Path) -> Vec<Router> {
        vec![
            crate::tests::server().router(directory.into()),
            crate::relay::RelayBroker::new(crate::relay::HostToken::parse("b".repeat(64)).unwrap())
                .router(directory.into()),
        ]
    }

    #[tokio::test]
    async fn direct_and_relay_compress_and_cache_browser_assets() {
        let dir = tempfile::tempdir().unwrap();
        let wasm = b"\0asm repeating application data ".repeat(2048);
        std::fs::write(dir.path().join("zuse-web-0123456789abcdef_bg.wasm"), &wasm).unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            "<!doctype html>current build",
        )
        .unwrap();
        for app in servers(dir.path()) {
            for gzip in [false, true] {
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri("/zuse-web-0123456789abcdef_bg.wasm")
                            .header(
                                header::ACCEPT_ENCODING,
                                if gzip { "gzip" } else { "identity" },
                            )
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(response.headers()[header::CONTENT_TYPE], "application/wasm");
                assert_eq!(
                    response.headers()[header::CACHE_CONTROL],
                    "public, max-age=31536000, immutable"
                );
                if gzip {
                    assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
                } else {
                    assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
                }
                let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
                if gzip {
                    assert!(body.len() < wasm.len() / 10);
                    let mut decoded = Vec::new();
                    flate2::read::GzDecoder::new(&body[..])
                        .read_to_end(&mut decoded)
                        .unwrap();
                    assert_eq!(decoded, wasm);
                } else {
                    assert_eq!(body.as_ref(), wasm);
                }
            }
            for path in ["/", "/chat/example", "/missing-0123456789abcdef.js"] {
                let response = app
                    .clone()
                    .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
            }
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/health")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert!(!response.headers().contains_key(header::CACHE_CONTROL));
        }
    }

    #[tokio::test]
    async fn precompressed_response_negotiates_without_double_encoding() {
        let dir = tempfile::tempdir().unwrap();
        let raw = b"export const value = 'cached application';".repeat(100);
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&raw).unwrap();
        let compressed = encoder.finish().unwrap();
        std::fs::write(dir.path().join("app-0123456789abcdef.js"), raw).unwrap();
        std::fs::write(dir.path().join("app-0123456789abcdef.js.gz"), &compressed).unwrap();
        for app in servers(dir.path()) {
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/app-0123456789abcdef.js")
                        .header(header::ACCEPT_ENCODING, "gzip")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
            assert!(
                response
                    .headers()
                    .get_all(header::VARY)
                    .iter()
                    .any(|value| value.to_str().unwrap().contains("accept-encoding"))
            );
            assert_eq!(
                to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .as_ref(),
                compressed
            );
        }
    }

    #[test]
    fn only_trunk_fingerprints_are_immutable() {
        for path in [
            "/zuse-web-0123456789abcdef_bg.wasm",
            "/zuse-web-0123456789abcdef.js",
            "/0123456789abcdef-initializer.js",
        ] {
            assert!(fingerprinted_asset(path));
        }
        for path in [
            "/index.html",
            "/worker.js",
            "/app-short.js",
            "/app-not-a-hash.wasm",
            "/chat-0123456789abcdef",
        ] {
            assert!(!fingerprinted_asset(path));
        }
    }
}
