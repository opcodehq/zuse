//! Explicit opt-in for clean managed images whose resident starts under API ownership.
//! These are operator pins, never inferred from runtime support or user settings.
use crate::{error::ApiError, providers::ProviderError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zuse_protocol::cloud_computers::ProviderId;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImagePins {
    pub runtime_sha256: String,
    pub supervisor_sha256: String,
    pub service_sha256: String,
    /// SHA of the actual installed file, not normalized JSON from the registry.
    pub manifest_sha256: String,
    pub helper_sha256: String,
}
fn hash(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl ImagePins {
    pub fn valid(&self) -> bool {
        [
            &self.runtime_sha256,
            &self.supervisor_sha256,
            &self.service_sha256,
            &self.manifest_sha256,
            &self.helper_sha256,
        ]
        .into_iter()
        .all(|s| hash(s))
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitialRuntimeBootstrap {
    pub version: u32,
    pub mode: String,
    pub runtime_sha256: String,
    pub supervisor_sha256: String,
    pub service_sha256: String,
    pub manifest_sha256: String,
    pub helper_sha256: String,
}
impl InitialRuntimeBootstrap {
    pub fn pins(&self) -> ImagePins {
        ImagePins {
            runtime_sha256: self.runtime_sha256.clone(),
            supervisor_sha256: self.supervisor_sha256.clone(),
            service_sha256: self.service_sha256.clone(),
            manifest_sha256: self.manifest_sha256.clone(),
            helper_sha256: self.helper_sha256.clone(),
        }
    }
    pub fn valid(&self, provider: ProviderId) -> bool {
        matches!(
            (provider, self.mode.as_str()),
            (ProviderId::Boxd, "stopped-memory-snapshot-v1")
                | (ProviderId::Boat, "post-hydration-first-start-v1")
        ) && self.version == 1
            && self.pins().valid()
    }
    pub fn validate(
        &self,
        provider: ProviderId,
        manifest: &zuse_cloud_runtime::image::ImageManifest,
    ) -> Result<(), ApiError> {
        if !self.valid(provider)
            || manifest.rust_runtime_supervisor_version != Some(1)
            || manifest
                .command_admission
                .as_ref()
                .is_some_and(|v| v.runtime_sha256 != self.runtime_sha256)
        {
            return Err(ApiError::bad_request(
                "managed_base_initial_bootstrap_invalid",
            ));
        }
        Ok(())
    }
    pub fn from_config(config: &Value) -> Result<Option<Self>, ApiError> {
        config
            .get("initialRuntimeBootstrap")
            .filter(|v| !v.is_null())
            .map(|v| {
                serde_json::from_value(v.clone())
                    .map_err(|_| ApiError::conflict("cloud_initial_runtime_bootstrap_invalid"))
            })
            .transpose()
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Purpose {
    InitialAllocation,
    InitialRestore,
    PublicationProof,
    CachePreparation,
    RetainedResume,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub provider: ProviderId,
    pub purpose: Purpose,
    pub operation_id: String,
    pub workspace_id: String,
    pub generation: i64,
    pub storage_incarnation_id: String,
    pub source_machine_id: String,
    pub image_pins: ImagePins,
}
fn identifier(v: &str, max: usize) -> bool {
    !v.is_empty()
        && v.len() <= max
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-:".contains(&b))
}
impl Request {
    pub fn valid(&self) -> bool {
        self.version == 1
            && matches!(self.provider, ProviderId::Boxd | ProviderId::Boat)
            && (!matches!(self.purpose, Purpose::RetainedResume)
                || self.provider == ProviderId::Boat)
            && self.generation > 0
            && identifier(&self.operation_id, 160)
            && identifier(&self.workspace_id, 180)
            && identifier(&self.storage_incarnation_id, 180)
            && identifier(&self.source_machine_id, 180)
            && self.image_pins.valid()
    }
    pub fn digest(&self) -> Result<String, ProviderError> {
        if !self.valid() {
            return Err(ProviderError::Rejected);
        }
        fn sorted(value: Value) -> Value {
            match value {
                Value::Object(map) => Value::Object(
                    map.into_iter()
                        .map(|(k, v)| (k, sorted(v)))
                        .collect::<std::collections::BTreeMap<_, _>>()
                        .into_iter()
                        .collect(),
                ),
                Value::Array(a) => Value::Array(a.into_iter().map(sorted).collect()),
                v => v,
            }
        }
        Ok(format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&sorted(
                    serde_json::to_value(self).map_err(|_| ProviderError::Rejected)?
                ))
                .map_err(|_| ProviderError::Rejected)?
            )
        ))
    }
    pub fn command(&self) -> Result<String, ProviderError> {
        use base64::Engine;
        self.digest()?;
        let body = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(self).map_err(|_| ProviderError::Rejected)?);
        let loader = include_str!("../../../../infra/cloud/managed-base-fresh-boot-loader.py");
        let quote = |v: &str| format!("'{}'", v.replace('\'', "'\\''"));
        Ok(format!(
            "sudo -n /usr/bin/python3 -I -B -c {} {} {}",
            quote(loader),
            quote(&self.image_pins.helper_sha256),
            quote(&body)
        ))
    }
    /// None is a bounded pending observation, never authorization to upload a grant.
    pub fn response(&self, value: &Value) -> Result<Option<Value>, ProviderError> {
        if !self.valid() {
            return Err(ProviderError::Rejected);
        }
        let request = serde_json::to_value(self).map_err(|_| ProviderError::Rejected)?;
        for key in [
            "version",
            "provider",
            "purpose",
            "operationId",
            "workspaceId",
            "generation",
            "storageIncarnationId",
            "sourceMachineId",
            "imagePins",
        ] {
            if value[key] != request[key] {
                return Err(ProviderError::Rejected);
            }
        }
        if matches!(value["status"].as_str(), Some("pending" | "starting")) {
            return Ok(None);
        }
        let resident = &value["resident"];
        if value["status"] != "ready"
            || value["requestSha256"] != self.digest()?
            || resident["ipcVerified"] != true
            || if self.purpose == Purpose::RetainedResume {
                resident["unbound"] != false
                    || resident["identityVerified"] != true
                    || !resident["childAlive"].is_boolean()
            } else {
                resident["unbound"] != true || resident["childAlive"] != false
            }
            || resident["recoveryAlive"] != false
            || !resident["residentPid"].as_u64().is_some_and(|v| v > 1)
            || !resident["residentStartTicks"]
                .as_u64()
                .is_some_and(|v| v > 0)
            || !resident["bootNonceSha256"].as_str().is_some_and(hash)
            || !value["unitStartSubmitted"].is_boolean()
        {
            return Err(ProviderError::Rejected);
        }
        Ok(Some(
            json!({"requestSha256":self.digest()?,"binding":request,"residentPid":resident["residentPid"],"residentStartTicks":resident["residentStartTicks"],"bootNonceSha256":resident["bootNonceSha256"],"unitStartSubmitted":value["unitStartSubmitted"],"ipcVerified":true}),
        ))
    }
    pub fn saved(&self, value: &Value) -> bool {
        if !self.valid() {
            return false;
        }
        value["requestSha256"] == self.digest().unwrap_or_default()
            && value["binding"] == serde_json::to_value(self).unwrap_or(Value::Null)
            && value["ipcVerified"] == true
            && value["residentPid"].as_u64().is_some_and(|v| v > 1)
            && value["residentStartTicks"].as_u64().is_some_and(|v| v > 0)
            && value["bootNonceSha256"].as_str().is_some_and(hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(crate) fn spec() -> InitialRuntimeBootstrap {
        InitialRuntimeBootstrap {
            version: 1,
            mode: "stopped-memory-snapshot-v1".into(),
            runtime_sha256: "a".repeat(64),
            supervisor_sha256: "b".repeat(64),
            service_sha256: "c".repeat(64),
            manifest_sha256: "d".repeat(64),
            helper_sha256: "e".repeat(64),
        }
    }
    fn request() -> Request {
        Request {
            version: 1,
            provider: ProviderId::Boxd,
            purpose: Purpose::InitialAllocation,
            operation_id: "original-op".into(),
            workspace_id: "workspace-original".into(),
            generation: 1,
            storage_incarnation_id: "original-disk".into(),
            source_machine_id: "original-machine".into(),
            image_pins: spec().pins(),
        }
    }
    #[test]
    fn boat_bootstrap_and_retained_resume_have_separate_explicit_authority() {
        let mut spec = spec();
        assert!(!spec.valid(ProviderId::Boat));
        spec.mode = "post-hydration-first-start-v1".into();
        assert!(spec.valid(ProviderId::Boat));
        assert!(!spec.valid(ProviderId::Boxd));
        let mut request = request();
        request.provider = ProviderId::Boat;
        request.purpose = Purpose::RetainedResume;
        assert!(request.valid());
        let mut response = serde_json::to_value(&request).unwrap();
        response["status"] = json!("ready");
        response["requestSha256"] = json!(request.digest().unwrap());
        response["unitStartSubmitted"] = json!(false);
        response["resident"] = json!({"residentPid":42,"residentStartTicks":7,"bootNonceSha256":"a".repeat(64),"ipcVerified":true,"unbound":false,"identityVerified":true,"childAlive":true,"recoveryAlive":false});
        assert!(request.response(&response).unwrap().is_some());
        response["resident"]["identityVerified"] = json!(false);
        assert!(request.response(&response).is_err());
        response["resident"]["identityVerified"] = json!(true);
        request.purpose = Purpose::InitialAllocation;
        response["purpose"] = json!("initial-allocation");
        response["requestSha256"] = json!(request.digest().unwrap());
        assert!(
            request.response(&response).is_err(),
            "fresh image may never accept a bound resident"
        );
        request.provider = ProviderId::Boxd;
        request.purpose = Purpose::RetainedResume;
        assert!(
            !request.valid(),
            "legacy Boxd retained path must stay unchanged"
        );
    }
    #[test]
    fn boat_boot_wire_matches_actual_python_scopes_without_any_effects() {
        use std::io::Write;
        let helper = include_str!("../../../../infra/cloud/managed-base-fresh-boot.py");
        let code = format!(
            "import json,sys\nn={{'__name__':'wire_validation'}}\nexec(compile({},'fresh-boot.py','exec'),n)\nv=n['validated'](json.load(sys.stdin))\nprint(n['digest'](n['canonical'](v)))",
            serde_json::to_string(helper).unwrap()
        );
        for purpose in [
            Purpose::InitialAllocation,
            Purpose::InitialRestore,
            Purpose::RetainedResume,
        ] {
            let mut request = request();
            request.provider = ProviderId::Boat;
            request.purpose = purpose;
            let mut child = std::process::Command::new("/usr/bin/python3")
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .args(["-I", "-B", "-c", &code])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&serde_json::to_vec(&request).unwrap())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8(output.stdout).unwrap().trim(),
                request.digest().unwrap()
            );
        }
    }
    #[test]
    #[ignore = "requires private captured candidate and installed helper source; no DB or provider effects"]
    fn existing_boxd_candidate_keeps_its_original_helper_pins_under_new_api() {
        use std::io::Write;
        let file = std::env::var("ZUSE_CLOUD_TEST_EXISTING_CANDIDATE_FILE").unwrap();
        let raw = std::fs::read(file).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&raw)),
            std::env::var("ZUSE_CLOUD_TEST_EXISTING_CANDIDATE_RAW_SHA256").unwrap()
        );
        let candidate: super::super::candidate::ManagedBaseCandidate =
            serde_json::from_slice(&raw).unwrap();
        candidate.validate().unwrap();
        let spec = candidate.initial_runtime_bootstrap.as_ref().unwrap();
        assert!(spec.valid(ProviderId::Boxd));
        let helper =
            std::fs::read(std::env::var("ZUSE_CLOUD_TEST_EXISTING_BOOTSTRAP_HELPER_FILE").unwrap())
                .unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&helper)), spec.helper_sha256);
        assert_ne!(
            spec.helper_sha256,
            format!(
                "{:x}",
                Sha256::digest(include_bytes!(
                    "../../../../infra/cloud/managed-base-fresh-boot.py"
                ))
            ),
            "regression must check a DIFFERENT installed helper from the newly bundled one"
        );
        let pin = candidate
            .pin(
                "isolated-fixture-owner",
                chrono::Utc::now().timestamp_millis() + 60000,
                true,
            )
            .unwrap();
        assert_eq!(
            pin.candidate_sha256,
            std::env::var("ZUSE_CLOUD_TEST_EXISTING_CANDIDATE_TYPED_SHA256").unwrap()
        );
        let settings = candidate.settings(&pin);
        assert!(
            InitialRuntimeBootstrap::from_config(&settings)
                .unwrap()
                .as_ref()
                == Some(spec)
        );
        let mut request = request();
        request.image_pins = spec.pins();
        assert!(request.command().unwrap().contains(&spec.helper_sha256));
        let code = format!(
            "import json,sys\nn={{'__name__':'wire_validation'}}\nexec(compile({},'old-installed-helper.py','exec'),n)\nv=n['validated'](json.load(sys.stdin))\nprint(n['digest'](n['canonical'](v)))",
            serde_json::to_string(&String::from_utf8(helper).unwrap()).unwrap()
        );
        let mut child = std::process::Command::new("/usr/bin/python3")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .args(["-I", "-B", "-c", &code])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&request).unwrap())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            request.digest().unwrap()
        );
    }
    #[test]
    fn initial_boot_is_explicit_and_legacy_release_serialization_stays_unchanged() {
        let mut release = super::super::tests::release();
        assert!(release.initial_runtime_bootstrap.is_none());
        assert!(
            serde_json::to_value(&release)
                .unwrap()
                .get("initialRuntimeBootstrap")
                .is_none()
        );
        assert!(release.settings().get("initialRuntimeBootstrap").is_none());
        let value = spec();
        release.initial_runtime_bootstrap = Some(value.clone());
        release.validate().unwrap();
        assert!(
            release.settings()["initialRuntimeBootstrap"] == serde_json::to_value(value).unwrap()
        );
        release.initial_runtime_bootstrap.as_mut().unwrap().mode = "retained-resume".into();
        assert!(release.validate().is_err());
        assert!(!spec().valid(ProviderId::Boat));
    }
    #[test]
    fn fresh_boot_wire_digest_matches_actual_python_validator_and_receipts_are_bound() {
        use std::io::Write;
        let request = request();
        let helper = include_str!("../../../../infra/cloud/managed-base-fresh-boot.py");
        let code = format!(
            "import json,sys\nn={{'__name__':'wire_validation'}}\nexec(compile({},'fresh-boot.py','exec'),n)\nv=n['validated'](json.load(sys.stdin))\nprint(n['digest'](n['canonical'](v)))",
            serde_json::to_string(helper).unwrap()
        );
        let mut child = std::process::Command::new("/usr/bin/python3")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .args(["-I", "-B", "-c", &code])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&request).unwrap())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            request.digest().unwrap()
        );
        let mut value = serde_json::to_value(&request).unwrap();
        value["status"] = json!("starting");
        assert!(request.response(&value).unwrap().is_none());
        value["status"] = json!("ready");
        value["requestSha256"] = json!(request.digest().unwrap());
        value["unitStartSubmitted"] = json!(true);
        value["resident"] = json!({"residentPid":42,"residentStartTicks":1,"bootNonceSha256":"a".repeat(64),"ipcVerified":true,"unbound":true,"childAlive":false,"recoveryAlive":false});
        let proof = request.response(&value).unwrap().unwrap();
        assert!(request.saved(&proof));
        let mut other = request.clone();
        other.source_machine_id = "other".into();
        assert!(!other.saved(&proof));
        assert!(other.response(&value).is_err());
        value["resident"]["childAlive"] = json!(true);
        assert!(request.response(&value).is_err());
        let command = request.command().unwrap();
        assert!(command.contains("managed-base-fresh-boot.py"));
        assert!(!command.contains("systemctl restart"));
        assert!(!command.contains("BOOT_TOKEN"));
    }
}
