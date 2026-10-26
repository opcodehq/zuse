//! Private content-addressed storage for immutable journal payloads. References
//! have a typed envelope, so user JSON cannot be mistaken for storage metadata.
use crate::{Error, journal::atomic_json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::{Path, PathBuf},
};
const INLINE_BYTES: usize = 32 * 1024;
const MAX_ARTIFACT: u64 = 128 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(tag = "storage", content = "value", rename_all = "kebab-case")]
pub(crate) enum ArchivedValue {
    Inline(Value),
    Object(BTreeMap<String, ArchivedValue>),
    Array(Vec<ArchivedValue>),
    Artifact(Reference),
}
#[derive(Serialize, Deserialize)]
pub(crate) struct Reference {
    sha256: String,
    bytes: u64,
}
pub(crate) struct Artifacts {
    directory: PathBuf,
}
struct HashWriter {
    hash: Sha256,
    bytes: u64,
}
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.hash.update(bytes);
        self.bytes += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Artifacts {
    pub fn new(root: &Path) -> Self {
        Self {
            directory: root.join("payloads"),
        }
    }
    fn contains_large(value: &Value) -> bool {
        match value {
            Value::String(s) => s.len() >= INLINE_BYTES,
            Value::Array(a) => a.iter().any(Self::contains_large),
            Value::Object(o) => o.values().any(Self::contains_large),
            _ => false,
        }
    }
    pub fn archive(&self, value: Value) -> Result<ArchivedValue, Error> {
        if Self::contains_large(&value) {
            match value {
                Value::String(_) => self.put(value),
                Value::Array(values) => Ok(ArchivedValue::Array(
                    values
                        .into_iter()
                        .map(|v| self.archive(v))
                        .collect::<Result<_, _>>()?,
                )),
                Value::Object(values) => Ok(ArchivedValue::Object(
                    values
                        .into_iter()
                        .map(|(k, v)| Ok((k, self.archive(v)?)))
                        .collect::<Result<_, Error>>()?,
                )),
                _ => unreachable!(),
            }
        } else {
            let mut count = HashWriter {
                hash: Sha256::new(),
                bytes: 0,
            };
            serde_json::to_writer(&mut count, &value).map_err(|_| Error::Storage)?;
            if count.bytes >= INLINE_BYTES as u64 {
                self.put_hashed(value, count)
            } else {
                Ok(ArchivedValue::Inline(value))
            }
        }
    }
    fn put(&self, value: Value) -> Result<ArchivedValue, Error> {
        let mut count = HashWriter {
            hash: Sha256::new(),
            bytes: 0,
        };
        serde_json::to_writer(&mut count, &value).map_err(|_| Error::Storage)?;
        self.put_hashed(value, count)
    }
    fn put_hashed(&self, value: Value, count: HashWriter) -> Result<ArchivedValue, Error> {
        if count.bytes > MAX_ARTIFACT {
            return Err(Error::Storage);
        }
        let reference = Reference {
            sha256: format!("{:x}", count.hash.finalize()),
            bytes: count.bytes,
        };
        let path = self.path(&reference)?;
        if path.exists() {
            self.verify(&reference)?;
        } else {
            atomic_json(&path, &value)?;
        }
        Ok(ArchivedValue::Artifact(reference))
    }
    fn path(&self, reference: &Reference) -> Result<PathBuf, Error> {
        if reference.sha256.len() != 64
            || !reference
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || reference.bytes > MAX_ARTIFACT
        {
            return Err(Error::InvalidCheckpoint);
        }
        Ok(self.directory.join(format!("{}.json", reference.sha256)))
    }
    fn verify(&self, reference: &Reference) -> Result<std::fs::File, Error> {
        let path = self.path(reference)?;
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| Error::InvalidCheckpoint)?;
        if !metadata.is_file() || metadata.len() != reference.bytes {
            return Err(Error::InvalidCheckpoint);
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&path).map_err(|_| Error::InvalidCheckpoint)?;
        let mut hash = Sha256::new();
        let mut bytes = [0; 16384];
        loop {
            let count = file
                .read(&mut bytes)
                .map_err(|_| Error::InvalidCheckpoint)?;
            if count == 0 {
                break;
            }
            hash.update(&bytes[..count]);
        }
        if format!("{:x}", hash.finalize()) != reference.sha256 {
            return Err(Error::InvalidCheckpoint);
        }
        use std::io::Seek;
        file.rewind().map_err(|_| Error::Storage)?;
        Ok(file)
    }
    pub fn restore(&self, value: ArchivedValue) -> Result<Value, Error> {
        match value {
            ArchivedValue::Inline(value) => Ok(value),
            ArchivedValue::Object(values) => Ok(Value::Object(
                values
                    .into_iter()
                    .map(|(k, v)| Ok((k, self.restore(v)?)))
                    .collect::<Result<_, Error>>()?,
            )),
            ArchivedValue::Array(values) => Ok(Value::Array(
                values
                    .into_iter()
                    .map(|v| self.restore(v))
                    .collect::<Result<_, _>>()?,
            )),
            ArchivedValue::Artifact(reference) => {
                serde_json::from_reader(std::io::BufReader::new(self.verify(&reference)?))
                    .map_err(|_| Error::InvalidCheckpoint)
            }
        }
    }
    /// Called only after the replacement snapshot is durable. Live state and
    /// recovery receipts are both represented in that complete snapshot.
    pub fn reclaim(&self, live: &ArchivedValue) -> Result<(), Error> {
        let mut retained = BTreeSet::new();
        live.references(&mut retained);
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(Error::Storage),
        };
        for entry in entries {
            let entry = entry.map_err(|_| Error::Storage)?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(hash) = name.strip_suffix(".json") else {
                continue;
            };
            if hash.len() == 64
                && hash.bytes().all(|b| b.is_ascii_hexdigit())
                && !retained.contains(hash)
            {
                std::fs::remove_file(entry.path()).map_err(|_| Error::Storage)?;
            }
        }
        #[cfg(unix)]
        std::fs::File::open(&self.directory)
            .and_then(|f| f.sync_all())
            .map_err(|_| Error::Storage)?;
        Ok(())
    }
}
impl ArchivedValue {
    fn references(&self, retained: &mut BTreeSet<String>) {
        match self {
            Self::Artifact(r) => {
                retained.insert(r.sha256.clone());
            }
            Self::Object(o) => {
                for v in o.values() {
                    v.references(retained)
                }
            }
            Self::Array(a) => {
                for v in a {
                    v.references(retained)
                }
            }
            Self::Inline(_) => {}
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn payloads_deduplicate_round_trip_and_reject_tampering() {
        let directory = tempfile::tempdir().unwrap();
        let artifacts = Artifacts::new(directory.path());
        let large = "image".repeat(20000);
        let value = json!({"a":large,"nested":[large,{"storage":"artifact","value":{"sha256":"user-data"}}]});
        let archived = artifacts.archive(value.clone()).unwrap();
        assert_eq!(
            std::fs::read_dir(directory.path().join("payloads"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(artifacts.restore(archived).unwrap(), value);
        let archived = artifacts.archive(value).unwrap();
        let path = std::fs::read_dir(directory.path().join("payloads"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[1] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            artifacts.restore(archived),
            Err(Error::InvalidCheckpoint)
        ));
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn reclamation_retains_only_the_published_snapshot_payloads() {
        let directory = tempfile::tempdir().unwrap();
        let artifacts = Artifacts::new(directory.path());
        let old = artifacts.archive(json!("old".repeat(20000))).unwrap();
        let live = artifacts.archive(json!("new".repeat(20000))).unwrap();
        assert_eq!(
            std::fs::read_dir(directory.path().join("payloads"))
                .unwrap()
                .count(),
            2
        );
        artifacts.reclaim(&live).unwrap();
        assert!(artifacts.restore(old).is_err());
        assert_eq!(artifacts.restore(live).unwrap(), json!("new".repeat(20000)));
    }
}
