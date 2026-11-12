use super::*;
use serde_json::Map;

fn script(root: &Path) -> PathBuf {
    let path = root.join("server.py");
    std::fs::write(&path, r#"import sys,json,os
with open(sys.argv[1],'a') as f:f.write('started\n')
for line in sys.stdin:
 p=json.loads(line)
 if 'id' not in p:continue
 method=p.get('method')
 if method=='initialize':result={'protocolVersion':p['params']['protocolVersion'],'capabilities':{'tools':{}},'serverInfo':{'name':'fixture','version':'1'}}
 elif method=='tools/list':result={'tools':[{'name':'echo','description':'Echo a message','inputSchema':{'type':'object','properties':{'message':{'type':'string'}}}},{'name':'other','description':'Unrelated','inputSchema':{'type':'object'}}]}
 elif method=='tools/call':result={'content':[{'type':'text','text':json.dumps({'arguments':p['params'].get('arguments'),'token':os.environ.get('FIXTURE_TOKEN'),'keys':sorted(os.environ)})}],'isError':False}
 else:
  print(json.dumps({'jsonrpc':'2.0','id':p['id'],'error':{'code':-32601,'message':'unknown'}}),flush=True);continue
 print(json.dumps({'jsonrpc':'2.0','id':p['id'],'result':result}),flush=True)
"#).unwrap();
    path
}
fn store(root: &Path) -> LocalServers {
    LocalServers::new(root, "user-a", Arc::new(base_environment))
}
fn starts(marker: &Path) -> usize {
    std::fs::read_to_string(marker)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}
async fn add(store: &LocalServers, root: &Path, request: &str) -> Result<String, PluginError> {
    let marker = root.join("starts");
    let env = [("FIXTURE_TOKEN".to_owned(), "fixture-secret".to_owned())].into();
    match store
        .add(
            request,
            " Fixture ",
            "python3",
            vec![
                script(root).display().to_string(),
                marker.display().to_string(),
            ],
            env,
        )
        .await?
    {
        PluginResponse::Attempt { id, state, .. } => {
            assert_eq!(state, "connected");
            Ok(id)
        }
        _ => panic!("expected a connected attempt"),
    }
}

#[tokio::test]
async fn verified_servers_persist_privately_and_serve_the_gateway() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("starts");
    let local = store(root.path());
    let id = add(&local, root.path(), "request-1").await.unwrap();
    assert!(id.starts_with(LOCAL_SERVER_PREFIX));
    assert_eq!(starts(&marker), 1);
    // A replayed request returns the same server without launching another one.
    assert_eq!(add(&local, root.path(), "request-1").await.unwrap(), id);
    assert_eq!(starts(&marker), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&local.dir), 0o700);
        assert_eq!(mode(&local.path), 0o600);
    }
    assert!(!local.path.to_string_lossy().contains("user-a"));

    let (catalog, connections) = local.snapshot().await.unwrap();
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].name, "Fixture");
    assert!(catalog[0].custom);
    assert!(
        catalog[0]
            .command
            .as_deref()
            .unwrap()
            .starts_with("python3 ")
    );
    let wire = serde_json::to_string(&(catalog, connections)).unwrap();
    assert!(!wire.contains("fixture-secret") && !wire.contains("FIXTURE_TOKEN"));

    let listed = local.tools(&PluginToolRequest::List).await.unwrap();
    assert_eq!(listed[0]["connectionId"], id.as_str());
    let found = local
        .tools(&PluginToolRequest::Search {
            query: "fixture ECHO".into(),
        })
        .await
        .unwrap();
    assert_eq!(found.as_array().unwrap().len(), 1);
    let address = found[0]["address"].as_str().unwrap().to_owned();
    assert!(owns_address(&address));
    let schema = local
        .tools(&PluginToolRequest::Schema {
            address: address.clone(),
        })
        .await
        .unwrap();
    assert_eq!(schema["name"], "echo");
    let mut arguments = Map::new();
    arguments.insert("message".into(), json!("hello"));
    let called = local
        .tools(&PluginToolRequest::Call {
            address: address.clone(),
            arguments,
        })
        .await
        .unwrap();
    let text: Value = serde_json::from_str(called["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["arguments"]["message"], "hello");
    assert_eq!(text["token"], "fixture-secret");
    // Only the launch allowlist and the server's own variables are inherited.
    let allowed = base_environment();
    for key in text["keys"].as_array().unwrap() {
        let key = key.as_str().unwrap();
        assert!(
            allowed.contains_key(key) || matches!(key, "FIXTURE_TOKEN" | "LC_CTYPE"),
            "{key}"
        );
    }
    // The verification process is reused rather than relaunched.
    assert_eq!(starts(&marker), 1);

    local.set_enabled(&id, false).await.unwrap();
    assert!(
        local
            .tools(&PluginToolRequest::List)
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        local
            .tools(&PluginToolRequest::Schema {
                address: address.clone()
            })
            .await
            .is_err()
    );
    local.set_enabled(&id, true).await.unwrap();
    local
        .tools(&PluginToolRequest::Schema {
            address: address.clone(),
        })
        .await
        .unwrap();
    assert_eq!(starts(&marker), 2);

    // A new store for the same account reads the saved server; others do not.
    drop(local);
    assert_eq!(store(root.path()).snapshot().await.unwrap().0.len(), 1);
    let other = LocalServers::new(root.path(), "user-b", Arc::new(base_environment));
    assert!(other.snapshot().await.unwrap().0.is_empty());
    let local = store(root.path());
    local.remove(&id).await.unwrap();
    assert!(local.snapshot().await.unwrap().0.is_empty());
    assert!(local.remove(&id).await.is_err());
}

#[tokio::test]
async fn failed_verification_saves_nothing_and_malformed_files_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let local = store(root.path());
    for (command, args) in [
        (
            "python3",
            vec!["-c".to_owned(), "import sys; sys.exit(3)".to_owned()],
        ),
        ("zuse-command-that-does-not-exist", vec![]),
    ] {
        assert!(
            local
                .add("request", "Broken", command, args, BTreeMap::new())
                .await
                .is_err()
        );
    }
    assert!(!local.path.exists());
    assert!(
        local
            .add(
                "request",
                "Bad",
                "python3",
                vec![],
                [("1BAD".to_owned(), "v".to_owned())].into()
            )
            .await
            .is_err()
    );

    std::fs::create_dir_all(&local.dir).unwrap();
    std::fs::write(&local.path, b"{not json").unwrap();
    assert!(local.snapshot().await.is_err());
    assert!(local.tools(&PluginToolRequest::List).await.is_err());
    assert!(add(&local, root.path(), "request-2").await.is_err());
    assert_eq!(std::fs::read(&local.path).unwrap(), b"{not json");

    let rows: Vec<Value> = (0..MAX_LOCAL_SERVERS)
        .map(|i| json!({"id":format!("local-{i}"),"requestId":format!("r{i}"),"name":"Saved","command":"python3","args":[],"env":{},"enabled":false,"createdAt":0}))
        .collect();
    std::fs::write(&local.path, json!({"servers":rows}).to_string()).unwrap();
    assert_eq!(local.snapshot().await.unwrap().0.len(), MAX_LOCAL_SERVERS);
    let marker = root.path().join("starts");
    assert!(add(&local, root.path(), "request-3").await.is_err());
    assert_eq!(starts(&marker), 0);
}

/// A minimal streamable HTTP MCP server: JSON responses, one request per
/// connection, and every request's headers recorded for assertions.
async fn http_fixture() -> (String, Arc<StdMutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/mcp?team=query-secret",
        listener.local_addr().unwrap()
    );
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let log = log.clone();
            tokio::spawn(async move {
                let mut data = Vec::new();
                let mut buffer = [0u8; 4096];
                let end = loop {
                    let Ok(n) = socket.read(&mut buffer).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    data.extend_from_slice(&buffer[..n]);
                    if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                };
                let head = String::from_utf8_lossy(&data[..end]).to_lowercase();
                let length = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while data.len() < end + length {
                    let Ok(n) = socket.read(&mut buffer).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    data.extend_from_slice(&buffer[..n]);
                }
                log.lock().unwrap().push(head.clone());
                let reply = |status: &str, body: String| {
                    format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\nmcp-session-id: fixture-session\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                let response = if !head.contains("x-fixture-key: fixture-header-secret") {
                    reply("401 Unauthorized", String::new())
                } else if head.starts_with("get ") {
                    reply("405 Method Not Allowed", String::new())
                } else if head.starts_with("delete ") {
                    reply("200 OK", String::new())
                } else {
                    let p: Value = serde_json::from_slice(&data[end..end + length]).unwrap();
                    match p.get("id") {
                        None => reply("202 Accepted", String::new()),
                        Some(id) => {
                            let result = match p["method"].as_str() {
                                Some("initialize") => {
                                    json!({"protocolVersion":p["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"http-fixture","version":"1"}})
                                }
                                Some("tools/list") => {
                                    json!({"tools":[{"name":"echo","description":"Echo a message","inputSchema":{"type":"object"}}]})
                                }
                                Some("tools/call") => {
                                    json!({"content":[{"type":"text","text":p["params"]["arguments"]["message"]}],"isError":false})
                                }
                                _ => json!({}),
                            };
                            reply(
                                "200 OK",
                                json!({"jsonrpc":"2.0","id":id,"result":result}).to_string(),
                            )
                        }
                    }
                };
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (url, seen)
}

#[tokio::test]
async fn url_servers_connect_with_their_headers_and_never_show_them() {
    let root = tempfile::tempdir().unwrap();
    let (url, seen) = http_fixture().await;
    let local = store(root.path());
    let headers: BTreeMap<String, String> = [
        (
            "X-Fixture-Key".to_owned(),
            "fixture-header-secret".to_owned(),
        ),
        (
            "Authorization".to_owned(),
            "Bearer fixture-bearer".to_owned(),
        ),
    ]
    .into();
    // Wrong credentials fail verification and save nothing.
    assert!(
        local
            .add_url(
                "request-0",
                "Dev",
                &url,
                [("X-Fixture-Key".to_owned(), "wrong".to_owned())].into()
            )
            .await
            .is_err()
    );
    assert!(!local.path.exists());
    let PluginResponse::Attempt { id, state, .. } = local
        .add_url("request-1", " Dev ", &url, headers)
        .await
        .unwrap()
    else {
        panic!("expected a connected attempt");
    };
    assert_eq!(state, "connected");
    let (catalog, connections) = local.snapshot().await.unwrap();
    let shown = format!("http://{}/mcp", url.split('/').nth(2).unwrap());
    assert_eq!(catalog[0].command.as_deref(), Some(shown.as_str()));
    let wire = serde_json::to_string(&(catalog, connections)).unwrap();
    assert!(
        !wire.contains("secret") && !wire.contains("fixture-bearer"),
        "{wire}"
    );

    let found = local
        .tools(&PluginToolRequest::Search {
            query: "echo".into(),
        })
        .await
        .unwrap();
    let address = found[0]["address"].as_str().unwrap().to_owned();
    let mut arguments = Map::new();
    arguments.insert("message".into(), json!("hello"));
    let called = local
        .tools(&PluginToolRequest::Call { address, arguments })
        .await
        .unwrap();
    assert_eq!(called["content"][0]["text"], "hello");
    {
        let seen = seen.lock().unwrap();
        // Every configured header rides on each request, with the session echoed.
        assert!(
            seen.iter()
                .any(|h| h.contains("authorization: bearer fixture-bearer"))
        );
        assert!(
            seen.iter()
                .any(|h| h.contains("mcp-session-id: fixture-session"))
        );
    }
    // Saved privately and reloaded by a fresh store for the same account.
    drop(local);
    let local = store(root.path());
    assert_eq!(local.snapshot().await.unwrap().0.len(), 1);
    local.remove(&id).await.unwrap();
}

#[tokio::test]
async fn saved_rows_stay_compatible_and_mixed_rows_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let local = store(root.path());
    std::fs::create_dir_all(&local.dir).unwrap();
    // Files written before URL servers existed still load unchanged.
    let old = json!({"servers":[{"id":"local-old","requestId":"r","name":"Old","command":"python3","args":["-V"],"env":{"K":"v"},"enabled":true,"createdAt":1}]});
    std::fs::write(&local.path, old.to_string()).unwrap();
    let (catalog, _) = local.snapshot().await.unwrap();
    assert_eq!(catalog[0].command.as_deref(), Some("python3 -V"));
    // Stdio rows serialize exactly as before, so older builds keep reading them.
    local.set_enabled("local-old", false).await.unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&local.path).unwrap()).unwrap();
    let row = saved["servers"][0].as_object().unwrap();
    assert!(!row.contains_key("url") && !row.contains_key("headers"));

    let url = json!({"servers":[{"id":"local-url","requestId":"r","name":"Dev","command":"","args":[],"env":{},"url":"http://localhost:9/mcp?k=secret","headers":{"X-Key":"header-secret"},"enabled":false,"createdAt":1}]});
    std::fs::write(&local.path, url.to_string()).unwrap();
    let (catalog, _) = local.snapshot().await.unwrap();
    assert_eq!(
        catalog[0].command.as_deref(),
        Some("http://localhost:9/mcp")
    );
    assert!(!serde_json::to_string(&catalog).unwrap().contains("secret"));

    for row in [
        json!({"id":"local-x","requestId":"r","name":"Dev","command":"python3","args":[],"env":{},"url":"http://localhost:9/mcp","enabled":false,"createdAt":1}),
        json!({"id":"local-x","requestId":"r","name":"Dev","command":"python3","args":[],"env":{},"headers":{"X-Key":"v"},"enabled":false,"createdAt":1}),
        json!({"id":"local-x","requestId":"r","name":"Dev","command":"","args":[],"env":{},"url":"file:///etc/passwd","enabled":false,"createdAt":1}),
    ] {
        std::fs::write(&local.path, json!({"servers":[row]}).to_string()).unwrap();
        assert!(local.snapshot().await.is_err(), "{row}");
    }
}
