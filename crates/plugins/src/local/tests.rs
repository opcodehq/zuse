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
