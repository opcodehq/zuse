//! Cloud-chat computer requests against a real PostgreSQL: approval scopes,
//! deadlines, stop, unknown outcomes and attachment boundaries.
use super::bridge_approvals::{Decision, decide_for, pending_for};
use super::bridge_computers::{attached_for, primary_for};
use super::bridge_tasks::{Action, cancel_for, host_action, results_for, runtime_request};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use zuse_protocol::computer_tasks::Request;

const STUBS: &str = "
CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY, account_id text NOT NULL,
    desired_state text NOT NULL, state text NOT NULL);
CREATE TABLE zuse_rust_task_command_sessions(workspace_id text NOT NULL, session_id text NOT NULL,
    state text NOT NULL, PRIMARY KEY(workspace_id, session_id));
INSERT INTO api_cloud_workspaces VALUES ('w','owner','ready','ready'),('w2','owner','ready','ready'),
    ('other-w','intruder','ready','ready');
INSERT INTO zuse_rust_task_command_sessions VALUES ('w','chat','active'),('w','chat2','active'),
    ('w2','chat','active'),('other-w','chat','active');
INSERT INTO api_bridge_computers VALUES ('owner','mac','key-mac','Mac','macos'),
    ('owner','linux','key-linux','Build box','linux'),('intruder','pc','key-pc','PC','windows');
";

async fn database() -> Option<PgPool> {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").ok()?;
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("bridge_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(6)
        .after_connect(move |c, _| {
            let schema = schema.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={schema}"))
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    let migration = include_str!("../../migrations/0055_device_command_approvals.sql");
    // The stubs must exist before the migration's foreign keys and triggers.
    let (computers, _) = STUBS
        .split_once("INSERT INTO api_bridge_computers")
        .unwrap();
    sqlx::raw_sql(computers).execute(&pool).await.unwrap();
    sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    sqlx::raw_sql(&format!(
        "INSERT INTO api_bridge_computers{}",
        STUBS
            .split_once("INSERT INTO api_bridge_computers")
            .unwrap()
            .1
    ))
    .execute(&pool)
    .await
    .unwrap();
    Some(pool)
}

async fn attach(pool: &PgPool, workspace: &str, chat: &str, device: &str) {
    sqlx::query("INSERT INTO api_bridge_attachments VALUES ('owner',$1,$2,$3)")
        .bind(workspace)
        .bind(chat)
        .bind(device)
        .execute(pool)
        .await
        .unwrap();
}

/// The device's shell connector offers its task, as the host does every loop.
async fn offer(pool: &PgPool, device: &str, task: uuid::Uuid) {
    host_action(
        pool,
        "owner",
        device,
        Action::Offer {
            task,
            dynamic: true,
            command: "/bin/sh".into(),
            cwd: "/home/me".into(),
            user: "me".into(),
        },
    )
    .await
    .unwrap();
}

async fn command(
    pool: &PgPool,
    workspace: &str,
    chat: &str,
    device: &str,
    text: &str,
    timeout: Option<u32>,
) -> Result<uuid::Uuid, crate::error::ApiError> {
    let id = uuid::Uuid::new_v4();
    runtime_request(
        pool,
        "owner",
        workspace,
        chat,
        Request::Command {
            device_id: Some(device.into()),
            command: text.into(),
            cwd: None,
            request_id: id,
            timeout_seconds: timeout,
        },
    )
    .await?;
    Ok(id)
}

async fn claim(pool: &PgPool, device: &str, task: uuid::Uuid) -> Value {
    host_action(pool, "owner", device, Action::Claim { task })
        .await
        .unwrap()
}

async fn check(pool: &PgPool, device: &str, task: uuid::Uuid, request: uuid::Uuid) -> Value {
    host_action(
        pool,
        "owner",
        device,
        Action::Check {
            task,
            request,
            output: Some("partial".into()),
        },
    )
    .await
    .unwrap()
}

async fn state_of(pool: &PgPool, workspace: &str, chat: &str, id: uuid::Uuid) -> Value {
    results_for(pool, "owner", workspace, chat)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row["id"] == id.to_string())
        .map(|row| json!({"state":row["state"],"status":row["result"]["status"]}))
        .unwrap_or(Value::Null)
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn approved_command_runs_once_with_its_deadline_and_streams_output() {
    let Some(pool) = database().await else { return };
    let task = uuid::Uuid::new_v4();
    attach(&pool, "w", "chat", "mac").await;
    offer(&pool, "mac", task).await;
    let id = command(&pool, "w", "chat", "mac", "cargo test", Some(1800))
        .await
        .unwrap();
    assert_eq!(
        state_of(&pool, "w", "chat", id).await,
        json!({"state":"pending","status":"Waiting for approval"})
    );
    // Nothing runs before the owner decides.
    assert_eq!(claim(&pool, "mac", task).await, Value::Null);
    let pending = pending_for(&pool, "owner", "w", "chat").await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["command"], "cargo test");
    assert_eq!(pending[0]["timeoutMs"], 1_800_000);
    // Another owner can neither see nor decide it.
    assert!(
        pending_for(&pool, "intruder", "w", "chat")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        decide_for(&pool, "intruder", "w", "chat", id, Decision::Once)
            .await
            .is_err()
    );
    decide_for(&pool, "owner", "w", "chat", id, Decision::Once)
        .await
        .unwrap();
    // A decision is final.
    assert!(
        decide_for(&pool, "owner", "w", "chat", id, Decision::Deny)
            .await
            .is_err()
    );
    let claimed = claim(&pool, "mac", task).await;
    assert_eq!(
        claimed,
        json!({"id":id,"command":"cargo test","cwd":"/home/me","timeoutMs":1_800_000})
    );
    assert_eq!(claim(&pool, "mac", task).await, Value::Null, "claimed once");
    assert_eq!(check(&pool, "mac", task, id).await, json!(true));
    let rows = results_for(&pool, "owner", "w", "chat").await.unwrap();
    assert_eq!(rows[0]["state"], "running");
    assert_eq!(rows[0]["result"]["output"], "partial");
    host_action(
        &pool,
        "owner",
        "mac",
        Action::Finish {
            task,
            request: id,
            output: "ok".into(),
            status: "exit status: 0".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        state_of(&pool, "w", "chat", id).await,
        json!({"state":"finished","status":"exit status: 0"})
    );
    // Its once-approval does not cover a later command.
    let next = command(&pool, "w", "chat", "mac", "ls", None)
        .await
        .unwrap();
    assert_eq!(claim(&pool, "mac", task).await, Value::Null);
    let pending = pending_for(&pool, "owner", "w", "chat").await.unwrap();
    assert_eq!(pending[0]["id"], next.to_string());
    assert_eq!(pending[0]["timeoutMs"], 600_000, "default deadline");
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn session_and_always_grants_cover_only_their_scope_and_end_on_revoke_or_detach() {
    let Some(pool) = database().await else { return };
    let task = uuid::Uuid::new_v4();
    for (workspace, chat) in [("w", "chat"), ("w", "chat2"), ("w2", "chat")] {
        attach(&pool, workspace, chat, "mac").await;
    }
    offer(&pool, "mac", task).await;
    let first = command(&pool, "w", "chat", "mac", "a", None).await.unwrap();
    decide_for(&pool, "owner", "w", "chat", first, Decision::Session)
        .await
        .unwrap();
    assert_eq!(claim(&pool, "mac", task).await["id"], first.to_string());
    // The same chat's next command needs no decision; another chat's does.
    let same = command(&pool, "w", "chat", "mac", "b", None).await.unwrap();
    let elsewhere = command(&pool, "w", "chat2", "mac", "c", None)
        .await
        .unwrap();
    assert_eq!(claim(&pool, "mac", task).await["id"], same.to_string());
    assert_eq!(claim(&pool, "mac", task).await, Value::Null);
    assert_eq!(
        state_of(&pool, "w", "chat2", elsewhere).await["state"],
        "pending"
    );
    // Detaching ends the chat's grant and its requests, including a running one.
    assert_eq!(check(&pool, "mac", task, same).await, json!(true));
    sqlx::query("DELETE FROM api_bridge_attachments WHERE workspace_id='w' AND chat_id='chat'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(check(&pool, "mac", task, same).await, json!(false));
    assert!(
        results_for(&pool, "owner", "w", "chat")
            .await
            .unwrap()
            .is_empty()
    );
    attach(&pool, "w", "chat", "mac").await;
    let after = command(&pool, "w", "chat", "mac", "d", None).await.unwrap();
    assert_eq!(
        state_of(&pool, "w", "chat", after).await["state"],
        "pending"
    );

    // "Always" covers the owner's other chats on this computer only.
    decide_for(&pool, "owner", "w", "chat2", elsewhere, Decision::Always)
        .await
        .unwrap();
    assert_eq!(claim(&pool, "mac", task).await["id"], elsewhere.to_string());
    let other_computer = uuid::Uuid::new_v4();
    attach(&pool, "w2", "chat", "linux").await;
    offer(&pool, "linux", other_computer).await;
    let linux = command(&pool, "w2", "chat", "linux", "e", None)
        .await
        .unwrap();
    assert_eq!(
        state_of(&pool, "w2", "chat", linux).await["state"],
        "pending"
    );
    let covered = command(&pool, "w2", "chat", "mac", "f", None)
        .await
        .unwrap();
    assert_eq!(
        state_of(&pool, "w2", "chat", covered).await["state"],
        "approved"
    );
    assert_eq!(claim(&pool, "mac", task).await["id"], covered.to_string());
    // Revoking stops work that relied on it.
    assert_eq!(check(&pool, "mac", task, covered).await, json!(true));
    sqlx::query(
        "DELETE FROM api_bridge_always_grants WHERE owner_id='owner' AND enrollment_id='mac'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(check(&pool, "mac", task, covered).await, json!(false));
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn denied_and_stopped_requests_never_run_and_keep_their_outcome() {
    let Some(pool) = database().await else { return };
    let task = uuid::Uuid::new_v4();
    attach(&pool, "w", "chat", "mac").await;
    offer(&pool, "mac", task).await;
    let denied = command(&pool, "w", "chat", "mac", "rm -rf ~", None)
        .await
        .unwrap();
    decide_for(&pool, "owner", "w", "chat", denied, Decision::Deny)
        .await
        .unwrap();
    assert_eq!(claim(&pool, "mac", task).await, Value::Null);
    assert_eq!(
        state_of(&pool, "w", "chat", denied).await,
        json!({"state":"denied","status":"Denied"})
    );

    // Stopped before approval: it can no longer be approved or claimed.
    let waiting = command(&pool, "w", "chat", "mac", "sleep 1", None)
        .await
        .unwrap();
    cancel_for(&pool, "owner", "w", "chat", waiting)
        .await
        .unwrap();
    assert!(
        pending_for(&pool, "owner", "w", "chat")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        decide_for(&pool, "owner", "w", "chat", waiting, Decision::Once)
            .await
            .is_err()
    );
    assert_eq!(claim(&pool, "mac", task).await, Value::Null);

    // Stopped while running: the host's next check stops it, and its own
    // late result does not replace the stop.
    let running = command(&pool, "w", "chat", "mac", "sleep 100", None)
        .await
        .unwrap();
    decide_for(&pool, "owner", "w", "chat", running, Decision::Once)
        .await
        .unwrap();
    assert_eq!(claim(&pool, "mac", task).await["id"], running.to_string());
    assert_eq!(check(&pool, "mac", task, running).await, json!(true));
    cancel_for(&pool, "owner", "w", "chat", running)
        .await
        .unwrap();
    assert_eq!(check(&pool, "mac", task, running).await, json!(false));
    host_action(
        &pool,
        "owner",
        "mac",
        Action::Finish {
            task,
            request: running,
            output: String::new(),
            status: "authorization ended".into(),
        },
    )
    .await
    .unwrap();
    let rows = results_for(&pool, "owner", "w", "chat").await.unwrap();
    let row = rows
        .iter()
        .find(|r| r["id"] == running.to_string())
        .unwrap();
    assert_eq!(row["state"], "cancelled");
    assert_eq!(
        row["result"]["output"], "partial",
        "keeps output seen so far"
    );
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn silent_or_overdue_runs_show_unknown_and_are_never_claimed_again() {
    let Some(pool) = database().await else { return };
    let task = uuid::Uuid::new_v4();
    attach(&pool, "w", "chat", "mac").await;
    offer(&pool, "mac", task).await;
    let silent = command(&pool, "w", "chat", "mac", "make", Some(3600))
        .await
        .unwrap();
    decide_for(&pool, "owner", "w", "chat", silent, Decision::Session)
        .await
        .unwrap();
    assert_eq!(claim(&pool, "mac", task).await["id"], silent.to_string());
    // Long runs stay "running" while the host keeps reporting...
    sqlx::query("UPDATE api_bridge_runs SET claimed_at=now()-interval '30 minutes' WHERE request_id=$1::text::uuid")
        .bind(silent.to_string())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(check(&pool, "mac", task, silent).await, json!(true));
    assert_eq!(
        state_of(&pool, "w", "chat", silent).await["state"],
        "running"
    );
    // ...and turn unknown once it goes quiet. Nothing re-runs it.
    sqlx::query("UPDATE api_bridge_runs SET heartbeat_at=now()-interval '81 seconds' WHERE request_id=$1::text::uuid")
        .bind(silent.to_string())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        state_of(&pool, "w", "chat", silent).await,
        json!({"state":"unknown","status":"Outcome unknown"})
    );
    assert_eq!(claim(&pool, "mac", task).await, Value::Null);

    // Past its own deadline a run is no longer authorized, even while reporting.
    let overdue = command(&pool, "w", "chat", "mac", "sleep 999", Some(60))
        .await
        .unwrap();
    assert_eq!(claim(&pool, "mac", task).await["id"], overdue.to_string());
    sqlx::query("UPDATE api_bridge_runs SET claimed_at=now()-interval '141 seconds' WHERE request_id=$1::text::uuid")
        .bind(overdue.to_string())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(check(&pool, "mac", task, overdue).await, json!(false));
    assert_eq!(
        state_of(&pool, "w", "chat", overdue).await["state"],
        "unknown"
    );
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn requests_are_bounded_idempotent_and_bound_to_their_chat() {
    let Some(pool) = database().await else { return };
    let task = uuid::Uuid::new_v4();
    attach(&pool, "w", "chat", "mac").await;
    offer(&pool, "mac", task).await;
    for timeout in [0, 3601] {
        let error = command(&pool, "w", "chat", "mac", "pwd", Some(timeout))
            .await
            .unwrap_err();
        assert_eq!(error.code, "invalid_computer_timeout");
    }
    // An offline computer is reported as such, attached or not.
    assert_eq!(
        command(&pool, "w", "chat", "linux", "pwd", None)
            .await
            .unwrap_err()
            .code,
        "computer_offline"
    );
    attach(&pool, "w", "chat", "linux").await;
    assert_eq!(
        command(&pool, "w", "chat", "linux", "pwd", None)
            .await
            .unwrap_err()
            .code,
        "computer_offline"
    );
    // Another owner's enrollment is never reachable from this chat, and
    // the owner's computers never serve another owner's chat.
    assert_eq!(
        command(&pool, "w", "chat", "pc", "pwd", None)
            .await
            .unwrap_err()
            .code,
        "computer_not_found"
    );
    assert_eq!(
        command(&pool, "other-w", "chat", "mac", "pwd", None)
            .await
            .unwrap_err()
            .code,
        "bridge_chat_unavailable"
    );

    // A retried submission is recognized; the same ID from another chat is not.
    let id = uuid::Uuid::new_v4();
    let submit = |chat: &'static str| {
        let pool = pool.clone();
        async move {
            runtime_request(
                &pool,
                "owner",
                "w",
                chat,
                Request::Command {
                    device_id: Some("mac".into()),
                    command: "pwd".into(),
                    cwd: None,
                    request_id: id,
                    timeout_seconds: None,
                },
            )
            .await
        }
    };
    submit("chat").await.unwrap();
    submit("chat").await.unwrap();
    attach(&pool, "w", "chat2", "mac").await;
    assert_eq!(
        submit("chat2").await.unwrap_err().code,
        "computer_request_id_reused"
    );
    assert_eq!(
        pending_for(&pool, "owner", "w", "chat")
            .await
            .unwrap()
            .len(),
        1
    );

    // A chat can't pile up unbounded approvals.
    for _ in 1..10 {
        command(&pool, "w", "chat", "mac", "pwd", None)
            .await
            .unwrap();
    }
    assert_eq!(
        command(&pool, "w", "chat", "mac", "pwd", None)
            .await
            .unwrap_err()
            .code,
        "too_many_pending_computer_requests"
    );
    // Output a host publishes is bounded.
    let error = host_action(
        &pool,
        "owner",
        "mac",
        Action::Check {
            task,
            request: id,
            output: Some("x".repeat(98_305)),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "output_too_large");
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn attached_computers_list_offline_ones_with_their_limits() {
    let Some(pool) = database().await else { return };
    attach(&pool, "w", "chat", "mac").await;
    attach(&pool, "w", "chat", "linux").await;
    offer(&pool, "mac", uuid::Uuid::new_v4()).await;
    let rows = runtime_request(&pool, "owner", "w", "chat", Request::Computers)
        .await
        .unwrap();
    assert_eq!(
        rows,
        json!([
            {"deviceId":"linux","name":"Build box","platform":"linux","role":"secondary","online":false,"shell":null,
             "user":null,"cwd":null,"defaultTimeoutSeconds":600,"maxTimeoutSeconds":3600},
            {"deviceId":"mac","name":"Mac","platform":"macos","role":"secondary","online":true,"shell":"/bin/sh",
             "user":"me","cwd":"/home/me","defaultTimeoutSeconds":600,"maxTimeoutSeconds":3600}
        ])
    );
}

/// The agent's default target, without a device ID.
async fn on_primary(pool: &PgPool, text: &str) -> Result<Value, crate::error::ApiError> {
    runtime_request(
        pool,
        "owner",
        "w",
        "chat",
        Request::Command {
            device_id: None,
            command: text.into(),
            cwd: None,
            request_id: uuid::Uuid::new_v4(),
            timeout_seconds: None,
        },
    )
    .await
}

fn id_of(created: &Value) -> uuid::Uuid {
    uuid::Uuid::parse_str(created["id"].as_str().unwrap()).unwrap()
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn the_primary_follows_the_sender_and_is_the_default_target() {
    let Some(pool) = database().await else { return };
    let (mac, linux) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    offer(&pool, "mac", mac).await;
    offer(&pool, "linux", linux).await;
    // Nobody has sent from an enabled computer yet: there is no default.
    assert_eq!(
        on_primary(&pool, "pwd").await.unwrap_err().code,
        "no_primary_computer"
    );
    // Sending from the Mac makes it primary without an explicit attach.
    primary_for(&pool, "owner", "w", "chat", "mac")
        .await
        .unwrap();
    let first = on_primary(&pool, "make test").await.unwrap();
    assert_eq!(
        (first["deviceId"].as_str(), first["role"].as_str()),
        (Some("mac"), Some("primary"))
    );
    assert_eq!(
        attached_for(&pool, "owner", "w", "chat").await.unwrap(),
        vec![json!({"deviceId":"mac","role":"primary"})]
    );
    let id = id_of(&first);
    decide_for(&pool, "owner", "w", "chat", id, Decision::Session)
        .await
        .unwrap();
    assert_eq!(claim(&pool, "mac", mac).await["id"], id.to_string());

    // The next message comes from the Linux box: it becomes the default.
    primary_for(&pool, "owner", "w", "chat", "linux")
        .await
        .unwrap();
    let next = on_primary(&pool, "uname").await.unwrap();
    assert_eq!(
        (next["deviceId"].as_str(), next["role"].as_str()),
        (Some("linux"), Some("primary"))
    );
    // The Mac's grant never covers the new primary.
    let pending = pending_for(&pool, "owner", "w", "chat").await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["deviceId"], "linux");
    assert_eq!(pending[0]["role"], "primary");
    // The Mac's running work continues, and it can still be asked for by
    // name: it is no longer the default.
    assert_eq!(check(&pool, "mac", mac, id).await, json!(true));
    let named = command(&pool, "w", "chat", "mac", "ls", None)
        .await
        .unwrap();
    assert_eq!(
        state_of(&pool, "w", "chat", named).await["state"],
        "approved"
    );
    let rows = results_for(&pool, "owner", "w", "chat").await.unwrap();
    let row = rows.iter().find(|r| r["id"] == named.to_string()).unwrap();
    assert_eq!(row["role"], "secondary");
    assert_eq!(
        attached_for(&pool, "owner", "w", "chat").await.unwrap(),
        vec![json!({"deviceId":"linux","role":"primary"})]
    );
    // A primary claim never reaches another owner's chat or computer.
    assert!(
        primary_for(&pool, "owner", "other-w", "chat", "mac")
            .await
            .is_err()
    );
    assert!(
        primary_for(&pool, "owner", "w", "chat", "pc")
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn another_computer_runs_only_when_named_and_keeps_its_own_approvals() {
    let Some(pool) = database().await else { return };
    let (mac, linux) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    offer(&pool, "mac", mac).await;
    offer(&pool, "linux", linux).await;
    primary_for(&pool, "owner", "w", "chat", "mac")
        .await
        .unwrap();
    // The owner's other computers need no attaching; they run when named.
    // The default still goes to the primary...
    let default = on_primary(&pool, "pwd").await.unwrap();
    assert_eq!(default["deviceId"], "mac");
    // ...and the secondary only when named, shown as such.
    let named = command(&pool, "w", "chat", "linux", "uname", None)
        .await
        .unwrap();
    let pending = pending_for(&pool, "owner", "w", "chat").await.unwrap();
    let secondary = pending
        .iter()
        .find(|row| row["id"] == named.to_string())
        .unwrap();
    assert_eq!(
        (
            secondary["computerName"].as_str(),
            secondary["role"].as_str()
        ),
        (Some("Build box"), Some("secondary"))
    );
    // "Always" on the primary never covers the secondary.
    decide_for(
        &pool,
        "owner",
        "w",
        "chat",
        id_of(&default),
        Decision::Always,
    )
    .await
    .unwrap();
    let again = command(&pool, "w", "chat", "linux", "uname -a", None)
        .await
        .unwrap();
    assert_eq!(
        state_of(&pool, "w", "chat", again).await["state"],
        "pending"
    );
    let covered = id_of(&on_primary(&pool, "ls").await.unwrap());
    assert_eq!(
        state_of(&pool, "w", "chat", covered).await["state"],
        "approved"
    );
    // The agent sees which is which.
    let computers = runtime_request(&pool, "owner", "w", "chat", Request::Computers)
        .await
        .unwrap();
    let roles: Vec<_> = computers
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["name"].as_str().unwrap(), c["role"].as_str().unwrap()))
        .collect();
    assert_eq!(roles, [("Mac", "primary"), ("Build box", "secondary")]);
}

/// The bridge is its own path (enrolled connector, outbound HTTPS, cloud
/// chat). It must never lean on remote access: pairing, relay or direct
/// routes, tunnels, SSH, device links or their proof keys.
#[test]
fn the_bridge_does_not_depend_on_remote_access() {
    let sources = [
        (
            "engine computer_bridge.rs",
            include_str!("../../../engine/src/computer_bridge.rs"),
        ),
        (
            "engine computer_bridge/proof.rs",
            include_str!("../../../engine/src/computer_bridge/proof.rs"),
        ),
        ("bridge_approvals.rs", include_str!("bridge_approvals.rs")),
        ("bridge_computers.rs", include_str!("bridge_computers.rs")),
        ("bridge_proof.rs", include_str!("bridge_proof.rs")),
        ("bridge_tasks.rs", include_str!("bridge_tasks.rs")),
    ];
    let remote_access = [
        "managed_access",
        "pairing",
        "relay_access",
        "direct_access",
        "browser_access",
        "zuse_remote",
        "tunnels",
        "cloud_ssh",
        "device_proof",
        "devices::",
        "store.devices",
    ];
    for (file, source) in sources {
        for module in remote_access {
            assert!(
                !source.contains(module),
                "{file} references remote-access code ({module})"
            );
        }
    }
}

async fn submit_as(
    pool: &PgPool,
    id: uuid::Uuid,
    device: &str,
) -> Result<Value, crate::error::ApiError> {
    runtime_request(
        pool,
        "owner",
        "w",
        "chat",
        Request::Command {
            device_id: Some(device.into()),
            command: "make".into(),
            cwd: None,
            request_id: id,
            timeout_seconds: None,
        },
    )
    .await
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn a_request_id_never_runs_again_after_detach_and_retries_survive_going_offline() {
    let Some(pool) = database().await else { return };
    let task = uuid::Uuid::new_v4();
    attach(&pool, "w", "chat", "mac").await;
    offer(&pool, "mac", task).await;
    let id = uuid::Uuid::new_v4();
    submit_as(&pool, id, "mac").await.unwrap();
    // The computer drops off; a retry of the same submission is still
    // recognized instead of failing as offline.
    sqlx::query("UPDATE api_bridge_tasks SET expires_at=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        submit_as(&pool, id, "mac").await.unwrap()["id"],
        id.to_string()
    );
    // Detach deletes its rows; the same ID is refused after reattaching.
    sqlx::query("DELETE FROM api_bridge_attachments")
        .execute(&pool)
        .await
        .unwrap();
    attach(&pool, "w", "chat", "mac").await;
    offer(&pool, "mac", task).await;
    assert_eq!(
        submit_as(&pool, id, "mac").await.unwrap_err().code,
        "computer_request_settled"
    );
    assert!(
        pending_for(&pool, "owner", "w", "chat")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn approvals_reach_the_same_computer_after_its_connector_reconnects() {
    let Some(pool) = database().await else { return };
    let before = uuid::Uuid::new_v4();
    attach(&pool, "w", "chat", "mac").await;
    offer(&pool, "mac", before).await;
    let id = command(&pool, "w", "chat", "mac", "make", None)
        .await
        .unwrap();
    decide_for(&pool, "owner", "w", "chat", id, Decision::Once)
        .await
        .unwrap();
    // The connector restarts with a new task ID for the same shell.
    let after = uuid::Uuid::new_v4();
    offer(&pool, "mac", after).await;
    assert_eq!(claim(&pool, "mac", after).await["id"], id.to_string());
    assert_eq!(check(&pool, "mac", after, id).await, json!(true));
    // A different computer's connector never adopts it.
    let other = uuid::Uuid::new_v4();
    offer(&pool, "linux", other).await;
    assert_eq!(claim(&pool, "linux", other).await, Value::Null);
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn revoking_always_returns_unstarted_requests_to_waiting() {
    let Some(pool) = database().await else { return };
    attach(&pool, "w", "chat", "mac").await;
    sqlx::query("INSERT INTO api_bridge_always_grants VALUES ('owner','mac')")
        .execute(&pool)
        .await
        .unwrap();
    let id = command(&pool, "w", "chat", "mac", "make", None).await;
    // Offline at first: queue it while the computer reconnects.
    assert_eq!(id.unwrap_err().code, "computer_offline");
    let task = uuid::Uuid::new_v4();
    offer(&pool, "mac", task).await;
    let id = command(&pool, "w", "chat", "mac", "make", None)
        .await
        .unwrap();
    assert_eq!(state_of(&pool, "w", "chat", id).await["state"], "approved");
    super::bridge_approvals::revoke_always_for(&pool, "owner", "mac")
        .await
        .unwrap();
    assert_eq!(state_of(&pool, "w", "chat", id).await["state"], "pending");
    // Granting "always" again later doesn't revive the old approval.
    sqlx::query("INSERT INTO api_bridge_always_grants VALUES ('owner','mac')")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(claim(&pool, "mac", task).await, Value::Null);
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn renaming_a_computer_changes_only_the_owners_own_and_later_requests() {
    let Some(pool) = database().await else { return };
    let task = uuid::Uuid::new_v4();
    offer(&pool, "mac", task).await;
    super::bridge_computers::rename_for(&pool, "owner", "mac", "  Swaraj's MacBook Pro ")
        .await
        .unwrap();
    for bad in ["", "   ", "line\nbreak"] {
        assert!(
            super::bridge_computers::rename_for(&pool, "owner", "mac", bad)
                .await
                .is_err()
        );
    }
    // Another owner's computer can't be renamed.
    assert!(
        super::bridge_computers::rename_for(&pool, "owner", "pc", "Mine")
            .await
            .is_err()
    );
    command(&pool, "w", "chat", "mac", "pwd", None)
        .await
        .unwrap();
    let pending = pending_for(&pool, "owner", "w", "chat").await.unwrap();
    assert_eq!(pending[0]["computerName"], "Swaraj's MacBook Pro");
}
