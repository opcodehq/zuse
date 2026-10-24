//! Public ACP catalogue metadata is resolved on the selected computer.
use std::{
    collections::HashMap,
    sync::OnceLock,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use zuse_protocol::custom_acp::catalog::{self, Catalog, RegistryAgent};

#[derive(Default)]
struct Cache {
    checked: Option<Instant>,
    error: Option<String>,
    icons: HashMap<String, String>,
    agents: Option<Vec<RegistryAgent>>,
}

impl Cache {
    fn accept(&mut self, result: Result<Vec<RegistryAgent>, String>) -> Catalog {
        let error = match result {
            Ok(agents) => {
                if self.agents.as_ref() != Some(&agents) {
                    self.icons.clear();
                }
                self.agents = Some(agents);
                None
            }
            Err(error) => Some(error),
        };
        self.checked = Some(Instant::now());
        self.error = error.clone();
        Catalog {
            platform: catalog::platform(),
            agents: self.agents.clone().unwrap_or_else(catalog::fallback),
            error,
        }
    }
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Vec<RegistryAgent>, String> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "The ACP registry is unavailable.")?
        .error_for_status()
        .map_err(|_| "The ACP registry is unavailable.")?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "The ACP registry download failed.")?
    {
        if bytes.len().saturating_add(chunk.len()) > catalog::MAX_REGISTRY_BYTES {
            return Err("The ACP registry exceeds the download limit.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    catalog::parse(&bytes).map_err(|_| "The ACP registry returned an invalid catalogue.".into())
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

pub(crate) async fn get(force: bool) -> Catalog {
    let mut cache = CACHE.get_or_init(Default::default).lock().await;
    if !force
        && cache.checked.is_some_and(|checked| {
            checked.elapsed()
                < Duration::from_secs(if cache.error.is_some() { 30 } else { 15 * 60 })
        })
    {
        return Catalog {
            platform: catalog::platform(),
            agents: cache.agents.clone().unwrap_or_else(catalog::fallback),
            error: cache.error.clone(),
        };
    }
    let result = match reqwest::Client::builder()
        .user_agent("Zuse/desktop")
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => fetch(&client, catalog::REGISTRY_URL).await,
        Err(_) => Err("The ACP registry is unavailable.".into()),
    };
    cache.accept(result)
}

pub(crate) async fn icon(id: &str) -> Option<String> {
    let catalogue = get(false).await;
    let url = catalogue
        .agents
        .iter()
        .find(|agent| agent.id == id)?
        .icon
        .as_ref()?;
    if !url.starts_with("https://cdn.agentclientprotocol.com/registry/") {
        return None;
    }
    if let Some(svg) = CACHE
        .get_or_init(Default::default)
        .lock()
        .await
        .icons
        .get(url)
        .cloned()
    {
        return Some(svg);
    }
    let client = reqwest::Client::builder()
        .user_agent("Zuse/desktop")
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    let mut response = client.get(url).send().await.ok()?.error_for_status().ok()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len().saturating_add(chunk.len()) > catalog::MAX_ICON_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    let svg = String::from_utf8(bytes)
        .ok()
        .filter(|svg| svg.contains("<svg"))?;
    CACHE
        .get_or_init(Default::default)
        .lock()
        .await
        .icons
        .insert(url.clone(), svg.clone());
    Some(svg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn downloaded_catalogue_keeps_metadata_and_survives_a_failed_refresh() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            let body = r#"{"agents":[{"id":"test","name":"Test","repository":"https://example.test/agent","version":"1.2.3","distribution":{"npx":{"package":"agent@1.2.3"}}}]}"#;
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let agents = fetch(&client, &format!("http://{address}")).await.unwrap();
        server.await.unwrap();
        let mut cache = Cache::default();
        let first = cache.accept(Ok(agents));
        assert_eq!(first.agents[0].version.as_deref(), Some("1.2.3"));
        assert_eq!(
            first.agents[0].repository.as_deref(),
            Some("https://example.test/agent")
        );
        assert_eq!(cache.accept(Err("offline".into())).agents, first.agents);
        assert_eq!(first.platform, catalog::platform());
        assert!(cache.accept(Err("offline".into())).error.is_some());
    }

    #[test]
    fn first_failure_has_truthful_builtin_fallback() {
        let result = Cache::default().accept(Err("offline".into()));
        assert_eq!(result.agents, catalog::fallback());
        assert_eq!(result.error.as_deref(), Some("offline"));
    }
}
