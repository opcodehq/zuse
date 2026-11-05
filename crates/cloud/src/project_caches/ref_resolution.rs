//! Anonymous public references are pinned once, before the first allocation.
//! A retained task never resolves its branch again.
use super::*;
use base64::Engine;
use zuse_protocol::cloud_computers::EnvironmentRecipe;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResolvedSource {
    pub commit: String,
    pub recipe: EnvironmentRecipe,
    pub recipe_source: String,
}

async fn document(
    http: &reqwest::Client,
    url: reqwest::Url,
    optional: bool,
    token: Option<&str>,
) -> Result<Option<Value>, ApiError> {
    let request = http
        .get(url)
        .header("accept", "application/vnd.github+json");
    let request = if let Some(token) = token {
        request.bearer_auth(token)
    } else {
        request
    };
    let mut response = request
        .send()
        .await
        .map_err(|_| ApiError::unavailable("project_cache_public_source_unavailable"))?;
    if optional && response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(ApiError::conflict("project_cache_public_source_required"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ApiError::unavailable("project_cache_source_response_invalid"))?
    {
        if bytes.len() + chunk.len() > 512 * 1024 {
            return Err(ApiError::unavailable(
                "project_cache_source_response_invalid",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| ApiError::unavailable("project_cache_source_response_invalid"))
}
pub(super) async fn resolve_at(
    origin: &str,
    repository: &str,
    reference: &str,
    configured: EnvironmentRecipe,
) -> Result<ResolvedSource, ApiError> {
    resolve_authenticated_at(origin, repository, reference, configured, None).await
}
/// Only the API credential authority supplies a token. The result is source
/// metadata; no bearer or helper state crosses into task configuration.
pub(crate) async fn resolve_authenticated_at(
    origin: &str,
    repository: &str,
    reference: &str,
    configured: EnvironmentRecipe,
    token: Option<&str>,
) -> Result<ResolvedSource, ApiError> {
    let name = repository
        .strip_prefix("github.com/")
        .ok_or_else(|| ApiError::conflict("project_cache_repository_invalid"))?;
    let parts: Vec<_> = name.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|p| {
            p.is_empty()
                || p.len() > 100
                || !p
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        })
        || reference.is_empty()
        || reference.len() > 512
        || reference.chars().any(char::is_control)
    {
        return Err(ApiError::conflict("project_cache_repository_invalid"));
    }
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5))
        .user_agent("Zuse public source resolver")
        .build()
        .map_err(|_| ApiError::unavailable("project_cache_public_source_unavailable"))?;
    let repo = reqwest::Url::parse(&format!("{origin}/repos/{name}"))
        .map_err(|_| ApiError::conflict("project_cache_repository_invalid"))?;
    let metadata = document(&http, repo.clone(), false, token).await?.unwrap();
    metadata_identity(name, &metadata, token.is_some())?;
    let mut commit_url = repo.clone();
    commit_url
        .path_segments_mut()
        .map_err(|_| ApiError::conflict("project_cache_repository_invalid"))?
        .extend(["commits", reference]);
    let value = document(&http, commit_url, false, token).await?.unwrap();
    let commit = value["sha"]
        .as_str()
        .filter(|s| {
            matches!(s.len(), 40 | 64)
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
        .ok_or_else(|| ApiError::unavailable("project_cache_source_commit_invalid"))?
        .to_owned();
    let mut recipe_url = repo.clone();
    recipe_url
        .path_segments_mut()
        .unwrap()
        .extend(["contents", ".zuse", "setup.json"]);
    recipe_url.query_pairs_mut().append_pair("ref", &commit);
    let (recipe, recipe_source) =
        if let Some(value) = document(&http, recipe_url, true, token).await? {
            if value["type"] != "file" || value["encoding"] != "base64" {
                return Err(ApiError::conflict("environment_recipe_invalid"));
            }
            let encoded = value["content"]
                .as_str()
                .ok_or_else(|| ApiError::conflict("environment_recipe_invalid"))?
                .replace(['\n', '\r'], "");
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| ApiError::conflict("environment_recipe_invalid"))?;
            let recipe: EnvironmentRecipe = serde_json::from_slice(&bytes)
                .map_err(|_| ApiError::conflict("environment_recipe_invalid"))?;
            (recipe, "repository")
        } else {
            (configured, "project")
        };
    recipe.validate().map_err(ApiError::bad_request)?;
    if !recipe.secret_bindings.is_empty() {
        return Err(ApiError::conflict("project_cache_runtime_secrets_required"));
    }
    let current = document(&http, repo, false, token).await?.unwrap();
    metadata_identity(name, &current, token.is_some())?;
    if current["id"] != metadata["id"] {
        return Err(ApiError::conflict(
            "project_cache_repository_identity_changed",
        ));
    }
    Ok(ResolvedSource {
        commit,
        recipe,
        recipe_source: recipe_source.into(),
    })
}

fn metadata_identity(name: &str, value: &Value, authenticated: bool) -> Result<(), ApiError> {
    if !authenticated {
        return super::source::public_metadata(name, value);
    }
    if !value["private"].is_boolean()
        || value["id"].as_i64().is_none_or(|id| id <= 0)
        || !value["full_name"]
            .as_str()
            .is_some_and(|full| full.eq_ignore_ascii_case(name))
    {
        return Err(ApiError::conflict(
            "project_cache_repository_identity_changed",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[tokio::test]
    async fn private_source_metadata_requires_scoped_bearer_and_pins_repository_identity() {
        let seen = Arc::new(AtomicUsize::new(0));
        let calls = seen.clone();
        let app = axum::Router::new().fallback(
            move |uri: axum::http::Uri, headers: axum::http::HeaderMap| {
                let calls = calls.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer scoped-fixture-only");
                    calls.fetch_add(1, Ordering::SeqCst);
                    if uri.path().contains("/contents/") {
                        return (axum::http::StatusCode::NOT_FOUND, axum::Json(json!({})));
                    }
                    let value = if uri.path().contains("/commits/") {
                        json!({"sha":"c".repeat(40)})
                    } else {
                        json!({"id":19,"private":true,"full_name":"example/private"})
                    };
                    (axum::http::StatusCode::OK, axum::Json(value))
                }
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let source = resolve_authenticated_at(
            &origin,
            "github.com/example/private",
            "main",
            EnvironmentRecipe::default(),
            Some("scoped-fixture-only"),
        )
        .await
        .unwrap();
        assert_eq!(source.commit, "c".repeat(40));
        assert_eq!(source.recipe_source, "project");
        assert_eq!(seen.load(Ordering::SeqCst), 4);
        assert!(
            !serde_json::to_string(&source)
                .unwrap()
                .contains("scoped-fixture-only")
        );
        server.abort();
        assert!(
            metadata_identity(
                "example/private",
                &json!({"id":19,"private":true,"full_name":"other/private"}),
                true
            )
            .is_err()
        );
        assert!(
            metadata_identity(
                "example/private",
                &json!({"id":19,"private":true,"full_name":"example/private"}),
                false
            )
            .is_err()
        );
    }
    #[tokio::test]
    async fn named_ref_is_encoded_pinned_and_repository_recipe_is_bound_to_commit() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let requests = seen.clone();
        let recipe = EnvironmentRecipe::default();
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&recipe).unwrap());
        let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
            let requests = requests.clone();
            let encoded = encoded.clone();
            async move {
                requests.lock().unwrap().push(uri.to_string());
                let value = if uri.path().contains("/commits/") {
                    json!({"sha":"a".repeat(40)})
                } else if uri.path().contains("/contents/") {
                    json!({"type":"file","encoding":"base64","content":encoded})
                } else {
                    json!({"id":7,"full_name":"example/repository","private":false})
                };
                axum::Json(value)
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let source = resolve_at(
            &origin,
            "github.com/example/repository",
            "release/one",
            recipe,
        )
        .await
        .unwrap();
        assert_eq!(source.commit, "a".repeat(40));
        assert_eq!(source.recipe_source, "repository");
        let seen = seen.lock().unwrap();
        assert!(seen[1].contains("release%2Fone"));
        assert_eq!(
            seen[2],
            format!(
                "/repos/example/repository/contents/.zuse/setup.json?ref={}",
                "a".repeat(40)
            )
        );
        assert_eq!(seen.len(), 4);
        server.abort();
    }
    #[tokio::test]
    async fn visibility_change_during_resolution_never_returns_cache_authority() {
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let app=axum::Router::new().fallback(move |uri:axum::http::Uri| {
            let calls=calls.clone();async move {
                if uri.path().contains("/contents/") {return (axum::http::StatusCode::NOT_FOUND,axum::Json(json!({})));}
                let value=if uri.path().contains("/commits/") {json!({"sha":"b".repeat(40)})}
                    else {json!({"id":7,"full_name":"example/repository","private":calls.fetch_add(1,Ordering::SeqCst)>0})};
                (axum::http::StatusCode::OK,axum::Json(value))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert_eq!(
            resolve_at(
                &origin,
                "github.com/example/repository",
                "main",
                EnvironmentRecipe::default()
            )
            .await
            .err()
            .unwrap()
            .code,
            "project_cache_public_source_required"
        );
        assert_eq!(count.load(Ordering::SeqCst), 2);
        server.abort();
    }
}
