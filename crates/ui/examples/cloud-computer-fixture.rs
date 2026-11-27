//! Renders the actual native Shared adapter; scripted data never represents a live provider.
use futures::{StreamExt, stream};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use zuse_protocol::methods;
use zuse_rpc::{RpcError, RpcReply, RpcService};
use zuse_ui::*;
struct Controller {
    calls: Mutex<Vec<String>>,
    seed: Value,
    cached: bool,
    mixed: bool,
    reconnect: bool,
    fresh_reads: std::sync::atomic::AtomicUsize,
    history_generation: std::sync::atomic::AtomicUsize,
    feeds: Mutex<Vec<futures::channel::mpsc::UnboundedSender<Value>>>,
    accounts: Mutex<Vec<futures::channel::mpsc::UnboundedSender<Value>>>,
    new_chat: bool,
    /// The account holds a Cloud plan, so a new chat in a cloud-backed
    /// project starts on Cloud (`native-mixed-first`).
    subscribed: bool,
    admitted: Mutex<Option<Value>>,
    /// `FIXTURE_ADMIT=provisioning`: the admitted computer has finished
    /// starting and the agent replied.
    ready: std::sync::atomic::AtomicBool,
}
#[async_trait::async_trait]
impl RpcService for Controller {
    async fn handle(&self, method: &str, _params: Value) -> Result<RpcReply, RpcError> {
        self.calls.lock().unwrap().push(method.into());
        if let Some(reply) = computers_reply(method, &_params) {
            return reply;
        }
        if let Some(reply) = checkout_reply(method, &_params) {
            return reply;
        }
        // Review captures (`FIXTURE_REVIEW=1`): an opened cloud chat's
        // computer is awake, and nothing is asserted about the call order.
        if review() && method == methods::CLOUD_BILLING_ENTITLEMENTS {
            return RpcReply::value(&json!({"accountId":"fixture-owner","organizationId":null,
                "data":{"entitlements":[{"offerId":"cloud-starter-v1","status":"active"}]}}));
        }
        if review() && method == methods::CLOUD_ENVIRONMENT {
            let mut frames = vec![json!({"state":"connecting","message":"Waking…"})];
            if !matches!(
                std::env::var("FIXTURE_COMPUTER_STATE").as_deref(),
                Ok("starting" | "asleep")
            ) {
                frames.push(json!({"state":"ready","epoch":1}));
            }
            return Ok(RpcReply::Stream(Box::pin(
                stream::iter(frames).chain(stream::pending()),
            )));
        }
        match method {
            methods::LOCAL_DEVICE if self.new_chat && !self.mixed => Err(RpcError::Capability(
                "fixture.local-device-unavailable".into(),
            )),
            methods::LOCAL_DEVICE => RpcReply::value(
                &json!({"deviceId":"fixture-controller","workspaceScope":"local","capabilities":[]}),
            ),
            methods::WATCH_SPACES if self.mixed => RpcReply::value(&json!([mixed_space()])),
            methods::WATCH_CHATS if self.mixed => RpcReply::value(&json!([mixed_chat()])),
            // The new chat's project is listed, so its sidebar row shows.
            methods::CLOUD_PROJECTS if (self.new_chat && admit_provisioning()) || queued() => {
                let mut context = new_chat_context();
                context["data"] = json!({"projects": context["projects"].clone()});
                RpcReply::value(&context)
            }
            methods::CLOUD_PROJECTS => RpcReply::value(
                &json!({"accountId":"fixture-owner","organizationId":null,"projects":[]}),
            ),
            methods::CLOUD_GITHUB => RpcReply::value(
                &json!({"accountId":"fixture-owner","organizationId":null,"configured":true,"user":null,"installations":[],"repositories":[]}),
            ),
            methods::CLOUD_AGENT_CONNECTIONS => RpcReply::value(
                &json!({"accountId":"fixture-owner","organizationId":null,"connections":[],"supportsDeviceReconnect":true}),
            ),
            methods::CLOUD_ACCOUNT_STATUS => {
                let (send, recv) = futures::channel::mpsc::unbounded();
                send.unbounded_send(
                    json!({"user":{"id":"fixture-owner","email":"alex@example.test"},"orgId":null}),
                )
                .unwrap();
                self.accounts.lock().unwrap().push(send);
                Ok(RpcReply::Stream(Box::pin(recv)))
            }
            methods::CLOUD_BILLING_ENTITLEMENTS if self.new_chat => {
                assert_eq!(_params["expectedAccountId"], "fixture-owner");
                assert!(_params["expectedOrganizationId"].is_null());
                if self.subscribed {
                    return RpcReply::value(
                        &json!({"accountId":"fixture-owner","organizationId":null,
                        "data":{"entitlements":[{"offerId":"cloud-starter-v1","status":"active"}]}}),
                    );
                }
                RpcReply::value(&json!({"data":{"entitlements":[]}}))
            }
            methods::CLOUD_CONTEXT if self.new_chat => RpcReply::value(&new_chat_context()),
            methods::CLOUD_CREATE if self.new_chat => {
                assert_eq!(_params["projectId"], "cloud-only-project");
                assert_eq!(_params["firstMessage"], "Fix the flaky sync test");
                assert_eq!(_params["agentConnectionId"], "fixture-codex");
                *self.admitted.lock().unwrap() = Some(_params.clone());
                // `FIXTURE_ADMIT=1`: the server admits the chat at once.
                // `FIXTURE_ADMIT=provisioning`: admitted while the computer
                // is still starting, as on a fresh account computer.
                if admit() {
                    let state = if admit_provisioning() {
                        "provisioning"
                    } else {
                        "ready"
                    };
                    return RpcReply::value(
                        &json!({"workspace":{"workspaceId":"admitted-computer","chatId":"admitted-thread","initialSessionId":"admitted-session","state":state,"desiredState":"ready","statusCode":state,"commandDeliveryMode":"server-admitted-v2","commandAdmissionVersion":2}}),
                    );
                }
                RpcReply::value(
                    &json!({"workspace":{"workspaceId":"admitted-computer","chatId":"admitted-thread","initialSessionId":"admitted-session","state":"provisioning","desiredState":"ready","statusCode":"allocating"}}),
                )
            }
            methods::CLOUD_CONTEXT => RpcReply::value(
                &json!({"accountId":"fixture-owner","organizationId":null,"apiOrigin":"https://fixture-api.example.test","projects":[{"id":"project","name":"Fieldnotes","repositoryFullName":"example/fieldnotes"}],"agentConnections":[{"connectionId":"claude","agent":"claude","state":"connected","label":"Alex’s Claude"},{"connectionId":"codex","agent":"codex","state":"connected","label":"Alex’s Codex"}],"agentCatalog":{},"providerAgentCatalogs":{},"computerCapabilities":{"providers":[]}}),
            ),
            // The account list has the new chat only once it is ready: the
            // sidebar keeps the created row until then.
            methods::CLOUD_WORKSPACES if self.new_chat && admit_provisioning() => {
                let ready = self.ready.load(std::sync::atomic::Ordering::SeqCst);
                let mut row = admitted_history()["workspace"].clone();
                row["projectId"] = json!("cloud-only-project");
                let workspaces = if ready { vec![row] } else { vec![] };
                RpcReply::value(
                    &json!({"accountId":"fixture-owner","organizationId":null,"apiOrigin":"https://fixture-api.example.test","workspaces":workspaces}),
                )
            }
            // `FIXTURE_QUEUE=1`: the open chat is listed, so its row spins.
            methods::CLOUD_WORKSPACES if queued() => {
                let mut row = computer_seed()["workspace"].clone();
                row["projectId"] = json!("cloud-only-project");
                RpcReply::value(
                    &json!({"accountId":"fixture-owner","organizationId":null,"apiOrigin":"https://fixture-api.example.test","workspaces":[row]}),
                )
            }
            methods::CLOUD_WORKSPACES => {
                let workspaces = if self.mixed {
                    let mut row = computer_seed()["workspace"].clone();
                    row["projectId"] = json!("cloud-only-project");
                    vec![row]
                } else {
                    vec![]
                };
                RpcReply::value(
                    &json!({"accountId":"fixture-owner","organizationId":null,"workspaces":workspaces}),
                )
            }
            methods::CLOUD_HISTORY
                if self.mixed && _params["workspaceId"] == "fixture-computer" =>
            {
                let mut value = computer_seed();
                if _params["cacheOnly"] == true {
                    value["provisional"] = json!(true);
                }
                RpcReply::value(&value)
            }
            // The computer records the first message under its own id and
            // replies; the launch command row still reads running (a missed
            // end event).
            methods::CLOUD_HISTORY
                if self.new_chat
                    && admit_provisioning()
                    && !self.ready.load(std::sync::atomic::Ordering::SeqCst) =>
            {
                RpcReply::value(&provisioning_history())
            }
            methods::CLOUD_HISTORY if self.new_chat && admit() => {
                RpcReply::value(&admitted_history())
            }
            methods::CLOUD_HISTORY if self.new_chat => RpcReply::value(
                &json!({"accountId":"fixture-owner","apiOrigin":"https://fixture-api.example.test","workspace":admitted_workspace()}),
            ),
            // Review captures of an opened chat can open its computer menu,
            // which reads the workspace for this account.
            methods::CLOUD_WORKSPACE
                if review() && self.mixed && _params["workspaceId"] == "fixture-computer" =>
            {
                RpcReply::value(&json!({"accountId":"fixture-owner","organizationId":null,
                    "workspace":computer_seed()["workspace"]}))
            }
            methods::CLOUD_WORKSPACE if self.new_chat => {
                RpcReply::value(&json!({"workspace":admitted_workspace()}))
            }
            methods::CLOUD_HISTORY => {
                if self.cached && _params["cacheOnly"] == true {
                    let mut cached = self.seed.clone();
                    cached["provisional"] = json!(true);
                    return RpcReply::value(&cached);
                }
                if self.reconnect {
                    self.fresh_reads
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if self
                        .history_generation
                        .load(std::sync::atomic::Ordering::SeqCst)
                        == 0
                    {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                    return RpcReply::value(&history_seed(
                        self.history_generation
                            .load(std::sync::atomic::Ordering::SeqCst),
                    ));
                }
                if self.cached {
                    tokio::time::sleep(Duration::from_secs(15)).await;
                }
                RpcReply::value(&self.seed)
            }
            methods::CLOUD_WORKSPACE => RpcReply::value(&self.seed["workspace"]),
            // `FIXTURE_QUEUE=1`: three messages wait behind the running turn.
            methods::CLOUD_RPC if queued() && _params["method"] == methods::WATCH_QUEUE => {
                let id = _params["requestId"].clone();
                let rows = queue_rows();
                Ok(RpcReply::Stream(Box::pin(
                    stream::once(async move { json!({"id":id,"item":{"sessionId":"fixture-session","items":rows,"paused":false}}) })
                        .chain(stream::pending()),
                )))
            }
            methods::CLOUD_RPC => Ok(RpcReply::Stream(Box::pin(
                stream::once(async move {
                    json!({"id":_params["requestId"],"item":{"items":[],"paused":false}})
                })
                .chain(stream::pending()),
            ))),
            methods::CLOUD_EVENTS => {
                let (send, recv) = futures::channel::mpsc::unbounded();
                self.feeds.lock().unwrap().push(send);
                Ok(RpcReply::Stream(Box::pin(recv)))
            }
            methods::CLOUD_ENVIRONMENT if self.new_chat => {
                assert!(
                    self.calls
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|m| m == methods::CLOUD_CREATE),
                    "Draft must never allocate/connect before Send"
                );
                let first = stream::once(async {
                    json!({"workspaceId":"admitted-computer","state":"provisioning","statusCode":"allocating","runtimeStatus":"offline"})
                });
                // `FIXTURE_LAUNCH_FAILS=1`: the server reports the start failed.
                if std::env::var("FIXTURE_LAUNCH_FAILS").is_ok() {
                    let failed = stream::once(async {
                        tokio::time::sleep(Duration::from_millis(600)).await;
                        json!({"workspaceId":"admitted-computer","state":"failed","statusCode":"failed","message":"Computer start needs attention"})
                    });
                    return Ok(RpcReply::Stream(Box::pin(
                        first.chain(failed).chain(stream::pending()),
                    )));
                }
                Ok(RpcReply::Stream(Box::pin(first.chain(stream::pending()))))
            }
            methods::CLOUD_ENVIRONMENT => {
                panic!("Opening Cloud setup must never acquire an execution environment")
            }
            name if name.starts_with("Watch") => Ok(RpcReply::Stream(Box::pin(stream::pending()))),
            _ => RpcReply::value(&json!({})),
        }
    }
}
fn main() -> anyhow::Result<()> {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("Native fixture assertion failed: {info}");
        std::process::exit(1);
    }));
    let runtime = tokio::runtime::Runtime::new()?;
    let _enter = runtime.enter();
    let output = std::path::PathBuf::from(std::env::args().nth(1).expect("screenshot path"));
    let mode = std::env::args().nth(2).unwrap_or_default();
    let native_mixed = mode.starts_with("native-mixed");
    let shared_mixed = mode.starts_with("shared-mixed");
    let mixed = native_mixed || shared_mixed;
    let inspect = mode.ends_with("inspect");
    let open_history = mode.ends_with("history");
    let new_chat = mode == "new-chat" || (mixed && !inspect && !open_history);
    let reconnect = mode.starts_with("history-reconnect");
    let cached = mode == "cached" || reconnect;
    let seed = if new_chat {
        json!({"apiOrigin":"https://fixture-api.example.test","project":new_chat_context()["projects"][0]})
    } else if reconnect {
        history_seed(0)
    } else if cached || std::env::args().nth(2).as_deref() == Some("computer") {
        computer_seed()
    } else {
        json!({"apiOrigin":"https://fixture-api.example.test"})
    };
    let controller = Arc::new(Controller {
        calls: Default::default(),
        seed: seed.clone(),
        cached,
        mixed,
        reconnect,
        fresh_reads: Default::default(),
        history_generation: Default::default(),
        feeds: Default::default(),
        accounts: Default::default(),
        new_chat: new_chat || mixed,
        subscribed: mode.contains("first"),
        admitted: Default::default(),
        ready: Default::default(),
    });
    let client = zuse_rpc::memory_client(controller.clone());
    let temp = tempfile::tempdir()?;
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let mut settings = settings::UiSettings::default();
            if mixed {settings.sidebar_organization=settings::SidebarOrganization::ByProject;}
            settings::init(settings.clone(), temp.path(), cx);
            let fonts = typography::register_fonts(cx);
            typography::init(
                settings.ui_font_family.clone(),
                settings.ui_font_size,
                settings.terminal_font_family.clone(),
                settings.terminal_font_size,
                settings.code_font_family.clone(),
                settings.code_font_size,
                fonts,
                cx,
            );
            theme_library::init(temp.path().to_path_buf(), cx);
            appearance::init(
                appearance::AppearanceMode::Dark,
                settings.theme_selection,
                settings.accent,
                settings.surface,
                cx,
            );
            history::init(settings.git_history_columns,settings.git_history_column_widths,settings.git_history_column_order,settings.git_history_author_display,cx);
            composer::init(cx,settings.composer_send_behavior);terminal::panel::init(cx);app_menus::init(cx);
            let window = if shared_mixed {
                shared_mixed_window(client,cx)
            } else if native_mixed {
                native_mixed_window(client,new_chat_context(),mode.contains("offline"),mode.contains("first"),temp.path(),cx)
            } else if cached {
                cloud_saved_history_fixture(client, cx)
            } else {
                cloud_computer_fixture(client, seed, cx)
            };
            cx.activate(true);
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(1200))
                    .await;
                if reconnect {
                    let provisional=output.with_file_name("history-provisional.png");
                    window.update(cx,|_,window,cx|capture(window,cx,&provisional)).unwrap().unwrap();
                    assert!(!controller.calls.lock().unwrap().iter().any(|m|m==methods::CLOUD_RPC),"Provisional cache must not attach a command/guest channel");
                    cx.background_executor().timer(Duration::from_millis(3000)).await;
                    let scroll=std::process::Command::new("python3").args(["-c",SCROLL_HISTORY]).status().unwrap();assert!(scroll.success());
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    let anchor=output.with_file_name("history-scroll-before.png");window.update(cx,|_,window,cx|capture(window,cx,&anchor)).unwrap().unwrap();
                    let before=controller.fresh_reads.load(std::sync::atomic::Ordering::SeqCst);
                    assert!(before>0);controller.history_generation.store(1,std::sync::atomic::Ordering::SeqCst);
                    for feed in controller.feeds.lock().unwrap().iter(){
                        for kind in ["reconnecting","reset","connected"]{let _=feed.unbounded_send(json!({"accountId":"fixture-owner","organizationId":null,"data":{"type":kind}}));}
                    }
                    cx.background_executor().timer(Duration::from_millis(1600)).await;
                    window.update(cx,|_,window,cx|capture(window,cx,&output)).unwrap().unwrap();
                    assert!(controller.fresh_reads.load(std::sync::atomic::Ordering::SeqCst)>before,"A reset/reconnect must revalidate the visible saved conversation, not only account metadata");
                    let status=std::process::Command::new("python3").args(["-c",COMPARE_HISTORY]).arg(&anchor).arg(&output).status().unwrap();assert!(status.success(),"Revalidated history moved the reader's scroll anchor");
                    if mode.ends_with("revoke") {
                        for account in controller.accounts.lock().unwrap().iter(){let _=account.unbounded_send(json!({"user":{"id":"different-account","email":"other@example.test"},"orgId":null}));}
                        cx.background_executor().timer(Duration::from_millis(500)).await;
                        let revoked=output.with_file_name("history-after-account-change.png");window.update(cx,|_,window,cx|capture(window,cx,&revoked)).unwrap().unwrap();
                        let before_calls=controller.calls.lock().unwrap().len();let status=std::process::Command::new("python3").args(["-c",TYPE_AND_SEND]).status().unwrap();assert!(status.success());
                        cx.background_executor().timer(Duration::from_millis(300)).await;assert_eq!(controller.calls.lock().unwrap().len(),before_calls,"A different account must not issue actions into the old cached conversation");
                    }
                    let calls=controller.calls.lock().unwrap();assert!(!calls.iter().any(|m|matches!(m.as_str(),methods::CLOUD_ENVIRONMENT|methods::CLOUD_CREATE|methods::MUTATE|methods::QUEUE_MESSAGE)),"History reconnect must not wake, allocate or resend");
                    cx.update(|cx|cx.quit());return;
                }
                if mixed {
                    let status=std::process::Command::new("python3").args(["-c",MIXED_CLICK,if inspect{"inspect"}else{"click"}]).status().unwrap();assert!(status.success());
                    cx.background_executor().timer(Duration::from_millis(600)).await;
                }
                if new_chat {
                    let before = output.with_file_name(format!(
                        "{}-composer.png",
                        output.file_stem().unwrap().to_string_lossy()
                    ));
                    window
                        .update(cx, |_, window, cx| capture(window, cx, &before))
                        .unwrap()
                        .unwrap();
                    let status = std::process::Command::new("python3")
                        .args(["-c", TYPE_AND_SEND])
                        .status()
                        .expect("type into real composer");
                    assert!(status.success());
                    cx.background_executor()
                        .timer(Duration::from_millis(1600))
                        .await;
                }
                // `FIXTURE_CLICKS="x,y;x,y"` clicks screen points before the capture.
                for point in std::env::var("FIXTURE_CLICKS").unwrap_or_default().split(';').filter(|p| !p.is_empty()) {
                    let status = std::process::Command::new("python3").env("ZUSE_MIXED_CLOUD_CLICK", point).args(["-c", MIXED_CLICK, "click"]).status().unwrap();
                    assert!(status.success());
                    cx.background_executor().timer(Duration::from_millis(900)).await;
                }
                let actual_window=if mixed&&!inspect {cx.update(|cx|cx.active_window().unwrap_or(window))}else{window};
                let result = actual_window.update(cx, |_, window, cx| capture(window, cx, &output));
                result.expect("fixture window").expect("native render");
                if review() {
                    std::process::exit(0);
                }
                if new_chat {
                    cx.background_executor().timer(Duration::from_millis(300)).await;
                    actual_window.update(cx,|_,window,cx|capture(window,cx,&output)).unwrap().unwrap();
                }
                // The computer finishes starting and the agent replies: the
                // message keeps its place and the reply streams in below it.
                if new_chat && admit_provisioning() {
                    controller.ready.store(true, std::sync::atomic::Ordering::SeqCst);
                    for feed in controller.feeds.lock().unwrap().iter() {
                        let _ = feed.unbounded_send(json!({"accountId":"fixture-owner","organizationId":null,"data":{"type":"revalidate"}}));
                    }
                    cx.background_executor().timer(Duration::from_millis(2000)).await;
                    let reply = output.with_file_name(format!("{}-reply.png", output.file_stem().unwrap().to_string_lossy()));
                    actual_window.update(cx,|_,window,cx|capture(window,cx,&reply)).unwrap().unwrap();
                }
                if let Ok(coordinates) = std::env::var("ZUSE_MIXED_RETURN_CLICK") {
                    let before = controller.calls.lock().unwrap().iter().filter(|m| m.as_str() == methods::WATCH_QUEUE).count();
                    let status = std::process::Command::new("python3").env("ZUSE_MIXED_CLOUD_CLICK", coordinates).args(["-c", MIXED_CLICK, "click"]).status().unwrap();
                    assert!(status.success());
                    cx.background_executor().timer(Duration::from_millis(600)).await;
                    actual_window.update(cx, |_, window, cx| capture(window, cx, &output.with_file_name("mixed-return-local.png"))).unwrap().unwrap();
                    assert!(controller.calls.lock().unwrap().iter().filter(|m| m.as_str() == methods::WATCH_QUEUE).count() > before, "The local row must return through parent authority, not the cloud guest");
                }
                if !new_chat {
                    assert!(
                        !controller
                            .calls
                            .lock()
                            .unwrap()
                            .iter()
                            .any(|m| m == methods::CLOUD_ENVIRONMENT)
                    );
                }
                if new_chat && !mode.contains("first") {
                    assert!(
                        controller.admitted.lock().unwrap().is_some(),
                        "Exact cloud admission payload was not accepted"
                    );
                    let calls = controller.calls.lock().unwrap();
                    assert!(
                        mixed || !calls.iter().any(|m| m == methods::LOCAL_DEVICE),
                        "API-only cloud draft must not request a local engine identity"
                    );
                    let create_at=calls.iter().position(|method|method==methods::CLOUD_CREATE).unwrap();
                    assert!(calls[create_at+1..].iter().any(|method|method==methods::CLOUD_BILLING_ENTITLEMENTS),"Pending cloud view must use the controller account API without waking a runtime");
                    assert_eq!(
                        calls
                            .iter()
                            .filter(|m| m.as_str() == methods::CLOUD_CREATE)
                            .count(),
                        1,
                        "Cloud-only Send must admit exactly one task without a local device"
                    );
                    assert!(
                        !calls.iter().any(|m| m == methods::MUTATE),
                        "Cloud draft must never create a local chat"
                    );
                }
                if open_history {
                    let calls=controller.calls.lock().unwrap();assert!(calls.iter().any(|m|m==methods::CLOUD_HISTORY));assert!(!calls.iter().any(|m|matches!(m.as_str(),methods::CLOUD_CREATE|methods::MUTATE|methods::QUEUE_MESSAGE)),"Opening an existing Cloud conversation while local engine is offline must stay account-read-only");
                }
                if cached {
                    let calls = controller.calls.lock().unwrap();
                    assert_eq!(
                        calls
                            .iter()
                            .filter(|m| m.as_str() == methods::CLOUD_HISTORY)
                            .count(),
                        2,
                        "cached and fresh reads must both start"
                    );
                    assert!(
                        !calls.iter().any(|m| m == methods::CLOUD_RPC),
                        "provisional history must not attach guest RPC"
                    );
                }
                // Let native composer/cursor tasks release their input handles
                // before GPUI's leak-checking application teardown.
                let windows = cx.update(|cx| cx.windows());
                for handle in windows { let _ = handle.update(cx, |_, window, _| window.remove_window()); }
                cx.background_executor().timer(Duration::from_millis(100)).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    Ok(())
}

fn computer_seed() -> Value {
    let mut workspace = json!({"workspaceId":"fixture-computer","chatId":"fixture-thread","initialSessionId":"fixture-session","title":"Improve cloud startup","branch":"cloud/startup","state":"paused","desiredState":"paused","statusCode":"paused","revision":3});
    // Review captures show a running computer; the flow checks a sleeping one.
    if review() {
        // `FIXTURE_COMPUTER_STATE=starting` shows a computer still booting.
        // `FIXTURE_COMPUTER_STATE=starting|asleep` shows a booting or
        // sleeping computer instead of a running one.
        let (state, desired, code) = match std::env::var("FIXTURE_COMPUTER_STATE").as_deref() {
            Ok("starting") => ("provisioning", "ready", "allocating"),
            Ok("asleep") => ("paused", "paused", "paused"),
            _ => ("ready", "ready", "ready"),
        };
        workspace["state"] = json!(state);
        workspace["desiredState"] = json!(desired);
        workspace["statusCode"] = json!(code);
    }
    // `FIXTURE_CHECKOUT`: the chat's repository has a checkout on this Mac.
    if checkout_mode().is_some() {
        workspace["repositoryIdentity"] = json!("github.com/example/cloud-only");
    }
    let chat = json!({"id":"fixture-session","threadId":"fixture-thread","deviceId":"cloud:fixture-computer","title":"Improve cloud startup","archived":false,"cwd":".","branch":"cloud/startup","createdAt":"2026-10-30T12:00:00Z","config":{"harness":"codex","model":"gpt-5.5","sandbox":"workspace-write"}});
    let messages = json!([
        {"id":"fixture-user","role":"user","parts":[{"id":"fixture-text","kind":"text","text":"Inspect the startup path and reduce redundant repository fetches."}],"createdAt":1793361600000_i64,"deviceId":"cloud:fixture-computer"},
        {"id":"fixture-assistant","role":"assistant","parts":[{"id":"fixture-result","kind":"text","text":"The clone step fetched every branch on each start. It now does a shallow fetch of the task branch only, and the setup cache is keyed by the lockfile, so a warm start skips the install. Startup went from 48 s to 19 s in three runs."}],"createdAt":1793361601000_i64,"deviceId":"cloud:fixture-computer","status":"complete"}
    ]);
    // `FIXTURE_QUEUE=1`: a follow-up is running (the agent mid-turn).
    let (messages, admissions) = if queued() {
        let mut messages = messages.as_array().unwrap().clone();
        messages.push(json!({"id":"fixture-follow-up","role":"user","parts":[{"id":"fixture-follow-up-text","kind":"text","text":"Now make the warm start skip the install entirely."}],"createdAt":1793361650000_i64,"deviceId":"cloud:fixture-computer"}));
        messages.push(json!({"id":"fixture-working","role":"assistant","parts":[{"id":"fixture-working-text","kind":"text","text":"Checking which install steps the lockfile cache already covers."}],"createdAt":1793361651000_i64,"deviceId":"cloud:fixture-computer","status":"streaming"}));
        (
            json!(messages),
            json!([{"commandId":"follow-up","messageId":"fixture-follow-up","text":"Now make the warm start skip the install entirely.","executionState":"running","createdAt":1793361650000_i64}]),
        )
    } else {
        (messages, json!([]))
    };
    // `FIXTURE_COMPUTERS=running|all`: the agent ran tests on this Mac and is
    // building on the Linux box.
    let messages = if matches!(
        std::env::var("FIXTURE_COMPUTERS").as_deref(),
        Ok("running" | "all")
    ) {
        let mut messages = messages.as_array().unwrap().clone();
        messages.push(json!({"id":"fixture-computer-user","role":"user","parts":[{"id":"fixture-computer-ask","kind":"text","text":"Run the tests on my Mac, then build a release on the build box."}],"createdAt":1793361640000_i64,"deviceId":"cloud:fixture-computer"}));
        messages.push(json!({"id":"fixture-computer-run","role":"assistant","parts":[
            {"id":"fixture-tool-tests","kind":"tool","resolved":true,"call":{"kind":"mcp","server":"zuse","tool":"run_on_computer","input":{"command":"cargo test --workspace"}}},
            {"id":"fixture-tool-tests-text","kind":"text","text":"All 214 tests pass on your Mac. Starting the release build on the build box."},
            {"id":"fixture-tool-build","kind":"tool","resolved":false,"call":{"kind":"mcp","server":"zuse","tool":"run_on_computer","input":{"command":"cargo build --release","device_id":"fixture-linux"}}}
        ],"createdAt":1793361641000_i64,"deviceId":"cloud:fixture-computer","status":"streaming"}));
        json!(messages)
    } else {
        messages
    };
    let queue = if queued() { queue_rows() } else { json!([]) };
    json!({"apiOrigin":"https://fixture-api.example.test","accountId":"fixture-owner","workspace":workspace,"commandDeliveryMode":"server-admitted-v2","commandAdmissionVersion":2,"taskCommandCanEdit":true,"commandAdmissions":admissions,"head":{"schemaVersion":1,"payloadFormat":"zuse-transcript-v1","workspaceId":"fixture-computer","sessionId":"fixture-session","runtimeGeneration":1,"cursor":{"epoch":"fixture-checkpoint","version":4},"chat":chat,"conversations":[chat],"transcript":{"reset":messages},"queue":queue,"queuePaused":false,"olderMessageSequence":null}})
}

/// Three messages queued behind the running turn, in server order.
fn queue_rows() -> Value {
    json!(["Then update the setup docs", "yo", "Also check the cache key"]
        .iter()
        .enumerate()
        .map(|(ix, text)| json!({"id":format!("queued-{ix}"),"messageId":format!("queued-message-{ix}"),"revision":1,"position":ix+1,"text":text,"attachments":[],"holdForTurnEnd":true,"issuedBy":"cloud-authority","issuedAt":1793361700000_i64 + ix as i64}))
        .collect::<Vec<_>>())
}
fn queued() -> bool {
    std::env::var("FIXTURE_QUEUE").is_ok()
}
fn collide() -> bool {
    std::env::var("FIXTURE_COLLIDE").is_ok()
}
fn new_chat_context() -> Value {
    let mut context = new_chat_context_base();
    if collide() {
        let base = context["projects"][0].clone();
        let project = |id: &str, name: &str, repo: &str| {
            let mut row = base.clone();
            row["projectId"] = json!(id);
            row["displayName"] = json!(name);
            row["repositoryIdentity"] = json!(format!("github.com/{repo}"));
            row
        };
        context["projects"] = json!([
            project("cloud-only-project", "zuse", "opcodehq/zuse"),
            project("fork-project", "zuse", "swarajbachu/zuse"),
            project("review-project", "zuse-review", "opcodehq/zuse-review"),
        ]);
    }
    context
}
fn new_chat_context_base() -> Value {
    let catalog = json!({"providers":[{"id":"codex","name":"Codex","defaultModel":"gpt-5.5","models":[{"id":"gpt-5.5","label":"GPT-5.5"}]}]});
    json!({"accountId":"fixture-owner","ownerId":"fixture-owner","organizationId":null,"apiOrigin":"https://fixture-api.example.test","projects":[{"projectId":"cloud-only-project","accountId":"fixture-owner","displayName":"opcodehq/web","repositoryIdentity":"github.com/example/cloud-only","defaultBranch":"main","providerId":"boxd","supportsModelConfiguration":true}],"agentConnections":[{"connectionId":"fixture-codex","agent":"codex","state":"connected","accountLabel":"Fixture Codex"}],"agentCatalog":catalog,"providerAgentCatalogs":{"boxd":catalog},"computerCapabilities":{"defaultProvider":"boxd","providers":[{"id":"boxd","configured":true,"managedBaseReady":true,"commandAdmissionVersion":2,"preservesDisk":true,"preservesMemory":true,"supportsSnapshots":true,"supportsDocker":false,"sizes":[{"id":"default","label":"Standard","vcpus":4,"memoryMib":16384,"diskGib":32}]}],"computerSizes":[{"id":"default","label":"Standard · 4 vCPU · 16 GB","vcpus":4,"memoryMib":16384,"machineRate":4,"available":true},{"id":"large","label":"Large · 8 vCPU · 32 GB","vcpus":8,"memoryMib":32768,"machineRate":8,"available":false,"requiresPlan":"cloud-max"}]}})
}

const TYPE_AND_SEND: &str = r#"
import ctypes as c,time
x=c.CDLL('libX11.so.6');t=c.CDLL('libXtst.so.6')
x.XOpenDisplay.argtypes=[c.c_char_p];x.XOpenDisplay.restype=c.c_void_p
d=x.XOpenDisplay(None);assert d
x.XStringToKeysym.argtypes=[c.c_char_p];x.XStringToKeysym.restype=c.c_ulong
x.XKeysymToKeycode.argtypes=[c.c_void_p,c.c_ulong];x.XKeysymToKeycode.restype=c.c_uint
x.XFlush.argtypes=[c.c_void_p]
t.XTestFakeKeyEvent.argtypes=[c.c_void_p,c.c_uint,c.c_int,c.c_ulong]
def key(name,down):
    code=x.XKeysymToKeycode(d,x.XStringToKeysym(name.encode()));assert code
    t.XTestFakeKeyEvent(d,code,down,0)
for char in 'Fix the flaky sync test':
    if char.isupper():key('Shift_L',1)
    name='space' if char==' ' else 'minus' if char=='-' else char.lower()
    key(name,1);key(name,0)
    if char.isupper():key('Shift_L',0)
key('Return',1);key('Return',0);x.XFlush(d)
"#;

fn admit() -> bool {
    std::env::var("FIXTURE_ADMIT").is_ok()
}
fn admit_provisioning() -> bool {
    std::env::var("FIXTURE_ADMIT").as_deref() == Ok("provisioning")
}
/// The admitted chat while its computer starts: the launch message is
/// admitted, and nothing has run yet.
fn provisioning_history() -> Value {
    let mut history = admitted_history();
    history["workspace"] = admitted_workspace();
    history["head"]["transcript"]["reset"] = json!([]);
    history["head"]["cursor"]["version"] = json!(1);
    history["commandAdmissions"][0]["executionState"] = json!("admitted");
    history
}
fn admitted_history() -> Value {
    let mut workspace = admitted_workspace();
    workspace["state"] = json!("ready");
    workspace["desiredState"] = json!("ready");
    workspace["statusCode"] = json!("ready");
    workspace["revision"] = json!(2);
    let chat = json!({"id":"admitted-session","threadId":"admitted-thread","deviceId":"cloud:admitted-computer","title":"Fix the flaky sync test","archived":false,"cwd":".","branch":"task/cloud-only","createdAt":"2026-11-20T10:50:00Z","config":{"harness":"codex","model":"gpt-5.5","sandbox":"workspace-write"}});
    let messages = json!([
        {"id":"runtime-user","role":"user","parts":[{"id":"runtime-user-text","kind":"text","text":"Fix the flaky sync test"}],"createdAt":1793361600000_i64,"deviceId":"cloud:admitted-computer"},
        {"id":"runtime-reply","role":"assistant","parts":[{"id":"runtime-reply-text","kind":"text","text":"The sync test raced the file watcher. It now waits for the watcher's ready signal before writing, and passed 50 runs in a row."}],"createdAt":1793361607000_i64,"deviceId":"cloud:admitted-computer","status":"complete","durationMs":7000}
    ]);
    json!({"apiOrigin":"https://fixture-api.example.test","accountId":"fixture-owner","workspace":workspace,"commandDeliveryMode":"server-admitted-v2","commandAdmissionVersion":2,"taskCommandCanEdit":true,
        "commandAdmissions":[{"commandId":"launch:admitted-computer","messageId":"admitted-first","text":"Fix the flaky sync test","executionState":"running","createdAt":1793361600000_i64}],
        "head":{"schemaVersion":1,"payloadFormat":"zuse-transcript-v1","workspaceId":"admitted-computer","sessionId":"admitted-session","runtimeGeneration":1,"cursor":{"epoch":"fixture-checkpoint","version":4},"chat":chat,"conversations":[chat],"transcript":{"reset":messages},"queue":[],"queuePaused":false,"olderMessageSequence":null}})
}
fn admitted_workspace() -> Value {
    json!({"workspaceId":"admitted-computer","chatId":"admitted-thread","initialSessionId":"admitted-session","state":"provisioning","desiredState":"ready","statusCode":"allocating","title":"Fix the flaky sync test","branch":"task/cloud-only","createdAt":1793361600000_i64})
}

fn capture(
    window: &mut gpui::Window,
    cx: &mut gpui::App,
    output: &std::path::Path,
) -> anyhow::Result<()> {
    window.draw(cx).clear();
    window.present_if_needed();
    match window.render_to_image() {
        Ok(image) => image.save(output)?,
        Err(_) => {
            let status = std::process::Command::new("import")
                .args(["-window", "root"])
                .arg(output)
                .status()?;
            anyhow::ensure!(status.success(), "X11 native screenshot failed");
        }
    }
    Ok(())
}

fn native_mixed_window(
    client: zuse_rpc::RpcClient,
    context: Value,
    offline: bool,
    subscribed: bool,
    data: &std::path::Path,
    cx: &mut gpui::App,
) -> gpui::AnyWindowHandle {
    use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, point, px, size};
    let state=cx.new(|_|{
        let mut s=state::AppState::new();
        s.connection=if offline{zuse_protocol::view::ConnectionStatus::Failed("Local engine is offline".into())}else{zuse_protocol::view::ConnectionStatus::Ready};
        s.workspace_scope=Some(zuse_protocol::WorkspaceScope::Local);s.local_device_id=Some("fixture-local".into());s.devices=vec![serde_json::from_value(json!({"id":"fixture-local","name":"Offline local computer","platform":std::env::consts::OS,"lastSeenAt":null})).unwrap()];s.auto_selected=true;s.spaces_synced=true;s.chats_synced=true;
        s.apply_cloud_account(json!({"user":{"id":"fixture-owner","email":"alex@example.test"},"orgId":null}));
        s.spaces=vec![serde_json::from_value(if collide() {
            json!({"id":"mixed-space","deviceId":"fixture-local","path":"/Users/alex/code/zuse","remoteRepository":"https://github.com/opcodehq/zuse.git","createdAt":"2026-10-30T12:00:00Z"})
        } else {
            json!({"id":"mixed-space","deviceId":"fixture-local","path":"/Users/alex/code/web","remoteRepository":"github.com/example/cloud-only","createdAt":"2026-10-30T12:00:00Z"})
        }).unwrap()];
        s.chats=vec![serde_json::from_value(json!({"id":"mixed-local-chat","deviceId":"fixture-local","spaceId":"mixed-space","title":"Tidy the settings layout","archived":false,"createdAt":"2026-10-30T12:00:00Z","config":{"harness":"codex","model":"gpt-5.5","sandbox":"workspace-write"}})).unwrap()];s.selected_chat=Some("mixed-local-chat".into());s.selected_space=Some("mixed-space".into());
        let mut workspace=computer_seed()["workspace"].clone();workspace["projectId"]=json!("cloud-only-project");
        assert!(s.fixture_cloud_sidebar_catalog(client,&context,&json!({"accountId":"fixture-owner","organizationId":null,"workspaces":[workspace]})));
        if subscribed {s.fixture_cloud_subscription(true);}
        s
    });
    let boot = EngineBootConfig {
        target: None,
        data_dir: data.to_path_buf(),
        ipc_port: 0,
        edge_url: String::new(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: HarnessId::Codex,
    };
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1160.), px(850.)),
                ))),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| shell::Shell::new(state, boot, cx)),
        )
        .unwrap();
    window.into()
}
const MIXED_CLICK: &str = r#"
import ctypes as c,sys,time,os
x=c.CDLL('libX11.so.6');t=c.CDLL('libXtst.so.6')
x.XOpenDisplay.argtypes=[c.c_char_p];x.XOpenDisplay.restype=c.c_void_p;d=x.XOpenDisplay(None);assert d
x.XFlush.argtypes=[c.c_void_p];t.XTestFakeMotionEvent.argtypes=[c.c_void_p,c.c_int,c.c_int,c.c_int,c.c_ulong];t.XTestFakeButtonEvent.argtypes=[c.c_void_p,c.c_uint,c.c_int,c.c_ulong]
a,b=map(int,os.environ.get('ZUSE_MIXED_CLOUD_CLICK','250,230').split(','));t.XTestFakeMotionEvent(d,-1,a,b,0);x.XFlush(d);time.sleep(.25)
if sys.argv[1]=='click':
 t.XTestFakeButtonEvent(d,1,1,0);t.XTestFakeButtonEvent(d,1,0,0);x.XFlush(d)
"#;

fn history_seed(generation: usize) -> Value {
    let mut seed = computer_seed();
    let mut messages = vec![];
    for i in 0..120 + generation {
        messages.push(json!({"id":format!("history-{i}"),"role":"assistant","parts":[{"id":format!("text-{i}"),"kind":"text","text":format!("HISTORY_ROW_{i:03} — Saved account conversation. Keep this reading position while reconnecting. The computer remains asleep and the original command is never replayed.")}],"createdAt":1793361600000_i64+i as i64,"deviceId":"cloud:fixture-computer","status":"complete"}));
    }
    seed["head"]["transcript"]["reset"] = json!(messages);
    seed["head"]["cursor"]["version"] = json!(4 + generation);
    seed
}
const SCROLL_HISTORY: &str = r#"
import ctypes as c,time
x=c.CDLL('libX11.so.6');t=c.CDLL('libXtst.so.6');x.XOpenDisplay.argtypes=[c.c_char_p];x.XOpenDisplay.restype=c.c_void_p;d=x.XOpenDisplay(None);assert d
x.XFlush.argtypes=[c.c_void_p];t.XTestFakeMotionEvent.argtypes=[c.c_void_p,c.c_int,c.c_int,c.c_int,c.c_ulong];t.XTestFakeButtonEvent.argtypes=[c.c_void_p,c.c_uint,c.c_int,c.c_ulong]
t.XTestFakeMotionEvent(d,-1,800,400,0)
for _ in range(14):t.XTestFakeButtonEvent(d,4,1,0);t.XTestFakeButtonEvent(d,4,0,0)
x.XFlush(d)
"#;
const COMPARE_HISTORY: &str = r#"
import subprocess,sys
result=subprocess.run(['compare','-metric','AE','-extract','650x420+430+180',sys.argv[1],sys.argv[2],'null:'],capture_output=True)
assert result.returncode==0,'visible transcript anchor changed across account feed reconnect: '+result.stderr.decode()
"#;

fn mixed_space() -> Value {
    json!({"id":"mixed-space","deviceId":"fixture-controller","path":"/Users/alex/code/web","remoteRepository":"github.com/example/cloud-only","createdAt":"2026-10-30T12:00:00Z"})
}
fn mixed_chat() -> Value {
    json!({"id":"mixed-local-chat","deviceId":"fixture-controller","spaceId":"mixed-space","cwd":"/Users/alex/code/web","title":"Tidy the settings layout","archived":false,"createdAt":"2026-10-30T12:00:00Z","config":{"harness":"codex","model":"gpt-5.5","sandbox":"workspace-write"}})
}
fn shared_mixed_window(client: zuse_rpc::RpcClient, cx: &mut gpui::App) -> gpui::AnyWindowHandle {
    use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, point, px, size};
    zuse_viewport::initialize(cx);
    let client = Arc::new(client);
    let (requests, mut outgoing) = futures::channel::mpsc::unbounded::<Value>();
    let (events, incoming) = futures::channel::mpsc::unbounded::<Value>();
    let replies = events.clone();
    tokio::spawn(async move {
        while let Some(frame) = outgoing.next().await {
            if frame["ui"] == "loadDrafts" {
                let _ = replies.unbounded_send(
                    json!({"browserDrafts":{"deviceId":frame["deviceId"],"journal":{}}}),
                );
                continue;
            }
            let Some(method) = frame["method"].as_str().map(str::to_owned) else {
                continue;
            };
            let id = frame["id"].as_u64().unwrap();
            let params = frame["params"].clone();
            let client = client.clone();
            let send = replies.clone();
            tokio::spawn(async move {
                match client.forward(&method, params).await {
                    Ok(mut stream) => {
                        while let Some(mut reply) = stream.recv().await {
                            reply.id = id;
                            let _ = send.unbounded_send(serde_json::to_value(reply).unwrap());
                        }
                    }
                    Err(error) => {
                        let _ = send.unbounded_send(json!({"id":id,"err":error.to_string()}));
                    }
                }
            });
        }
    });
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1160.), px(850.)),
                ))),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    zuse_viewport::WorkspaceView::new(
                        zuse_viewport::Transport {
                            requests,
                            events: incoming,
                        },
                        window,
                        cx,
                    )
                })
            },
        )
        .unwrap();
    events
        .unbounded_send(json!({"preferences":{"sidebarView":"projects"}}))
        .unwrap();
    events
        .unbounded_send(json!({"connection":"connected","epoch":1}))
        .unwrap();
    window.into()
}

/// `FIXTURE_REVIEW=1`: capture the real app for review screenshots without
/// the flow assertions the other modes check.
fn review() -> bool {
    std::env::var("FIXTURE_REVIEW").is_ok()
}

/// `FIXTURE_COMPUTERS=idle|pending|running|all`: the chat runs on this Mac
/// by default; a Linux box is online too; a PC is offline. `running` also
/// shows the agent's commands in the transcript. `pending` adds an approval, `running`
/// a command with output.
fn computers_reply(method: &str, params: &Value) -> Option<Result<RpcReply, RpcError>> {
    let mode = std::env::var("FIXTURE_COMPUTERS").ok()?;
    let pending = matches!(mode.as_str(), "pending" | "all");
    let running = matches!(mode.as_str(), "running" | "all");
    let value = match (method, params["action"].as_str()) {
        ("ComputerBridgeStatus", _) => json!({"state":"online","deviceId":"fixture-mac",
            "name":"Alex’s MacBook Pro","error":null,"running":usize::from(running)}),
        ("CloudComputerControl", Some("list")) => json!([
            {"deviceId":"fixture-linux","name":"Build box","platform":"linux","connected":true,"alwaysAllowed":false},
            {"deviceId":"fixture-pc","name":"Studio PC","platform":"windows","connected":false,"alwaysAllowed":false},
            {"deviceId":"fixture-mac","name":"Alex’s MacBook Pro","platform":"macos","connected":true,"alwaysAllowed":false}
        ]),
        ("CloudComputerControl", Some("pending")) if pending => json!([
            {"id":"fixture-request","deviceId":"fixture-mac","computerName":"Alex’s MacBook Pro","osUser":"alex",
             "shell":"/bin/zsh","cwd":"/Users/alex/code/fieldnotes","command":"cargo test --workspace",
             "timeoutMs":600000,"createdAt":1793361700000_i64}
        ]),
        ("CloudComputerControl", Some("results")) if running => json!([
            {"id":"fixture-run","deviceId":"fixture-linux","computer":"Build box","command":"cargo build --release",
             "cwd":"/home/alex/code/fieldnotes","timeoutMs":1800000,"createdAt":1793361650000_i64,"state":"running",
             "result":{"status":"Running","output":"   Compiling serde v1.0.228\n   Compiling tokio v1.48.0\n   Compiling fieldnotes-core v0.4.0 (/home/alex/code/fieldnotes/core)\n   Compiling fieldnotes v0.4.0 (/home/alex/code/fieldnotes)"}}
        ]),
        ("CloudComputerControl", Some("pending" | "results")) => json!([]),
        ("CloudComputerControl", Some("primary")) => json!({"deviceId":"fixture-mac"}),
        ("ClaimComputerPrimary", _) => json!({"deviceId":"fixture-mac"}),
        ("CloudComputerControl", _) => Value::Null,
        _ => return None,
    };
    Some(RpcReply::value(&value))
}

fn checkout_mode() -> Option<String> {
    std::env::var("FIXTURE_CHECKOUT").ok()
}

/// `FIXTURE_CHECKOUT=off|synced|paused|behind`: this chat in the user's own
/// checkout of its repository on this Mac.
fn checkout_reply(method: &str, params: &Value) -> Option<Result<RpcReply, RpcError>> {
    let mode = checkout_mode()?;
    if !matches!(
        method,
        methods::CLOUD_CHECKOUT_SYNC_STATUS
            | methods::CLOUD_CHECKOUT_SYNC_CONFIGURE
            | methods::CLOUD_CHECKOUT_SYNC_RESUME
    ) {
        return None;
    }
    let base = json!({"workspaceId":params["workspaceId"],"chatId":params["chatId"],
        "lastSyncedAt":1793361700000_i64,"files":1284,"error":null,"notice":null});
    let mut status = base.clone();
    let (enabled, state, error) = match mode.as_str() {
        "synced" => (true, "synced", None),
        "paused" => (
            true,
            "paused",
            Some("Files in this folder were edited here, so sync paused to keep them."),
        ),
        "behind" => (
            true,
            "behind",
            Some("The cloud computer is asleep. Its changes arrive when it wakes."),
        ),
        _ => (false, "off", None),
    };
    status["enabled"] = json!(enabled);
    status["state"] = json!(state);
    status["error"] = json!(error);
    status["localPath"] = if enabled {
        json!("/Users/alex/code/web")
    } else {
        Value::Null
    };
    Some(RpcReply::value(
        &json!({"accountId":"fixture-owner","status":status}),
    ))
}
