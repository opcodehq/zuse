use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::{
    io::AsyncReadExt,
    net::TcpListener,
    sync::{Semaphore, mpsc},
    task::JoinHandle,
};

const OLD: &str = r#"{"cached-model":{"input_cost_per_token":2}}"#;
const NEW: &str = r#"{"cached-model":{"input_cost_per_token":3}}"#;
struct Server {
    url: String,
    hits: Arc<AtomicUsize>,
    received: mpsc::UnboundedReceiver<()>,
    gate: Arc<Semaphore>,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(body: &'static str, status: u16, hold: bool) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/catalogue", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let gate = Arc::new(Semaphore::new(if hold { 0 } else { 100 }));
    let wait = gate.clone();
    let (sender, received) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let mut request = [0; 2048];
            let _ = socket.read(&mut request).await;
            count.fetch_add(1, Ordering::SeqCst);
            let _ = sender.send(());
            let _permit = wait.acquire().await.unwrap();
            let response = format!(
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    Server {
        url,
        hits,
        received,
        gate,
        task,
    }
}
fn write(path: &Path, data: &str, stale: bool) {
    std::fs::create_dir_all(path).unwrap();
    let file = path.join("model-prices.json");
    std::fs::write(&file, data).unwrap();
    if stale {
        std::fs::File::options()
            .write(true)
            .open(file)
            .unwrap()
            .set_modified(SystemTime::now() - TTL - Duration::from_secs(1))
            .unwrap();
    }
}
fn estimate(table: &Arc<Catalogue>) -> Option<f64> {
    table.estimate(
        "cached-model",
        &crate::TokenCounts {
            input_tokens: 1,
            ..Default::default()
        },
        false,
    )
}
async fn hit(server: &mut Server) {
    tokio::time::timeout(Duration::from_secs(3), server.received.recv())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn fresh_disk_cache_skips_network_even_for_forced_reports() {
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), OLD, false);
    let server = serve(NEW, 200, false).await;
    let prices = Pricing::remote(temp.path(), &server.url, TIMEOUT);
    assert_eq!(estimate(&prices.load(true).await), Some(2.));
    assert_eq!(server.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stale_cache_returns_immediately_and_concurrent_refreshes_share_one_download() {
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), OLD, true);
    let mut server = serve(NEW, 200, true).await;
    let prices = Pricing::remote(temp.path(), &server.url, TIMEOUT);
    let stale = tokio::time::timeout(Duration::from_secs(1), prices.load(false))
        .await
        .unwrap();
    assert_eq!(estimate(&stale), Some(2.));
    hit(&mut server).await;
    let mut callers = Vec::new();
    for _ in 0..8 {
        let prices = prices.clone();
        callers.push(tokio::spawn(async move { prices.load(true).await }));
    }
    server.gate.add_permits(1);
    for caller in callers {
        assert_eq!(estimate(&caller.await.unwrap()), Some(3.));
    }
    assert_eq!(server.hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read_to_string(temp.path().join("model-prices.json")).unwrap(),
        NEW
    );
    let reopened = Pricing::remote(temp.path(), &server.url, TIMEOUT);
    assert_eq!(estimate(&reopened.load(true).await), Some(3.));
    assert_eq!(server.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn bundled_fallback_does_not_wait_and_expired_memory_refreshes() {
    let temp = tempfile::tempdir().unwrap();
    let mut server = serve(NEW, 200, true).await;
    let prices = Pricing::remote(temp.path(), &server.url, TIMEOUT);
    let available = tokio::time::timeout(Duration::from_secs(1), prices.load(false))
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&available, &Catalogue::bundled()));
    hit(&mut server).await;
    server.gate.add_permits(1);
    assert_eq!(estimate(&prices.load(true).await), Some(3.));
    prices.remote.as_ref().unwrap().state.lock().await.fresh_at =
        Some(SystemTime::now() - TTL - Duration::from_secs(1));
    assert_eq!(estimate(&prices.load(true).await), Some(3.));
    assert_eq!(server.hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn bad_responses_and_timeouts_preserve_valid_stale_prices() {
    for (body, status) in [("{}", 200), ("{broken", 200), (NEW, 503)] {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), OLD, true);
        let server = serve(body, status, false).await;
        let prices = Pricing::remote(temp.path(), &server.url, TIMEOUT);
        assert_eq!(estimate(&prices.load(true).await), Some(2.));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("model-prices.json")).unwrap(),
            OLD
        );
    }
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), OLD, true);
    let server = serve(NEW, 200, true).await;
    let prices = Pricing::remote(temp.path(), &server.url, Duration::from_millis(50));
    let fallback = tokio::time::timeout(Duration::from_secs(3), prices.load(true))
        .await
        .unwrap();
    assert_eq!(estimate(&fallback), Some(2.));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("model-prices.json")).unwrap(),
        OLD
    );
}

#[tokio::test]
async fn canceled_caller_does_not_abandon_a_shared_refresh() {
    let temp = tempfile::tempdir().unwrap();
    let mut server = serve(NEW, 200, true).await;
    let prices = Pricing::remote(temp.path(), &server.url, TIMEOUT);
    let first_prices = prices.clone();
    let first = tokio::spawn(async move { first_prices.load(true).await });
    hit(&mut server).await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    server.gate.add_permits(1);
    assert_eq!(estimate(&prices.load(true).await), Some(3.));
    assert_eq!(server.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn malformed_cache_and_storage_failure_keep_downloaded_prices_in_memory() {
    for blocked in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("cache");
        if blocked {
            std::fs::write(&directory, "file").unwrap();
        } else {
            write(&directory, "not JSON", false);
        }
        let server = serve(NEW, 200, false).await;
        let prices = Pricing::remote(&directory, &server.url, TIMEOUT);
        assert_eq!(estimate(&prices.load(true).await), Some(3.));
        assert_eq!(estimate(&prices.load(true).await), Some(3.));
        assert_eq!(server.hits.load(Ordering::SeqCst), 1);
    }
}
