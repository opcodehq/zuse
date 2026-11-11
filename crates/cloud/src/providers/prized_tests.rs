use super::*;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use std::sync::{Arc, Mutex as StdMutex};

#[derive(Clone, Default)]
struct Seen(Arc<StdMutex<Vec<(String, Value)>>>);
impl Seen {
    fn push(&self, route: &str, value: Value) {
        self.0.lock().unwrap().push((route.into(), value));
    }
    fn routes(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(r, _)| r.clone())
            .collect()
    }
    fn body(&self, route: &str) -> Value {
        self.0
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(r, _)| r == route)
            .map(|(_, v)| v.clone())
            .unwrap()
    }
}

fn sandbox(id: &str, state: &str, name: &str, edge: &str) -> Value {
    json!({"id":id,"state":state,"resources":{"vcpu":2,"memoryMb":4096,"diskGb":10},
        "labels":{"zuse-allocation-name":name},"toolboxBase":format!("{edge}/{id}")})
}

async fn provider(seen: Seen) -> (Prized, tokio_util::task::AbortOnDropHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let edge = origin.clone();
    let app = Router::new()
        .route(
            "/api/v1/sandboxes/sbx_parent",
            get({
                let edge = edge.clone();
                move || async move { Json(json!({"sandbox":sandbox("sbx_parent","paused","base",&edge)})) }
            }),
        )
        .route(
            "/api/v1/sandboxes/sbx_parent/fork",
            post({
                let (seen, edge) = (seen.clone(), edge.clone());
                move |headers: HeaderMap, Query(q): Query<HashMap<String, String>>, Json(body): Json<Value>| {
                    let (seen, edge) = (seen.clone(), edge.clone());
                    async move {
                        assert_eq!(headers["authorization"], "Bearer dcp_test");
                        assert_eq!(headers["idempotency-key"], "operation-one");
                        assert_eq!(q["wait"], "running");
                        seen.push("fork", body);
                        (StatusCode::ACCEPTED, Json(json!({"sandboxes":[sandbox("sbx_child","running","task-one",&edge)]})))
                    }
                }
            }),
        )
        .route(
            "/api/v1/sandboxes",
            get({
                let edge = edge.clone();
                move |Query(q): Query<HashMap<String, String>>| {
                    let edge = edge.clone();
                    async move {
                        let rows = if q["label"] == "zuse-allocation-name=task-one" {
                            vec![sandbox("sbx_child","running","task-one",&edge), sandbox("sbx_old","deleted","task-one",&edge)]
                        } else {
                            vec![]
                        };
                        Json(json!({"sandboxes":rows}))
                    }
                }
            }),
        )
        .route(
            "/api/v1/sandboxes/sbx_child",
            get({
                let edge = edge.clone();
                move || async move { Json(json!({"sandbox":sandbox("sbx_child","running","task-one",&edge)})) }
            }),
        )
        .route(
            "/api/v1/sandboxes/sbx_child/timeout",
            post({
                let seen = seen.clone();
                move |Json(body): Json<Value>| {
                    let seen = seen.clone();
                    async move {
                        seen.push("timeout", body);
                        Json(json!({"capped":false}))
                    }
                }
            }),
        )
        .route(
            "/sbx_child/process/execute",
            post(|Json(body): Json<Value>| async move {
                assert_eq!(body["timeout"], 60);
                Json(json!({"stdout":"hello\n","stderr":"warn\n","exitCode":3,"durationMs":5}))
            }),
        )
        .route(
            "/sbx_child/files/upload",
            post({
                let seen = seen.clone();
                move |State(()): State<()>, Query(q): Query<HashMap<String, String>>, bytes: Bytes| {
                    let seen = seen.clone();
                    async move {
                        seen.push("upload", json!({"path":q["path"],"size":bytes.len()}));
                        Json(json!({"path":q["path"],"size":bytes.len()}))
                    }
                }
            }),
        )
        .with_state(());
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let mut provider = Prized::new("dcp_test".into()).unwrap();
    provider.rest.test_endpoint(format!("{origin}/api/v1"));
    provider.edge_origins = vec![origin];
    (provider, server)
}

#[tokio::test]
async fn a_computer_is_a_labeled_fork_of_the_paused_prepared_parent() {
    let seen = Seen::default();
    let (prized, _server) = provider(seen.clone()).await;
    let machine = prized
        .allocate(&Allocation {
            artifact: "sbx_parent",
            artifact_binding: None,
            name: "task-one",
            operation_id: "operation-one",
            size: "default",
            lifetime_seconds: 86400,
        })
        .await
        .unwrap();
    assert_eq!(
        (machine.id.as_str(), machine.name.as_str()),
        ("sbx_child", "task-one")
    );
    assert_eq!(machine.state, MachineState::Running);
    let fork = seen.body("fork");
    assert_eq!(fork["count"], 1);
    assert_eq!(fork["maxRuntimeMin"], 1440);
    assert_eq!(fork["labels"]["zuse-allocation-operation"], "operation-one");
    // Reconcile by label, ignoring deleted sandboxes with the same name.
    let found = prized.find_allocation("task-one").await.unwrap().unwrap();
    assert_eq!(found.id, "sbx_child");
    assert!(prized.find_allocation("absent").await.unwrap().is_none());
}

#[tokio::test]
async fn commands_uploads_and_running_limits_use_the_sandbox_edge() {
    let seen = Seen::default();
    let (prized, _server) = provider(seen.clone()).await;
    let result = prized.execute("sbx_child", "echo hello").await.unwrap();
    assert_eq!(
        (result.exit_code, result.stdout, result.stderr),
        (3, b"hello\n".to_vec(), b"warn\n".to_vec())
    );
    prized
        .upload("sbx_child", "/home/prized/.zuse/script.sh", b"echo")
        .await
        .unwrap();
    assert_eq!(seen.body("upload")["path"], "/home/prized/.zuse/script.sh");
    for outside in [
        "/tmp/script.sh",
        "/home/prized/../../etc/passwd",
        "relative",
    ] {
        assert_eq!(
            prized.upload("sbx_child", outside, b"x").await,
            Err(ProviderError::Rejected),
            "{outside}"
        );
    }
    assert!(
        prized.has_running_limit(),
        "the worker must visit Prized computers"
    );
    prized.keep_running("sbx_child").await.unwrap();
    assert_eq!(seen.body("timeout"), json!({"inMin":1440}));
    assert_eq!(seen.routes(), ["upload", "timeout"]);
}

#[tokio::test]
async fn only_a_paused_parent_or_a_template_is_a_valid_artifact() {
    let (prized, _server) = provider(Seen::default()).await;
    prized.verify_artifact("sbx_parent", None).await.unwrap();
    prized
        .verify_artifact("prized-base-v1", None)
        .await
        .unwrap();
    assert_eq!(
        prized.verify_artifact("sbx_child", None).await,
        Err(ProviderError::Conflict),
        "a running sandbox is not a quiet base"
    );
}
