//! Source bundles are built outside computers. Private fetch credentials stay in
//! one isolated Git process environment and never enter a reusable image.
//! Every R2 object is content-addressed and its original intent precedes PUT.
use super::*;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path, process::Stdio, sync::Arc};
use tokio::io::AsyncReadExt;

const MAX_BUNDLE: usize = 32 * 1024 * 1024;
const CHUNK: usize = 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Chunk {
    key: String,
    sha256: String,
    bytes: usize,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    encrypted: bool,
}
#[derive(Clone)]
pub struct Bundle {
    pub commit: String,
    pub sha256: String,
    pub bytes: Vec<u8>,
    pub dependency_inputs: BTreeMap<String, String>,
    pub recipe: zuse_protocol::cloud_computers::EnvironmentRecipe,
    pub recipe_source: String,
}
#[async_trait]
pub trait Bundles: Send + Sync {
    /// Recheck immediately before a provider receives source bytes. Object IO may
    /// have outlived the worker's initial step authorization.
    async fn authorize_placement(&self, _request: &worker::Request) -> Result<(), ApiError> {
        Ok(())
    }

    async fn reap(&self, _now: i64) -> Result<bool, ApiError> {
        Ok(false)
    }
    async fn resolve(&self, request: &worker::Request) -> Result<Bundle, ApiError>;
    async fn load(&self, sha256: &str, owner: &str, project: &str) -> Result<Vec<u8>, ApiError>;
}
pub struct PublicBundles {
    pool: PgPool,
    objects: Arc<dyn crate::objects::Objects>,
    http: reqwest::Client,
    authority: Option<Arc<dyn SourceAuthority>>,
    cipher: Option<Arc<dyn SourceCipher>>,
}
/// The API owns GitHub App/user authority; preparation workers receive only a
/// repository-scoped, expiring fetch credential. No signing key enters workers.
pub trait SourceCipher: Send + Sync {
    fn seal(&self, object_key: &str, bytes: &[u8]) -> Result<Vec<u8>, ApiError>;
    fn open(&self, object_key: &str, bytes: &[u8]) -> Result<Vec<u8>, ApiError>;
}
pub struct SourceCredential {
    pub token: zeroize::Zeroizing<String>,
    pub github_user_id: i64,
    /// Local authority deadline; GitHub alone controls upstream bearer expiry.
    pub expires_at: i64,
    pub provider_expires_at: i64,
    pub authorization: String,
}
#[async_trait]
pub trait SourceAuthority: Send + Sync {
    async fn authorize_request(&self, request: &worker::Request) -> Result<(), ApiError>;
    async fn credential(&self, request: &worker::Request) -> Result<SourceCredential, ApiError>;
    async fn revalidate(
        &self,
        request: &worker::Request,
        credential: &SourceCredential,
    ) -> Result<(), ApiError>;
}
impl PublicBundles {
    pub fn with_cipher(mut self, cipher: Arc<dyn SourceCipher>) -> Self {
        self.cipher = Some(cipher);
        self
    }
    pub fn with_source_authority(mut self, authority: Arc<dyn SourceAuthority>) -> Self {
        self.authority = Some(authority);
        self
    }

    pub fn new(pool: PgPool, objects: Arc<dyn crate::objects::Objects>) -> Result<Self, ApiError> {
        if !std::process::Command::new("git")
            .arg("--version")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return Err(ApiError::unavailable("project_cache_git_not_installed"));
        }
        Self::for_cleanup(pool, objects)
    }
    pub(crate) fn for_cleanup(
        pool: PgPool,
        objects: Arc<dyn crate::objects::Objects>,
    ) -> Result<Self, ApiError> {
        Ok(Self {
            pool,
            objects,
            authority: None,
            cipher: None,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(10))
                .user_agent("Zuse project preparation")
                .build()
                .map_err(|_| ApiError::unavailable("project_source_transport_unavailable"))?,
        })
    }
    async fn prove_public(&self, repository: &str) -> Result<(), ApiError> {
        let name = repository
            .strip_prefix("github.com/")
            .ok_or_else(|| ApiError::conflict("project_cache_repository_invalid"))?;
        let parts: Vec<_> = name.split('/').collect();
        if parts.len() != 2
            || parts.iter().any(|part| {
                part.is_empty()
                    || part.len() > 100
                    || !part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            })
        {
            return Err(ApiError::conflict("project_cache_repository_invalid"));
        }
        let response = self
            .http
            .get(format!("https://api.github.com/repos/{name}"))
            .header("accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|_| ApiError::unavailable("project_cache_public_source_unavailable"))?;
        if !response.status().is_success() {
            return Err(ApiError::conflict("project_cache_public_source_required"));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| ApiError::unavailable("project_cache_public_source_unavailable"))?;
        if bytes.len() > 512 * 1024 {
            return Err(ApiError::unavailable(
                "project_cache_source_response_invalid",
            ));
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::unavailable("project_cache_source_response_invalid"))?;
        public_metadata(name, &value)
    }
    async fn store(
        &self,
        request: &worker::Request,
        bundle: &Bundle,
        now: i64,
    ) -> Result<(), ApiError> {
        let prefix = format!(
            "project-environments/{:x}/{:x}/{}",
            Sha256::digest(request.owner.as_bytes()),
            Sha256::digest(request.project.as_bytes()),
            bundle.sha256
        );
        let chunks: Vec<Chunk> = bundle
            .bytes
            .chunks(CHUNK)
            .enumerate()
            .map(|(n, data)| Chunk {
                key: format!("{prefix}/chunk-{n}"),
                sha256: format!("{:x}", Sha256::digest(data)),
                bytes: data.len(),
                encrypted: self.cipher.is_some(),
            })
            .collect();
        // Immutable object keys/checksums survive request and project removal. PUT
        // retries compare original bytes; no new object/nonce is invented on timeout.
        sqlx::query("INSERT INTO zuse_rust_project_source_bundles(owner_id,project_id,source_commit,bundle_sha256,bytes,chunks,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,'writing',$7,$7) ON CONFLICT(owner_id,project_id,bundle_sha256) DO NOTHING")
   .bind(&request.owner).bind(&request.project).bind(&bundle.commit).bind(&bundle.sha256).bind(bundle.bytes.len() as i64).bind(json!(chunks)).bind(now).execute(&self.pool).await?;
        let original:Value=sqlx::query_scalar("SELECT chunks FROM zuse_rust_project_source_bundles WHERE owner_id=$1 AND project_id=$2 AND bundle_sha256=$3").bind(&request.owner).bind(&request.project).bind(&bundle.sha256).fetch_one(&self.pool).await?;
        if original != json!(chunks) {
            return Err(ApiError::conflict("project_cache_source_identity_changed"));
        }
        let state:String=sqlx::query_scalar("SELECT state FROM zuse_rust_project_source_bundles WHERE owner_id=$1 AND project_id=$2 AND bundle_sha256=$3").bind(&request.owner).bind(&request.project).bind(&bundle.sha256).fetch_one(&self.pool).await?;
        if state == "cleaning" {
            return Err(ApiError::conflict("project_cache_source_retired"));
        }
        for (chunk, data) in chunks.iter().zip(bundle.bytes.chunks(CHUNK)) {
            if let Some(existing) = self.objects.get(&chunk.key).await? {
                let existing = if chunk.encrypted {
                    self.cipher
                        .as_ref()
                        .ok_or_else(|| {
                            ApiError::unavailable("project_cache_source_key_unavailable")
                        })?
                        .open(&chunk.key, &existing)?
                } else {
                    existing
                };
                if existing != data {
                    return Err(ApiError::conflict("project_cache_source_object_changed"));
                }
            } else {
                if state == "ready" {
                    return Err(ApiError::conflict("project_cache_source_object_missing"));
                }
                let stored = if chunk.encrypted {
                    self.cipher.as_ref().unwrap().seal(&chunk.key, data)?
                } else {
                    data.to_vec()
                };
                self.objects.put(&chunk.key, &stored).await?;
            }
        }
        sqlx::query("UPDATE zuse_rust_project_source_bundles SET state='ready',updated_at=$4 WHERE owner_id=$1 AND project_id=$2 AND bundle_sha256=$3 AND state='writing'").bind(&request.owner).bind(&request.project).bind(&bundle.sha256).bind(now).execute(&self.pool).await?;
        Ok(())
    }
}
pub(super) fn public_metadata(name: &str, value: &Value) -> Result<(), ApiError> {
    if value["private"] != false
        || !value["full_name"]
            .as_str()
            .is_some_and(|full| full.eq_ignore_ascii_case(name))
        || value["id"].as_i64().is_none_or(|id| id <= 0)
    {
        return Err(ApiError::conflict("project_cache_public_source_required"));
    }
    Ok(())
}
struct GitChild {
    child: tokio::process::Child,
    pid: Option<u32>,
}
impl Drop for GitChild {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = self.child.start_kill();
    }
}
fn bounded_tree_bytes(root: &Path) -> Result<u64, ApiError> {
    let mut pending = vec![root.to_path_buf()];
    let mut total = 0u64;
    let mut count = 0usize;
    while let Some(path) = pending.pop() {
        for item in std::fs::read_dir(path)
            .map_err(|_| ApiError::unavailable("project_cache_staging_unavailable"))?
        {
            let item =
                item.map_err(|_| ApiError::unavailable("project_cache_staging_unavailable"))?;
            let meta = std::fs::symlink_metadata(item.path())
                .map_err(|_| ApiError::unavailable("project_cache_staging_unavailable"))?;
            count += 1;
            if count > 100000 {
                return Err(ApiError::conflict("project_cache_source_too_large"));
            }
            if meta.is_dir() {
                pending.push(item.path());
            } else {
                total = total.saturating_add(meta.len());
                if total > 128 * 1024 * 1024 {
                    return Err(ApiError::conflict("project_cache_source_too_large"));
                }
            }
        }
    }
    Ok(total)
}
fn askpass() -> Result<tempfile::TempPath, ApiError> {
    let mut file = tempfile::NamedTempFile::new()
        .map_err(|_| ApiError::unavailable("project_cache_staging_unavailable"))?;
    use std::io::Write;
    file.write_all(b"#!/bin/sh\ncase \"$1\" in\n\"Username for 'https://github.com': \") printf '%s' x-access-token;;\n\"Password for 'https://x-access-token@github.com': \") printf '%s' \"$ZUSE_PROJECT_FETCH_TOKEN\";;\n*) exit 1;;\nesac\n")
    .map_err(|_| ApiError::unavailable("project_cache_staging_unavailable"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o700))
            .map_err(|_| ApiError::unavailable("project_cache_staging_unavailable"))?;
    }
    let path = file.into_temp_path();
    Ok(path)
}
async fn git(root: &Path, args: &[&str], capture: bool) -> Result<String, ApiError> {
    git_authenticated(root, args, capture, None).await
}
async fn git_authenticated(
    root: &Path,
    args: &[&str],
    capture: bool,
    credential: Option<&SourceCredential>,
) -> Result<String, ApiError> {
    let mut c = tokio::process::Command::new("git");
    c.current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "pack.threads=1",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(if capture {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .kill_on_drop(true);
    // Only fetch receives the credential, through an isolated askpass process.
    // The script contains no credential. Git configuration and source.bundle do
    // not contain authentication material; stderr is deliberately suppressed.
    let _askpass = if let Some(credential) = credential {
        if credential.expires_at <= chrono::Utc::now().timestamp_millis() {
            return Err(ApiError::forbidden(
                "project_cache_source_credential_expired",
            ));
        }
        let path = askpass()?;
        c.env("GIT_ASKPASS", path.as_os_str())
            .env("ZUSE_PROJECT_FETCH_TOKEN", credential.token.as_str())
            .args([
                "-c",
                "credential.helper=",
                "-c",
                "http.followRedirects=false",
                "-c",
                "protocol.file.allow=never",
                "-c",
                "protocol.ext.allow=never",
                "-c",
                "fetch.recurseSubmodules=false",
            ]);
        Some(path)
    } else {
        None
    };
    c.args(args);
    #[cfg(unix)]
    unsafe {
        c.pre_exec(|| {
            if libc::setpgid(0, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = c
        .spawn()
        .map_err(|_| ApiError::unavailable("project_cache_git_unavailable"))?;
    let pid = child.id();
    let mut child = GitChild { child, pid };
    let mut out = Vec::new();
    if capture {
        child
            .child
            .stdout
            .take()
            .expect("piped")
            .take(1024 * 1024 + 1)
            .read_to_end(&mut out)
            .await
            .map_err(|_| ApiError::unavailable("project_cache_git_failed"))?;
        if out.len() > 1024 * 1024 {
            return Err(ApiError::conflict("project_cache_source_too_large"));
        }
    }
    let status = loop {
        tokio::select! {
          result=child.child.wait()=>break result.map_err(|_|ApiError::unavailable("project_cache_git_failed"))?,
          _=tokio::time::sleep(std::time::Duration::from_millis(250))=>{let path=root.to_path_buf();tokio::task::spawn_blocking(move||bounded_tree_bytes(&path)).await.map_err(|_|ApiError::unavailable("project_cache_staging_unavailable"))??;}
        }
    };
    child.pid = None;
    if !status.success() {
        return Err(ApiError::conflict("project_cache_git_failed"));
    }
    String::from_utf8(out)
        .map(|s| s.trim().into())
        .map_err(|_| ApiError::conflict("project_cache_git_failed"))
}
async fn build_bundle(
    url: &str,
    reference: &str,
    recipe: zuse_protocol::cloud_computers::EnvironmentRecipe,
) -> Result<Bundle, ApiError> {
    build_bundle_authenticated(url, reference, recipe, None).await
}
async fn build_bundle_authenticated(
    url: &str,
    reference: &str,
    recipe: zuse_protocol::cloud_computers::EnvironmentRecipe,
    credential: Option<&SourceCredential>,
) -> Result<Bundle, ApiError> {
    let temp = tempfile::tempdir()
        .map_err(|_| ApiError::unavailable("project_cache_staging_unavailable"))?;
    let repo = temp.path().join("repository.git");
    git(
        temp.path(),
        &[
            "init",
            "--bare",
            repo.to_str()
                .ok_or_else(|| ApiError::unavailable("project_cache_staging_unavailable"))?,
        ],
        false,
    )
    .await?;
    git_authenticated(
        &repo,
        &["fetch", "--no-tags", "--", url, reference],
        false,
        credential,
    )
    .await?;
    let commit = git(&repo, &["rev-parse", "FETCH_HEAD^{commit}"], true).await?;
    git(&repo, &["update-ref", "refs/heads/source", &commit], false).await?;
    let tree = git(
        &repo,
        &["ls-tree", "-r", "--format=%(objectname) %(path)", &commit],
        true,
    )
    .await?;
    let mut inputs = BTreeMap::new();
    for line in tree.lines() {
        if let Some((sha, path)) = line.split_once(' ') {
            if matches!(
                path,
                "package-lock.json"
                    | "pnpm-lock.yaml"
                    | "yarn.lock"
                    | "Cargo.lock"
                    | "uv.lock"
                    | "poetry.lock"
                    | "requirements.txt"
                    | ".zuse/setup.json"
            ) {
                inputs.insert(path.into(), sha.into());
            }
        }
    }
    let (recipe, source) = if inputs.contains_key(".zuse/setup.json") {
        let text = git(
            &repo,
            &["show", &format!("{commit}:.zuse/setup.json")],
            true,
        )
        .await?;
        (
            serde_json::from_str(&text)
                .map_err(|_| ApiError::conflict("environment_recipe_invalid"))?,
            "repository",
        )
    } else {
        (recipe, "project")
    };
    recipe.validate().map_err(ApiError::bad_request)?;
    if !recipe.secret_bindings.is_empty() {
        return Err(ApiError::conflict("project_cache_runtime_secrets_required"));
    }

    let file = temp.path().join("source.bundle");
    git(
        &repo,
        &[
            "bundle",
            "create",
            file.to_str().expect("temporary path"),
            "refs/heads/source",
        ],
        false,
    )
    .await?;
    let size = std::fs::metadata(&file)
        .map_err(|_| ApiError::unavailable("project_cache_source_unavailable"))?
        .len();
    if size > MAX_BUNDLE as u64 {
        return Err(ApiError::conflict("project_cache_source_too_large"));
    }
    let bytes = std::fs::read(file)
        .map_err(|_| ApiError::unavailable("project_cache_source_unavailable"))?;
    let bundle = Bundle {
        commit,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        bytes,
        dependency_inputs: inputs,
        recipe,
        recipe_source: source.into(),
    };
    Ok(bundle)
}
#[async_trait]
impl Bundles for PublicBundles {
    async fn authorize_placement(&self, request: &worker::Request) -> Result<(), ApiError> {
        if let Some(authority) = &self.authority {
            authority.authorize_request(request).await
        } else {
            // Anonymous sources still prove public visibility; private sources
            // cannot enter this implementation without an API authority.
            self.prove_public(&request.repository_identity).await
        }
    }

    async fn reap(&self, now: i64) -> Result<bool, ApiError> {
        PublicBundles::reap(self, now).await
    }
    async fn resolve(&self, request: &worker::Request) -> Result<Bundle, ApiError> {
        let project:Option<(String,String,Value)>=sqlx::query_as("SELECT repository_identity,visibility,secret_bindings FROM api_cloud_projects WHERE account_id=$1 AND project_id=$2 AND included=true").bind(&request.owner).bind(&request.project).fetch_optional(&self.pool).await?;
        if project
            .as_ref()
            .is_none_or(|(repository, visibility, bindings)| {
                repository != &request.repository_identity
                    || !matches!(visibility.as_str(), "public" | "private")
                    || bindings != &json!([])
            })
        {
            return Err(ApiError::conflict("project_cache_public_source_required"));
        }
        let private = project
            .as_ref()
            .is_some_and(|(_, visibility, _)| visibility == "private");
        if private && self.cipher.is_none() {
            return Err(ApiError::unavailable(
                "project_cache_private_source_encryption_required",
            ));
        }
        let credential = if private {
            Some(
                self.authority
                    .as_ref()
                    .ok_or_else(|| {
                        ApiError::unavailable("project_cache_source_authority_unavailable")
                    })?
                    .credential(request)
                    .await?,
            )
        } else {
            self.prove_public(&request.repository_identity).await?;
            None
        };
        let configured:Value=sqlx::query_scalar("SELECT recipe FROM cloud_environment_versions WHERE owner_id=$1 AND project_id=$2 AND revision=$3").bind(&request.owner).bind(&request.project).bind(request.environment_revision).fetch_one(&self.pool).await?;
        let recipe: zuse_protocol::cloud_computers::EnvironmentRecipe =
            serde_json::from_value(configured)
                .map_err(|_| ApiError::conflict("environment_recipe_invalid"))?;
        if !recipe.secret_bindings.is_empty() {
            return Err(ApiError::conflict("project_cache_runtime_secrets_required"));
        }
        let bundle = build_bundle_authenticated(
            &format!("https://{}.git", request.repository_identity),
            &request.requested_ref,
            recipe,
            credential.as_ref(),
        )
        .await?;
        if let Some(credential) = &credential {
            if credential.expires_at <= chrono::Utc::now().timestamp_millis() {
                return Err(ApiError::forbidden(
                    "project_cache_source_credential_expired",
                ));
            }
            self.authority
                .as_ref()
                .unwrap()
                .revalidate(request, credential)
                .await?;
        } else {
            self.prove_public(&request.repository_identity).await?;
        }
        self.store(request, &bundle, chrono::Utc::now().timestamp_millis())
            .await?;
        Ok(bundle)
    }
    async fn load(&self, sha: &str, owner: &str, project: &str) -> Result<Vec<u8>, ApiError> {
        let row:Option<(i64,Value)>=sqlx::query_as("SELECT bytes,chunks FROM zuse_rust_project_source_bundles WHERE owner_id=$1 AND project_id=$2 AND bundle_sha256=$3 AND state='ready'").bind(owner).bind(project).bind(sha).fetch_optional(&self.pool).await?;
        let (size, value) =
            row.ok_or_else(|| ApiError::conflict("project_cache_source_missing"))?;
        if size < 0 || size as usize > MAX_BUNDLE {
            return Err(ApiError::conflict("project_cache_source_too_large"));
        }
        let chunks: Vec<Chunk> = serde_json::from_value(value)
            .map_err(|_| ApiError::conflict("project_cache_source_invalid"))?;
        if chunks.is_empty() || chunks.len() > 32 {
            return Err(ApiError::conflict("project_cache_source_invalid"));
        }
        let mut out = Vec::new();
        let prefix = format!(
            "project-environments/{:x}/{:x}/{sha}",
            Sha256::digest(owner.as_bytes()),
            Sha256::digest(project.as_bytes())
        );
        for (n, chunk) in chunks.into_iter().enumerate() {
            if chunk.key != format!("{prefix}/chunk-{n}") || chunk.bytes == 0 || chunk.bytes > CHUNK
            {
                return Err(ApiError::conflict("project_cache_source_invalid"));
            }
            let data = self
                .objects
                .get(&chunk.key)
                .await?
                .ok_or_else(|| ApiError::conflict("project_cache_source_missing"))?;
            let data = if chunk.encrypted {
                self.cipher
                    .as_ref()
                    .ok_or_else(|| ApiError::unavailable("project_cache_source_key_unavailable"))?
                    .open(&chunk.key, &data)?
            } else {
                data
            };
            if data.len() != chunk.bytes || format!("{:x}", Sha256::digest(&data)) != chunk.sha256 {
                return Err(ApiError::conflict("project_cache_source_invalid"));
            }
            out.extend_from_slice(&data);
            if out.len() > MAX_BUNDLE {
                return Err(ApiError::conflict("project_cache_source_too_large"));
            }
        }
        if out.len() != size as usize || format!("{:x}", Sha256::digest(&out)) != sha {
            return Err(ApiError::conflict("project_cache_source_invalid"));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests;

mod retirement;
