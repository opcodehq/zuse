//! User images: the repository-less cache kind. One image line per account
//! (personal or organization) and provider, built from the account's setup
//! script on a verified managed base. Like a project cache it keeps the base's
//! pinned runtime files byte-identical, is checked by a second boot of the
//! captured snapshot, and never replaces a ready image when a build fails.
//! It also carries the account's repositories as depth-1 snapshots of their
//! default branches (see `repositories`), so a new computer fetches only what
//! changed instead of cloning. It is rebuilt when the setup script, the base or
//! the repository set changes, or when the baked commits grow old.
//!
//! Instructions, skills and dotfiles are delivered per chat and are not part
//! of an image; only the setup script is.
use crate::error::ApiError;
use crate::managed_bases::bootstrap::InitialRuntimeBootstrap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use zuse_protocol::cloud_computers::{MachineRecipe, ProviderId};

pub mod boat;
pub mod boxd;
pub mod prized;
pub mod repositories;
pub mod repository_source;
pub mod schedule;

pub mod selection;
pub mod worker;

pub const MIGRATION: &str = include_str!("../../../migrations/0037_user_images.sql");
/// Which repositories an image holds, and which were left out.
pub const REPOSITORIES_MIGRATION: &str =
    include_str!("../../../migrations/0041_user_image_repositories.sql");
/// Guest helper run as root on builder and verifier VMs.
pub const GUEST_HELPER: &str = include_str!("../../../../../infra/cloud/user-image-guest.py");
/// Guest helper run as the only user on Prized builders and verifiers.
pub const PRIZED_GUEST_HELPER: &str =
    include_str!("../../../../../infra/cloud/prized-base/user-image-guest.py");
/// Providers with a user image builder. Others start from the managed base.
pub const PROVIDERS: &[ProviderId] = &[ProviderId::Boxd, ProviderId::Boat, ProviderId::Prized];
/// The provider every account with a setup script gets an image on. Other
/// providers get one only once the account uses them ([`used_providers_tx`]).
pub const DEFAULT_PROVIDER: ProviderId = ProviderId::Boxd;

/// SQL: does account `$1` use provider `$2`, i.e. has a computer there that
/// is not deleted or was active since `$3`? (`w` is free for the caller.)
pub(crate) const USES_PROVIDER: &str = "EXISTS(SELECT 1 FROM api_cloud_workspaces w WHERE w.account_id=$1 AND w.provider=$2 AND (w.state<>'deleted' OR w.last_activity_at>$3))";

/// The builder providers an account's image belongs on: the default provider,
/// plus each other provider where the account has a computer that is not
/// deleted or was active within the staleness window. A first computer on a
/// new provider boots that provider's base and queues its image on use.
pub(crate) async fn used_providers_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner: &str,
    now: i64,
) -> Result<Vec<ProviderId>, ApiError> {
    let mut used = Vec::new();
    for provider in PROVIDERS {
        if *provider == DEFAULT_PROVIDER
            || sqlx::query_scalar::<_, bool>(&format!("SELECT {USES_PROVIDER}"))
                .bind(owner)
                .bind(provider.as_str())
                .bind(now - schedule::ACTIVE_WINDOW_MS)
                .fetch_one(&mut **tx)
                .await?
        {
            used.push(*provider);
        }
    }
    Ok(used)
}

/// Prized bases live in the task user's home with no root, so they carry no
/// fresh-boot pins; the base runtime comes from the manifest's admission
/// receipt and the guest pins the rest of the managed tree itself.
pub(crate) fn home_base(provider: ProviderId) -> bool {
    provider == ProviderId::Prized
}
/// Longest log excerpt kept for the owner.
pub const LOG_EXCERPT: usize = 4096;

/// The image-relevant part of a machine recipe.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageRecipe<'a> {
    version: u32,
    setup_script: &'a str,
    setup_timeout_seconds: u32,
}

/// Digest of what goes into the image. `None` means the recipe needs no image
/// (no setup script) and the account starts from the managed base.
pub fn image_digest(recipe: &MachineRecipe) -> Option<String> {
    if recipe.setup_script.trim().is_empty() {
        return None;
    }
    script_digest(&recipe.setup_script, recipe.setup_timeout_seconds)
}

fn script_digest(script: &str, timeout: u32) -> Option<String> {
    serde_json::to_vec(&ImageRecipe {
        version: 1,
        setup_script: script,
        setup_timeout_seconds: timeout,
    })
    .ok()
    .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) fn provider_id(value: &str) -> Option<ProviderId> {
    ProviderId::ALL.into_iter().find(|p| p.as_str() == value)
}

pub(crate) fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
pub(crate) fn image_id_valid(value: &str) -> bool {
    value.strip_prefix("uimg_").is_some_and(|rest| {
        rest.len() == 32
            && rest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// What makes two images interchangeable. The repository set is an input
/// (which repositories, their default branches); commits, credentials and
/// agent connections are not.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserImageIdentity {
    pub version: u32,
    pub owner_id: String,
    pub provider: ProviderId,
    pub architecture: String,
    pub base_release_id: String,
    pub base_artifact_id: String,
    pub base_artifact_version: String,
    pub base_runtime_sha256: String,
    pub recipe_digest: String,
    /// `repositories::set_digest` of the account's repositories; absent for
    /// an account without repositories (and for images built before them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repositories_digest: Option<String>,
}
impl UserImageIdentity {
    pub fn validate(&self) -> Result<(), ApiError> {
        if self.version != 1
            || !id(&self.owner_id)
            || !id(&self.base_release_id)
            || !id(&self.base_artifact_id)
            || !id(&self.base_artifact_version)
            || !matches!(self.architecture.as_str(), "x86_64" | "aarch64")
            || !hash(&self.base_runtime_sha256)
            || !hash(&self.recipe_digest)
            || self
                .repositories_digest
                .as_deref()
                .is_some_and(|d| !hash(d))
        {
            return Err(ApiError::bad_request("user_image_identity_invalid"));
        }
        Ok(())
    }
    pub fn key(&self) -> Result<String, ApiError> {
        self.validate()?;
        Ok(format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(self)
                    .map_err(|_| ApiError::bad_request("user_image_identity_invalid"))?
            )
        ))
    }
}

/// Frozen when a build starts; every later phase uses exactly this plan.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildPlan {
    pub version: u32,
    pub image_id: String,
    pub identity: UserImageIdentity,
    pub setup_script: String,
    pub setup_timeout_seconds: u32,
    /// Required on root bases; absent on home-directory bases ([`home_base`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_runtime_bootstrap: Option<InitialRuntimeBootstrap>,
    /// Repositories to bake, in order. Empty for images without repositories.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repositories: Vec<repositories::PlannedRepository>,
    /// Included repositories left out at planning time, with the reason.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_repositories: Vec<repositories::SkippedRepository>,
}
impl BuildPlan {
    pub fn validate(&self) -> Result<(), ApiError> {
        self.identity.validate()?;
        if self.version != 1
            || !image_id_valid(&self.image_id)
            || self.setup_script.trim().is_empty()
            || self.setup_script.len() > 64 * 1024
            || !(60..=3600).contains(&self.setup_timeout_seconds)
            || script_digest(&self.setup_script, self.setup_timeout_seconds).as_deref()
                != Some(self.identity.recipe_digest.as_str())
            || !repositories::validate_plan(&self.repositories, &self.skipped_repositories)
            || (!self.repositories.is_empty() && self.identity.repositories_digest.is_none())
        {
            return Err(ApiError::bad_request("user_image_plan_invalid"));
        }
        let provider = self.identity.provider;
        let valid = match &self.initial_runtime_bootstrap {
            None => home_base(provider),
            Some(bootstrap) => {
                !home_base(provider)
                    && bootstrap.valid(provider)
                    && bootstrap.runtime_sha256 == self.identity.base_runtime_sha256
            }
        };
        if !valid {
            return Err(ApiError::conflict("user_image_initial_bootstrap_invalid"));
        }
        Ok(())
    }
    /// Names derived from the image ID: a lost reply is always reconciled by
    /// inspecting these exact resources, never by creating another.
    pub fn names(&self) -> (String, String, String) {
        let id = self.image_id.trim_start_matches("uimg_");
        (
            format!("ui-b-{id}"),
            format!("ui-v-{id}"),
            format!("ui-s-{id}"),
        )
    }
}

/// One durable build or retirement, as the backend sees it.
#[derive(Clone)]
pub struct Build {
    pub image_id: String,
    pub owner_id: String,
    pub plan: BuildPlan,
    pub phase: String,
    /// `effects` (what was requested) and `provider` (opaque receipts).
    pub resources: Value,
    /// Published artifact, for retirement.
    pub artifact_id: Option<String>,
}

pub use crate::project_caches::worker::StepMode;

pub enum Step {
    /// Nothing changed phase; keep these receipts and come back.
    Pending(Value),
    Advance {
        phase: String,
        resources: Value,
    },
    /// Only from `publish`: the verified, captured artifact.
    Ready {
        artifact_id: String,
        artifact_version: Option<String>,
        resources: Value,
    },
    /// The build failed for a reason the owner can act on. Cleanup follows.
    Failed {
        code: &'static str,
        log: Option<String>,
        resources: Value,
    },
}

/// The ordered build phases. Cleanup and retirement are separate chains.
/// `repositories` is run by the worker for every backend (see
/// `UserImageBackend::builder_guest`); a backend's `setup` may advance
/// straight to `sanitize` and the worker inserts it.
pub const PHASES: &[&str] = &[
    "allocate",
    "setup",
    "repositories",
    "sanitize",
    "capture",
    "retire-builder",
    "verify",
    "retire-verifier",
    "publish",
];
pub fn next_phase(phase: &str) -> Option<&'static str> {
    match phase {
        "cleanup" => Some("complete"),
        "retire" => Some("retired"),
        _ => PHASES
            .iter()
            .position(|p| *p == phase)
            .and_then(|i| PHASES.get(i + 1).copied()),
    }
}

#[async_trait::async_trait]
pub trait UserImageBackend: Send + Sync {
    /// Exact provider version of a base artifact, or `None` if it is not
    /// usable. Part of the identity: a re-captured base is a different base.
    async fn base_version(
        &self,
        provider: ProviderId,
        artifact: &str,
    ) -> Result<Option<String>, ApiError>;
    /// Run or reconcile one phase. `Reconcile` inspects the original
    /// resources and never submits a second create, launch or capture.
    async fn step(&self, build: &Build, mode: StepMode) -> Result<Step, ApiError>;
    /// The running builder, for the provider-neutral repository step. `None`
    /// (the default) bakes no repositories; computers on such an image clone.
    fn builder_guest(
        &self,
        _build: &Build,
    ) -> Option<std::sync::Arc<dyn repositories::BuilderGuest>> {
        None
    }
}

/// Receipts carry only opaque IDs and booleans. Provider stdout, scripts and
/// environment values have no path into the database through this check.
pub(crate) fn validate_resources(value: &Value) -> Result<(), ApiError> {
    let invalid = || ApiError::unavailable("user_image_resource_receipt_invalid");
    let fields = value.as_object().ok_or_else(invalid)?;
    for (key, value) in fields {
        let flag = matches!(
            key.as_str(),
            "builderAbsent"
                | "verifierAbsent"
                | "unpublishedArtifactAbsent"
                | "artifactAbsent"
                | "builderNotSubmitted"
                | "verifierNotSubmitted"
                | "captureNotSubmitted"
                | "pinsVerified"
                | "setupStarted"
                | "sanitized"
        );
        let text = matches!(
            key.as_str(),
            "builderId" | "verifierId" | "artifactId" | "artifactVersion" | "baseTreeSha256"
        );
        if flag {
            if !value.is_boolean() {
                return Err(invalid());
            }
        } else if text {
            if !value.as_str().is_some_and(|v| {
                !v.is_empty()
                    && v.len() <= 256
                    && v.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
            }) {
                return Err(invalid());
            }
        } else {
            return Err(invalid());
        }
    }
    Ok(())
}

/// A bounded, redacted tail of the setup output. The builder holds no secret,
/// but a script can still print something token-shaped it fetched itself.
pub fn log_excerpt(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect();
    let mut lines = Vec::new();
    for line in cleaned.lines() {
        lines.push(redact_line(line));
    }
    let mut text = lines.join("\n");
    if text.len() > LOG_EXCERPT {
        let mut start = text.len() - LOG_EXCERPT;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text = text[start..].to_owned();
    }
    text
}

fn redact_line(line: &str) -> String {
    const PREFIXES: &[&str] = &[
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "glpat-",
        "xoxb-",
        "xoxp-",
        "sk-",
        "AKIA",
        "ASIA",
        "npm_",
    ];
    const KEYS: &[&str] = &[
        "token",
        "secret",
        "password",
        "passwd",
        "authorization",
        "api_key",
        "apikey",
        "_authtoken",
    ];
    let lower = line.to_ascii_lowercase();
    if let Some(key) = KEYS.iter().find(|key| lower.contains(*key)) {
        let position = lower.find(key).unwrap_or(0);
        if let Some(offset) = line[position..].find(['=', ':']) {
            return format!("{}[redacted]", &line[..position + offset + 1]);
        }
    }
    line.split(' ')
        .map(|word| {
            let bare =
                word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-');
            if PREFIXES
                .iter()
                .any(|p| bare.starts_with(p) && bare.len() >= p.len() + 12)
                || (bare.len() >= 32
                    && bare.chars().all(|c| c.is_ascii_alphanumeric())
                    && bare.chars().any(|c| c.is_ascii_digit())
                    && bare.chars().any(|c| c.is_ascii_uppercase()))
            {
                "[redacted]".to_owned()
            } else {
                word.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod boat_tests;
#[cfg(test)]
mod prized_tests;
#[cfg(test)]
mod repositories_tests;
#[cfg(test)]
mod schedule_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod worker_tests;
