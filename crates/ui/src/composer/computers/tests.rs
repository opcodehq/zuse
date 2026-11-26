use super::*;

type Requests = tokio::sync::mpsc::Receiver<String>;
type Replies = tokio::sync::mpsc::Sender<String>;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// A composer bound to cloud chat `session` of computer `task`.
fn cloud_composer(
    cx: &mut gpui::TestAppContext,
    cloud: bool,
) -> (Entity<Composer>, Entity<AppState>, Requests, Replies) {
    cx.update(|cx| cx.set_global(Theme::dark()));
    let (send, receive) = tokio::sync::mpsc::channel(64);
    let (replies_tx, replies) = tokio::sync::mpsc::channel(64);
    let controller =
        crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, replies));
    let state = cx.new(|_| {
        let mut state = crate::state::AppState::new();
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
        state.selected_chat = Some("session".into());
        state
    });
    let composer = cx.new(|cx| Composer::new(state.clone(), cx));
    composer.update(cx, |c, cx| {
        if cloud {
            let target = crate::state::CloudTarget {
                controller,
                subject: "owner".into(),
                organization: None,
                workspace: "task".into(),
                initial_session: "session".into(),
            };
            c.set_cloud_task(target, json!({"agent":"codex","model":"gpt"}), cx);
        }
        c.sync_computers(cx);
    });
    (composer, state, receive, replies_tx)
}

fn settle(cx: &mut gpui::TestAppContext, runtime: &tokio::runtime::Runtime) {
    for _ in 0..8 {
        cx.run_until_parked();
        runtime.block_on(async { tokio::task::yield_now().await });
    }
    cx.run_until_parked();
}

fn next(requests: &mut Requests) -> Option<Value> {
    requests
        .try_recv()
        .ok()
        .map(|line| serde_json::from_str(line.trim()).expect("request frame"))
}

/// Answer requests until none are left; returns what was asked.
fn serve(
    cx: &mut gpui::TestAppContext,
    runtime: &tokio::runtime::Runtime,
    requests: &mut Requests,
    replies: &Replies,
    answer: impl Fn(&str, &Value) -> Result<Value, &'static str>,
) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        settle(cx, runtime);
        let Some(request) = next(requests) else {
            return seen;
        };
        let method = request["method"].as_str().unwrap_or_default().to_owned();
        let label = if method == "CloudComputerControl" {
            request["params"]["action"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        } else {
            method.clone()
        };
        let frame = match answer(&label, &request["params"]) {
            Ok(value) => json!({"id":request["id"],"ok":value}),
            Err(error) => json!({"id":request["id"],"err":error}),
        };
        runtime
            .block_on(replies.send(format!("{frame}\n")))
            .unwrap();
        seen.push(request);
    }
}

fn account(label: &str, _: &Value) -> Result<Value, &'static str> {
    Ok(match label {
        "ComputerBridgeStatus" => {
            json!({"state":"online","deviceId":"mac","name":"Alex's Mac","error":null,"running":0})
        }
        "list" => json!([
            {"deviceId":"box","name":"Build box","platform":"linux","connected":false,"alwaysAllowed":false},
            {"deviceId":"mac","name":"Zed's Mac","platform":"macos","connected":true,"alwaysAllowed":false},
            {"deviceId":"other-mac","name":"Alex's Mac","platform":"macos","connected":true,"alwaysAllowed":false}
        ]),
        "attached" => {
            json!([{"deviceId":"mac","role":"primary"},{"deviceId":"box","role":"secondary"}])
        }
        "pending" => json!([
            {"id":"r1","deviceId":"mac","computerName":"Zed's Mac","osUser":"alex","shell":"/bin/sh",
             "cwd":"/Users/alex","command":"cargo test","timeoutMs":600000,"createdAt":1},
            {"id":"r2","deviceId":"box","role":"secondary","computerName":"Build box","osUser":"alex","shell":"/bin/sh",
             "cwd":"/Users/alex","command":"ls","timeoutMs":600000,"createdAt":2}
        ]),
        "results" => json!([
            {"id":"run","deviceId":"mac","computer":"Zed's Mac","command":"make","cwd":"/","timeoutMs":600000,
             "createdAt":0,"state":"running","result":{"status":"Running","output":"a\nb"}}
        ]),
        _ => Value::Null,
    })
}

#[gpui::test]
fn the_chip_belongs_to_cloud_chats_only(cx: &mut gpui::TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (local, _, mut requests, _) = cloud_composer(cx, false);
    settle(cx, &runtime);
    local.update(cx, |c, cx| {
        let theme = Theme::of(cx).clone();
        assert!(c.render_computers_chip(&theme, cx).is_none());
    });
    assert!(
        next(&mut requests).is_none(),
        "a local chat asks for nothing"
    );
    let (cloud, _, mut requests, replies) = cloud_composer(cx, true);
    serve(cx, &runtime, &mut requests, &replies, account);
    cloud.update(cx, |c, cx| {
        let theme = Theme::of(cx).clone();
        assert!(c.render_computers_chip(&theme, cx).is_some());
    });
}

#[gpui::test]
fn this_computer_is_first_by_enrollment_not_by_name(cx: &mut gpui::TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (composer, _, mut requests, replies) = cloud_composer(cx, true);
    let asked = serve(cx, &runtime, &mut requests, &replies, account);
    assert!(asked.iter().all(|r| r["method"] == "ComputerBridgeStatus"
        || (r["params"]["workspaceId"] == "task"
            && r["params"]["chatId"] == "session"
            && r["params"]["expectedAccountId"] == "owner")));
    composer.read_with(cx, |c, _| {
        let rows = c.computer_rows();
        let order: Vec<(&str, bool, bool, bool)> = rows
            .iter()
            .map(|r| {
                (
                    r.computer.device_id.as_str(),
                    r.this_computer,
                    r.primary,
                    r.secondary,
                )
            })
            .collect();
        // "Alex's Mac" shares the connector's name but is another enrollment.
        assert_eq!(
            order,
            vec![
                ("mac", true, true, false),
                ("box", false, false, true),
                ("other-mac", false, false, false)
            ]
        );
        assert_eq!(c.computers.pending.len(), 2);
    });
}

#[gpui::test]
fn attach_names_the_chat_and_device_and_undoes_a_refusal(cx: &mut gpui::TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (composer, _, mut requests, replies) = cloud_composer(cx, true);
    serve(cx, &runtime, &mut requests, &replies, account);
    composer.update(cx, |c, cx| c.set_computer_attached("other-mac", true, cx));
    composer.read_with(cx, |c, _| {
        assert!(c.computers.secondary("other-mac"), "optimistic");
    });
    let asked = serve(cx, &runtime, &mut requests, &replies, |label, params| {
        if label == "attach" {
            Err("computer offline")
        } else {
            account(label, params)
        }
    });
    let attach = asked
        .iter()
        .find(|r| r["params"]["action"] == "attach")
        .expect("attach sent");
    assert_eq!(attach["params"]["deviceId"], "other-mac");
    assert_eq!(attach["params"]["workspaceId"], "task");
    assert_eq!(attach["params"]["chatId"], "session");
    composer.read_with(cx, |c, _| {
        let ids: Vec<&str> = c
            .computers
            .attached
            .iter()
            .map(|a| a.device_id.as_str())
            .collect();
        assert_eq!(ids, vec!["mac", "box"]);
        assert!(c.computers.error.is_some());
    });
}

#[gpui::test]
fn approval_choices_send_their_decision_and_show_the_next_request(cx: &mut gpui::TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (composer, _, mut requests, replies) = cloud_composer(cx, true);
    serve(cx, &runtime, &mut requests, &replies, account);
    composer.update(cx, |c, cx| {
        let theme = Theme::of(cx).clone();
        assert!(c.render_computer_approval(&theme, cx).is_some());
        c.decide_computer_request("r1", "session", cx);
        let waiting: Vec<_> = c
            .computers
            .pending
            .iter()
            .filter(|p| !c.computers.decided.contains(&p.id))
            .map(|p| p.id.clone())
            .collect();
        assert_eq!(waiting, vec!["r2".to_owned()], "the next request at once");
    });
    let asked = serve(cx, &runtime, &mut requests, &replies, account);
    let decide = asked
        .iter()
        .find(|r| r["params"]["action"] == "decide")
        .expect("decision sent");
    assert_eq!(decide["params"]["requestId"], "r1");
    assert_eq!(decide["params"]["decision"], "session");
}

#[gpui::test]
fn stop_cancels_the_running_command(cx: &mut gpui::TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (composer, _, mut requests, replies) = cloud_composer(cx, true);
    serve(cx, &runtime, &mut requests, &replies, account);
    composer.update(cx, |c, cx| {
        let theme = Theme::of(cx).clone();
        assert_eq!(c.computer_tray_rows(&theme, cx).len(), 1, "one running row");
        c.stop_computer_request("run", cx);
    });
    let asked = serve(cx, &runtime, &mut requests, &replies, account);
    let cancel = asked
        .iter()
        .find(|r| r["params"]["action"] == "cancel")
        .expect("cancel sent");
    assert_eq!(cancel["params"]["requestId"], "run");
    assert_eq!(cancel["params"]["chatId"], "session");
}

#[gpui::test]
fn replies_for_a_previous_chat_are_ignored(cx: &mut gpui::TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (composer, state, mut requests, replies) = cloud_composer(cx, true);
    settle(cx, &runtime);
    // The first read is in flight when the owner switches chats.
    let first = next(&mut requests).expect("status read");
    state.update(cx, |s, _| s.selected_chat = Some("elsewhere".into()));
    composer.update(cx, |c, cx| c.sync_computers(cx));
    runtime
        .block_on(replies.send(format!(
            "{}\n",
            json!({"id":first["id"],"ok":account("ComputerBridgeStatus",&Value::Null).unwrap()})
        )))
        .unwrap();
    // Answer the old chat's remaining reads too; none may land.
    let asked = serve(cx, &runtime, &mut requests, &replies, |label, params| {
        if params["chatId"] == "session" {
            account(label, params)
        } else {
            Ok(json!([]))
        }
    });
    assert!(asked.iter().any(|r| r["params"]["chatId"] == "elsewhere"));
    composer.read_with(cx, |c, _| {
        assert!(c.computers.attached.is_empty());
        assert!(c.computers.pending.is_empty());
        assert!(c.computers.runs.is_empty());
    });
}

#[gpui::test]
fn sends_claim_this_computer_as_primary_before_the_message(cx: &mut gpui::TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (composer, _, mut requests, replies) = cloud_composer(cx, true);
    serve(cx, &runtime, &mut requests, &replies, account);
    let claim = composer.read_with(cx, |c, cx| {
        c.primary_claim(&json!({"sessionId":"session"}), cx)
    });
    let (target, chat) = claim.expect("a known connector claims on send");
    assert_eq!(
        (target.workspace.as_str(), chat.as_str()),
        ("task", "session")
    );
    let executor = cx.background_executor.clone();
    let done = std::rc::Rc::new(std::cell::Cell::new(false));
    let finished = done.clone();
    cx.spawn(async move |_| {
        claim_primary(target, chat, executor).await;
        finished.set(true);
    })
    .detach();
    let asked = serve(cx, &runtime, &mut requests, &replies, |_, _| {
        Err("this computer's connector is off")
    });
    assert_eq!(asked[0]["method"], "ClaimComputerPrimary");
    assert_eq!(asked[0]["params"]["chatId"], "session");
    assert_eq!(asked[0]["params"]["expectedAccountId"], "owner");
    assert!(done.get(), "a refused claim never holds the send");
    // Without a connector answer (older engine) nothing is claimed.
    let (fresh, _, _, _) = cloud_composer(cx, true);
    assert!(
        fresh
            .read_with(cx, |c, cx| c
                .primary_claim(&json!({"sessionId":"session"}), cx))
            .is_none()
    );
}
