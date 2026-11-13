//! The account's machine, applied at computer start without an image:
//! user-level instructions, skills and home files from the bootstrap's
//! `machineDelivery` (see [`zuse_protocol::cloud_computers::MachineDelivery`]).
//!
//! - Instructions go where each agent reads user-level instructions:
//!   `$CLAUDE_CONFIG_DIR/CLAUDE.md` (default `~/.claude/CLAUDE.md`) and
//!   `$CODEX_HOME/AGENTS.md` (default `~/.codex/AGENTS.md`). The repository's
//!   own `AGENTS.md`/`CLAUDE.md` is never touched: nothing is written inside
//!   the checkout.
//! - Skills go to `<claude config>/skills/<name>/SKILL.md` and the shared
//!   `~/.agents/skills/<name>/SKILL.md`, which the other agents read.
//! - Files go under the home directory. The recipe already refuses
//!   credential and runtime paths; this module repeats that check.
//!
//! Every file is written atomically (temporary file, fsync, rename) with
//! owner-only permissions, and never through a symbolic link. A small receipt
//! in the runtime data directory remembers what was written, so a restart
//! leaves an agent's later edits alone and a newer delivery removes entries
//! the account deleted (only when they are still exactly as written).
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};
use zuse_protocol::cloud_computers::MachineDelivery;

/// The bootstrap field the server fills for runtimes advertising
/// [`zuse_protocol::cloud_computers::MACHINE_DELIVERY_CAPABILITY`].
pub const FIELD: &str = "machineDelivery";
const RECEIPT: &str = "machine-delivery.json";

/// Where the agents on this computer look for user-level configuration.
#[derive(Clone, Debug)]
pub struct Layout {
    pub home: PathBuf,
    pub claude: PathBuf,
    pub codex: PathBuf,
    /// Never written into, even when it lives under the home directory.
    pub checkout: PathBuf,
}

impl Layout {
    /// The agents inherit this process's environment, so resolve the same
    /// variables they do. `None` without a home directory.
    pub fn from_env(checkout: &Path) -> Option<Self> {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        let home = var("HOME")?;
        Some(Self {
            claude: var("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
            codex: var("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
            home,
            checkout: checkout.to_path_buf(),
        })
    }

    /// Every destination for `delivery`, later entries winning: home files
    /// first, so instructions and skills are never replaced by one of them.
    fn targets(&self, delivery: &MachineDelivery) -> BTreeMap<PathBuf, String> {
        let mut targets = BTreeMap::new();
        for file in &delivery.files {
            targets.insert(
                self.home.join(file.path.trim_start_matches("./")),
                file.content.clone(),
            );
        }
        if !delivery.instructions.trim().is_empty() {
            targets.insert(self.claude.join("CLAUDE.md"), delivery.instructions.clone());
            targets.insert(self.codex.join("AGENTS.md"), delivery.instructions.clone());
        }
        for skill in &delivery.skills {
            for root in [self.claude.join("skills"), self.home.join(".agents/skills")] {
                targets.insert(
                    root.join(&skill.name).join("SKILL.md"),
                    skill.content.clone(),
                );
            }
        }
        targets
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Receipt {
    revision: i64,
    /// Path written → SHA-256 of the content written there.
    written: BTreeMap<PathBuf, String>,
}

/// What [`apply`] did, for logs and tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub written: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    /// Destinations refused (inside the checkout, through a link, or not a
    /// regular file). The rest of the delivery still applies.
    pub skipped: Vec<PathBuf>,
}

/// Read the bootstrap field. A missing field means nothing to apply; an
/// unreadable one is an error the caller logs (it must not stop the task).
pub fn from_bootstrap(
    extra: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<MachineDelivery>> {
    let Some(value) = extra.get(FIELD).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let delivery: MachineDelivery = serde_json::from_value(value.clone())
        .map_err(|_| anyhow::anyhow!("machine_delivery_invalid"))?;
    delivery.validate().map_err(anyhow::Error::msg)?;
    Ok(Some(delivery))
}

/// Apply `delivery` (or retire a previous one when `None`). Idempotent:
/// entries already written with the same content are left alone, even if an
/// agent has since edited them.
pub fn apply(data: &Path, layout: &Layout, delivery: Option<&MachineDelivery>) -> Result<Applied> {
    if let Some(delivery) = delivery {
        delivery.validate().map_err(anyhow::Error::msg)?;
    }
    ensure!(
        layout.home.is_absolute() && layout.claude.is_absolute() && layout.codex.is_absolute(),
        "machine_delivery_home_invalid"
    );
    let receipt_path = data.join(RECEIPT);
    let previous: Receipt = match std::fs::read(&receipt_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Receipt::default(),
        Err(error) => return Err(error.into()),
    };
    let targets = delivery.map(|d| layout.targets(d)).unwrap_or_default();
    let mut applied = Applied::default();
    let mut written = BTreeMap::new();

    for (path, digest) in &previous.written {
        if targets.contains_key(path) {
            continue;
        }
        // Only remove what is still exactly what this module wrote.
        if safe_destination(layout, path)
            && std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
            && std::fs::read(path).is_ok_and(|bytes| &sha256(&bytes) == digest)
            && std::fs::remove_file(path).is_ok()
        {
            applied.removed.push(path.clone());
        }
    }

    for (path, content) in &targets {
        let digest = sha256(content.as_bytes());
        if previous.written.get(path) == Some(&digest) {
            written.insert(path.clone(), digest);
            continue;
        }
        if !safe_destination(layout, path) {
            applied.skipped.push(path.clone());
            continue;
        }
        match write_private(path, content.as_bytes()) {
            Ok(()) => {
                written.insert(path.clone(), digest);
                applied.written.push(path.clone());
            }
            Err(_) => applied.skipped.push(path.clone()),
        }
    }

    let receipt = Receipt {
        revision: delivery.map_or(0, |d| d.revision),
        written,
    };
    crate::identity::atomic_write(&receipt_path, &serde_json::to_vec(&receipt)?)?;
    Ok(applied)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Under one of the agent roots, outside the checkout, and reachable
/// without following a link below that root.
fn safe_destination(layout: &Layout, path: &Path) -> bool {
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    let inside_checkout =
        |checkout: &Path| path.starts_with(checkout) || checkout.starts_with(path);
    if inside_checkout(&layout.checkout)
        || std::fs::canonicalize(&layout.checkout).is_ok_and(|c| inside_checkout(&c))
    {
        return false;
    }
    let Some(root) = [&layout.claude, &layout.codex, &layout.home]
        .into_iter()
        .find(|root| path.starts_with(root))
    else {
        return false;
    };
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => return false,
            Ok(meta) if current != path && !meta.is_dir() => return false,
            Ok(meta) if current == path && !meta.is_file() => return false,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return false,
        }
    }
    // Resolved through any link above the root, the target must still not
    // land in the checkout.
    let resolved = std::fs::canonicalize(root)
        .map(|r| r.join(relative))
        .unwrap_or_else(|_| path.to_path_buf());
    !(resolved.starts_with(&layout.checkout)
        || std::fs::canonicalize(&layout.checkout).is_ok_and(|c| resolved.starts_with(c)))
}

/// Create missing parents owner-only, then replace the file atomically.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("machine_delivery_path_invalid"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(parent)?;
    crate::identity::atomic_write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zuse_protocol::cloud_computers::{MachineFile, MachineSkill};

    fn fixture() -> (tempfile::TempDir, PathBuf, Layout) {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let home = root.path().join("home");
        let checkout = home.join("workspace");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::write(checkout.join("AGENTS.md"), "repository rules").unwrap();
        let layout = Layout {
            claude: home.join(".claude"),
            codex: home.join(".codex"),
            home,
            checkout,
        };
        (root, data, layout)
    }

    fn delivery(revision: i64) -> MachineDelivery {
        MachineDelivery {
            revision,
            instructions: "Keep commits small.".into(),
            skills: vec![MachineSkill {
                name: "release-notes".into(),
                content: "# Release notes".into(),
            }],
            files: vec![MachineFile {
                path: ".gitconfig".into(),
                content: "[user]\n\tname = Zuse\n".into(),
            }],
        }
    }

    fn read(path: impl AsRef<Path>) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn writes_user_level_instructions_skills_and_files_for_every_agent() {
        let (_root, data, layout) = fixture();
        let applied = apply(&data, &layout, Some(&delivery(1))).unwrap();
        let home = &layout.home;
        assert_eq!(read(home.join(".claude/CLAUDE.md")), "Keep commits small.");
        assert_eq!(read(home.join(".codex/AGENTS.md")), "Keep commits small.");
        assert_eq!(
            read(home.join(".claude/skills/release-notes/SKILL.md")),
            "# Release notes"
        );
        assert_eq!(
            read(home.join(".agents/skills/release-notes/SKILL.md")),
            "# Release notes"
        );
        assert_eq!(read(home.join(".gitconfig")), "[user]\n\tname = Zuse\n");
        assert_eq!(applied.written.len(), 5);
        assert!(applied.skipped.is_empty());
        assert_eq!(
            read(layout.checkout.join("AGENTS.md")),
            "repository rules",
            "the repository's own instructions stay as they are"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(home.join(".claude/CLAUDE.md")), 0o600);
            assert_eq!(mode(home.join(".claude/skills/release-notes")), 0o700);
            assert_eq!(mode(home.join(".gitconfig")), 0o600);
        }
        // No temporary files are left behind.
        assert_eq!(
            std::fs::read_dir(home.join(".codex")).unwrap().count(),
            1,
            "only AGENTS.md"
        );
    }

    #[test]
    fn honours_agent_config_directories_from_the_environment() {
        let (root, data, mut layout) = fixture();
        layout.claude = root.path().join("claude-profile");
        layout.codex = root.path().join("codex-profile");
        apply(&data, &layout, Some(&delivery(1))).unwrap();
        assert!(root.path().join("claude-profile/CLAUDE.md").is_file());
        assert!(
            root.path()
                .join("claude-profile/skills/release-notes/SKILL.md")
                .is_file()
        );
        assert!(root.path().join("codex-profile/AGENTS.md").is_file());
    }

    #[test]
    fn never_writes_into_the_checkout_or_through_links() {
        let (root, data, layout) = fixture();
        let mut d = delivery(1);
        d.files = vec![
            MachineFile {
                path: "workspace/AGENTS.md".into(),
                content: "overwritten".into(),
            },
            MachineFile {
                path: "linked/config".into(),
                content: "escaped".into(),
            },
            MachineFile {
                path: ".npmrc".into(),
                content: "registry=https://registry.npmjs.org/\n".into(),
            },
        ];
        let outside = root.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, layout.home.join("linked")).unwrap();
        let applied = apply(&data, &layout, Some(&d)).unwrap();
        assert_eq!(read(layout.checkout.join("AGENTS.md")), "repository rules");
        assert!(!outside.join("config").exists());
        assert!(
            applied
                .skipped
                .contains(&layout.home.join("workspace/AGENTS.md"))
        );
        assert!(
            layout.home.join(".npmrc").is_file(),
            "the rest still applies"
        );
    }

    #[test]
    fn rejects_credential_paths_even_if_the_server_sent_them() {
        let (_root, data, layout) = fixture();
        let mut d = delivery(1);
        d.files = vec![MachineFile {
            path: ".claude/.credentials.json".into(),
            content: "{}".into(),
        }];
        assert_eq!(
            apply(&data, &layout, Some(&d)).unwrap_err().to_string(),
            "invalid_machine_file"
        );
        assert!(!layout.home.join(".claude").exists());
    }

    #[test]
    fn restarts_keep_agent_edits_and_new_versions_retire_removed_entries() {
        let (_root, data, layout) = fixture();
        apply(&data, &layout, Some(&delivery(1))).unwrap();
        let instructions = layout.home.join(".claude/CLAUDE.md");
        std::fs::write(&instructions, "edited by the agent").unwrap();
        let again = apply(&data, &layout, Some(&delivery(1))).unwrap();
        assert!(again.written.is_empty(), "same delivery is a no-op");
        assert_eq!(read(&instructions), "edited by the agent");

        let mut next = delivery(2);
        next.skills.clear();
        next.instructions = "Run the tests you touched.".into();
        let applied = apply(&data, &layout, Some(&next)).unwrap();
        assert_eq!(read(&instructions), "Run the tests you touched.");
        assert!(
            !layout
                .home
                .join(".claude/skills/release-notes/SKILL.md")
                .exists()
        );
        assert_eq!(applied.removed.len(), 2);

        // An emptied recipe removes what it wrote but leaves edited files.
        std::fs::write(layout.home.join(".gitconfig"), "[user]\n\tname = Edited\n").unwrap();
        apply(&data, &layout, None).unwrap();
        assert!(!instructions.exists());
        assert!(!layout.home.join(".codex/AGENTS.md").exists());
        assert_eq!(
            read(layout.home.join(".gitconfig")),
            "[user]\n\tname = Edited\n"
        );
    }

    #[test]
    fn bootstrap_field_is_optional_and_validated() {
        let mut extra = serde_json::Map::new();
        assert!(from_bootstrap(&extra).unwrap().is_none());
        extra.insert(FIELD.into(), serde_json::to_value(delivery(4)).unwrap());
        assert_eq!(from_bootstrap(&extra).unwrap().unwrap().revision, 4);
        extra.insert(
            FIELD.into(),
            serde_json::json!({"revision":4,"files":[{"path":"../etc/passwd","content":""}]}),
        );
        assert!(from_bootstrap(&extra).is_err());
    }
}
