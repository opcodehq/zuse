//! User images: the repository-less cache kind. One image line per account
//! (personal or organization) and provider, built from the account's setup
//! script on a verified managed base. Like a project cache it keeps the base's
//! pinned runtime files byte-identical, is checked by a second boot of the
//! captured snapshot, and never replaces a ready image when a build fails.
//! Unlike a project cache it holds no repository, commit or source bundle, so
//! it is rebuilt only when the setup script or the base changes.
//!
//! Instructions, skills and dotfiles are delivered per chat and are not part
//! of an image; only the setup script is.
use crate::error::ApiError;
use crate::managed_bases::bootstrap::InitialRuntimeBootstrap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use zuse_protocol::cloud_computers::{MachineRecipe, ProviderId};

pub mod boxd;
pub mod worker;

pub const MIGRATION: &str = include_str!("../../../migrations/0036_user_images.sql");
/// Guest helper run as root on builder and verifier VMs.
pub const GUEST_HELPER: &str = include_str!("../../../../../infra/cloud/user-image-guest.py");
/// Providers with a user image builder. Others start from the managed base.
pub const PROVIDERS: &[ProviderId] = &[ProviderId::Boxd];
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

fn hash(value: &str) -> bool {
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

/// What makes two images interchangeable. No repository, commit, credential
/// or agent connection is an input.
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
    pub initial_runtime_bootstrap: InitialRuntimeBootstrap,
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
        {
            return Err(ApiError::bad_request("user_image_plan_invalid"));
        }
        let bootstrap = &self.initial_runtime_bootstrap;
        if !bootstrap.valid(self.identity.provider)
            || bootstrap.runtime_sha256 != self.identity.base_runtime_sha256
        {
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
pub const PHASES: &[&str] = &[
    "allocate",
    "setup",
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
            "builderId" | "verifierId" | "artifactId" | "artifactVersion"
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
mod tests;
#[cfg(test)]
mod worker_tests;
