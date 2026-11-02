//! Explicit, resumable desktop import jobs. The running engine remains the only writer.
use super::*;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::mpsc;

pub const DISCOVER: &str = "DiscoverDesktopImport";
pub const IMPORT: &str = "ImportDesktopProfile";
const SETTINGS: &[&str] = &[
    "appearanceMode",
    "completionSoundEnabled",
    "completionSoundPreset",
    "defaultRuntimeMode",
    "defaultProviderId",
    "defaultModelByProvider",
    "providerEnabled",
    "defaultAutoCreateWorktree",
];

fn gates() -> &'static Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>> {
    static VALUE: OnceLock<Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    VALUE.get_or_init(Default::default)
}
fn bindings() -> &'static Mutex<HashMap<String, PathBuf>> {
    static VALUE: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    VALUE.get_or_init(Default::default)
}
pub fn gate(profile: &Path) -> Arc<tokio::sync::Mutex<()>> {
    let mut map = gates()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(gate) = map.get(profile).and_then(Weak::upgrade) {
        return gate;
    }
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    map.insert(profile.to_path_buf(), Arc::downgrade(&gate));
    gate
}
fn private_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn snapshot(source: &Path, target: &Path) -> Result<()> {
    if target.exists() {
        return Ok(());
    }
    let pending = target.with_extension(format!("{}.pending", uuid::Uuid::new_v4()));
    let db = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.backup(rusqlite::DatabaseName::Main, &pending, None)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&pending, std::fs::Permissions::from_mode(0o600))?;
    }
    let check = Connection::open_with_flags(&pending, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let status: String = check.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    ensure!(status == "ok", "The backup could not be verified");
    drop(check);
    std::fs::rename(pending, target)?;
    Ok(())
}
fn preferences(directory: &Path) -> Result<(Value, Vec<String>)> {
    let path = directory.join("settings.json");
    let value: Value = if path.exists() {
        serde_json::from_slice(&std::fs::read(path)?)?
    } else {
        json!({})
    };
    let obj = value
        .as_object()
        .context("Electron settings are not a JSON object")?;
    let mut safe = obj
        .iter()
        .filter(|(key, _)| SETTINGS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    let keybindings = directory.join("keybindings.json");
    if keybindings.is_file() {
        let value: Value = serde_json::from_slice(&std::fs::read(keybindings)?)?;
        // Validate the complete file before publishing any imported chats. The
        // viewport receives only the public rule fields, never arbitrary data.
        let _ = zuse_protocol::keybindings::import(&value).map_err(anyhow::Error::msg)?;
        let rules = value["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|rule| {
                let mut safe = json!({"key":rule["key"],"command":rule["command"]});
                if let Some(when) = rule.get("when") {
                    safe["when"] = when.clone();
                }
                safe
            })
            .collect::<Vec<_>>();
        safe.insert(
            "keybindings".into(),
            json!({"schemaVersion":1,"rules":rules}),
        );
    }
    let retained = obj
        .keys()
        .filter(|key| {
            !SETTINGS.contains(&key.as_str())
                && !matches!(key.as_str(), "schemaVersion" | "onboardingCompleted")
        })
        .cloned()
        .collect();
    Ok((Value::Object(safe), retained))
}
// Local plugin definitions can contain environment secrets. Keep them private in
// the backup; only their count is included in the UI report.
fn local_plugins(directory: &Path) -> Result<(usize, Vec<(std::ffi::OsString, Value)>)> {
    let folder = directory.join("local-plugins");
    if !folder.exists() {
        return Ok((0, Vec::new()));
    }
    let mut count = 0;
    let mut files = Vec::new();
    for file in std::fs::read_dir(folder)? {
        let file = file?;
        if !file.file_type()?.is_file() || file.path().extension().is_none_or(|s| s != "json") {
            continue;
        }
        let value: Value = serde_json::from_slice(&std::fs::read(file.path())?)?;
        count += value.as_array().map_or(0, Vec::len);
        files.push((file.file_name(), value));
    }
    Ok((count, files))
}
pub fn discover(custom: Option<PathBuf>, profile: &Path) -> Result<Value> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let base = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_default()
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
    };
    let mut paths = ["Zuse Alpha", "Zuse (Beta)", "Zuse Preview", "Zuse", "zuse"]
        .into_iter()
        .map(|name| base.join(name))
        .collect::<Vec<_>>();
    if let Some(path) = custom {
        paths.insert(
            0,
            if path.is_file() {
                path.parent()
                    .context("Choose an Electron profile folder")?
                    .to_path_buf()
            } else {
                path
            },
        );
    }
    if let Ok(jobs) = std::fs::read_dir(profile.join("desktop-import-jobs")) {
        for job in jobs.flatten().take(64) {
            if let Ok(bytes) = std::fs::read(job.path().join("request.json")) {
                if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some(path) = value["source"].as_str() {
                        paths.push(path.into());
                    }
                }
            }
        }
    }
    let imported = std::fs::read(profile.join("desktop-import/source.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let mut seen = HashSet::new();
    let mut sources = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        if !path.join("zuse.sqlite").is_file() {
            continue;
        }
        let path = path.canonicalize()?;
        if !seen.insert(path.clone()) {
            continue;
        }
        let source_id = id("source", &path.to_string_lossy());
        match DesktopImport::open(&path,&path.join("zuse.sqlite")) {
            Ok(importer)=>{
                let mut report=importer.inspect()?;
                let (prefs,retained)=preferences(&path)?;
                report.settings_mapped=prefs.as_object().unwrap().keys().cloned().collect();report.settings_retained=retained;
                let (local_count, _)=local_plugins(&path)?;
                let missing=importer.attachments.values().filter(|row|text(row,"abs_path").is_none_or(|path|!Path::new(path).is_file())).count();
                bindings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(source_id.clone(),path.clone());
                sources.push(json!({"id":source_id,"name":path.file_name().unwrap_or_default().to_string_lossy(),"path":path,"summary":report,"missingAttachments":missing,"localPlugins":local_count,"importedBefore":imported.as_ref().is_some_and(|v|v["source"]==path.to_string_lossy().as_ref())}));
            }
            Err(_)=>errors.push(json!({"name":path.file_name().unwrap_or_default().to_string_lossy(),"message":"This Electron profile could not be read. Update Electron or choose another folder."})),
        }
    }
    Ok(
        json!({"sources":sources,"errors":errors,"destinationId":id("destination",&profile.to_string_lossy())}),
    )
}
pub fn resolve(source_id: &str) -> Result<PathBuf> {
    bindings()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(source_id)
        .cloned()
        .context("Choose an Electron profile before importing")
}
pub fn run(
    destination: Destination,
    source: PathBuf,
    request_id: uuid::Uuid,
    include_settings: bool,
    events: mpsc::UnboundedSender<Value>,
) -> Result<()> {
    let directory = destination
        .profile_root
        .join("desktop-import-jobs")
        .join(request_id.to_string());
    private_directory(&directory)?;
    let request = json!({"source":source,"includeSettings":include_settings});
    let request_path = directory.join("request.json");
    if request_path.exists() {
        ensure!(
            serde_json::from_slice::<Value>(&std::fs::read(&request_path)?)? == request,
            "This import request belongs to another selection"
        );
    } else {
        private_json(&request_path, &request)?;
    }
    let finished = directory.join("result.json");
    if finished.exists() {
        let result = serde_json::from_slice(&std::fs::read(finished)?)?;
        let _ = events.send(result);
        return Ok(());
    }
    let _ = events.send(
        json!({"kind":"progress","phase":"backup","message":"Backing up Electron and GPUI data…"}),
    );
    snapshot(
        &source.join("zuse.sqlite"),
        &directory.join("source.sqlite3"),
    )?;
    for name in ["settings.json", "keybindings.json"] {
        let from = source.join(name);
        let to = directory.join(name);
        if from.is_file() && !to.exists() {
            let data: Value = serde_json::from_slice(&std::fs::read(from)?)?;
            private_json(&to, &data)?;
        }
    }
    let (local_count, definitions) = local_plugins(&source)?;
    if !definitions.is_empty() {
        let folder = directory.join("local-plugins");
        private_directory(&folder)?;
        for (name, value) in definitions {
            let path = folder.join(name);
            if !path.exists() {
                private_json(&path, &value)?;
            }
        }
    }
    let target_db = destination.profile_root.join("docs.sqlite3");
    if target_db.exists() {
        destination.workspace.flush_checked()?;
        destination.doc_host.flush_all();
        snapshot(&target_db, &directory.join("gpui-before.sqlite3"))?;
    }
    for name in [
        "ui-settings.json",
        "composer-defaults.json",
        "harness-prefs.json",
    ] {
        let from = destination.device_root.join(name);
        let to = directory.join(format!("gpui-before-{name}"));
        if from.is_file() && !to.exists() {
            let data: Value = serde_json::from_slice(&std::fs::read(from)?)?;
            private_json(&to, &data)?;
        }
    }
    let (prefs, retained) = preferences(&directory)?;
    let importer = DesktopImport::open(&source, &directory.join("source.sqlite3"))?;
    let mut report=importer.apply_to(&destination,false,|index,total|{let _=events.send(json!({"kind":"progress","phase":"chats","completed":index,"total":total,"message":"Importing chats and attachments…"}));})?;
    report.settings_mapped = if include_settings {
        prefs.as_object().unwrap().keys().cloned().collect()
    } else {
        Vec::new()
    };
    report.settings_retained = retained;
    destination.workspace.flush_checked()?;
    let open_chat = destination
        .workspace
        .read_chats()?
        .into_iter()
        .filter(|c| c.id.starts_with("desktop_session_") && !c.archived && c.thread_id() == c.id)
        .max_by_key(|c| c.last_message_at)
        .map(|c| c.id);
    let result = json!({"kind":"complete","report":report,"preferences":if include_settings{prefs}else{json!({})},"backupPath":directory,"localPlugins":local_count,"requestId":request_id,"openChatId":open_chat});
    private_json(&finished, &result)?;
    let _ = events.send(result);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn live_job_keeps_ui_owned_settings_and_replays_its_verified_result() {
        let (temp, source) = crate::desktop_import::tests::fixture();
        let target = temp.path().join("GPUI");
        let core = crate::desktop_import::tests::assemble(&target);
        let profile = target.join("orgs/dev-org/dev-user");
        std::fs::write(
            source.join("settings.json"),
            json!({"appearanceMode":"light","subagents":{"presets":{}}}).to_string(),
        )
        .unwrap();
        let plugins = source.join("local-plugins");
        std::fs::create_dir(&plugins).unwrap();
        let definition = json!([{"name":"Sample local plugin","command":"example","env":{"PRIVATE_KEY":"fixture-secret"}}]);
        std::fs::write(plugins.join("account.json"), definition.to_string()).unwrap();
        let ui_before = json!({"appearance":"dark","sidebarWidth":244}).to_string();
        std::fs::write(target.join("ui-settings.json"), &ui_before).unwrap();
        // Keep a WAL writer open: the backup must include this uncheckpointed row.
        let writer = Connection::open(source.join("zuse.sqlite")).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; INSERT INTO messages VALUES('wal-user','second','user','user','{\"text\":\"From WAL\"}','2026-01-02T00:00:00Z',1);").unwrap();
        let wal = source.join("zuse.sqlite-wal");
        let before = std::fs::read(&wal).unwrap();
        let preview = discover(Some(source.clone()), &profile).unwrap();
        assert_eq!(preview["sources"][0]["summary"]["threads"], 1);
        assert_eq!(preview["sources"][0]["localPlugins"], 1);
        assert!(!preview.to_string().contains("fixture-secret"));
        assert!(
            core.workspace.read_chats().unwrap().is_empty(),
            "Preview cannot import any data"
        );
        let request = uuid::Uuid::new_v4();
        let destination = Destination {
            workspace: core.workspace.clone(),
            doc_host: core.doc_host.clone(),
            device_id: core.device_id.clone(),
            device_root: target.clone(),
            profile_root: profile.clone(),
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        run(destination.clone(), source.clone(), request, true, tx).unwrap();
        let mut events = Vec::new();
        while let Ok(value) = rx.try_recv() {
            events.push(value);
        }
        let result = events.last().unwrap().clone();
        assert_eq!(result["kind"], "complete");
        assert_eq!(result["report"]["importedSessions"], 2);
        assert_eq!(result["report"]["sourceMessages"], 7);
        assert_eq!(result["preferences"]["appearanceMode"], "light");
        assert_eq!(
            std::fs::read_to_string(target.join("ui-settings.json")).unwrap(),
            ui_before,
            "The engine must not race GPUI's settings writer"
        );
        assert_eq!(
            std::fs::read(wal).unwrap(),
            before,
            "Import must not modify Electron WAL bytes"
        );
        let backup = profile
            .join("desktop-import-jobs")
            .join(request.to_string());
        assert!(backup.join("gpui-before.sqlite3").is_file());
        assert_eq!(result["localPlugins"], 1);
        assert!(!result.to_string().contains("fixture-secret"));
        assert_eq!(
            serde_json::from_slice::<Value>(
                &std::fs::read(backup.join("local-plugins/account.json")).unwrap()
            )
            .unwrap(),
            definition
        );
        let snapshot = Connection::open_with_flags(
            backup.join("source.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(
            snapshot
                .query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            7
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        run(destination.clone(), source.clone(), request, true, tx).unwrap();
        assert_eq!(rx.try_recv().unwrap(), result);
        let (tx, _) = mpsc::unbounded_channel();
        assert!(
            run(destination, source, request, false, tx).is_err(),
            "A request cannot change selection after admission"
        );
        core.shutdown().await;
    }
    #[tokio::test]
    async fn rpc_import_survives_a_closed_view_and_recovers_without_duplicates() {
        let (temp, source) = crate::desktop_import::tests::fixture();
        let target = temp.path().join("GPUI");
        let core = crate::desktop_import::tests::assemble(&target);
        let client = zuse_rpc::memory_client(core.rpc_service());
        let preview = client.call(DISCOVER, json!({"path":source})).await.unwrap();
        let request = uuid::Uuid::new_v4();
        let params = json!({"sourceId":preview["sources"][0]["id"],"requestId":request,"includeSettings":false});
        let mut stream = client.forward(IMPORT, params.clone()).await.unwrap();
        let first = stream.recv().await.unwrap();
        assert!(first.err.is_none());
        drop(stream);
        let result_file = target
            .join("orgs/dev-org/dev-user/desktop-import-jobs")
            .join(request.to_string())
            .join("result.json");
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !result_file.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let mut recovered = client.forward(IMPORT, params).await.unwrap();
        let complete = loop {
            let frame = recovered.recv().await.unwrap();
            assert!(frame.err.is_none());
            if let Some(item) = frame.item {
                if item["kind"] == "complete" {
                    break item;
                }
            }
        };
        assert_eq!(complete["report"]["importedSessions"], 2);
        assert_eq!(core.workspace.read_chats().unwrap().len(), 2);
        drop(recovered);
        drop(client);
        core.shutdown().await;
    }
}

#[cfg(test)]
mod keyboard_tests {
    use super::*;
    #[test]
    fn keyboard_import_is_independent_of_general_settings_and_strips_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let file = json!({"schemaVersion":1,"secret":"must-stay-local","rules":[{"key":"mod+shift+k","command":"open-chat-switcher","secret":"must-stay-local"}]});
        private_json(&dir.path().join("keybindings.json"), &file).unwrap();
        let (prefs, retained) = preferences(dir.path()).unwrap();
        assert!(retained.is_empty());
        assert_eq!(prefs["keybindings"]["rules"][0]["key"], "mod+shift+k");
        assert!(!prefs.to_string().contains("must-stay-local"));
        let parsed = zuse_protocol::keybindings::import(&prefs["keybindings"]).unwrap();
        assert_eq!(parsed[0].key, "mod-shift-k");
    }
}
