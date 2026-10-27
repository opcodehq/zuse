//! Small profile-owned scheduling policy, independent of models and credentials.
use crate::{Error, Options};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Scheduling {
    pub parallel_reads: usize,
    pub children: usize,
}
impl Default for Scheduling {
    fn default() -> Self {
        Self {
            parallel_reads: 4,
            children: 3,
        }
    }
}
impl Scheduling {
    pub fn load(profile: &Path) -> Result<Self, Error> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let settings = match options.open(profile.join("harness/runtime.json")) {
            Ok(file) => {
                if !file.metadata().map_err(|_| Error::Storage)?.is_file() {
                    return Err(Error::Tool(
                        "Runtime settings must be a regular file".into(),
                    ));
                }
                let mut data = vec![];
                file.take(4097)
                    .read_to_end(&mut data)
                    .map_err(|_| Error::Storage)?;
                if data.len() > 4096 {
                    return Err(Error::Tool("Runtime settings exceed 4 KiB".into()));
                }
                serde_json::from_slice(&data)
                    .map_err(|_| Error::Tool("Invalid runtime settings".into()))?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(_) => return Err(Error::Storage),
        };
        let mut options = Options::new("validation");
        settings.apply(&mut options)?;
        Ok(settings)
    }
    pub fn apply(&self, options: &mut Options) -> Result<(), Error> {
        if !(1..=16).contains(&self.parallel_reads) || !(1..=32).contains(&self.children) {
            return Err(Error::Tool(
                "parallelReads must be 1..16 and children 1..32".into(),
            ));
        }
        options.max_parallel_reads = self.parallel_reads;
        options.max_children = self.children;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_limits_are_explicit_and_strict() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(Scheduling::load(root.path()).unwrap().parallel_reads, 4);
        std::fs::create_dir(root.path().join("harness")).unwrap();
        let path = root.path().join("harness/runtime.json");
        for value in [
            r#"{"parallelReads":0}"#,
            r#"{"children":33}"#,
            r#"{"children":-1}"#,
            r#"{"unknown":true}"#,
        ] {
            std::fs::write(&path, value).unwrap();
            assert!(Scheduling::load(root.path()).is_err());
        }
        std::fs::write(&path, r#"{"parallelReads":2,"children":6}"#).unwrap();
        let settings = Scheduling::load(root.path()).unwrap();
        let mut options = Options::new("fixture");
        settings.apply(&mut options).unwrap();
        assert_eq!((options.max_parallel_reads, options.max_children), (2, 6));
    }
}
