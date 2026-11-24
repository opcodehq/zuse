//! Private connection recipes and public sidebar snapshots, separate from UI preferences.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const FILE: &str = "computer-connections.json";

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Recipe {
    Paired {
        link: String,
    },
    Account {
        subject: String,
        environment: String,
        label: String,
        controller: String,
    },
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    device: Option<String>,
    spaces: Vec<zuse_protocol::Space>,
    chats: Vec<zuse_protocol::Chat>,
    selected: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct SavedComputer {
    key: String,
    label: String,
    recipe: Recipe,
    snapshot: Snapshot,
}
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    version: u32,
    active: Option<String>,
    computers: Vec<SavedComputer>,
}
#[derive(Default)]
pub(in crate::shell) struct Registry {
    loaded: bool,
    pending: Vec<SavedComputer>,
    active: Option<String>,
    digest: Option<[u8; 32]>,
    data: Option<Vec<u8>>,
    removing: std::collections::HashMap<String, Task<()>>,
    save: Option<Task<()>>,
    catalog: Option<Task<()>>,
}

fn read(root: &Path) -> anyhow::Result<Saved> {
    use std::io::Read;
    let path = root.join(FILE);
    let metadata = match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Saved::default()),
        value => value?,
    };
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= MAX_BYTES,
        "Invalid saved computer registry"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        anyhow::ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "Saved computer registry must be private"
        );
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "Saved computer registry is too large"
    );
    let saved: Saved = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        saved.version == 1 && saved.computers.len() <= 64,
        "Invalid saved computer registry"
    );
    for computer in &saved.computers {
        anyhow::ensure!(
            computer.key.len() <= 1024 && computer.label.len() <= 1024,
            "Invalid saved computer"
        );
        if let Recipe::Paired { link } = &computer.recipe {
            zuse_remote::PairedTarget::parse_link(link)?;
        }
    }
    Ok(saved)
}
fn write(root: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "Saved computer registry is too large"
    );
    std::fs::create_dir_all(root)?;
    let mut temporary = tempfile::NamedTempFile::new_in(root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(root.join(FILE))?;
    Ok(())
}

impl Shell {
    pub(in crate::shell) fn restore_computers(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_owner.is_some() {
            return;
        }
        if !self.computer_registry.loaded {
            self.computer_registry.loaded = true;
            match read(&self.boot.data_dir) {
                Ok(saved) => {
                    self.computer_registry.pending = saved.computers;
                    self.computer_registry.active = saved.active;
                }
                Err(_) => crate::toast::error(cx, "Saved computer connections could not be read."),
            }
        }
        self.watch_account_computers(cx);
        let pending = std::mem::take(&mut self.computer_registry.pending);
        for saved in pending {
            if self
                .remote_workspaces
                .iter()
                .any(|workspace| workspace.key == saved.key)
            {
                continue;
            }
            let target = match &saved.recipe {
                Recipe::Paired { link } => zuse_remote::PairedTarget::parse_link(link)
                    .ok()
                    .map(crate::state::RemoteTarget::Paired),
                Recipe::Account {
                    subject,
                    environment,
                    label,
                    controller,
                } => {
                    let state = if controller == "local" {
                        Some(self.state.clone())
                    } else {
                        self.remote_workspaces
                            .iter()
                            .find(|workspace| &workspace.key == controller)
                            .map(|workspace| workspace.shell.read(cx).state.clone())
                    };
                    state.and_then(|state| {
                        let state = state.read(cx);
                        if state
                            .cloud_account
                            .as_ref()
                            .and_then(|account| account["user"]["id"].as_str())
                            != Some(subject.as_str())
                        {
                            return None;
                        }
                        Some(crate::state::RemoteTarget::Account(
                            crate::state::AccountTarget {
                                controller: state.engine()?.clone(),
                                subject: subject.clone(),
                                environment: environment.clone(),
                                label: label.clone(),
                            },
                        ))
                    })
                }
            };
            let Some(target) = target else {
                self.computer_registry.pending.push(saved);
                continue;
            };
            let active = self.active_remote_workspace;
            self.open_remote_workspace(target, cx);
            if let Some(workspace) = self
                .remote_workspaces
                .iter()
                .find(|workspace| workspace.key == saved.key)
            {
                workspace.shell.update(cx, |shell, cx| {
                    shell.state.update(cx, |state, cx| {
                        state.local_device_id = saved.snapshot.device;
                        state.spaces = saved.snapshot.spaces;
                        state.chats = saved.snapshot.chats;
                        state.selected_chat = saved.snapshot.selected;
                        cx.notify();
                    });
                });
            }
            if self.computer_registry.active.as_deref() != Some(saved.key.as_str()) {
                self.active_remote_workspace = active;
            }
        }
    }

    pub(in crate::shell) fn account_computer_removed(
        &mut self,
        environment: &str,
        subject: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(owner) = self.sidebar_owner.clone() {
            let environment = environment.to_owned();
            let subject = subject.to_owned();
            cx.defer(move |cx| {
                let _ = owner.update(cx, |owner, cx| {
                    owner.account_computer_removed(&environment, &subject, cx)
                });
            });
            return;
        }
        self.computer_registry.pending.retain(|computer| !matches!(&computer.recipe, Recipe::Account { environment: id, subject: account, .. } if id == environment && account == subject));
        let selected = self
            .active_remote_workspace
            .and_then(|index| self.remote_workspaces.get(index))
            .map(|workspace| workspace.key.clone());
        self.remote_workspaces.retain_mut(|workspace| {
            let shell = workspace.shell.read(cx);
            let matches = matches!(shell.boot.target.as_ref(), Some(crate::state::RemoteTarget::Account(target)) if target.environment == environment && target.subject == subject)
                || shell.state.read(cx).local_device_id.as_deref() == Some(environment);
            if !matches { return true; }
            if !shell.all_file_edits_flushed(cx) || shell.composer.read(cx).has_draft(cx) {
                workspace.removed = true;
                return true;
            }
            false
        });
        self.active_remote_workspace = selected.and_then(|selected| {
            self.remote_workspaces
                .iter()
                .position(|workspace| workspace.key == selected)
        });
        self.persist_computers(cx);
        cx.notify();
    }

    fn watch_account_computers(&mut self, cx: &mut Context<Self>) {
        if self.computer_registry.catalog.is_some() || self.state.read(cx).engine().is_none() {
            return;
        }
        self.computer_registry.catalog = Some(cx.spawn(async move |this, cx| {
            loop {
                let controllers = this.update(cx, |shell, cx| {
                    shell
                        .computer_sources(cx)
                        .into_iter()
                        .filter_map(|source| {
                            let state = source.state.read(cx);
                            Some((
                                source.state.clone(),
                                state.cloud_account.as_ref()?["user"]["id"]
                                    .as_str()?
                                    .to_owned(),
                                state.engine()?.clone(),
                            ))
                        })
                        .collect::<Vec<_>>()
                });
                let Ok(controllers) = controllers else {
                    return;
                };
                for (state, subject, engine) in controllers {
                    let request_engine = engine.clone();
                    let job = gpui_tokio::Tokio::spawn(cx, async move {
                        request_engine
                            .client()
                            .call(
                                zuse_rpc::methods::LIST_ACCOUNT_ENVIRONMENTS,
                                serde_json::json!({}),
                            )
                            .await
                    });
                    let Ok(Ok(value)) = job.await else {
                        continue;
                    };
                    let Some(environments) = value["environments"].as_array() else {
                        continue;
                    };
                    let environments: std::collections::HashSet<String> = environments
                        .iter()
                        .filter_map(|value| value["environmentId"].as_str().map(str::to_owned))
                        .collect();
                    let _ = this.update(cx, |shell, cx| {
                        let current = state.read(cx);
                        if current
                            .cloud_account
                            .as_ref()
                            .and_then(|account| account["user"]["id"].as_str())
                            != Some(subject.as_str())
                            || !current
                                .engine()
                                .is_some_and(|current| current.same_connection(&engine))
                        {
                            return;
                        }
                        let removed: Vec<String> =
                            shell
                                .remote_workspaces
                                .iter()
                                .filter_map(|workspace| {
                                    match workspace.shell.read(cx).boot.target.as_ref() {
                                        Some(crate::state::RemoteTarget::Account(target))
                                            if target.subject == subject
                                                && target.controller.same_connection(&engine)
                                                && !environments.contains(&target.environment) =>
                                        {
                                            Some(target.environment.clone())
                                        }
                                        _ => None,
                                    }
                                })
                                .chain(shell.computer_registry.pending.iter().filter_map(
                                    |computer| match &computer.recipe {
                                        Recipe::Account {
                                            subject: account,
                                            environment,
                                            ..
                                        } if account == &subject
                                            && !environments.contains(environment) =>
                                        {
                                            Some(environment.clone())
                                        }
                                        _ => None,
                                    },
                                ))
                                .collect();
                        for environment in removed {
                            shell.account_computer_removed(&environment, &subject, cx);
                        }
                    });
                }
                cx.background_executor()
                    .timer(Duration::from_secs(15))
                    .await;
            }
        }));
    }

    pub(in crate::shell) fn flush_computers(&mut self, cx: &mut Context<Self>) -> bool {
        if self.sidebar_owner.is_some() {
            return true;
        }
        self.persist_computers(cx);
        self.computer_registry.save = None;
        if let Some(bytes) = &self.computer_registry.data {
            if write(&self.boot.data_dir, bytes).is_err() {
                crate::toast::error(cx, "Computer connections could not be saved.");
                return false;
            }
        }
        true
    }

    pub(in crate::shell) fn request_remove_computer(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.computer_registry.removing.contains_key(key) {
            return;
        }
        let Some(workspace) = self
            .remote_workspaces
            .iter()
            .find(|workspace| workspace.key == key)
        else {
            return;
        };
        let shell = workspace.shell.read(cx);
        if !shell.all_file_edits_flushed(cx) || shell.composer.read(cx).has_draft(cx) {
            self.active_remote_workspace = self
                .remote_workspaces
                .iter()
                .position(|workspace| workspace.key == key);
            crate::toast::error(
                cx,
                "Save files and clear the draft before removing this computer.",
            );
            cx.notify();
            return;
        }
        let target = shell.boot.target.clone();
        let key = key.to_owned();
        let task_key = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = if let Some(crate::state::RemoteTarget::Account(target)) = &target {
                let controller = target.controller.clone();
                let params = serde_json::json!({"environmentId": target.environment, "expectedSubject": target.subject});
                gpui_tokio::Tokio::spawn(cx, async move { controller.client().call(zuse_rpc::methods::REMOVE_ACCOUNT_COMPUTER, params).await }).await.map_err(|error| error.to_string()).and_then(|result| result.map_err(|error| error.to_string()))
            } else { Ok(serde_json::Value::Null) };
            let _ = this.update(cx, |shell, cx| {
                shell.computer_registry.removing.remove(&task_key);
                if let Err(error) = result { crate::toast::error(cx, error); return; }
                if let Some(crate::state::RemoteTarget::Account(target)) = target {
                    shell.account_computer_removed(&target.environment, &target.subject, cx);
                } else {
                    let selected = shell.active_remote_workspace.and_then(|index| shell.remote_workspaces.get(index)).map(|workspace| workspace.key.clone());
                    shell.remote_workspaces.retain(|workspace| workspace.key != task_key);
                    shell.computer_registry.pending.retain(|computer| computer.key != task_key);
                    shell.active_remote_workspace = selected.and_then(|selected| shell.remote_workspaces.iter().position(|workspace| workspace.key == selected));
                }
                shell.flush_computers(cx);
                cx.notify();
            });
        });
        self.computer_registry.removing.insert(key, task);
        cx.notify();
    }

    pub(in crate::shell) fn persist_computers(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_owner.is_some() || !self.computer_registry.loaded {
            return;
        }
        let mut computers = self.computer_registry.pending.clone();
        for workspace in &self.remote_workspaces {
            if workspace.removed {
                continue;
            }
            let shell = workspace.shell.read(cx);
            let state = shell.state.read(cx);
            // New invalid invitations never become a launch target.
            if state.local_device_id.is_none() {
                continue;
            }
            let recipe = match shell.boot.target.as_ref() {
                Some(crate::state::RemoteTarget::Paired(target)) => match target.export_link() {
                    Ok(link) => Recipe::Paired { link },
                    Err(_) => continue,
                },
                Some(crate::state::RemoteTarget::Account(target)) => {
                    let controller = if self
                        .state
                        .read(cx)
                        .engine()
                        .is_some_and(|engine| engine.same_connection(&target.controller))
                    {
                        "local".into()
                    } else {
                        match self.remote_workspaces.iter().find(|workspace| {
                            workspace
                                .shell
                                .read(cx)
                                .state
                                .read(cx)
                                .engine()
                                .is_some_and(|engine| engine.same_connection(&target.controller))
                        }) {
                            Some(workspace) => workspace.key.clone(),
                            None => continue,
                        }
                    };
                    Recipe::Account {
                        subject: target.subject.clone(),
                        environment: target.environment.clone(),
                        label: target.label.clone(),
                        controller,
                    }
                }
                _ => continue,
            };
            computers.push(SavedComputer {
                key: workspace.key.clone(),
                label: workspace.label.clone(),
                recipe,
                snapshot: Snapshot {
                    device: state.local_device_id.clone(),
                    spaces: state.spaces.clone(),
                    chats: state.chats.clone(),
                    selected: state.selected_chat.clone(),
                },
            });
        }
        if computers.len() > 64 {
            return;
        }
        let active = self
            .active_remote_workspace
            .and_then(|index| self.remote_workspaces.get(index))
            .map(|workspace| workspace.key.clone());
        let Ok(bytes) = serde_json::to_vec(&Saved {
            version: 1,
            active,
            computers,
        }) else {
            return;
        };
        let digest = Sha256::digest(&bytes).into();
        if self.computer_registry.digest == Some(digest) {
            return;
        }
        self.computer_registry.digest = Some(digest);
        self.computer_registry.data = Some(bytes.clone());
        let root = self.boot.data_dir.clone();
        self.computer_registry.save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let outcome = cx
                .background_executor()
                .spawn(async move { write(&root, &bytes) })
                .await;
            if outcome.is_err() {
                let _ = this.update(cx, |shell, cx| {
                    shell.computer_registry.digest = None;
                    crate::toast::error(cx, "Computer connections could not be saved.");
                });
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_registry_round_trips_and_rejects_corrupt_or_shared_files() {
        let root = tempfile::tempdir().unwrap();
        let authority = zuse_remote::authority::Authority::open(root.path()).unwrap();
        let (_, invitation) = authority.create("Saved computer").unwrap();
        let target = zuse_remote::PairedTarget::new("http://127.0.0.1:1234", invitation).unwrap();
        let saved = Saved {
            version: 1,
            active: Some("paired".into()),
            computers: vec![SavedComputer {
                key: "paired".into(),
                label: "MacBook".into(),
                recipe: Recipe::Paired {
                    link: target.export_link().unwrap(),
                },
                snapshot: Snapshot::default(),
            }],
        };
        write(root.path(), &serde_json::to_vec(&saved).unwrap()).unwrap();
        let restored = read(root.path()).unwrap();
        assert_eq!(restored.computers[0].label, "MacBook");
        assert_eq!(restored.active.as_deref(), Some("paired"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file = root.path().join(FILE);
            assert_eq!(
                std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(read(root.path()).is_err());
        }
        std::fs::write(root.path().join(FILE), "invalid").unwrap();
        assert!(read(root.path()).is_err());
    }
    #[gpui::test]
    fn reopening_restores_saved_connections_and_cached_sidebar_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        let root = tempfile::tempdir().unwrap();
        let authority = zuse_remote::authority::Authority::open(root.path()).unwrap();
        let (_, invitation) = authority.create("Saved remote").unwrap();
        let target = zuse_remote::PairedTarget::new("http://127.0.0.1:9", invitation).unwrap();
        let key = format!(
            "paired:{}:{}",
            target.origin(),
            target.invitation().id.as_str()
        );
        let snapshot = Snapshot {
            device: Some("remote".into()), selected: Some("remote-chat".into()),
            spaces: vec![serde_json::from_value(serde_json::json!({"id":"remote-project","deviceId":"remote","path":"/remote/project","createdAt":"2026-01-01T00:00:00Z"})).unwrap()],
            chats: vec![serde_json::from_value(serde_json::json!({"id":"remote-chat","deviceId":"remote","spaceId":"remote-project","title":"Cached remote thread","archived":false,"createdAt":"2026-01-01T00:00:00Z"})).unwrap()],
        };
        write(
            root.path(),
            &serde_json::to_vec(&Saved {
                version: 1,
                active: Some(key.clone()),
                computers: vec![SavedComputer {
                    key,
                    label: "Remote".into(),
                    recipe: Recipe::Paired {
                        link: target.export_link().unwrap(),
                    },
                    snapshot,
                }],
            })
            .unwrap(),
        )
        .unwrap();
        cx.update(|cx| {
            gpui_tokio::init(cx);
            settings::init(settings::UiSettings::default(), root.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let boot = EngineBootConfig {
            target: None,
            data_dir: root.path().into(),
            ipc_port: 0,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None,
            org_id: None,
            workos_client_id: None,
            default_harness: zuse_protocol::HarnessId::Mock,
        };
        for _ in 0..2 {
            let window = cx.add_window(|_, cx| {
                let state = cx.new(|_| {
                    let mut state = AppState::new();
                    state.local_device_id = Some("local".into());
                    state
                });
                Shell::new(state, boot.clone(), cx)
            });
            window
                .update(cx, |shell, window, cx| {
                    shell.restore_computers(cx);
                    assert_eq!(shell.remote_workspaces.len(), 1);
                    assert_eq!(shell.active_remote_workspace, Some(0));
                    let remote = shell.remote_workspaces[0].shell.read(cx).state.read(cx);
                    assert_eq!(remote.local_device_id.as_deref(), Some("remote"));
                    assert_eq!(remote.selected_chat.as_deref(), Some("remote-chat"));
                    assert_eq!(
                        remote.chats[0].title.as_deref(),
                        Some("Cached remote thread")
                    );
                    assert!(shell.flush_computers(cx));
                    window.remove_window();
                })
                .unwrap();
        }
    }
}
