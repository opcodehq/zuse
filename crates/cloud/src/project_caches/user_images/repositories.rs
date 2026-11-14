//! Repositories baked into a user image. Each of the account's included
//! repositories is captured on the server as a snapshot of its default branch
//! (a bare repository, no remote, no credential), shipped into the
//! builder in upload-sized chunks, and unpacked by the guest helper to
//! `/opt/zuse/repos/<sha256(repository identity)>.git`, owned by `zuse`. A new
//! computer whose image holds its repository starts the checkout from that
//! path and fetches only what changed since.
//!
//! History: the whole default branch while it fits
//! `repository_source::FULL_HISTORY_MAX_BYTES`, else only the tip. The fetch
//! delta is the same either way (the runtime fetches `--depth=1
//! --filter=blob:none` and offers the baked tip as a `have`), but the runtime
//! starts by cloning the baked copy locally, and Git copies a complete
//! repository's files while it repacks a shallow one (measured on a 1-vCPU
//! computer for opcodehq/zuse: ~70 ms vs ~2.2 s).
//!
//! The step is provider-neutral: a backend only has to expose its builder
//! through [`BuilderGuest`] (exec as a sudo-capable user, and file upload).
use super::*;
use crate::providers::ExecResult;
use serde_json::json;
use std::{path::PathBuf, time::Duration};

/// Repositories baked into one image; the rest start with a clone.
pub const MAX_REPOSITORIES: usize = 8;
/// Largest snapshot of one repository (a bare repository, as a tar).
pub const MAX_REPOSITORY_BYTES: u64 = 1024 * 1024 * 1024;
/// Largest total of all snapshots in one image.
pub const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// A ready image whose baked commits are older than this is rebuilt by the
/// hourly pass, so a new computer's fetch delta stays small.
pub const MAX_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Bytes per provider upload call.
pub const UPLOAD_CHUNK: usize = 2 * 1024 * 1024;
/// A repository whose snapshot or unpack keeps failing is skipped after this
/// many attempts in one build.
pub const MAX_ATTEMPTS: u64 = 3;
/// Where baked repositories live in the image.
pub const ROOT: &str = "/opt/zuse/repos";
/// Upload staging on the builder; sanitize removes it with the rest of /var/tmp.
pub const STAGE: &str = "/var/tmp/zuse-repo-stage";
/// Time one worker tick may spend starting new uploads before handing back.
const STEP_BUDGET: Duration = Duration::from_secs(45);

/// One of the account's included repositories, as frozen into the plan.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedRepository {
    pub project_id: String,
    pub repository_identity: String,
    pub repository_url: String,
    pub default_branch: String,
    pub visibility: String,
}
impl PlannedRepository {
    pub fn key(&self) -> String {
        key(&self.repository_identity)
    }
    fn valid(&self) -> bool {
        id(&self.project_id)
            && identity_valid(&self.repository_identity)
            && url_valid(&self.repository_url, &self.repository_identity)
            && branch_valid(&self.default_branch)
            && matches!(self.visibility.as_str(), "public" | "private")
    }
}

/// A repository left out of the image, with the reason the owner sees.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkippedRepository {
    pub project_id: String,
    pub repository_identity: String,
    pub reason: String,
}

/// A repository present in the image.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BakedRepository {
    pub project_id: String,
    pub repository_identity: String,
    pub branch: String,
    pub commit: String,
    pub bytes: u64,
    pub path: String,
}

/// What `cloud_machine_images.repositories` records for a build.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageRepositories {
    pub version: u32,
    #[serde(default)]
    pub baked: Vec<BakedRepository>,
    #[serde(default)]
    pub skipped: Vec<SkippedRepository>,
}
impl ImageRepositories {
    /// The baked copy of `identity`, if this image has one at a valid path.
    pub fn find(&self, identity: &str) -> Option<&BakedRepository> {
        self.baked.iter().find(|r| {
            r.repository_identity.eq_ignore_ascii_case(identity)
                && r.path == path(&r.repository_identity)
                && commit_valid(&r.commit)
        })
    }
}

pub fn key(identity: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(identity.to_ascii_lowercase().as_bytes())
    )
}
/// The fixed path of a repository in every image.
pub fn path(identity: &str) -> String {
    format!("{ROOT}/{}.git", key(identity))
}
fn identity_valid(identity: &str) -> bool {
    identity.strip_prefix("github.com/").is_some_and(|name| {
        let parts: Vec<_> = name.split('/').collect();
        parts.len() == 2
            && parts.iter().all(|part| {
                !part.is_empty()
                    && part.len() <= 100
                    && !part.starts_with('.')
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            })
    })
}
fn url_valid(url: &str, identity: &str) -> bool {
    let Some(name) = identity.strip_prefix("github.com/") else {
        return false;
    };
    url.strip_prefix("https://github.com/")
        .and_then(|rest| rest.strip_suffix(".git").or(Some(rest)))
        .is_some_and(|rest| rest.eq_ignore_ascii_case(name))
}
pub(crate) fn branch_valid(branch: &str) -> bool {
    !branch.is_empty()
        && branch.len() <= 255
        && !branch.starts_with(['-', '/', '.'])
        && !branch.ends_with(['/', '.'])
        && !branch.ends_with(".lock")
        && !branch.contains("..")
        && !branch.contains("//")
        && !branch.contains("@{")
        && branch
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./+".contains(&b))
}
pub(crate) fn commit_valid(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64) && commit.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The account's included repositories, oldest first.
pub(crate) async fn account_repositories_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner: &str,
) -> Result<Vec<PlannedRepository>, ApiError> {
    let rows: Vec<(String, String, String, String, String)> = sqlx::query_as("SELECT project_id,repository_identity,repository_url,default_branch,visibility FROM api_cloud_projects WHERE account_id=$1 AND included=true ORDER BY created_at,project_id")
        .bind(owner)
        .fetch_all(&mut **tx)
        .await?;
    Ok(rows
        .into_iter()
        .map(
            |(project_id, repository_identity, repository_url, default_branch, visibility)| {
                PlannedRepository {
                    project_id,
                    repository_identity,
                    repository_url,
                    default_branch,
                    visibility,
                }
            },
        )
        .collect())
}

/// Digest of the account's repository set: adding or removing a repository,
/// or changing its default branch or visibility, rebuilds the image. New
/// commits do not; the fetch delta (and `MAX_AGE_MS`) covers them. `None`
/// for an account without repositories, so its image identity is unchanged.
pub fn set_digest(repositories: &[PlannedRepository]) -> Option<String> {
    if repositories.is_empty() {
        return None;
    }
    let mut entries: Vec<_> = repositories
        .iter()
        .map(|r| {
            (
                r.repository_identity.to_ascii_lowercase(),
                r.default_branch.clone(),
                r.visibility.clone(),
            )
        })
        .collect();
    entries.sort();
    entries.dedup();
    serde_json::to_vec(&json!({"version":1,"repositories":entries}))
        .ok()
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
}

/// Which repositories go into the image, which are left out and why.
/// Organization images serve every member, so an organization's private
/// repositories are not baked: a member without access to one would
/// otherwise get its source.
pub fn select(
    owner: &str,
    repositories: &[PlannedRepository],
) -> (Vec<PlannedRepository>, Vec<SkippedRepository>) {
    let mut planned: Vec<PlannedRepository> = Vec::new();
    let mut skipped = Vec::new();
    let skip = |r: &PlannedRepository, reason: &str| SkippedRepository {
        project_id: r.project_id.clone(),
        repository_identity: r.repository_identity.clone(),
        reason: reason.into(),
    };
    for repository in repositories {
        if !repository.valid() {
            skipped.push(skip(repository, "repository_invalid"));
        } else if owner.starts_with("organization:") && repository.visibility == "private" {
            skipped.push(skip(repository, "organization_private_repository"));
        } else if planned.iter().any(|p| p.key() == repository.key()) {
            skipped.push(skip(repository, "repository_duplicate"));
        } else if planned.len() >= MAX_REPOSITORIES {
            skipped.push(skip(repository, "repository_count_cap"));
        } else {
            planned.push(repository.clone());
        }
    }
    (planned, skipped)
}

pub(super) fn validate_plan(planned: &[PlannedRepository], skipped: &[SkippedRepository]) -> bool {
    planned.len() <= MAX_REPOSITORIES
        && skipped.len() <= 256
        && planned.iter().all(PlannedRepository::valid)
        && skipped.iter().all(|s| {
            id(&s.project_id) && s.repository_identity.len() <= 512 && s.reason.len() <= 64
        })
}

/// The builder as the repository step sees it. `exec` runs as a user that can
/// `sudo -n`; `upload` writes a file as that same user.
#[async_trait::async_trait]
pub trait BuilderGuest: Send + Sync {
    async fn exec(&self, command: &str) -> Result<ExecResult, ApiError>;
    async fn upload(&self, path: &str, data: &[u8]) -> Result<(), ApiError>;
}

/// A server-made snapshot waiting on the worker's disk.
#[derive(Clone, Debug)]
pub struct SourceSnapshot {
    pub commit: String,
    pub sha256: String,
    pub bytes: u64,
    pub file: PathBuf,
}
#[derive(Debug)]
pub enum SourceError {
    /// This repository cannot be baked in this build (too large, no access).
    Skip(&'static str),
    /// Try again on a later tick (network, credential service).
    Retry(&'static str),
}

/// Produces repository snapshots outside any computer. Credentials stay in
/// the producer; the snapshot holds no remote, config or credential.
#[async_trait::async_trait]
pub trait RepositorySource: Send + Sync {
    async fn snapshot(
        &self,
        image_id: &str,
        owner: &str,
        repository: &PlannedRepository,
    ) -> Result<SourceSnapshot, SourceError>;
    /// Drop whatever this build left on the worker's disk.
    fn discard(&self, image_id: &str);
}

pub enum RepoStep {
    Pending,
    Done(ImageRepositories),
}

pub(crate) fn helper_command(args: &[&str]) -> String {
    format!(
        "sudo -n /usr/bin/python3 -I -B -c {} {}",
        super::boxd::quote(GUEST_HELPER),
        args.iter()
            .map(|a| super::boxd::quote(a))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

async fn helper(guest: &dyn BuilderGuest, args: &[&str]) -> Result<Value, ApiError> {
    let result = guest.exec(&helper_command(args)).await?;
    if result.stdout.len() > 256 * 1024 {
        return Err(ApiError::conflict("user_image_guest_receipt_invalid"));
    }
    let value: Value = serde_json::from_slice(&result.stdout)
        .map_err(|_| ApiError::conflict("user_image_guest_receipt_invalid"))?;
    if value["version"] != 1 {
        return Err(ApiError::conflict("user_image_guest_receipt_invalid"));
    }
    Ok(value)
}

fn record(
    plan: &BuildPlan,
    status: &Value,
    skipped_now: &[SkippedRepository],
) -> ImageRepositories {
    let mut out = ImageRepositories {
        version: 1,
        baked: vec![],
        skipped: plan.skipped_repositories.clone(),
    };
    for repository in &plan.repositories {
        let key = repository.key();
        let baked = &status["baked"][&key];
        if let (Some(commit), Some(branch)) = (baked["commit"].as_str(), baked["branch"].as_str())
            && commit_valid(commit)
            && branch == repository.default_branch
        {
            out.baked.push(BakedRepository {
                project_id: repository.project_id.clone(),
                repository_identity: repository.repository_identity.clone(),
                branch: branch.into(),
                commit: commit.to_ascii_lowercase(),
                bytes: baked["bytes"].as_u64().unwrap_or(0),
                path: path(&repository.repository_identity),
            });
        } else {
            let reason = skipped_now
                .iter()
                .find(|s| s.project_id == repository.project_id)
                .map(|s| s.reason.clone())
                .or_else(|| {
                    status["skipped"][&key]["reason"]
                        .as_str()
                        .filter(|r| r.len() <= 64)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "repository_not_baked".into());
            out.skipped.push(SkippedRepository {
                project_id: repository.project_id.clone(),
                repository_identity: repository.repository_identity.clone(),
                reason,
            });
        }
    }
    out
}

/// One bounded unit of repository work on the builder. Progress lives on the
/// builder (what is baked, staged or given up), so a lost reply or a worker
/// restart resumes where the guest is, and nothing is unpacked twice.
pub async fn step(
    guest: &dyn BuilderGuest,
    source: Option<&dyn RepositorySource>,
    build: &Build,
) -> Result<RepoStep, ApiError> {
    let plan = &build.plan;
    if plan.repositories.is_empty() {
        return Ok(RepoStep::Done(ImageRepositories {
            version: 1,
            baked: vec![],
            skipped: plan.skipped_repositories.clone(),
        }));
    }
    let started = std::time::Instant::now();
    let mut status = helper(guest, &["repos-status"]).await?;
    let mut skipped_now = Vec::new();
    let mut total: u64 = plan
        .repositories
        .iter()
        .filter_map(|r| status["baked"][r.key()]["bytes"].as_u64())
        .sum();
    for repository in &plan.repositories {
        let key = repository.key();
        if status["baked"][&key].is_object() || status["skipped"][&key]["final"] == true {
            continue;
        }
        let give_up = |reason: &'static str, attempts_left: bool| {
            let key = key.clone();
            async move {
                helper(
                    guest,
                    &[
                        "repos-skip",
                        &key,
                        reason,
                        if attempts_left { "0" } else { "1" },
                    ],
                )
                .await
            }
        };
        let Some(source) = source else {
            give_up("repository_source_unavailable", false).await?;
            skipped_now.push(SkippedRepository {
                project_id: repository.project_id.clone(),
                repository_identity: repository.repository_identity.clone(),
                reason: "repository_source_unavailable".into(),
            });
            continue;
        };
        if started.elapsed() > STEP_BUDGET {
            return Ok(RepoStep::Pending);
        }
        let attempts = status["skipped"][&key]["attempts"].as_u64().unwrap_or(0);
        let snapshot = match source
            .snapshot(&build.image_id, &plan.owner(), repository)
            .await
        {
            Ok(snapshot) => snapshot,
            Err(SourceError::Skip(reason)) => {
                give_up(reason, false).await?;
                continue;
            }
            Err(SourceError::Retry(reason)) => {
                tracing::warn!(image = %build.image_id, reason, "User image repository snapshot deferred");
                give_up(reason, attempts + 1 < MAX_ATTEMPTS).await?;
                return Ok(RepoStep::Pending);
            }
        };
        if snapshot.bytes == 0
            || snapshot.bytes > MAX_REPOSITORY_BYTES
            || !commit_valid(&snapshot.commit)
            || !super::hash(&snapshot.sha256)
        {
            give_up("repository_too_large", false).await?;
            continue;
        }
        if total + snapshot.bytes > MAX_TOTAL_BYTES {
            give_up("repository_total_size_cap", false).await?;
            continue;
        }
        let chunks = snapshot.bytes.div_ceil(UPLOAD_CHUNK as u64);
        let staged = &status["staged"][&key];
        if staged["sha256"] != snapshot.sha256.as_str() || staged["chunkCount"] != chunks {
            status["staged"][&key] = helper(
                guest,
                &[
                    "repos-stage",
                    &key,
                    &snapshot.sha256,
                    &snapshot.bytes.to_string(),
                    &chunks.to_string(),
                ],
            )
            .await?;
        }
        let present = status["staged"][&key]["chunks"].clone();
        let file = snapshot.file.clone();
        for n in 0..chunks {
            let size = if n + 1 == chunks {
                snapshot.bytes - n * UPLOAD_CHUNK as u64
            } else {
                UPLOAD_CHUNK as u64
            };
            if present[n.to_string()].as_u64() == Some(size) {
                continue;
            }
            if started.elapsed() > STEP_BUDGET {
                return Ok(RepoStep::Pending);
            }
            let data = {
                let file = file.clone();
                tokio::task::spawn_blocking(move || read_chunk(&file, n, size))
                    .await
                    .map_err(|_| ApiError::unavailable("user_image_repository_snapshot_lost"))?
            };
            let Ok(data) = data else {
                // The worker's copy vanished (restart); make it again next tick.
                source.discard(&build.image_id);
                return Ok(RepoStep::Pending);
            };
            guest
                .upload(&format!("{STAGE}/{key}/chunk.{n}"), &data)
                .await?;
        }
        let unpacked = helper(
            guest,
            &[
                "repos-unpack",
                &key,
                &repository.default_branch,
                &snapshot.commit,
            ],
        )
        .await?;
        if unpacked["baked"] != true {
            let code = unpacked["error"]
                .as_str()
                .unwrap_or("repository_unpack_failed");
            tracing::warn!(image = %build.image_id, code, "User image repository unpack failed");
            give_up("repository_unpack_failed", attempts + 1 < MAX_ATTEMPTS).await?;
            return Ok(RepoStep::Pending);
        }
        total += snapshot.bytes;
        status["baked"][&key] = unpacked;
    }
    let status = helper(guest, &["repos-status"]).await?;
    if let Some(source) = source {
        source.discard(&build.image_id);
    }
    Ok(RepoStep::Done(record(plan, &status, &skipped_now)))
}

fn read_chunk(file: &std::path::Path, n: u64, size: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut handle = std::fs::File::open(file)?;
    handle.seek(SeekFrom::Start(n * UPLOAD_CHUNK as u64))?;
    let mut data = vec![0; size as usize];
    handle.read_exact(&mut data)?;
    Ok(data)
}
