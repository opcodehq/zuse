//! Shared native browser assets. Platform callbacks never choose output paths.
use std::path::PathBuf;

pub(super) fn save_screenshot(bytes: Vec<u8>) -> Result<(PathBuf, Vec<u8>), String> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("The browser did not return a PNG image".into());
    }
    let directory = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .ok_or("Could not locate the home directory")?
        .join("Downloads")
        .join("Zuse Screenshots");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(format!("screenshot-{}.png", uuid::Uuid::new_v4()));
    std::fs::write(&path, &bytes).map_err(|error| error.to_string())?;
    Ok((path, bytes))
}
