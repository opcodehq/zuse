//! Per-document loopback origin for local media and web documents.
//! Relative web assets stay under the workspace; archive entries are never extracted.
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
        let browser =
            cx.new(|cx| crate::browser::BrowserSurface::new(Default::default(), false, window, cx));
        browser.update(cx, |view, cx| {
            view.set_presentation(crate::browser::model::Presentation::Live, cx);
            view.navigate(&url, window, cx);
        });
        Ok(Self { browser, stop })
    }
}

fn serve(root: &Path, path: &Path) -> Result<(String, Arc<AtomicBool>), String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let relative = path.strip_prefix(&root).map_err(|e| e.to_string())?;
    let token = uuid::Uuid::new_v4().to_string();
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut url = url::Url::parse(&format!(
        "http://127.0.0.1:{}/{token}/",
        listener.local_addr().map_err(|e| e.to_string())?.port()
    ))
    .unwrap();
    url.path_segments_mut().unwrap().pop_if_empty().extend(
        relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
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
                        let _ = stream
                            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                        continue;
                    };
                    let decoded = percent_encoding::percent_decode_str(encoded).decode_utf8_lossy();
                    let Ok(mut file_path) = root.join(decoded.as_ref()).canonicalize() else {
                        continue;
                    };
                    if file_path.is_dir() {
                        let Ok(index) = file_path.join("index.html").canonicalize() else {
                            let _ = stream
                                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                            continue;
                        };
                        file_path = index;
                    }
                    if !file_path.starts_with(&root) || !file_path.is_file() {
                        let _ = stream
                            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                        continue;
                    }
                    let Ok(mut file) = std::fs::File::open(&file_path) else {
                        continue;
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
                        continue;
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
                            continue;
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
                        let _ = std::io::copy(&mut file.take(count), &mut stream);
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(30)),
            }
        }
    });
    Ok((url.into(), stop))
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
            write!(
                socket,
                "GET {path} HTTP/1.1\r\nHost: localhost\r\n{headers}Connection: close\r\n\r\n"
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
}
