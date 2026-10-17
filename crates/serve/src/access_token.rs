//! Stable local browser access without placing credentials in client storage.
use super::BrowserServer;
use std::{io::Write, path::Path};
pub(super) fn load_or_create(directory: &Path) -> anyhow::Result<String> {
    std::fs::create_dir_all(directory)?;
    let target = directory.join("browser-access-token");
    fn read(path: &Path) -> anyhow::Result<String> {
        anyhow::ensure!(
            std::fs::symlink_metadata(path)?.file_type().is_file(),
            "Browser access token must be a regular file"
        );
        let token = std::fs::read_to_string(path)?;
        anyhow::ensure!(
            token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid saved browser access token"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(token)
    }
    if target.try_exists()? {
        return read(&target);
    }
    let temporary = directory.join(format!(".browser-token-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let token = BrowserServer::random_token();
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(token.as_bytes())?;
        file.sync_all()?;
        match std::fs::hard_link(&temporary, &target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        };
        let _ = std::fs::File::open(directory).and_then(|directory| directory.sync_all());
        read(&target)
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_and_concurrent_startups_share_one_private_token() {
        let root = tempfile::tempdir().unwrap();
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let root = root.path().to_owned();
                std::thread::spawn(move || load_or_create(&root).unwrap())
            })
            .collect();
        let tokens: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert!(tokens.iter().all(|token| token == &tokens[0]));
        assert_eq!(load_or_create(root.path()).unwrap(), tokens[0]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(root.path().join("browser-access-token"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
