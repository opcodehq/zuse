//! Offline migration. Inspect is the default; --apply requires an exclusive destination lock.
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use zuse_engine::{
    EngineCore, EngineProfile, HarnessRegistry, desktop_import::DesktopImport,
    instance_lock::InstanceLock,
};
use zuse_protocol::HarnessId;

fn argument(name: &str) -> Option<PathBuf> {
    let args = std::env::args().collect::<Vec<_>>();
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| PathBuf::from(&pair[1]))
}
fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o700))?;
    }
    for item in std::fs::read_dir(source)? {
        let item = item?;
        let name = item.file_name();
        if name == "engine.lock" {
            continue;
        }
        let destination = target.join(&name);
        if item.file_type()?.is_dir() {
            copy_tree(&item.path(), &destination)?;
        } else if item.file_type()?.is_file() {
            std::fs::copy(item.path(), &destination)?;
        } else if item.file_type()?.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(item.path())?, &destination)?;
            #[cfg(not(unix))]
            anyhow::bail!("Symlink backups require a Unix host");
        } else {
            anyhow::bail!(
                "A destination profile backup contains a symlink; inspect it before migration"
            );
        }
    }
    Ok(())
}
#[tokio::main]
async fn main() -> Result<()> {
    let source = argument("--source").context("--source ELECTRON_PROFILE is required")?;
    let snapshot = argument("--snapshot").context("--snapshot SQLITE_BACKUP is required")?;
    let importer = DesktopImport::open(&source, &snapshot)?;
    if !std::env::args().any(|arg| arg == "--apply") {
        println!("{}", serde_json::to_string_pretty(&importer.inspect()?)?);
        return Ok(());
    }
    let target = argument("--target").context("--target GPUI_DATA_DIRECTORY is required")?;
    let backup = argument("--backup").context("--backup NEW_BACKUP_DIRECTORY is required")?;
    let target = target.canonicalize()?;
    let backup = backup
        .parent()
        .context("Backup has no parent")?
        .canonicalize()?
        .join(backup.file_name().context("Backup has no name")?);
    ensure!(
        !backup.exists(),
        "The GPUI backup directory already exists; choose a new directory"
    );
    ensure!(
        !backup.starts_with(&target),
        "The backup must be outside the GPUI profile"
    );
    let lock = InstanceLock::acquire(&target)
        .context("Quit GPUI before importing; its data must not have two writers")?;
    copy_tree(&target, &backup)
        .context("Could not finish the GPUI backup; import has not started")?;
    let organization = argument("--organization")
        .context("--organization must explicitly identify the destination workspace")?;
    let user =
        argument("--user").context("--user must explicitly identify the destination workspace")?;
    let organization = organization.to_str().context("Invalid organization")?;
    let user = user.to_str().context("Invalid user")?;
    let profile = EngineProfile::development(&target, organization, user);
    ensure!(
        profile.store_root().exists(),
        "The selected GPUI workspace does not exist; inspect the destination first"
    );
    let profile_root = profile.store_root().to_path_buf();
    // An empty registry and no edge connection make execution impossible during import.
    let core = EngineCore::assemble_with_profile_locked(
        profile,
        Arc::new(HarnessRegistry::new()),
        HarnessId::ClaudeCode,
        None,
        lock,
    )?;
    let report = importer.apply(&core, &target, &profile_root)?;
    core.doc_host.flush_all();
    core.shutdown().await;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
