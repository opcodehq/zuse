//! Per-document loopback origin for local media and web documents.
//! Local assets stay under the workspace or an external document's containing directory;
//! archive entries are never extracted.
//! A page on another machine (a cloud computer) is served from memory, its
//! assets read through the workspace file service, never from this disk.
use gpui::AppContext;
use std::{
    io::{Read, Seek, SeekFrom, Write},
    net::TcpListener,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub(super) fn is_html(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, extension)| {
        extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("htm")
    })
}

pub(super) fn supported(path: &str) -> bool {
    matches!(
        path.rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "html"
            | "htm"
            | "mp4"
            | "webm"
            | "mov"
            | "m4v"
            | "ogv"
            | "mp3"
            | "wav"
            | "ogg"
            | "m4a"
            | "aac"
            | "flac"
            | "pdf"
            | "zip"
    )
}
pub(super) struct WebPreview {
    pub browser: gpui::Entity<crate::browser::BrowserSurface>,
    stop: Arc<AtomicBool>,
    /// The remote entry path and content hash this preview was built from.
    source: Option<(String, Option<String>)>,
    missing_assets: Arc<AtomicBool>,
    /// Answers a remote page's asset reads; dropped with the preview.
    _bridge: Option<gpui::Task<()>>,
}
impl Drop for WebPreview {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" | "map" => "application/json",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "m4a" | "aac" => "audio/mp4",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}
impl WebPreview {
    pub fn new(
        root: &Path,
        path: &Path,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Result<Self, String> {
        let (url, stop) = serve(root, path)?;
        Ok(Self::open(url, stop, None, window, cx))
    }

    /// A page whose files live on another machine: `html` is the document
    /// already read through the workspace file service, and `fetch` reads
    /// each relative asset the page asks for from that same machine.
    #[allow(clippy::too_many_arguments)]
    pub fn remote(
        entry: &str,
        html: String,
        content_hash: Option<String>,
        fetch: RemoteFetch,
        missing_assets: Arc<AtomicBool>,
        bridge: gpui::Task<()>,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Result<Self, String> {
        let (url, stop) = serve_remote(entry, html, fetch)?;
        let mut preview = Self::open(url, stop, Some(bridge), window, cx);
        preview.source = Some((entry.to_owned(), content_hash));
        preview.missing_assets = missing_assets;
        Ok(preview)
    }

    fn open(
        url: String,
        stop: Arc<AtomicBool>,
        bridge: Option<gpui::Task<()>>,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Self {
        let browser =
            cx.new(|cx| crate::browser::BrowserSurface::new(Default::default(), false, window, cx));
        browser.update(cx, |view, cx| {
            view.set_presentation(crate::browser::model::Presentation::Live, cx);
            view.navigate(&url, window, cx);
        });
        Self {
            browser,
            stop,
            source: None,
            missing_assets: Arc::new(AtomicBool::new(false)),
            _bridge: bridge,
        }
    }

    /// Whether a remote page asked for a file that could not be read.
    pub fn missing_assets(&self) -> bool {
        self.missing_assets.load(Ordering::Relaxed)
    }

    /// Whether this remote preview serves `entry` at `content_hash`. A page
    /// that was renamed or changed needs a fresh origin: its relative
    /// assets resolve against the entry's folder.
    pub fn serves(&self, entry: &str, content_hash: Option<&str>) -> bool {
        self.source
            .as_ref()
            .is_some_and(|(served, hash)| served == entry && hash.as_deref() == content_hash)
    }
}

/// Reads one workspace-relative asset of a remote page; `None` when it
/// cannot be read. Called on the preview's server thread, so it may block.
pub(super) type RemoteFetch = Arc<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync>;

/// Where a preview's files come from.
enum Source {
    /// This device's disk, confined to the canonical root.
    Local(std::path::PathBuf),
    /// Another machine: the entry document is held in memory and every
    /// other file is read through `fetch`. Nothing touches this disk.
    Remote {
        entry: String,
        html: Vec<u8>,
        fetch: RemoteFetch,
    },
}

fn serve(root: &Path, path: &Path) -> Result<(String, Arc<AtomicBool>), String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    // Preserve workspace-wide assets for internal files; external files only
    // expose assets in their canonical containing directory.
    let root = if path.starts_with(&root) {
        root
    } else {
        path.parent()
            .ok_or_else(|| "This file cannot be previewed.".to_string())?
            .to_path_buf()
    };
    let relative = path
        .strip_prefix(&root)
        .map_err(|e| e.to_string())?
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    serve_source(relative, Source::Local(root))
}

fn serve_remote(
    entry: &str,
    html: String,
    fetch: RemoteFetch,
) -> Result<(String, Arc<AtomicBool>), String> {
    let entry =
        remote_relative_path(entry).ok_or_else(|| "This file cannot be previewed.".to_string())?;
    let segments = entry.split('/').map(str::to_owned).collect();
    serve_source(
        segments,
        Source::Remote {
            entry,
            html: html.into_bytes(),
            fetch,
        },
    )
}

/// The workspace-relative file a remote page may request: no absolute
/// paths, no parent hops, no odd components. A directory means its
/// `index.html`.
pub(super) fn remote_relative_path(path: &str) -> Option<String> {
    if path.starts_with('/') {
        return None;
    }
    let directory = path.is_empty() || path.ends_with('/');
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            part if part.contains(['\\', '\0']) => return None,
            part => parts.push(part),
        }
    }
    if directory {
        parts.push("index.html");
    }
    Some(parts.join("/"))
}

/// A remote page's text files arrive as UTF-8 strings from the workspace
/// file service, so they say so; binary types keep their plain type.
fn remote_content_type(path: &str) -> String {
    let mime = mime(Path::new(path));
    if mime.starts_with("text/") || mime == "application/json" || mime == "image/svg+xml" {
        format!("{mime}; charset=utf-8")
    } else {
        mime.to_owned()
    }
}

fn not_found(stream: &mut impl Write) {
    let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
}

fn serve_source(
    relative: Vec<String>,
    source: Source,
) -> Result<(String, Arc<AtomicBool>), String> {
    let token = uuid::Uuid::new_v4().to_string();
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut url = url::Url::parse(&format!(
        "http://127.0.0.1:{}/{token}/",
        listener.local_addr().map_err(|e| e.to_string())?.port()
    ))
    .unwrap();
    url.path_segments_mut()
        .unwrap()
        .pop_if_empty()
        .extend(relative);
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // Accepted sockets can inherit the listener's nonblocking mode.
                    if stream.set_nonblocking(false).is_err() {
                        continue;
                    }
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                    let mut request = [0; 16384];
                    let mut n = 0;
                    while n < request.len() && !request[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut request[n..]) {
                            Ok(0) | Err(_) => break,
                            Ok(count) => n += count,
                        }
                    }
                    if !request[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                        continue;
                    }
                    let request = String::from_utf8_lossy(&request[..n]);
                    let target = request.split_whitespace().nth(1).unwrap_or("");
                    let target_path = target.split('?').next().unwrap_or("");
                    let prefix = format!("/{token}/");
                    let cookie = format!("zuse_preview_{token}=1");
                    let authorized = request.lines().any(|line| {
                        line.split_once(':').is_some_and(|(key, value)| {
                            key.eq_ignore_ascii_case("cookie")
                                && value.split(';').any(|part| part.trim() == cookie)
                        })
                    });
                    let encoded = if let Some(encoded) = target_path.strip_prefix(&prefix) {
                        encoded
                    } else if authorized {
                        target_path.trim_start_matches('/')
                    } else {
                        not_found(&mut stream);
                        continue;
                    };
                    let decoded = percent_encoding::percent_decode_str(encoded).decode_utf8_lossy();
                    match &source {
                        Source::Local(root) => {
                            respond_local(&mut stream, &request, root, &decoded, &token)
                        }
                        Source::Remote { entry, html, fetch } => {
                            let Some(path) = remote_relative_path(&decoded) else {
                                not_found(&mut stream);
                                continue;
                            };
                            let fetched;
                            let body = if &path == entry {
                                html.as_slice()
                            } else if let Some(bytes) = fetch(&path) {
                                fetched = bytes;
                                fetched.as_slice()
                            } else {
                                not_found(&mut stream);
                                continue;
                            };
                            let _ = write!(
                                stream,
                                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nSet-Cookie: zuse_preview_{token}=1; Path=/; HttpOnly; SameSite=Strict\r\n\r\n",
                                remote_content_type(&path),
                                body.len()
                            );
                            if !request.starts_with("HEAD ") {
                                let _ = stream.write_all(body);
                            }
                        }
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(30)),
            }
        }
    });
    Ok((url.into(), stop))
}

fn respond_local(
    stream: &mut std::net::TcpStream,
    request: &str,
    root: &Path,
    decoded: &str,
    token: &str,
) {
    let Ok(mut file_path) = root.join(decoded).canonicalize() else {
        return;
    };
    if file_path.is_dir() {
        let Ok(index) = file_path.join("index.html").canonicalize() else {
            not_found(stream);
            return;
        };
        file_path = index;
    }
    if !file_path.starts_with(root) || !file_path.is_file() {
        not_found(stream);
        return;
    }
    let Ok(mut file) = std::fs::File::open(&file_path) else {
        return;
    };
    if file_path
        .extension()
        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("zip"))
    {
        let body = match zip::ZipArchive::new(file) {
            Ok(mut archive) => {
                let mut rows = String::new();
                for i in 0..archive.len().min(10000) {
                    if rows.len() > 2 * 1024 * 1024 {
                        break;
                    }
                    if let Ok(entry) = archive.by_index(i) {
                        rows.push_str(&format!(
                            "<tr><td>{}</td><td>{} bytes</td></tr>",
                            escape(entry.name()),
                            entry.size()
                        ));
                    }
                }
                format!(
                    "<!doctype html><meta charset=utf-8><style>body{{font:14px system-ui;background:#181818;color:#ddd;padding:24px}}td{{padding:6px 20px}}table{{width:100%}}</style><h2>Archive contents</h2><p>{} entries (showing up to 10,000)</p><table>{rows}</table>",
                    archive.len()
                )
            }
            Err(e) => escape(&e.to_string()),
        };
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nSet-Cookie: zuse_preview_{token}=1; Path=/; HttpOnly; SameSite=Strict\r\n\r\n{}",
            body.len(),
            body
        );
        return;
    }
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let range = request.lines().find_map(|l| {
        l.split_once(':')
            .filter(|(k, _)| k.eq_ignore_ascii_case("range"))
            .map(|(_, v)| v.trim())
    });
    let (start, end) = match byte_range(range, len) {
        Ok(value) => value,
        Err(()) => {
            let _ = write!(
                stream,
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\n\r\n"
            );
            return;
        }
    };
    let count = if len == 0 { 0 } else { end - start + 1 };
    let _ = write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\nSet-Cookie: zuse_preview_{token}=1; Path=/; HttpOnly; SameSite=Strict\r\n",
        if range.is_some() {
            "206 Partial Content"
        } else {
            "200 OK"
        },
        mime(&file_path),
        count
    );
    if range.is_some() {
        let _ = write!(stream, "Content-Range: bytes {start}-{end}/{len}\r\n");
    }
    let _ = stream.write_all(b"\r\n");
    if !request.starts_with("HEAD ") {
        let _ = file.seek(SeekFrom::Start(start));
        let _ = std::io::copy(&mut file.take(count), stream);
    }
}

/// How long a remote page's asset may take to arrive before it counts as
/// missing.
const REMOTE_ASSET_TIMEOUT: Duration = Duration::from_secs(20);

type AssetRequest = (String, std::sync::mpsc::SyncSender<Option<Vec<u8>>>);

/// The asset reader for a remote page: the preview's server thread asks
/// for a file, and a task on the surface reads it through the workspace
/// file service. Any unreadable asset (other than the browser's automatic
/// favicon probe) raises `missing` so the tab can say so quietly.
pub(super) fn remote_fetch(
    client: super::client::WorkspaceFilesClient,
    context: super::client::FilesRequestContext,
    missing: Arc<AtomicBool>,
    cx: &mut gpui::Context<super::FilesSurface>,
) -> (RemoteFetch, gpui::Task<()>) {
    use futures::StreamExt as _;
    let (requests, mut incoming) = futures::channel::mpsc::unbounded::<AssetRequest>();
    let fetch: RemoteFetch = Arc::new(move |path: &str| {
        let (reply, answer) = std::sync::mpsc::sync_channel(1);
        requests.unbounded_send((path.to_owned(), reply)).ok()?;
        answer.recv_timeout(REMOTE_ASSET_TIMEOUT).ok().flatten()
    });
    let task = cx.spawn(async move |this, cx| {
        while let Some((path, reply)) = incoming.next().await {
            let bytes = read_remote_asset(&client, &context, &path).await;
            let found = bytes.is_some();
            // A reply nobody waits for any more (the page's request timed
            // out) is as missing as a file that could not be read.
            let delivered = reply.send(bytes).is_ok();
            if (!found || !delivered)
                && path != "favicon.ico"
                && !missing.swap(true, Ordering::Relaxed)
            {
                let _ = this.update(cx, |_, cx| cx.notify());
            }
        }
    });
    (fetch, task)
}

/// Read one workspace-relative asset of a remote page through the
/// workspace file service: images as bytes, everything else as text.
pub(super) async fn read_remote_asset(
    client: &super::client::WorkspaceFilesClient,
    context: &super::client::FilesRequestContext,
    path: &str,
) -> Option<Vec<u8>> {
    let request = zuse_protocol::ReadWorkspaceFileRequest {
        target: context.target.clone(),
        path: path.to_owned(),
    };
    if super::image_preview::is_image(path) {
        let checkout = match context.checkout_id.clone().filter(|id| !id.is_empty()) {
            Some(id) => Some(id),
            None => client
                .read_file(request.clone())
                .await
                .ok()
                .map(|file| file.checkout_id),
        };
        if let Some(checkout) = checkout.filter(|id| !id.is_empty())
            && let Ok((_, bytes)) = client.read_image(path.to_owned(), checkout).await
        {
            return Some(bytes);
        }
    }
    client
        .read_file(request)
        .await
        .ok()?
        .text
        .map(String::into_bytes)
}

fn byte_range(header: Option<&str>, len: u64) -> Result<(u64, u64), ()> {
    let Some(header) = header else {
        return Ok((0, len.saturating_sub(1)));
    };
    if len == 0 {
        return Err(());
    }
    let (a, b) = header
        .strip_prefix("bytes=")
        .ok_or(())?
        .split_once('-')
        .ok_or(())?;
    if a.is_empty() {
        let count = b.parse::<u64>().map_err(|_| ())?;
        if count == 0 {
            return Err(());
        }
        return Ok((len.saturating_sub(count), len - 1));
    }
    let start = a.parse::<u64>().map_err(|_| ())?;
    let end = if b.is_empty() {
        len - 1
    } else {
        b.parse::<u64>().map_err(|_| ())?.min(len - 1)
    };
    if start >= len || start > end {
        return Err(());
    }
    Ok((start, end))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn media_seeking_ranges() {
        assert_eq!(byte_range(Some("bytes=0-1"), 100), Ok((0, 1)));
        assert_eq!(byte_range(Some("bytes=50-"), 100), Ok((50, 99)));
        assert_eq!(byte_range(Some("bytes=-10"), 100), Ok((90, 99)));
        assert!(byte_range(Some("bytes=100-"), 100).is_err());
        assert!(byte_range(Some("bytes=5-2"), 100).is_err());
        assert!(byte_range(Some("bytes=0-1,4-5"), 100).is_err());
    }
    #[test]
    fn serves_assets_ranges_and_blocks_paths_outside_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            "<script src='/app.js'></script>",
        )
        .unwrap();
        std::fs::write(dir.path().join("app.js"), "hello").unwrap();
        let (address, stop) = serve(dir.path(), &dir.path().join("index.html")).unwrap();
        let url = url::Url::parse(&address).unwrap();
        let token = url.path().split('/').nth(1).unwrap();
        let fetch = |path: &str, headers: &str| {
            let mut socket =
                std::net::TcpStream::connect(("127.0.0.1", url.port().unwrap())).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            socket
                .write_all(
                    format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n{headers}Connection: close\r\n\r\n")
                        .as_bytes(),
                )
                .unwrap();
            let mut response = String::new();
            socket.read_to_string(&mut response).unwrap();
            response
        };
        assert!(fetch(url.path(), "").contains("<script src="));
        let auth = format!("Cookie: zuse_preview_{token}=1\r\nRange: bytes=1-3\r\n");
        let response = fetch("/app.js", &auth);
        assert!(response.starts_with("HTTP/1.1 206"));
        assert!(response.ends_with("ell"));
        assert!(fetch("/app.js", "").starts_with("HTTP/1.1 404"));
        assert!(fetch(&format!("/{token}/../"), "").starts_with("HTTP/1.1 404"));
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn archive_names_are_html_escaped() {
        assert_eq!(escape("<script>\"&"), "&lt;script&gt;&quot;&amp;");
    }

    #[test]
    fn remote_paths_stay_inside_the_workspace() {
        assert_eq!(
            remote_relative_path("guide/./app.css").as_deref(),
            Some("guide/app.css")
        );
        assert_eq!(
            remote_relative_path("guide/").as_deref(),
            Some("guide/index.html")
        );
        assert_eq!(remote_relative_path("").as_deref(), Some("index.html"));
        assert_eq!(remote_relative_path("guide/../../etc/passwd"), None);
        assert_eq!(remote_relative_path("/etc/passwd"), None);
        assert_eq!(remote_relative_path("guide\\..\\x"), None);
    }

    fn get(address: &str, path: &str, headers: &str) -> String {
        let url = url::Url::parse(address).unwrap();
        let mut socket = std::net::TcpStream::connect(("127.0.0.1", url.port().unwrap())).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .write_all(
                format!(
                    "GET {path} HTTP/1.1\r\nHost: localhost\r\n{headers}Connection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn waits_for_complete_request_headers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<h1>Ready</h1>").unwrap();
        let (address, stop) = serve(dir.path(), &dir.path().join("index.html")).unwrap();
        let url = url::Url::parse(&address).unwrap();
        let mut socket = std::net::TcpStream::connect(("127.0.0.1", url.port().unwrap())).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        std::thread::sleep(Duration::from_millis(80));
        socket
            .write_all(format!("GET {} HTTP/1.1\r\n", url.path()).as_bytes())
            .unwrap();
        std::thread::sleep(Duration::from_millis(80));
        socket
            .write_all(b"Host: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).unwrap();
        stop.store(true, Ordering::Relaxed);
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.ends_with("<h1>Ready</h1>"));
    }

    #[test]
    fn external_page_serves_sibling_assets() {
        let workspace = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let html = "<link rel=stylesheet href=style.css><img src=logo.svg>";
        std::fs::write(external.path().join("index.html"), html).unwrap();
        std::fs::write(external.path().join("style.css"), "body{color:red}").unwrap();
        std::fs::write(external.path().join("logo.svg"), "<svg/>").unwrap();
        let (address, stop) = serve(workspace.path(), &external.path().join("index.html")).unwrap();
        let url = url::Url::parse(&address).unwrap();
        let token = url.path().split('/').nth(1).unwrap();
        let page = get(&address, url.path(), "");
        assert!(page.starts_with("HTTP/1.1 200"));
        assert!(page.ends_with(html));
        for (asset, content_type, body) in [
            ("style.css", "text/css", "body{color:red}"),
            ("logo.svg", "image/svg+xml", "<svg/>"),
        ] {
            let asset_url = url.join(asset).unwrap();
            let response = get(&address, asset_url.path(), "");
            assert!(response.starts_with("HTTP/1.1 200"));
            assert!(response.contains(&format!("Content-Type: {content_type}")));
            assert!(response.ends_with(body));
            assert!(get(&address, &format!("/{asset}"), "").starts_with("HTTP/1.1 404"));
            let cookie = format!("Cookie: zuse_preview_{token}=1\r\n");
            assert!(get(&address, &format!("/{asset}"), &cookie).ends_with(body));
        }
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn nested_workspace_page_retains_workspace_assets() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("pages")).unwrap();
        std::fs::write(
            workspace.path().join("pages/index.html"),
            "<link rel=stylesheet href=../style.css><script src=/app.js></script>",
        )
        .unwrap();
        std::fs::write(workspace.path().join("style.css"), "body{color:red}").unwrap();
        std::fs::write(workspace.path().join("app.js"), "hello").unwrap();
        let (address, stop) =
            serve(workspace.path(), &workspace.path().join("pages/index.html")).unwrap();
        let url = url::Url::parse(&address).unwrap();
        assert!(url.path().ends_with("/pages/index.html"));
        assert!(get(&address, url.path(), "").starts_with("HTTP/1.1 200"));
        let css_url = url.join("../style.css").unwrap();
        assert!(get(&address, css_url.path(), "").ends_with("body{color:red}"));
        let token = url.path().split('/').nth(1).unwrap();
        let cookie = format!("Cookie: zuse_preview_{token}=1\r\n");
        assert!(get(&address, "/app.js", &cookie).ends_with("hello"));
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn external_page_cannot_escape_its_directory() {
        let workspace = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let directory = external.path().join("gallery");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("index.html"), "<h1>Gallery</h1>").unwrap();
        std::fs::write(external.path().join("secret.txt"), "private").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(external.path(), directory.join("outside")).unwrap();
        let (address, stop) = serve(workspace.path(), &directory.join("index.html")).unwrap();
        let url = url::Url::parse(&address).unwrap();
        let token = url.path().split('/').nth(1).unwrap();
        let cookie = format!("Cookie: zuse_preview_{token}=1\r\n");
        for path in ["../secret.txt", "%2e%2e/secret.txt", "%2e%2e%2fsecret.txt"] {
            assert!(get(&address, &format!("/{token}/{path}"), "").starts_with("HTTP/1.1 404"));
            assert!(get(&address, &format!("/{path}"), &cookie).starts_with("HTTP/1.1 404"));
        }
        #[cfg(unix)]
        {
            assert!(
                get(&address, &format!("/{token}/outside/secret.txt"), "")
                    .starts_with("HTTP/1.1 404")
            );
            assert!(get(&address, "/outside/secret.txt", &cookie).starts_with("HTTP/1.1 404"));
        }
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn remote_page_is_served_from_memory_and_assets_through_fetch() {
        let requested = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = requested.clone();
        let fetch: RemoteFetch = Arc::new(move |path: &str| {
            seen.lock().unwrap().push(path.to_owned());
            (path == "sandbox-guide/style.css").then(|| b"body{color:red}".to_vec())
        });
        let (address, stop) = serve_remote(
            "sandbox-guide/index.html",
            "<link rel=stylesheet href=style.css><h1>Guide</h1>".into(),
            fetch,
        )
        .unwrap();
        let url = url::Url::parse(&address).unwrap();
        assert!(url.path().ends_with("/sandbox-guide/index.html"));
        let token = url.path().split('/').nth(1).unwrap().to_owned();
        let page = get(&address, url.path(), "");
        assert!(page.starts_with("HTTP/1.1 200"));
        assert!(page.contains("Content-Type: text/html; charset=utf-8"));
        assert!(page.ends_with("<h1>Guide</h1>"));
        let css = get(&address, &format!("/{token}/sandbox-guide/style.css"), "");
        assert!(css.contains("Content-Type: text/css"));
        assert!(css.ends_with("body{color:red}"));
        assert!(
            get(&address, &format!("/{token}/sandbox-guide/gone.js"), "")
                .starts_with("HTTP/1.1 404")
        );
        // Parent hops and unauthorized paths never reach the reader.
        assert!(get(&address, &format!("/{token}/%2e%2e/secret"), "").starts_with("HTTP/1.1 404"));
        assert!(get(&address, "/sandbox-guide/style.css", "").starts_with("HTTP/1.1 404"));
        assert_eq!(
            *requested.lock().unwrap(),
            ["sandbox-guide/style.css", "sandbox-guide/gone.js"]
        );
        stop.store(true, Ordering::Relaxed);
    }

    #[derive(Default)]
    struct RemoteFiles {
        texts: std::collections::HashMap<String, String>,
        calls: std::sync::Mutex<Vec<(String, String)>>,
    }

    #[async_trait::async_trait]
    impl super::super::client::WorkspaceFilesTransport for RemoteFiles {
        async fn call(
            &self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<serde_json::Value, zuse_rpc::RpcError> {
            let path = params["path"].as_str().unwrap_or_default().to_owned();
            self.calls
                .lock()
                .unwrap()
                .push((method.to_owned(), path.clone()));
            if method != zuse_rpc::methods::READ_WORKSPACE_FILE {
                return Err(zuse_rpc::RpcError::Closed);
            }
            let text = self.texts.get(&path).ok_or(zuse_rpc::RpcError::Closed)?;
            Ok(serde_json::json!({
                "checkoutId": "checkout", "path": path, "text": text,
                "contentHash": "hash", "size": text.len(), "encoding": "utf8",
                "truncated": false
            }))
        }

        async fn subscribe(
            &self,
            _: &str,
            _: serde_json::Value,
        ) -> Result<tokio::sync::mpsc::Receiver<serde_json::Value>, zuse_rpc::RpcError> {
            Err(zuse_rpc::RpcError::Closed)
        }
    }

    #[tokio::test]
    async fn remote_assets_are_read_through_the_workspace_file_service() {
        let transport = Arc::new(RemoteFiles {
            texts: [("guide/app.js".to_owned(), "console.log(1)".to_owned())].into(),
            ..Default::default()
        });
        let context = super::super::client::FilesRequestContext {
            target: zuse_protocol::WorkspaceTarget {
                chat_id: Some("chat".into()),
                space_id: None,
                checkout_path: None,
            },
            target_device_id: None,
            cwd: "/home/zuse/project".into(),
            checkout_id: Some("checkout".into()),
        };
        let client = super::super::client::WorkspaceFilesClient::with_transport(
            transport.clone(),
            context.clone(),
        );
        assert_eq!(
            read_remote_asset(&client, &context, "guide/app.js").await,
            Some(b"console.log(1)".to_vec())
        );
        assert_eq!(
            read_remote_asset(&client, &context, "guide/missing.css").await,
            None
        );
        // An image is asked for as bytes first, then as text (an SVG).
        assert_eq!(
            read_remote_asset(&client, &context, "guide/logo.png").await,
            None
        );
        let calls = transport.calls.lock().unwrap().clone();
        assert_eq!(
            calls[0],
            ("ReadWorkspaceFile".into(), "guide/app.js".into())
        );
        assert_eq!(
            calls[2],
            ("ReadWorkspaceImage".into(), "guide/logo.png".into())
        );
    }
}
