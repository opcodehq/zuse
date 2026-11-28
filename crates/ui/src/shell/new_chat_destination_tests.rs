//! Where a new chat opens: an explicit project wins, New thread reopens the
//! user's last destination (across restarts), and Cloud only chooses itself
//! before the user has picked anything.
use super::*;
use crate::settings::composer::{ComposerDefaults, NewChatDestination};
use gpui::{AppContext, TestAppContext, WindowHandle};
use serde_json::{Value, json};

struct Pending;
#[async_trait::async_trait]
impl zuse_rpc::RpcService for Pending {
    async fn handle(&self, _: &str, _: Value) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
        Ok(zuse_rpc::RpcReply::Stream(Box::pin(
            futures::stream::pending(),
        )))
    }
}

fn space(id: &str, path: &str, remote: Option<&str>) -> zuse_protocol::Space {
    zuse_protocol::Space {
        id: id.into(),
        device_id: "mac".into(),
        path: path.into(),
        name: None,
        git_detected: true,
        git_checked_at: None,
        checkout_id: None,
        repository_id: None,
        remote_repository: remote.map(str::to_owned),
        created_at: chrono::Utc::now(),
    }
}

/// The owner's sidebar: "zuse" is a local checkout backed by a Cloud
/// repository, "legion" a local checkout Cloud doesn't know, and "zkit" a
/// repository that only exists in Cloud.
fn boot(cx: &mut TestAppContext, dir: &std::path::Path) -> WindowHandle<Shell> {
    let dir = dir.to_path_buf();
    let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
        std::sync::Arc::new(Pending),
    ));
    cx.add_window(move |_, cx| {
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.data_dir = Some(dir.clone());
            state.local_device_id = Some("mac".into());
            state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
            state.set_cloud_subscription_for_test(true);
            assert!(state.apply_cloud_sidebar_catalog(
                &json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[
                    {"projectId":"project-zuse","displayName":"opcodehq/zuse","repositoryIdentity":"github.com/opcodehq/zuse","canCreate":true},
                    {"projectId":"project-zkit","displayName":"opcodehq/zkit","repositoryIdentity":"github.com/opcodehq/zkit","canCreate":true},
                ]}),
                &json!({"accountId":"owner","organizationId":null,"workspaces":[]})
            ));
            state.spaces.push(space(
                "space-zuse",
                "/work/zuse",
                Some("github.com/OpcodeHQ/zuse"),
            ));
            state.spaces.push(space("space-legion", "/work/legion", None));
            state.spaces_synced = true;
            state
        });
        state.update(cx, |state, _| state.set_test_engine(controller));
        let mut shell = Shell::new(
            state,
            EngineBootConfig {
                target: None,
                data_dir: dir,
                ipc_port: 0,
                edge_url: "http://127.0.0.1:1".into(),
                edge_token: None,
                org_id: None,
                workos_client_id: None,
                default_harness: zuse_protocol::HarnessId::Mock,
            },
            cx,
        );
        shell.reduced_motion = true;
        shell.settings.space_filter = None;
        shell
    })
}

fn init(cx: &mut TestAppContext, dir: &std::path::Path, runtime: &tokio::runtime::Runtime) {
    cx.update(|cx| {
        gpui_base::init(cx);
        gpui_tokio::init_from_handle(cx, runtime.handle().clone());
        cx.set_global(Theme::default());
        crate::app_menus::init(cx);
        settings::init(UiSettings::default(), dir, cx);
        crate::history::init(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            cx,
        );
    });
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// The new-chat canvas's destination: the Cloud repository, else `None`
/// (a computer), with the selected project.
fn destination(shell: &Shell, cx: &App) -> (Option<String>, Option<String>) {
    let state = shell.state.read(cx);
    (
        state
            .cloud_project
            .as_ref()
            .and_then(|row| row["projectId"].as_str().map(str::to_owned)),
        state.selected_space.clone(),
    )
}

fn pickers(shell: &Shell, cx: &App) -> Entity<crate::pickers::Pickers> {
    shell.composer.read(cx).pickers().clone()
}

fn cloud_row(shell: &Shell, project: &str, cx: &App) -> Value {
    shell
        .state
        .read(cx)
        .cloud_sidebar_projects()
        .into_iter()
        .find(|row| row["projectId"] == project)
        .unwrap()
}

/// Leave the canvas for an existing chat, the way New thread is reached.
fn into_a_chat(shell: &mut Shell, cx: &mut Context<Shell>) {
    shell.state.update(cx, |state, cx| {
        state.select_chat(Some("some-chat".into()), cx)
    });
}

#[gpui::test]
fn first_run_opens_cloud_for_a_cloud_backed_project(cx: &mut TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let dir = tempfile::tempdir().unwrap();
    init(cx, dir.path(), &runtime);
    let window = boot(cx, dir.path());
    window
        .update(cx, |shell, _, cx| {
            shell.state.update(cx, |state, cx| {
                state.select_space(Some("space-zuse".into()), cx)
            });
            shell.open_new_session(None, cx);
            assert_eq!(
                destination(shell, cx),
                (Some("project-zuse".into()), Some("space-zuse".into())),
                "no pick yet: a cloud-backed project opens on Cloud"
            );
            assert!(
                pickers(shell, cx)
                    .read(cx)
                    .remembered_destination()
                    .is_none()
            );
        })
        .unwrap();
}

/// The owner's report: after Cloud on "zuse", the plus on "legion" opened
/// the new chat on zuse's Cloud repository.
#[gpui::test]
fn project_plus_opens_that_project(cx: &mut TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let dir = tempfile::tempdir().unwrap();
    init(cx, dir.path(), &runtime);
    let window = boot(cx, dir.path());
    window
        .update(cx, |shell, _, cx| {
            let zuse = cloud_row(shell, "project-zuse", cx);
            shell.open_new_chat_in_project(Some(zuse.clone()), Some("space-zuse".into()), cx);
            assert_eq!(
                destination(shell, cx),
                (Some("project-zuse".into()), Some("space-zuse".into()))
            );
            // A local project.
            shell.open_new_chat_in_project(None, Some("space-legion".into()), cx);
            assert_eq!(
                destination(shell, cx),
                (None, Some("space-legion".into())),
                "the plus on legion opens legion, not zuse on Cloud"
            );
            // A project that only exists in Cloud.
            let zkit = cloud_row(shell, "project-zkit", cx);
            shell.open_new_chat_in_project(Some(zkit), None, cx);
            assert_eq!(destination(shell, cx).0.as_deref(), Some("project-zkit"));
            // And back to the cloud-backed project from a chat.
            into_a_chat(shell, cx);
            shell.open_new_chat_in_project(Some(zuse), Some("space-zuse".into()), cx);
            assert_eq!(
                destination(shell, cx),
                (Some("project-zuse".into()), Some("space-zuse".into()))
            );
            // An explicit project (pull request, sidebar filter) is local too.
            shell.open_new_session(Some("space-legion".into()), cx);
            assert_eq!(destination(shell, cx), (None, Some("space-legion".into())));
        })
        .unwrap();
}

#[gpui::test]
fn new_thread_reopens_the_last_chosen_destination(cx: &mut TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let dir = tempfile::tempdir().unwrap();
    init(cx, dir.path(), &runtime);
    let window = boot(cx, dir.path());
    window
        .update(cx, |shell, _, cx| {
            shell.state.update(cx, |state, cx| {
                state.select_space(Some("space-zuse".into()), cx)
            });
            shell.open_new_session(None, cx);
            assert_eq!(destination(shell, cx).0.as_deref(), Some("project-zuse"));
            // The user moves the chat to this computer, on the same
            // cloud-backed project, as a worktree.
            let pickers = pickers(shell, cx);
            pickers.update(cx, |pickers, cx| {
                pickers.pick_space_for_test("space-zuse".into(), cx);
                pickers.pick_worktree_for_test(true, cx);
            });
            assert_eq!(destination(shell, cx), (None, Some("space-zuse".into())));
            into_a_chat(shell, cx);
            shell.open_new_session(None, cx);
            assert_eq!(
                destination(shell, cx),
                (None, Some("space-zuse".into())),
                "New thread keeps the computer the user chose, not Cloud"
            );
            assert!(pickers.read(cx).on_worktree_for_test());
            // The plus on the cloud-backed project keeps the computer too.
            into_a_chat(shell, cx);
            let zuse = cloud_row(shell, "project-zuse", cx);
            shell.open_new_chat_in_project(Some(zuse.clone()), Some("space-zuse".into()), cx);
            assert_eq!(destination(shell, cx), (None, Some("space-zuse".into())));
            // Local, on another project.
            pickers.update(cx, |pickers, cx| {
                pickers.pick_space_for_test("space-legion".into(), cx);
                pickers.pick_worktree_for_test(false, cx);
            });
            into_a_chat(shell, cx);
            shell.open_new_session(None, cx);
            assert_eq!(destination(shell, cx), (None, Some("space-legion".into())));
            assert!(!pickers.read(cx).on_worktree_for_test());
            // Cloud: New thread opens Cloud on that repository, even from a
            // project Cloud doesn't know.
            pickers.update(cx, |pickers, cx| pickers.select_cloud_for_test(zuse, cx));
            assert_eq!(destination(shell, cx).0.as_deref(), Some("project-zuse"));
            shell.state.update(cx, |state, cx| {
                state.select_space(Some("space-legion".into()), cx)
            });
            into_a_chat(shell, cx);
            shell.open_new_session(None, cx);
            assert_eq!(destination(shell, cx).0.as_deref(), Some("project-zuse"));
            assert!(matches!(
                pickers.read(cx).remembered_destination(),
                Some(NewChatDestination::Cloud { project_id, .. }) if project_id == "project-zuse"
            ));
        })
        .unwrap();
}

#[gpui::test]
fn the_last_destination_survives_a_restart(cx: &mut TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let dir = tempfile::tempdir().unwrap();
    init(cx, dir.path(), &runtime);
    // Last time: Worktree on this computer, in the cloud-backed project.
    let first = boot(cx, dir.path());
    first
        .update(cx, |shell, _, cx| {
            let pickers = pickers(shell, cx);
            pickers.update(cx, |pickers, cx| {
                pickers.pick_space_for_test("space-zuse".into(), cx);
                pickers.pick_worktree_for_test(true, cx);
            });
        })
        .unwrap();
    let saved = ComposerDefaults::load(dir.path());
    assert_eq!(saved.destination, Some(NewChatDestination::Computer));
    assert_eq!(saved.project.as_deref(), Some("space-zuse"));
    let restarted = boot(cx, dir.path());
    restarted
        .update(cx, |shell, _, cx| {
            shell.open_new_session(None, cx);
            assert_eq!(destination(shell, cx), (None, Some("space-zuse".into())));
            assert!(pickers(shell, cx).read(cx).on_worktree_for_test());
        })
        .unwrap();
    // Last time: Cloud on zuse, then a restart into the legion project.
    ComposerDefaults {
        project: Some("space-legion".into()),
        destination: Some(NewChatDestination::Cloud {
            project_id: "project-zuse".into(),
            account_id: "owner".into(),
        }),
        ..Default::default()
    }
    .save(dir.path())
    .unwrap();
    let restarted = boot(cx, dir.path());
    restarted
        .update(cx, |shell, _, cx| {
            shell.open_new_session(None, cx);
            assert_eq!(destination(shell, cx).0.as_deref(), Some("project-zuse"));
        })
        .unwrap();
}
