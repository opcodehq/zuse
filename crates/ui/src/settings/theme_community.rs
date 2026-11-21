use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;
use url::Url;
use zuse_theme::CustomThemeLibrary;
use zuse_theme::vscode::{CompileOptions, SourceCompilation};

const REGISTRY: &str = "https://open-vsx.org";
const MAX_SEARCH_BYTES: usize = 1024 * 1024;
const MAX_DOWNLOAD_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CommunityTheme {
    pub name: String,
    pub namespace: String,
    pub display_name: Option<String>,
    version: String,
    files: ThemeFiles,
}

#[derive(Clone, Deserialize)]
struct ThemeFiles {
    download: Option<String>,
}

impl CommunityTheme {
    pub fn label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Deserialize)]
struct SearchResults {
    extensions: Vec<CommunityTheme>,
}

pub(super) async fn search(query: String) -> Result<Vec<CommunityTheme>> {
    let url = format!("{REGISTRY}/api/-/search");
    let response = client()?
        .get(url)
        .query(&[
            ("query", query.as_str()),
            ("category", "Themes"),
            ("size", "12"),
        ])
        .send()
        .await?;
    let bytes = read_bounded(response, MAX_SEARCH_BYTES).await?;
    let results: SearchResults = serde_json::from_slice(&bytes)?;
    Ok(results
        .extensions
        .into_iter()
        .filter(|entry| entry.files.download.is_some())
        .take(12)
        .collect())
}

pub(super) async fn download(theme: CommunityTheme) -> Result<(Vec<u8>, CompileOptions)> {
    let url = Url::parse(
        theme
            .files
            .download
            .as_deref()
            .context("theme download is unavailable")?,
    )?;
    if !registry_url(&url) {
        bail!("theme download is outside Open VSX");
    }
    let bytes = read_bounded(client()?.get(url).send().await?, MAX_DOWNLOAD_BYTES).await?;
    let slug = super::appearance::slug(&format!("{}-{}", theme.namespace, theme.name));
    let mut source = Url::parse(REGISTRY)?;
    source
        .path_segments_mut()
        .expect("registry URL is a base URL")
        .extend(["extension", &theme.namespace, &theme.name]);
    let options = CompileOptions {
        family_id: format!("custom-{slug}"),
        family_name: theme.label().into(),
        source_url: source.to_string(),
        revision: theme.version,
        license: "See Open VSX listing".into(),
    };
    Ok((bytes, options))
}

pub(super) fn compile(bytes: &[u8], options: CompileOptions) -> Result<SourceCompilation> {
    CustomThemeLibrary::compile_community_package(bytes, options)
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .redirect(reqwest::redirect::Policy::custom(|redirect| {
            if redirect.previous().len() < 4 && registry_url(redirect.url()) {
                redirect.follow()
            } else {
                redirect.error("theme registry redirected to an unsupported URL")
            }
        }))
        .build()?)
}

fn registry_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && matches!(
            url.host_str(),
            Some("open-vsx.org" | "openvsx.eclipsecontent.org")
        )
}

async fn read_bounded(response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let mut response = response.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        bail!("theme registry response is too large");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > limit - bytes.len() {
            bail!("theme registry response is too large");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
