//! Release track policy and local package identity.
use std::path::Path;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use super::{Manifest, current_version, fetch_release_metadata, parse_version};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseChannel {
    Stable,
    Preview,
}

impl ReleaseChannel {
    pub const ALL: [Self; 2] = [Self::Stable, Self::Preview];

    pub fn for_version(version: &str) -> Self {
        if parse_version(version).is_some_and(|version| !version.pre.is_empty()) {
            Self::Preview
        } else {
            Self::Stable
        }
    }

    pub fn accepts(self, version: &str) -> bool {
        parse_version(version)
            .is_some_and(|version| self == Self::Preview || version.pre.is_empty())
    }
}

/// Absent preferences follow the installed build; invalid preferences fail closed.
pub fn load_channel(data_dir: &Path) -> anyhow::Result<ReleaseChannel> {
    match std::fs::read(data_dir.join("release-channel.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("reading the release channel"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ReleaseChannel::for_version(current_version()))
        }
        Err(error) => Err(error).context("reading the release channel"),
    }
}

/// Remember the installed track on first launch, so a preview installation
/// stays opted into previews even after it installs a stable promotion.
pub fn initialize_channel(data_dir: &Path) -> anyhow::Result<ReleaseChannel> {
    use std::io::Write as _;
    let channel = load_channel(data_dir)?;
    if data_dir.join("release-channel.json").exists() {
        return Ok(channel);
    }
    std::fs::create_dir_all(data_dir)?;
    let mut file = tempfile::NamedTempFile::new_in(data_dir)?;
    file.write_all(&serde_json::to_vec(&channel)?)?;
    file.as_file().sync_all()?;
    match file.persist_noclobber(data_dir.join("release-channel.json")) {
        Ok(_) => Ok(channel),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            load_channel(data_dir)
        }
        Err(error) => Err(error).context("saving the initial release channel"),
    }
}

/// Persist before changing the running updater. An unsuccessful save keeps the old track.
pub fn save_channel(data_dir: &Path, channel: ReleaseChannel) -> anyhow::Result<()> {
    use std::io::Write as _;
    std::fs::create_dir_all(data_dir)?;
    let mut file = tempfile::NamedTempFile::new_in(data_dir)?;
    file.write_all(&serde_json::to_vec(&channel)?)?;
    file.as_file().sync_all()?;
    file.persist(data_dir.join("release-channel.json"))?;
    Ok(())
}

/// Packagers can reuse compiled binaries across preview tags. This small file
/// travels beside executables, or in Resources inside a signed macOS app bundle.
pub(super) fn packaged_version(exe: &Path, compiled: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Identity {
        version: String,
    }
    let directory = exe.parent()?;
    let identity = if directory.ends_with("Contents/MacOS") {
        directory.parent()?.join("Resources/release.json")
    } else {
        directory.join("release.json")
    };
    let bytes = std::fs::read(identity).ok()?;
    if bytes.len() > 1024 {
        return None;
    }
    let identity: Identity = serde_json::from_slice(&bytes).ok()?;
    let version = parse_version(&identity.version)?;
    if identity.version != version.to_string() {
        return None;
    }
    let compiled = parse_version(compiled)?;
    ((version.major, version.minor, version.patch)
        == (compiled.major, compiled.minor, compiled.patch))
        .then_some(identity.version)
}

#[derive(Debug, Deserialize)]
pub(super) struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
}

/// Preview includes stable promotions as well as prereleases, ordered by
/// version rather than publish time. Ignore drafts, unrelated tags and releases
/// whose complete, checksummed artifacts have not been published yet.
fn newest_release(releases: &[GitHubRelease]) -> Option<&str> {
    releases
        .iter()
        .filter_map(|release| {
            let version = release.tag_name.strip_prefix('v')?;
            let parsed = parse_version(version)?;
            let is_preview = !parsed.pre.is_empty();
            if release.draft
                || release.prerelease != is_preview
                || !release
                    .assets
                    .iter()
                    .any(|asset| asset.name == "SHA256SUMS")
            {
                return None;
            }
            Some(version)
        })
        .max_by(|left, right| {
            parse_version(left)
                .unwrap()
                .cmp_precedence(&parse_version(right).unwrap())
        })
}

pub(super) async fn fetch_github_preview(client: &reqwest::Client) -> anyhow::Result<Manifest> {
    let mut releases = Vec::new();
    // Bound the work and response size. Twenty release bodies fit within the
    // metadata limit; five pages cover a hundred recent published builds.
    for page in 1..=5 {
        let url =
            format!("https://api.github.com/repos/opcodehq/zuse/releases?per_page=20&page={page}");
        let response = fetch_release_metadata(client, &url).await?;
        anyhow::ensure!(
            response.status.is_success(),
            "checking preview releases: HTTP {}",
            response.status
        );
        let batch: Vec<GitHubRelease> =
            serde_json::from_slice(&response.body).context("parsing preview releases")?;
        let last = batch.len() < 20;
        releases.extend(batch);
        if last {
            break;
        }
    }
    let Some(version) = newest_release(&releases) else {
        return Ok(Manifest {
            version: current_version().to_owned(),
            ..Default::default()
        });
    };
    let url = format!("{}/download/v{version}/SHA256SUMS", super::RELEASES_PAGE);
    let manifest = super::fetch_github_latest(client, &url).await?;
    anyhow::ensure!(
        manifest.version == version,
        "release tag and checksums disagree"
    );
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version_newer;

    #[test]
    fn previews_order_numerically_and_promote_to_stable() {
        assert!(version_newer("0.26.0-preview.10", "0.26.0-preview.9"));
        assert!(version_newer("0.26.0", "0.26.0-preview.10"));
        assert!(version_newer("0.27.0-preview.1", "0.26.0"));
        assert!(!version_newer("0.26.0-preview.1", "0.26.0"));
        assert!(!version_newer("0.26.0+build.2", "0.26.0+build.1"));
        assert!(!ReleaseChannel::Stable.accepts("0.27.0-preview.1"));
        assert!(ReleaseChannel::Preview.accepts("0.26.0"));
        assert!(!ReleaseChannel::Preview.accepts("../../release"));
    }

    #[test]
    fn preference_survives_restart_and_failed_saves() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load_channel(dir.path()).unwrap(),
            ReleaseChannel::for_version(current_version())
        );
        assert_eq!(
            initialize_channel(dir.path()).unwrap(),
            ReleaseChannel::for_version(current_version())
        );
        assert!(dir.path().join("release-channel.json").is_file());
        save_channel(dir.path(), ReleaseChannel::Preview).unwrap();
        assert_eq!(load_channel(dir.path()).unwrap(), ReleaseChannel::Preview);
        assert_eq!(
            initialize_channel(dir.path()).unwrap(),
            ReleaseChannel::Preview
        );
        save_channel(dir.path(), ReleaseChannel::Stable).unwrap();
        assert_eq!(load_channel(dir.path()).unwrap(), ReleaseChannel::Stable);
        let blocked = dir.path().join("file");
        std::fs::write(&blocked, "keep").unwrap();
        assert!(save_channel(&blocked, ReleaseChannel::Preview).is_err());
        assert_eq!(load_channel(dir.path()).unwrap(), ReleaseChannel::Stable);
        std::fs::write(dir.path().join("release-channel.json"), "invalid").unwrap();
        assert!(load_channel(dir.path()).is_err());
    }

    #[test]
    fn package_identity_keeps_the_full_version_and_rejects_foreign_versions() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("zuse");
        let identity = dir.path().join("release.json");
        std::fs::write(&identity, r#"{"version":"0.26.0-preview.12"}"#).unwrap();
        assert_eq!(
            packaged_version(&exe, "0.26.0").as_deref(),
            Some("0.26.0-preview.12")
        );
        assert_eq!(
            ReleaseChannel::for_version("0.26.0-preview.12"),
            ReleaseChannel::Preview
        );
        assert!(packaged_version(&exe, "0.27.0").is_none());
        std::fs::write(&identity, r#"{"version":"../../elsewhere"}"#).unwrap();
        assert!(packaged_version(&exe, "0.26.0").is_none());
    }

    #[test]
    fn macos_package_identity_reads_the_bundle_resource() {
        let dir = tempfile::tempdir().unwrap();
        let contents = dir.path().join("Zuse.app/Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::create_dir_all(contents.join("Resources")).unwrap();
        std::fs::write(
            contents.join("Resources/release.json"),
            r#"{"version":"0.26.1-preview.2"}"#,
        )
        .unwrap();
        for name in ["zuse", "zuse-server", "zuse-relay"] {
            let exe = contents.join("MacOS").join(name);
            assert_eq!(
                packaged_version(&exe, "0.26.1").as_deref(),
                Some("0.26.1-preview.2")
            );
            assert!(packaged_version(&exe, "0.26.2").is_none());
        }
    }

    #[test]
    fn preview_selection_ignores_drafts_unrelated_and_incomplete_releases() {
        let releases: Vec<GitHubRelease> = serde_json::from_value(serde_json::json!([
            {"tag_name":"v0.26.0-preview.9", "prerelease":true,"assets":[{"name":"SHA256SUMS"}]},
            {"tag_name":"v0.26.0-preview.10","prerelease":true,"assets":[{"name":"SHA256SUMS"}]},
            {"tag_name":"v99.0.0","draft":true,"assets":[{"name":"SHA256SUMS"}]},
            {"tag_name":"v98.0.0","assets":[]},
            {"tag_name":"control-v99.0.0","assets":[{"name":"SHA256SUMS"}]},
            {"tag_name":"v0.27.0-preview.1","assets":[{"name":"SHA256SUMS"}]}
        ]))
        .unwrap();
        assert_eq!(newest_release(&releases), Some("0.26.0-preview.10"));
        let mut releases = releases;
        releases.push(
            serde_json::from_value(
                serde_json::json!({"tag_name":"v0.26.0","assets":[{"name":"SHA256SUMS"}]}),
            )
            .unwrap(),
        );
        assert_eq!(newest_release(&releases), Some("0.26.0"));
    }
}
