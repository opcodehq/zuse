//! Downloads from the curated, commit-pinned GitHub skill index.
use std::collections::BTreeMap;
use std::time::Duration;

use sha2::{Digest, Sha256};
use zuse_protocol::skill_catalog::{CatalogEntry, SkillFile, repositories};

fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent("Zuse-Skills")
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
}

pub async fn repository_stars() -> BTreeMap<String, u64> {
    #[derive(serde::Deserialize)]
    struct Stars {
        stargazers_count: u64,
    }

    let Ok(client) = client() else {
        return BTreeMap::new();
    };
    futures::future::join_all(repositories().iter().map(|repo| {
        let client = &client;
        async move {
            let url = format!("https://api.github.com/repos/{}", repo.repository);
            let response = client.get(url).send().await.ok()?.error_for_status().ok()?;
            let bytes = bounded_body(response, 128 * 1024).await.ok()?;
            let stars: Stars = serde_json::from_slice(&bytes).ok()?;
            Some((repo.repository.clone(), stars.stargazers_count))
        }
    }))
    .await
    .into_iter()
    .flatten()
    .collect()
}

pub async fn preview(entry: CatalogEntry, file: &'static SkillFile) -> Result<Vec<u8>, String> {
    let client = client().map_err(|error| error.to_string())?;
    fetch_file(&client, entry, file).await
}

pub(crate) async fn download(
    entry: CatalogEntry,
) -> Result<Vec<(&'static SkillFile, Vec<u8>)>, String> {
    let client = client().map_err(|error| error.to_string())?;
    let mut contents = Vec::with_capacity(entry.package.files.len());
    for batch in entry.package.files.chunks(8) {
        let mut transfers = tokio::task::JoinSet::new();
        for file in batch {
            let client = client.clone();
            transfers.spawn(async move {
                fetch_file(&client, entry, file)
                    .await
                    .map(|bytes| (file, bytes))
            });
        }
        while let Some(result) = transfers.join_next().await {
            contents.push(result.map_err(|error| error.to_string())??);
        }
    }
    Ok(contents)
}

async fn fetch_file(
    client: &reqwest::Client,
    entry: CatalogEntry,
    file: &SkillFile,
) -> Result<Vec<u8>, String> {
    let mut url =
        reqwest::Url::parse("https://raw.githubusercontent.com/").expect("fixed base URL");
    url.path_segments_mut()
        .expect("hierarchical base")
        .pop_if_empty()
        .extend(entry.source.repository.split('/'))
        .push(&entry.source.revision)
        .extend(entry.package.path.split('/'))
        .extend(file.path.split('/'));
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("Could not download {} from GitHub: {error}", file.path))?;
    let data = bounded_body(response, file.bytes).await?;
    if data.len() != file.bytes || format!("{:x}", Sha256::digest(&data)) != file.sha256 {
        return Err(format!(
            "{} does not match the indexed GitHub revision.",
            file.path
        ));
    }
    Ok(data)
}

async fn bounded_body(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if chunk.len() > limit.saturating_sub(output.len()) {
            return Err("GitHub returned a file larger than expected.".into());
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}
