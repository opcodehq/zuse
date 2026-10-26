#![cfg(unix)]
use async_trait::async_trait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;
use zuse_harness::{
    native::RootStore,
    native_tools::{permission::RuntimeMode, *},
    processes::*,
    *,
};
struct Notices;
#[async_trait]
impl ProcessNotifier for Notices {
    async fn notify(&self, _: ProcessNotice) -> Result<(), Error> {
        Ok(())
    }
}
#[derive(Default)]
struct Callbacks {
    allow: AtomicBool,
    approvals: AtomicUsize,
    plans: Mutex<Vec<Vec<PlanStep>>>,
}
#[async_trait]
impl NativeCallbacks for Callbacks {
    fn approvals(&self) -> bool {
        true
    }
    fn questions(&self) -> bool {
        true
    }
    fn plans(&self) -> bool {
        true
    }
    async fn approve(&self, _: ApprovalRequest, _: ToolContext) -> Result<bool, Error> {
        self.approvals.fetch_add(1, Ordering::SeqCst);
        Ok(self.allow.load(Ordering::Acquire))
    }
    async fn question(
        &self,
        question: String,
        options: Option<Vec<String>>,
        _: ToolContext,
    ) -> Result<String, Error> {
        assert_eq!(question, "Which approach?");
        assert_eq!(options.unwrap(), ["A", "B"]);
        Ok("B".into())
    }
    async fn plan(&self, steps: Vec<PlanStep>, _: ToolContext) -> Result<(), Error> {
        self.plans.lock().unwrap().push(steps);
        Ok(())
    }
}
fn context(agent: &str) -> ToolContext {
    ToolContext {
        agent_id: agent.into(),
        call_id: uuid::Uuid::new_v4().to_string(),
        cancel: CancellationToken::new(),
        permission_mode: PermissionMode::Default,
        read_only: false,
    }
}
fn value(output: ToolOutput) -> Value {
    let ToolOutput::Text(text) = output else {
        panic!("Expected text")
    };
    serde_json::from_str(&text).unwrap()
}
struct Fixture {
    tools: Arc<NativeTools>,
    _owner: RootStore,
    root: tempfile::TempDir,
    checkout: PathBuf,
    mode: Arc<AtomicU8>,
}
impl Fixture {
    async fn new(callbacks: Option<Arc<dyn NativeCallbacks>>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        let owner = RootStore::acquire(root.path(), "root").unwrap();
        let mode = Arc::new(AtomicU8::new(0));
        let mut processes = ProcessOptions::new(
            "root".into(),
            owner.directory().join("artifacts"),
            Arc::new(Notices),
        )
        .unwrap();
        processes.command_host = env!("CARGO_BIN_EXE_zuse-command-host").into();
        let current = mode.clone();
        let tools = Arc::new(
            NativeTools::open(NativeOptions {
                cwd: checkout.clone(),
                lock_directory: root.path().join("harness/file-locks"),
                processes,
                runtime_mode: Arc::new(move || match current.load(Ordering::Acquire) {
                    1 => RuntimeMode::Auto,
                    2 => RuntimeMode::ApprovalRequired,
                    _ => RuntimeMode::FullAccess,
                }),
                callbacks,
                ripgrep: "rg".into(),
            })
            .await
            .unwrap(),
        );
        Self {
            tools,
            _owner: owner,
            root,
            checkout,
            mode,
        }
    }
    async fn call(&self, name: &str, input: Value) -> Result<ToolOutput, Error> {
        self.tools.execute(name, input, context("root")).await
    }
    async fn read(&self, path: &str) -> Value {
        value(self.call("read_file", json!({"path":path})).await.unwrap())
    }
}
#[tokio::test]
async fn file_edits_require_fresh_hashes_and_commit_all_replacements_together() {
    let fixture = Fixture::new(None).await;
    assert!(
        fixture
            .call("write_file", json!({"path":"missing.txt","content":"x"}))
            .await
            .is_err()
    );
    assert!(!fixture.checkout.join("missing.txt").exists());
    fixture
        .call(
            "write_file",
            json!({"path":"src/script","content":"alpha beta\n","expected_hash":null}),
        )
        .await
        .unwrap();
    std::fs::set_permissions(
        fixture.checkout.join("src/script"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let initial = fixture.read("src/script").await;
    assert!(fixture.call("multi_edit",json!({"path":"src/script","expected_hash":initial["hash"],"edits":[{"old_string":"alpha","new_string":"first"},{"old_string":"missing","new_string":"second"}]})).await.is_err());
    assert_eq!(
        std::fs::read_to_string(fixture.checkout.join("src/script")).unwrap(),
        "alpha beta\n"
    );
    fixture.call("multi_edit",json!({"path":"src/script","expected_hash":initial["hash"],"edits":[{"old_string":"alpha","new_string":"first"},{"old_string":"beta","new_string":"second"}]})).await.unwrap();
    assert_eq!(
        fixture.read("src/script").await["content"],
        "first second\n"
    );
    assert_eq!(
        std::fs::metadata(fixture.checkout.join("src/script"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert!(
        fixture
            .call(
                "write_file",
                json!({"path":"src/script","content":"overwrite","expected_hash":null})
            )
            .await
            .is_err()
    );
    assert!(fixture.call("edit_file",json!({"path":"src/script","expected_hash":initial["hash"],"old_string":"first","new_string":"stale"})).await.is_err());
    std::fs::write(fixture.checkout.join("overlap"), "aaa").unwrap();
    let overlap = fixture.read("overlap").await;
    assert!(fixture.call("edit_file",json!({"path":"overlap","expected_hash":overlap["hash"],"old_string":"aa","new_string":"b"})).await.is_err());
    fixture.tools.close().await;
}
#[tokio::test]
async fn checkout_paths_cannot_escape_through_traversal_or_symlinks() {
    let fixture = Fixture::new(None).await;
    let outside = fixture.root.path().join("outside.txt");
    std::fs::write(&outside, "private outside").unwrap();
    symlink(&outside, fixture.checkout.join("escape")).unwrap();
    for path in ["../outside.txt", "escape"] {
        assert!(
            fixture
                .call("read_file", json!({"path":path}))
                .await
                .is_err()
        );
        assert!(
            fixture
                .call(
                    "write_file",
                    json!({"path":path,"content":"bad","expected_hash":null})
                )
                .await
                .is_err()
        );
    }
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "private outside");
    std::fs::write(fixture.checkout.join("ordinary"), "inside").unwrap();
    symlink("ordinary", fixture.checkout.join("alias")).unwrap();
    assert_eq!(fixture.read("alias").await["content"], "inside");
    std::fs::write(fixture.checkout.join(".env"), "sensitive").unwrap();
    symlink(".env", fixture.checkout.join("innocent-name")).unwrap();
    assert!(
        fixture
            .call("read_file", json!({"path":"innocent-name"}))
            .await
            .is_err()
    );
    let entries = value(fixture.call("list_directory", json!({})).await.unwrap());
    assert!(
        entries
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "escape" && e["type"] == "symlink")
    );
    fixture.tools.close().await;
}
#[tokio::test]
async fn read_only_context_and_runtime_policy_cannot_be_overridden_by_tool_arguments() {
    let callback = Arc::new(Callbacks::default());
    let fixture = Fixture::new(Some(callback.clone())).await;
    let mut readonly = context("child");
    readonly.read_only = true;
    assert!(
        fixture
            .tools
            .execute(
                "write_file",
                json!({"path":"denied","content":"x","expected_hash":null}),
                readonly
            )
            .await
            .is_err()
    );
    assert!(!fixture.checkout.join("denied").exists());
    assert_eq!(callback.approvals.load(Ordering::SeqCst), 0);
    fixture.mode.store(1, Ordering::Release);
    let mut accepts = context("root");
    accepts.permission_mode = PermissionMode::AcceptEdits;
    assert!(
        fixture
            .tools
            .execute(
                "write_file",
                json!({"path":"denied","content":"x","expected_hash":null}),
                accepts.clone()
            )
            .await
            .is_err()
    );
    fixture.mode.store(2, Ordering::Release);
    fixture
        .tools
        .execute(
            "write_file",
            json!({"path":"allowed","content":"x","expected_hash":null}),
            accepts,
        )
        .await
        .unwrap();
    assert!(fixture.call("write_file",json!({"path":"denied","content":"x","expected_hash":null,"permissionMode":"full-access"})).await.is_err());
    fixture.mode.store(0, Ordering::Release);
    std::fs::write(fixture.checkout.join(".env"), "secret").unwrap();
    assert!(
        fixture
            .call("read_file", json!({"path":".env"}))
            .await
            .is_err()
    );
    callback.allow.store(true, Ordering::Release);
    assert_eq!(fixture.read(".env").await["content"], "secret");
    fixture.tools.close().await;
}
#[tokio::test]
async fn concurrent_roots_share_file_locks_and_only_one_stale_hash_writer_commits() {
    let fixture = Fixture::new(None).await;
    std::fs::write(fixture.checkout.join("shared"), "initial").unwrap();
    let initial = fixture.read("shared").await;
    let owner = RootStore::acquire(fixture.root.path(), "root-two").unwrap();
    let processes = ProcessOptions::new(
        "root-two".into(),
        owner.directory().join("artifacts"),
        Arc::new(Notices),
    )
    .unwrap();
    let second = NativeTools::open(NativeOptions {
        cwd: fixture.checkout.clone(),
        lock_directory: fixture.root.path().join("harness/file-locks"),
        processes,
        runtime_mode: Arc::new(|| RuntimeMode::FullAccess),
        callbacks: None,
        ripgrep: "rg".into(),
    })
    .await
    .unwrap();
    let left = fixture.tools.execute(
        "write_file",
        json!({"path":"shared","content":"left","expected_hash":initial["hash"]}),
        context("root"),
    );
    let right = second.execute(
        "write_file",
        json!({"path":"shared","content":"right","expected_hash":initial["hash"]}),
        context("root-two"),
    );
    let (left, right) = tokio::join!(left, right);
    assert_ne!(left.is_ok(), right.is_ok());
    let content = std::fs::read_to_string(fixture.checkout.join("shared")).unwrap();
    assert!(content == "left" || content == "right");
    second.close().await;
    fixture.tools.close().await;
}
#[tokio::test]
async fn large_reads_preserve_edit_metadata_and_binary_and_image_reads_stay_bounded() {
    let fixture = Fixture::new(None).await;
    let large = "界".repeat(20000);
    std::fs::write(fixture.checkout.join("large"), &large).unwrap();
    let result = fixture.read("large").await;
    assert_eq!(result["truncated"], true);
    assert!(result["hash"].as_str().unwrap().len() == 64);
    let artifact = PathBuf::from(result["artifact"].as_str().unwrap());
    let full: Value = serde_json::from_slice(&std::fs::read(&artifact).unwrap()).unwrap();
    assert_eq!(full["content"], large);
    assert_eq!(full["hash"], result["hash"]);
    let mut cursor = 0u64;
    let mut retained = String::new();
    loop {
        let page = value(
            fixture
                .call(
                    "read_artifact",
                    json!({"artifact":artifact,"offset":cursor,"max_bytes":16000}),
                )
                .await
                .unwrap(),
        );
        retained.push_str(page["text"].as_str().unwrap());
        cursor = page["next_cursor"].as_u64().unwrap();
        if page["has_more"] == false {
            break;
        }
    }
    assert_eq!(serde_json::from_str::<Value>(&retained).unwrap(), full);
    assert!(
        fixture
            .call("read_artifact", json!({"artifact":"../../checkpoint.json"}))
            .await
            .is_err()
    );
    let linked = artifact
        .parent()
        .unwrap()
        .join(format!("{}.result.json", uuid::Uuid::new_v4()));
    std::os::unix::fs::symlink(fixture.checkout.join("large"), &linked).unwrap();
    assert!(
        fixture
            .call("read_artifact", json!({"artifact":linked}))
            .await
            .is_err()
    );

    assert_eq!(
        std::fs::metadata(artifact).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::write(fixture.checkout.join("binary"), b"a\0b").unwrap();
    assert!(
        fixture
            .call("read_file", json!({"path":"binary"}))
            .await
            .is_err()
    );
    std::fs::write(
        fixture.checkout.join("oversized"),
        vec![b'x'; 2 * 1024 * 1024 + 1],
    )
    .unwrap();
    assert!(
        fixture
            .call("read_file", json!({"path":"oversized"}))
            .await
            .is_err()
    );
    std::fs::write(fixture.checkout.join("image.png"), b"\x89PNG\r\n\x1a\n").unwrap();
    let output = fixture
        .call("read_image", json!({"path":"image.png"}))
        .await
        .unwrap();
    assert!(output.validate());
    assert!(matches!(output, ToolOutput::Content { .. }));
    fixture.tools.close().await;
}
#[tokio::test]
async fn tool_dispatch_keeps_background_commands_alive_and_routes_search_and_questions() {
    let callback = Arc::new(Callbacks::default());
    let fixture = Fixture::new(Some(callback.clone())).await;
    let turn = context("root");
    let execution = value(
        fixture
            .tools
            .execute(
                "exec_command",
                json!({"command":"read line; printf '%s' \"$line\"","yield_time_ms":0}),
                turn.clone(),
            )
            .await
            .unwrap(),
    );
    let id = execution["process"]["id"].as_str().unwrap();
    turn.cancel.cancel();
    fixture
        .call(
            "write_stdin",
            json!({"process_id":id,"data":"hello\n","eof":true}),
        )
        .await
        .unwrap();
    let completed = value(
        fixture
            .call("wait_process", json!({"process_id":id,"timeout_ms":3000}))
            .await
            .unwrap(),
    );
    assert_eq!(completed["process"]["exitCode"], 0);
    assert_eq!(completed["text"], "hello");
    std::fs::write(fixture.checkout.join("needle.txt"), "needle text\n").unwrap();
    let grep = value(
        fixture
            .call("grep", json!({"pattern":"needle","literal":true}))
            .await
            .unwrap(),
    );
    assert_eq!(grep["process"]["exitCode"], 0);
    assert!(
        grep["text"]
            .as_str()
            .unwrap()
            .contains("needle.txt:1:needle text")
    );
    let glob = value(
        fixture
            .call("glob", json!({"pattern":"*.txt"}))
            .await
            .unwrap(),
    );
    assert!(glob["text"].as_str().unwrap().contains("needle.txt"));
    assert!(fixture.call("update_plan",json!({"steps":[{"step":"A","status":"in_progress"},{"step":"B","status":"in_progress"}]})).await.is_err());
    fixture.call("update_plan",json!({"steps":[{"step":"A","status":"completed"},{"step":"B","status":"in_progress"}]})).await.unwrap();
    assert_eq!(callback.plans.lock().unwrap().len(), 1);
    assert!(
        matches!(fixture.call("request_user_input",json!({"question":"Which approach?","options":["A","B"]})).await.unwrap(),ToolOutput::Text(answer) if answer=="B")
    );
    fixture.tools.close().await;
}
#[tokio::test]
async fn cancellation_during_an_edit_lock_prevents_late_filesystem_commits() {
    let fixture = Fixture::new(None).await;
    let path = fixture.checkout.join("locked");
    std::fs::write(&path, "before").unwrap();
    let initial = fixture.read("locked").await;
    use std::os::unix::ffi::OsStrExt;
    let key = format!("{:x}", Sha256::digest(path.as_os_str().as_bytes()));
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(
            fixture
                .root
                .path()
                .join(format!("harness/file-locks/{key}.lock")),
        )
        .unwrap();
    lock.try_lock().unwrap();
    for abort in [false, true] {
        let context = context("root");
        let cancel = context.cancel.clone();
        let tools = fixture.tools.clone();
        let hash = initial["hash"].clone();
        let edit = tokio::spawn(async move {
            tools
                .execute(
                    "write_file",
                    json!({"path":"locked","content":"after","expected_hash":hash}),
                    context,
                )
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        if abort {
            edit.abort();
            assert!(edit.await.unwrap_err().is_cancelled());
        } else {
            cancel.cancel();
            assert!(matches!(edit.await.unwrap(), Err(Error::Interrupted)));
        }
    }
    fixture.tools.close().await;
    lock.unlock().unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), "before");
}

struct KernelHost {
    requests: AtomicUsize,
    events: Mutex<Vec<Event>>,
}
#[async_trait]
impl Host for KernelHost {
    fn tools(&self) -> Tools {
        Tools::new()
    }
    async fn load(&self) -> Result<Option<Checkpoint>, Error> {
        panic!("native composition owns its journal")
    }
    async fn save(&self, _: &Checkpoint) -> Result<(), Error> {
        panic!("native composition owns its journal")
    }
    async fn instructions(&self) -> Result<String, Error> {
        Ok("Complete the fixture coding task.".into())
    }
    async fn model(&self, input: ModelInput) -> Result<ModelOutput, Error> {
        let turn = self.requests.fetch_add(1, Ordering::SeqCst);
        let calls = match turn {
            0 => vec![Call {
                id: "write".into(),
                name: "write_file".into(),
                input: json!({"path":"result.txt","content":"native result\n","expected_hash":null}),
            }],
            1 => vec![Call {
                id: "read".into(),
                name: "read_file".into(),
                input: json!({"path":"result.txt"}),
            }],
            2 => vec![Call {
                id: "command".into(),
                name: "exec_command".into(),
                input: json!({"command":"cat result.txt","yield_time_ms":1000}),
            }],
            _ => {
                assert!(
                    input
                        .messages
                        .iter()
                        .filter(|m| m.role == Role::Tool)
                        .any(|m| m.content.to_string().contains("native result"))
                );
                vec![]
            }
        };
        let text = if calls.is_empty() {
            "Completed native task"
        } else {
            ""
        };
        let mut parts = vec![json!({"type":"text","text":text})];
        parts.extend(calls.iter().map(|call|json!({"type":"tool-call","toolCallId":call.id,"toolName":call.name,"input":call.input})));
        Ok(ModelOutput {
            messages: vec![Message {
                role: Role::Assistant,
                content: Value::Array(parts).into(),
                metadata: Default::default(),
            }],
            calls,
            text: text.into(),
            usage: Usage::default(),
        })
    }
    async fn execute(&self, _: &str, _: Value, _: ToolContext) -> Result<ToolOutput, Error> {
        panic!("native dispatch must own these tools")
    }
    fn notify(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }
}
#[tokio::test]
async fn native_tools_run_through_the_durable_kernel_and_root_owned_host_composition() {
    let root = tempfile::tempdir().unwrap();
    let checkout = root.path().join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    let application = Arc::new(KernelHost {
        requests: AtomicUsize::new(0),
        events: Mutex::new(vec![]),
    });
    let app = application.clone();
    let cwd = checkout.clone();
    let locks = root.path().join("harness/file-locks");
    let engine = Engine::open_native_with(
        root.path(),
        "root",
        Options::new("chatgpt/fixture"),
        move |directory| async move {
            let mut processes = ProcessOptions::new(
                "root".into(),
                directory.join("artifacts"),
                Arc::new(Notices),
            )?;
            processes.command_host = env!("CARGO_BIN_EXE_zuse-command-host").into();
            let tools = Arc::new(
                NativeTools::open(NativeOptions {
                    cwd,
                    lock_directory: locks,
                    processes,
                    runtime_mode: Arc::new(|| RuntimeMode::FullAccess),
                    callbacks: None,
                    ripgrep: "rg".into(),
                })
                .await?,
            );
            tools.attach(app)
        },
    )
    .await
    .unwrap();
    engine
        .send("Write, read, and inspect a local result.".into(), vec![])
        .await
        .unwrap();
    let state = engine.snapshot().await;
    assert_eq!(state.tools.len(), 3);
    assert!(
        state
            .tools
            .values()
            .all(|receipt| matches!(receipt.status, ReceiptStatus::Completed))
    );
    assert_eq!(
        std::fs::read_to_string(checkout.join("result.txt")).unwrap(),
        "native result\n"
    );
    assert!(application.events.lock().unwrap().iter().any(
        |event| matches!(event,Event::Completed{summary,..} if summary=="Completed native task")
    ));
    engine.close().await.unwrap();
    let recovered = RootStore::acquire(root.path(), "root").unwrap();
    assert_eq!(recovered.load().unwrap().unwrap().tools.len(), 3);
}
