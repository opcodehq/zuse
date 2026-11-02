//! Host PTYs for verified cloud mirrors. No guest enrollment or compute wake is required.
use super::*;
use anyhow::Context;
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;
use zuse_protocol::methods;
use zuse_rpc::{RpcError, RpcReply};
#[derive(Clone, PartialEq, Eq)]
struct Owner(String, String, String);
struct Session {
    owner: Owner,
    cancel: CancellationToken,
}
#[derive(Default)]
struct State {
    sessions: HashMap<String, Session>,
    lifecycle: HashMap<(String, String), (i64, bool)>,
}
#[derive(Default)]
pub(super) struct Manager {
    terminals: OnceLock<crate::terminals::Terminals>,
    state: Mutex<State>,
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
impl Manager {
    fn pool(&self) -> &crate::terminals::Terminals {
        self.terminals.get_or_init(crate::terminals::Terminals::new)
    }
    fn close(&self, id: &str) {
        if let Some(session) = lock(&self.state).sessions.remove(id) {
            session.cancel.cancel();
            let _ = self.pool().close(id);
        }
    }
    pub(super) fn stop_workspace(&self, account: &str, workspace: &str) {
        let ids = lock(&self.state)
            .sessions
            .iter()
            .filter(|(_, s)| s.owner.0 == account && s.owner.1 == workspace)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            self.close(&id);
        }
    }
    pub(super) fn observe(&self, account: &str, row: &Workspace) {
        let blocked = row.archived()
            || row.deleting()
            || matches!(row.state.as_str(), "deleted" | "deleting");
        let ids = {
            let mut state = lock(&self.state);
            let key = (account.to_owned(), row.workspace_id.clone());
            if state
                .lifecycle
                .get(&key)
                .is_some_and(|(revision, _)| *revision > row.revision)
            {
                return;
            }
            state.lifecycle.insert(key, (row.revision, blocked));
            state
                .sessions
                .iter()
                .filter(|(_, s)| blocked && s.owner.0 == account && s.owner.1 == row.workspace_id)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>()
        };
        for id in ids {
            self.close(&id);
        }
    }
    fn open(
        self: &Arc<Self>,
        auth: Auth,
        owner: Owner,
        row: &Workspace,
        path: &std::path::Path,
        cols: u16,
        rows: u16,
    ) -> Result<zuse_protocol::TerminalSession> {
        let mut state = lock(&self.state);
        ensure!(
            !state
                .lifecycle
                .get(&(owner.0.clone(), owner.1.clone()))
                .is_some_and(|(revision, blocked)| *blocked && *revision >= row.revision),
            "Unarchive this workspace before opening a local terminal."
        );
        ensure!(
            super::sync::authorized(&auth, &owner.0),
            "The account changed. Select the workspace again."
        );
        let key = serde_json::to_string(&(&owner.0, &owner.1, &owner.2))?;
        let session = self.pool().open_owned(
            &key,
            path.to_str().context("Unsupported local folder path.")?,
            cols,
            rows,
        )?;
        let cancel = CancellationToken::new();
        state.sessions.insert(
            session.id.clone(),
            Session {
                owner: owner.clone(),
                cancel: cancel.clone(),
            },
        );
        let manager = Arc::downgrade(self);
        let id = session.id.clone();
        let mut changes = auth.watch_state();
        tokio::spawn(async move {
            loop {
                if !super::sync::authorized(&auth, &owner.0) {
                    if let Some(manager) = manager.upgrade() {
                        manager.close(&id);
                    }
                    return;
                }
                tokio::select! {biased;_=cancel.cancelled()=>return,result=changes.changed()=>if result.is_err(){if let Some(manager)=manager.upgrade(){manager.close(&id);}return;}}
            }
        });
        Ok(session)
    }
    fn owned(&self, owner: &Owner, id: &str) -> bool {
        lock(&self.state)
            .sessions
            .get(id)
            .is_some_and(|s| &s.owner == owner)
    }
}
fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value[name]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512)
        .with_context(|| format!("Missing {name}."))
}
pub async fn handle(auth: Auth, request: Value) -> Result<RpcReply, RpcError> {
    execute(auth, request)
        .await
        .map_err(|e| RpcError::Failed(e.to_string()))
}
async fn execute(auth: Auth, request: Value) -> Result<RpcReply> {
    let api = CloudApi::shared()?;
    ensure!(
        auth.workos_enabled() || api.loopback,
        "Sign in to access local cloud terminals."
    );
    let account = field(&request, "expectedAccountId")?.to_owned();
    ensure!(
        super::sync::authorized(&auth, &account),
        "The account changed. Select the workspace again."
    );
    let workspace = field(&request, "workspaceId")?.to_owned();
    let chat = field(&request, "chatId")?.to_owned();
    let owner = Owner(account.clone(), workspace.clone(), chat);
    let name = field(&request, "method")?;
    ensure!(
        matches!(
            name,
            methods::LIST_TERMINALS
                | methods::OPEN_TERMINAL
                | methods::SUBSCRIBE_TERMINAL
                | methods::WRITE_TERMINAL
                | methods::RESIZE_TERMINAL
                | methods::CLOSE_TERMINAL
        ),
        "Unsupported local terminal operation."
    );
    let id = request["requestId"]
        .as_u64()
        .context("Missing request ID.")?;
    api.syncs.hydrate(&api, &auth, &account)?;
    api.syncs.ensure_reconciliation(auth.clone());
    // Metadata authorization and the shared publication gate fence archive, deletion,
    // destination changes and every mutation. This path never acquires compute.
    let token = auth
        .access_token()
        .await
        .map_err(|_| anyhow::anyhow!("Sign in to access local terminals."))?;
    let value = api
        .request(&token, &["v1", "cloud", "workspaces", &workspace], None)
        .await?;
    let row: Workspace = serde_json::from_value(value.get("workspace").unwrap_or(&value).clone())?;
    ensure!(row.workspace_id == workspace, "Workspace identity changed.");
    api.local_terminals.observe(&account, &row);
    let blocked =
        row.archived() || row.deleting() || matches!(row.state.as_str(), "deleted" | "deleting");
    ensure!(
        !blocked,
        "Unarchive this workspace before using its local terminals."
    );
    ensure!(
        super::sync::authorized(&auth, &account),
        "The account changed. Select the workspace again."
    );
    let _guard = api.mirrors.sync_guard(&account, &workspace).await?;
    let params = &request["params"];
    let result = match name {
        methods::LIST_TERMINALS => {
            let key = serde_json::to_string(&(&owner.0, &owner.1, &owner.2))?;
            serde_json::to_value(api.local_terminals.pool().list_owned(&key))?
        }
        methods::OPEN_TERMINAL => {
            let path = api.syncs.local_root(&account, &workspace).context(
                "Enable local sync and wait for the first copy before opening a local terminal.",
            )?;
            ensure!(
                path.is_dir() && path.join(".zuse-sync.json").is_file(),
                "Wait for local sync to finish before opening a local terminal."
            );
            mirror::prepare_target(&path, &workspace)?;
            let cols = serde_json::from_value(params["cols"].clone())?;
            let rows = serde_json::from_value(params["rows"].clone())?;
            serde_json::to_value(
                api.local_terminals
                    .open(auth, owner, &row, &path, cols, rows)?,
            )?
        }
        _ => {
            let terminal = field(params, "terminalId")?;
            ensure!(
                api.local_terminals.owned(&owner, terminal),
                "Terminal does not belong to this account, workspace and conversation."
            );
            let pool = api.local_terminals.pool();
            match name {
                methods::SUBSCRIBE_TERMINAL => {
                    let after = serde_json::from_value(
                        params.get("afterSeq").cloned().unwrap_or(Value::Null),
                    )?;
                    let rx = pool.subscribe(terminal, after)?;
                    let events = futures::stream::unfold(rx, move |mut rx| async move {
                        let event = rx.recv().await?;
                        Some((json!({"id":id,"item":event} ), rx))
                    })
                    .chain(futures::stream::once(async move {
                        json!({"id":id,"done":true})
                    }))
                    .boxed();
                    return Ok(RpcReply::Stream(events));
                }
                methods::WRITE_TERMINAL => pool.write(
                    terminal,
                    params["data"].as_str().context("Missing terminal input.")?,
                )?,
                methods::RESIZE_TERMINAL => pool.resize(
                    terminal,
                    serde_json::from_value(params["cols"].clone())?,
                    serde_json::from_value(params["rows"].clone())?,
                )?,
                methods::CLOSE_TERMINAL => api.local_terminals.close(terminal),
                _ => unreachable!(),
            }
            json!({"ok":true})
        }
    };
    Ok(RpcReply::Stream(
        futures::stream::once(async move { json!({"id":id,"ok":result,"done":true}) }).boxed(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Auth, tempfile::TempDir, Workspace) {
        let data = tempfile::tempdir().unwrap();
        let mut config = crate::auth::AuthConfig::new("http://127.0.0.1:1", data.path());
        config.workos_client_id = None;
        let auth = Auth::new(config);
        let row=serde_json::from_value(json!({"workspaceId":"workspace","chatId":"thread","initialSessionId":"chat","branch":"feature","state":"paused","desiredState":"paused","statusCode":"paused","revision":3})).unwrap();
        (auth, data, row)
    }
    #[tokio::test]
    async fn local_shell_owns_real_files_and_archive_fences_stale_opens() {
        use base64::Engine as _;
        let (auth, data, row) = fixture();
        let manager = Arc::new(Manager::default());
        let owner = Owner(auth.user_id().unwrap(), "workspace".into(), "chat".into());
        let session = manager
            .open(auth.clone(), owner.clone(), &row, data.path(), 80, 24)
            .unwrap();
        assert!(manager.owned(&owner, &session.id));
        assert!(!manager.owned(
            &Owner(owner.0.clone(), owner.1.clone(), "other-chat".into()),
            &session.id
        ));
        manager
            .pool()
            .write(
                &session.id,
                &base64::engine::general_purpose::STANDARD
                    .encode("printf 'local bytes' > shell-proof\r"),
            )
            .unwrap();
        for _ in 0..100 {
            if data.path().join("shell-proof").exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(
            std::fs::read(data.path().join("shell-proof")).unwrap(),
            b"local bytes"
        );
        let mut archived = row.clone();
        archived.revision = 4;
        archived.state = "archived".into();
        archived.desired_state = "archived".into();
        manager.observe(&owner.0, &archived);
        assert!(!manager.owned(&owner, &session.id));
        assert!(
            manager
                .open(auth.clone(), owner.clone(), &row, data.path(), 80, 24)
                .is_err(),
            "An old ready/paused response cannot reopen a reclaimed checkout"
        );
        let mut restored = row;
        restored.revision = 5;
        manager.observe(&owner.0, &restored);
        let reopened = manager
            .open(auth, owner, &restored, data.path(), 80, 24)
            .unwrap();
        manager.close(&reopened.id);
    }
    #[tokio::test]
    async fn signout_closes_local_shell_without_a_viewport() {
        let (auth, data, row) = fixture();
        let manager = Arc::new(Manager::default());
        let owner = Owner(auth.user_id().unwrap(), "workspace".into(), "chat".into());
        let session = manager
            .open(auth.clone(), owner.clone(), &row, data.path(), 80, 24)
            .unwrap();
        auth.sign_out();
        for _ in 0..100 {
            if !manager.owned(&owner, &session.id) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(!manager.owned(&owner, &session.id));
        assert!(
            manager
                .open(auth, owner, &row, data.path(), 80, 24)
                .is_err()
        );
    }
}
