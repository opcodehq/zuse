//! Application storage paths. Provider credentials keep their own locations.

use std::ffi::OsString;
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    resolve_data_dir(|name| std::env::var_os(name)).expect(MISSING)
}

/// The default profile, or None when the environment names no home.
pub fn default_data_dir() -> Option<PathBuf> {
    resolve_data_dir(|name| std::env::var_os(name))
}

#[cfg(windows)]
const MISSING: &str = "LOCALAPPDATA and USERPROFILE not set; set ZUSE_DATA_DIR";
#[cfg(not(windows))]
const MISSING: &str = "HOME not set";

fn resolve_data_dir(mut env: impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(dir) = env("ZUSE_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    #[cfg(windows)]
    {
        // Explorer does not set HOME. Do not let a shell-specific HOME select
        // a different workspace from a desktop launch, or migrate credentials
        // between Unix-style and native Windows directories implicitly.
        let local = env("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                env("USERPROFILE")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join("AppData").join("Local"))
            })?;
        Some(local.join("Zuse"))
    }
    #[cfg(not(windows))]
    {
        Some(PathBuf::from(env("HOME")?).join(".zuse-native"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(vars: &[(&str, &str)]) -> PathBuf {
        resolve_data_dir(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.into())
        })
        .unwrap()
    }

    #[test]
    fn explicit_data_dir_needs_no_home() {
        assert_eq!(
            resolve(&[("ZUSE_DATA_DIR", "custom data")]),
            PathBuf::from("custom data")
        );
    }

    #[test]
    fn a_missing_home_is_reported_not_guessed() {
        assert_eq!(resolve_data_dir(|_| None), None);
    }

    #[cfg(windows)]
    #[test]
    fn explorer_launch_without_home_uses_local_app_data() {
        assert_eq!(
            resolve(&[("LOCALAPPDATA", r"C:\Users\Test User\AppData\Local")]),
            PathBuf::from(r"C:\Users\Test User\AppData\Local\Zuse"),
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_profile_fallback_handles_unicode_and_apostrophes() {
        assert_eq!(
            resolve(&[("USERPROFILE", r"C:\Users\O'Brien 日本語")]),
            PathBuf::from(r"C:\Users\O'Brien 日本語\AppData\Local\Zuse"),
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_default_does_not_depend_on_shell_home() {
        assert_eq!(
            resolve(&[("HOME", r"D:\msys-home"), ("LOCALAPPDATA", r"C:\Local")]),
            PathBuf::from(r"C:\Local\Zuse"),
        );
    }
}
