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
            c.set_cloud_task(
                target,
                json!({"agent":"codex","model":"gpt-6.1-sol","runtimeMode":"full-access"}),
                cx,
            );
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
        "primary" => json!({"deviceId":"mac"}),
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
fn approval_choices_send_their_decision_and_show_the_next_request(cx: &mut gpui::TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (composer, _, mut requests, replies) = cloud_composer(cx, true);
    serve(cx, &runtime, &mut requests, &replies, account);
    composer.update(cx, |c, cx| {
        assert_eq!(
            c.computer_request_waiting().map(|p| p.id.as_str()),
            Some("r1")
        );
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
    let first = next(&mut requests).expect("first read");
    state.update(cx, |s, _| s.selected_chat = Some("elsewhere".into()));
    composer.update(cx, |c, cx| c.sync_computers(cx));
    runtime
        .block_on(replies.send(format!(
            "{}\n",
            json!({"id":first["id"],"ok":account("list",&Value::Null).unwrap()})
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
        assert!(c.computers.computers.is_empty());
        assert!(c.computers.pending.is_empty());
        assert!(c.computers.runs.is_empty());
    });
}

#[gpui::test]
fn a_claim_names_its_message_and_a_refusal_holds_nothing(cx: &mut gpui::TestAppContext) {
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
        claim_primary(target, chat, Some("admitted-command".into()), executor).await;
        finished.set(true);
    })
    .detach();
    let asked = serve(cx, &runtime, &mut requests, &replies, |_, _| {
        Err("this computer's connector is off")
    });
    assert_eq!(asked[0]["method"], "ClaimComputerPrimary");
    assert_eq!(asked[0]["params"]["chatId"], "session");
    assert_eq!(asked[0]["params"]["expectedAccountId"], "owner");
    assert_eq!(asked[0]["params"]["commandId"], "admitted-command");
    assert!(done.get(), "a refused claim never holds the send");
    // A send right after opening claims too; it never waits for the first
    // connector status read.
    let (fresh, _, _, _) = cloud_composer(cx, true);
    assert!(
        fresh
            .read_with(cx, |c, cx| c
                .primary_claim(&json!({"sessionId":"session"}), cx))
            .is_some()
    );
}

#[gpui::test]
fn an_admitted_send_claims_its_computer_after_and_a_rejected_one_never_does(
    cx: &mut gpui::TestAppContext,
) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (composer, _, mut requests, replies) = cloud_composer(cx, true);
    serve(cx, &runtime, &mut requests, &replies, account);
    composer.update(cx, |c, cx| {
        assert!(c.send_cloud_task("Fix the build".into(), false, cx));
    });
    let asked = serve(
        cx,
        &runtime,
        &mut requests,
        &replies,
        |label, params| match label {
            "ClaimComputerPrimary" => Err("this computer's connector is off"),
            methods::CLOUD_TASK_COMMAND => Ok(json!({"accountId":"owner","organizationId":null,
                "data":{"commandId":params["request"]["commandId"],"durablyAdmitted":true}})),
            _ => account(label, params),
        },
    );
    let order: Vec<&str> = asked
        .iter()
        .filter_map(|r| r["method"].as_str())
        .filter(|m| matches!(*m, "ClaimComputerPrimary" | methods::CLOUD_TASK_COMMAND))
        .collect();
    assert_eq!(
        order,
        vec![methods::CLOUD_TASK_COMMAND, "ClaimComputerPrimary"]
    );
    let command = asked
        .iter()
        .find(|r| r["method"] == methods::CLOUD_TASK_COMMAND)
        .unwrap();
    let claim = asked
        .iter()
        .find(|r| r["method"] == "ClaimComputerPrimary")
        .unwrap();
    assert_eq!(claim["params"]["workspaceId"], "task");
    assert_eq!(claim["params"]["chatId"], "session");
    assert_eq!(
        claim["params"]["commandId"],
        command["params"]["request"]["commandId"]
    );
    composer.read_with(cx, |c, _| {
        assert!(
            !c.cloud_send_unconfirmed(),
            "the refused claim left the send admitted"
        )
    });

    // A send the server refuses never moves the default computer.
    let (rejected, _, mut requests, replies) = cloud_composer(cx, true);
    serve(cx, &runtime, &mut requests, &replies, account);
    rejected.update(cx, |c, cx| {
        assert!(c.send_cloud_task("Fix the build".into(), false, cx));
    });
    let asked = serve(
        cx,
        &runtime,
        &mut requests,
        &replies,
        |label, params| match label {
            methods::CLOUD_TASK_COMMAND => Err("cloud_task_rejected"),
            _ => account(label, params),
        },
    );
    assert!(
        asked
            .iter()
            .any(|r| r["method"] == methods::CLOUD_TASK_COMMAND)
    );
    assert!(!asked.iter().any(|r| r["method"] == "ClaimComputerPrimary"));
}

/// Escape denies and ⌘/Ctrl+Enter allows once, like a local permission prompt.
#[gpui::test]
fn approval_keys_deny_or_allow_once(cx: &mut gpui::TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (composer, _, mut requests, replies) = cloud_composer(cx, true);
    serve(cx, &runtime, &mut requests, &replies, account);
    let key = |text: &str| gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse(text).unwrap(),
        is_held: false,
        prefer_character_input: false,
    };
    let held = |text: &str| gpui::KeyDownEvent {
        is_held: true,
        ..key(text)
    };
    composer.update(cx, |c, cx| {
        c.on_computer_approval_key(&key("enter"), cx);
        assert_eq!(
            c.computer_request_waiting().map(|p| p.id.as_str()),
            Some("r1"),
            "plain Enter decides nothing"
        );
        c.on_computer_approval_key(&key("escape"), cx);
        // Still holding the keys from before decides nothing for the next one.
        c.on_computer_approval_key(&held("ctrl-enter"), cx);
        c.on_computer_approval_key(&held("escape"), cx);
        assert_eq!(
            c.computer_request_waiting().map(|p| p.id.as_str()),
            Some("r2"),
            "a held key never approves the next request"
        );
        c.on_computer_approval_key(&key("ctrl-enter"), cx);
        assert!(c.computer_request_waiting().is_none());
    });
    let asked = serve(cx, &runtime, &mut requests, &replies, account);
    let decisions: Vec<(String, String)> = asked
        .iter()
        .filter(|r| r["params"]["action"] == "decide")
        .map(|r| {
            (
                r["params"]["requestId"].as_str().unwrap().to_owned(),
                r["params"]["decision"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        decisions,
        vec![("r1".into(), "deny".into()), ("r2".into(), "once".into())]
    );
}

/// Tool rows name a command's computer; one without an ID ran on the chat's own.
#[gpui::test]
fn the_directory_names_a_commands_computer(cx: &mut gpui::TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (_composer, _, mut requests, replies) = cloud_composer(cx, true);
    serve(cx, &runtime, &mut requests, &replies, account);
    cx.update(|cx| {
        let directory = cx.global::<ComputerDirectory>();
        assert_eq!(
            directory.label(None),
            Some(("Zed's Mac".to_owned(), icons::LAPTOP))
        );
        assert_eq!(
            directory.label(Some("box")),
            Some(("Build box".to_owned(), icons::MONITOR))
        );
        assert_eq!(directory.label(Some("gone")), None);
    });
}

/// With nothing waiting, a full read still clears requests that went away.
#[gpui::test]
fn a_full_read_clears_requests_that_went_away(cx: &mut gpui::TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (composer, _, mut requests, replies) = cloud_composer(cx, true);
    serve(cx, &runtime, &mut requests, &replies, account);
    composer.read_with(cx, |c, _| assert_eq!(c.computers.pending.len(), 2));
    composer.update(cx, |c, cx| c.refresh_computers(cx));
    serve(
        cx,
        &runtime,
        &mut requests,
        &replies,
        |label, params| match label {
            "pending" | "results" => Ok(json!([])),
            _ => account(label, params),
        },
    );
    composer.read_with(cx, |c, _| {
        assert!(c.computer_request_waiting().is_none());
        assert!(c.computers.runs.is_empty());
    });
}

/// A pushed `computer-request` hint shows a new approval at once, without
/// waiting for the next poll.
#[gpui::test]
fn a_pushed_request_hint_fetches_the_approval_at_once(cx: &mut gpui::TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (composer, _, mut requests, replies) = cloud_composer(cx, true);
    let quiet = |label: &str, params: &Value| match label {
        "pending" | "results" => Ok(json!([])),
        _ => account(label, params),
    };
    let asked = serve(cx, &runtime, &mut requests, &replies, quiet);
    let feed = asked
        .iter()
        .find(|r| r["method"] == methods::CLOUD_EVENTS)
        .expect("the composer follows the account feed");
    assert_eq!(feed["params"]["expectedAccountId"], "owner");
    composer.update(cx, |c, _| assert!(c.computer_request_waiting().is_none()));
    let hint = |workspace: &str| {
        json!({"id":feed["id"],"item":{"accountId":"owner","organizationId":null,
            "data":{"type":"events","events":[{"kind":"computer-request","resourceId":"session","workspaceId":workspace}]}}})
    };
    // Another computer's hint is not this chat's business.
    runtime
        .block_on(replies.send(format!("{}\n", hint("another-task"))))
        .unwrap();
    let asked = serve(cx, &runtime, &mut requests, &replies, account);
    assert!(
        !asked.iter().any(|r| r["params"]["action"] == "pending"),
        "{asked:?}"
    );
    runtime
        .block_on(replies.send(format!("{}\n", hint("task"))))
        .unwrap();
    let asked = serve(cx, &runtime, &mut requests, &replies, account);
    assert!(asked.iter().any(|r| r["params"]["action"] == "pending"));
    composer.update(cx, |c, _| {
        assert_eq!(
            c.computer_request_waiting().map(|p| p.id.as_str()),
            Some("r1")
        );
    });
}

#[test]
fn only_this_accounts_hints_for_this_computer_refetch() {
    let frame = |account: &str, org: Value, data: Value| json!({"accountId":account,"organizationId":org,"data":data});
    let hint = json!({"type":"events","events":[{"kind":"computer-request","workspaceId":"task","resourceId":"chat"}]});
    assert!(requests_changed(
        &frame("owner", Value::Null, hint.clone()),
        "owner",
        "task"
    ));
    assert!(!requests_changed(
        &frame("owner", Value::Null, hint.clone()),
        "owner",
        "other"
    ));
    assert!(!requests_changed(
        &frame("someone", Value::Null, hint.clone()),
        "owner",
        "task"
    ));
    assert!(!requests_changed(
        &frame("owner", json!("org"), hint),
        "owner",
        "task"
    ));
    let other = json!({"type":"events","events":[{"kind":"billing","workspaceId":"task"}]});
    assert!(!requests_changed(
        &frame("owner", Value::Null, other),
        "owner",
        "task"
    ));
    // Hints may have been missed: read again.
    for kind in ["connected", "reset"] {
        assert!(requests_changed(
            &frame("owner", Value::Null, json!({"type":kind})),
            "owner",
            "task"
        ));
    }
}

fn renewal_miss(label: &str, params: &Value) -> Result<Value, &'static str> {
    match label {
        "list" | "pending" | "results" => Err(zuse_protocol::cloud::SESSION_RENEWAL_UNAVAILABLE),
        _ => account(label, params),
    }
}

fn notice_shown(composer: &Entity<Composer>, cx: &mut gpui::TestAppContext) -> bool {
    composer.update(cx, |c, cx| {
        let theme = Theme::of(cx).clone();
        c.computer_tray_rows(&theme, cx)
            .iter()
            .any(|row| row.id().as_ref() == "computers-error")
    })
}

/// A session renewal that misses once (a network blip, a laptop waking up)
/// stays silent, backs the reads off, and the next good read forgets it.
#[gpui::test]
fn a_renewal_miss_is_retried_quietly_and_cleared_by_the_next_read(cx: &mut gpui::TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (composer, _, mut requests, replies) = cloud_composer(cx, true);
    serve(cx, &runtime, &mut requests, &replies, renewal_miss);
    assert!(!notice_shown(&composer, cx), "one miss shows nothing");
    composer.read_with(cx, |c, _| {
        assert!(c.computers.bridge_on, "this computer's connector is on");
        assert_eq!(c.computers.reads.failures, 1);
        assert!(c.computers.reads.delay() > FAST_POLL, "reads back off");
    });
    composer.update(cx, |c, cx| c.refresh_computers(cx));
    serve(cx, &runtime, &mut requests, &replies, account);
    composer.read_with(cx, |c, _| {
        assert_eq!(c.computers.reads.failures, 0);
        assert_eq!(c.computers.reads.delay(), FAST_POLL);
    });
    assert!(!notice_shown(&composer, cx));
}

#[test]
fn only_a_lasting_relevant_failure_becomes_an_actionable_notice() {
    use zuse_protocol::cloud::{SESSION_RENEWAL_UNAVAILABLE, SESSION_SIGNED_OUT};
    let start = Instant::now();
    let mut state = State {
        bridge_on: true,
        ..Default::default()
    };
    let miss = || Some(SESSION_RENEWAL_UNAVAILABLE.to_owned());
    state.reads.record(miss(), start);
    state.reads.record(miss(), start + Duration::from_secs(5));
    assert_eq!(
        state.read_notice(start + Duration::from_secs(90)),
        None,
        "two misses"
    );
    state.reads.record(miss(), start + Duration::from_secs(15));
    assert_eq!(
        state.read_notice(start + Duration::from_secs(30)),
        None,
        "not lasting yet"
    );
    let later = start + SUSTAINED_FOR;
    assert_eq!(state.read_notice(later), Some(ReadNotice::Unreachable));
    assert!(!ReadNotice::Unreachable.text().contains("Settings →"));

    // Not using this computer from cloud chats and nothing waiting: silent.
    state.bridge_on = false;
    assert_eq!(state.read_notice(later), None);
    // A request waiting in this chat makes it the owner's business again.
    state.pending.push(Pending {
        id: "r1".into(),
        ..Default::default()
    });
    assert_eq!(state.read_notice(later), Some(ReadNotice::Unreachable));

    // Dismissed, it stays away until the reads recover and fail again.
    state.reads.dismissed = true;
    assert_eq!(state.read_notice(later), None);
    state.reads.record(None, later);
    assert_eq!(state.reads, ReadHealth::default());

    // A lost sign-in can't fix itself: a second read agreeing is enough.
    state
        .reads
        .record(Some(SESSION_SIGNED_OUT.to_owned()), later);
    assert_eq!(state.read_notice(later), None);
    state
        .reads
        .record(Some(SESSION_SIGNED_OUT.to_owned()), later);
    assert_eq!(state.read_notice(later), Some(ReadNotice::SignedOut));

    // Other errors keep their own words.
    let mut failing = State {
        bridge_on: true,
        ..Default::default()
    };
    for _ in 0..SUSTAINED_FAILURES {
        failing
            .reads
            .record(Some("The cloud refused this request".into()), start);
    }
    assert_eq!(
        failing.read_notice(later),
        Some(ReadNotice::Failing("The cloud refused this request".into()))
    );
}

#[test]
fn failing_reads_back_off_to_a_cap() {
    let mut reads = ReadHealth::default();
    assert_eq!(reads.delay(), FAST_POLL);
    let now = Instant::now();
    let mut last = FAST_POLL;
    for _ in 0..12 {
        reads.record(Some("down".into()), now);
        assert!(reads.delay() >= last);
        last = reads.delay();
    }
    assert_eq!(last, BACKOFF_CAP);
}

/// While reads fail, the next poll reads everything again, so a slow read
/// that keeps failing can't be hidden by the cheap reads succeeding.
#[gpui::test]
fn a_failing_poll_rereads_everything(cx: &mut gpui::TestAppContext) {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (composer, _, mut requests, replies) = cloud_composer(cx, true);
    let list_down = |label: &str, params: &Value| match label {
        "list" => Err(zuse_protocol::cloud::SESSION_RENEWAL_UNAVAILABLE),
        _ => account(label, params),
    };
    serve(cx, &runtime, &mut requests, &replies, list_down);
    composer.read_with(cx, |c, _| assert_eq!(c.computers.reads.failures, 1));
    cx.executor().advance_clock(Duration::from_secs(31));
    let asked = serve(cx, &runtime, &mut requests, &replies, list_down);
    assert!(
        asked.iter().any(|r| r["params"]["action"] == "list"),
        "{asked:?}"
    );
    composer.read_with(cx, |c, _| assert_eq!(c.computers.reads.failures, 2));
}
