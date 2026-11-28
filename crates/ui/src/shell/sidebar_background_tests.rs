//! A local chat's sidebar row while its agent's background work outlives the
//! turn, without booting an engine.
use super::*;
use gpui::{AppContext, TestAppContext, VisualTestContext};
use zuse_protocol::{ChatIndicator, SessionStatus};

struct SidebarHost(Entity<Shell>);

impl Render for SidebarHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0.update(cx, |shell, cx| {
            div()
                .w(px(280.0))
                .h(px(800.0))
                .child(shell.render_chat_sidebar(&Theme::default(), cx))
        })
    }
}

fn setup(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(Theme::default());
        crate::app_menus::init(cx);
        settings::init(UiSettings::default(), dir.path(), cx);
        crate::history::init(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            cx,
        );
    });
    let (host, cx) = cx.add_window_view(|_, cx| {
        SidebarHost(cx.new(|cx| {
            let state = cx.new(|_| AppState::new());
            let mut shell = Shell::new(
                state,
                EngineBootConfig {
                    target: None,
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zuse_protocol::HarnessId::Mock,
                },
                cx,
            );
            shell.settings.sidebar_organization = SidebarOrganization::InOneList;
            shell.reduced_motion = true;
            shell.state.update(cx, |state, cx| {
                state.workspace_scope = Some(WorkspaceScope::Local);
                state.local_device_id = Some("local".into());
                state.no_project = true;
                let at = Utc::now() - chrono::Duration::minutes(10);
                state.chats = ["open", "away"]
                    .into_iter()
                    .map(|id| {
                        serde_json::from_value(serde_json::json!({
                            "id": id, "title": id, "deviceId": "local", "archived": false,
                            "createdAt": at, "lastMessageAt": at, "lastSeenAt": at,
                        }))
                        .unwrap()
                    })
                    .collect();
                state.select_chat(Some("open".into()), cx);
            });
            shell
        }))
    });
    let shell = host.read_with(cx, |host, _| host.0.clone());
    redraw(cx);
    (shell, cx)
}

fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
}

/// `chat_id`'s session: `status`, holding a background shell when
/// `background` (else one that ended).
fn session(chat_id: &str, status: SessionStatus, background: bool) -> zuse_protocol::Session {
    let now = Utc::now();
    zuse_protocol::Session {
        last_completed_turn: None,
        chat_id: chat_id.into(),
        device_id: "local".into(),
        status,
        started_at: None,
        updated_at: now,
        running_subagents: 0,
        awaiting_input: None,
        background_tasks: vec![zuse_protocol::BackgroundTask {
            id: "watch".into(),
            kind: zuse_protocol::BackgroundTaskKind::Shell,
            description: "Watch CI".into(),
            command: Some("gh pr checks --watch".into()),
            started_at: now - chrono::Duration::seconds(84),
            ended_at: (!background).then_some(now),
            outcome: None,
            stoppable: true,
        }],
        notice: None,
    }
}

fn apply(shell: &Entity<Shell>, sessions: Vec<zuse_protocol::Session>, cx: &mut VisualTestContext) {
    shell.update(cx, |shell, cx| {
        shell.state.update(cx, |state, cx| {
            state.apply_sessions(sessions);
            cx.notify();
        })
    });
    redraw(cx);
}

fn marked(id: &str, cx: &mut VisualTestContext) -> bool {
    // Selectors are looked up by `&'static str`.
    let selector: &'static str = Box::leak(format!("chat-{id}-background").into_boxed_str());
    cx.debug_bounds(selector).is_some()
}

fn status(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> ChatIndicator {
    shell.read_with(cx, |shell, cx| {
        let state = shell.state.read(cx);
        let chat = state.chats.iter().find(|chat| chat.id == id).unwrap();
        state.display_status_for(chat, Utc::now())
    })
}

/// The owner's report: a chat whose turn ended with background work still
/// running looked idle in the sidebar. Its row wears the quiet background
/// mark, open or not; a running turn's spinner wins; and the work ending
/// out of sight reads done until the chat is opened.
#[gpui::test]
fn background_work_marks_local_rows_open_or_not_and_settles_done(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    assert!(!marked("open", cx) && !marked("away", cx));

    apply(
        &shell,
        vec![
            session("open", SessionStatus::Idle, true),
            session("away", SessionStatus::Idle, true),
        ],
        cx,
    );
    assert!(marked("open", cx), "the open chat's row shows its work");
    assert!(marked("away", cx), "a chat not open shows its work");

    // A turn runs again: its spinner, not the background mark.
    apply(
        &shell,
        vec![
            session("open", SessionStatus::Idle, true),
            session("away", SessionStatus::Working, true),
        ],
        cx,
    );
    assert_eq!(status(&shell, "away", cx), ChatIndicator::Working);
    assert!(!marked("away", cx));
    assert!(marked("open", cx));

    // The turn settles, then the work ends: done out of sight, quiet in
    // front.
    apply(
        &shell,
        vec![
            session("open", SessionStatus::Idle, true),
            session("away", SessionStatus::Idle, true),
        ],
        cx,
    );
    apply(
        &shell,
        vec![
            session("open", SessionStatus::Idle, false),
            session("away", SessionStatus::Idle, false),
        ],
        cx,
    );
    assert!(!marked("open", cx) && !marked("away", cx));
    assert_eq!(status(&shell, "open", cx), ChatIndicator::Idle);
    assert_eq!(status(&shell, "away", cx), ChatIndicator::Completed);
    shell.update(cx, |shell, cx| {
        shell
            .state
            .update(cx, |state, cx| state.select_chat(Some("away".into()), cx))
    });
    assert_eq!(status(&shell, "away", cx), ChatIndicator::Idle);
}
