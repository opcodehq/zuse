use serde_json::{Value, json};
use std::{path::Path, sync::Arc};
use tokio_util::sync::CancellationToken;
use zuse_harness::{Error, PermissionMode, ToolContext, ToolOutput};
use zuse_harness_extensions::Registry;
fn context() -> ToolContext {
    ToolContext {
        agent_id: "agent".into(),
        call_id: uuid(),
        cancel: CancellationToken::new(),
        permission_mode: PermissionMode::Default,
        read_only: false,
    }
}
fn uuid() -> String {
    format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}
fn configure(profile: &Path, value: Value) {
    std::fs::create_dir_all(profile.join("harness")).unwrap();
    std::fs::write(profile.join("harness/extensions.json"), value.to_string()).unwrap();
}
fn script(root: &Path) -> std::path::PathBuf {
    let path = root.join("peer.py");
    std::fs::write(&path,r#"import sys,json,os
with open(sys.argv[1],'a') as f:f.write('started\n')
for line in sys.stdin:
 p=json.loads(line)
 if 'id' not in p:continue
 method=p.get('method')
 if method=='initialize':result={'protocolVersion':p['params']['protocolVersion'],'capabilities':{'tools':{}},'serverInfo':{'name':'fixture','version':'1'}}
 elif method=='tools/list':result={'tools':[{'name':'echo','description':'Echo input','inputSchema':{'type':'object','properties':{'message':{'type':'string'}}}}]}
 elif method=='tools/call':
  if p['params']['name']=='disconnect':
   with open(sys.argv[1],'a') as f:f.write('effect\n')
   os._exit(0)
  if p['params']['name']=='hold':
   import subprocess,time
   child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(120)'])
   with open(sys.argv[1]+'.child','w') as f:f.write(str(child.pid))
   with open(sys.argv[1],'a') as f:f.write('hold-effect\n')
   time.sleep(120)
  if p['params']['name']=='oversized':
   print('x'*(1024*1024+1),flush=True);continue
  if p['params']['name']=='image':result={'content':[{'type':'text','text':'preview'},{'type':'image','mimeType':'image/png','data':'AQID'}],'isError':False}
  elif p['params']['name']=='known-error':result={'content':[{'type':'text','text':'invalid fixture arguments'}],'isError':True}
  else:result={'content':[{'type':'text','text':json.dumps({'arguments':p['params'].get('arguments'), 'env_keys':list(os.environ)})}],'isError':False}
 else:
  print(json.dumps({'jsonrpc':'2.0','id':p['id'],'error':{'code':-32601,'message':'unknown'}}),flush=True);continue
 print(json.dumps({'jsonrpc':'2.0','id':p['id'],'result':result}),flush=True)
"#).unwrap();
    path
}
#[tokio::test]
async fn lazy_singleflight_pool_isolates_checkouts_and_does_not_inherit_model_secrets() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("starts");
    let peer = script(root.path());
    configure(
        root.path(),
        json!({"servers":{"fixture":{"trusted":true,"connection":{"type":"stdio","executable":"python3","args":[peer,marker]},"readOnlyTools":["echo"]}}}),
    );
    let registry = Registry::load(root.path()).unwrap();
    assert!(!marker.exists());
    let a = registry
        .scope(root.path().into(), Arc::new(|| "policy-a".into()))
        .unwrap();
    let b = registry
        .scope(root.path().into(), Arc::new(|| "policy-a".into()))
        .unwrap();
    assert!(a.read_only("fixture", "echo"));
    assert!(!a.read_only("fixture", "disconnect"));
    let (one, two) = tokio::join!(
        a.tools("fixture", None, context()),
        b.tools("fixture", None, context())
    );
    assert_eq!(one.unwrap()["tools"][0]["name"], "echo");
    two.unwrap();
    assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 1);
    let ToolOutput::Text(output) = a
        .call("fixture", "echo", json!({"message":"hello"}), context())
        .await
        .unwrap()
    else {
        panic!()
    };
    let result: Value = serde_json::from_str(&output).unwrap();
    let response: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(response["arguments"]["message"], "hello");
    assert!(
        !response["env_keys"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "CLAUDE_CODE_OAUTH_TOKEN"
                || v == "OPENAI_API_KEY"
                || v == "ANTHROPIC_API_KEY")
    );
    let other = root.path().join("other");
    std::fs::create_dir(&other).unwrap();
    registry
        .scope(other, Arc::new(|| "policy-a".into()))
        .unwrap()
        .tools("fixture", None, context())
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 2);
    let result = a.call("fixture", "disconnect", json!({}), context()).await;
    assert!(matches!(result, Err(Error::Paused(_))));
    assert_eq!(
        std::fs::read_to_string(&marker)
            .unwrap()
            .lines()
            .filter(|s| *s == "effect")
            .count(),
        1
    );
}
#[tokio::test]
async fn untrusted_configuration_cannot_start_a_process() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("starts");
    let peer = script(root.path());
    configure(
        root.path(),
        json!({"servers":{"fixture":{"trusted":false,"connection":{"type":"stdio","executable":"python3","args":[peer,marker]}}}}),
    );
    let registry = Registry::load(root.path()).unwrap();
    let scope = registry
        .scope(root.path().into(), Arc::new(|| "default".into()))
        .unwrap();
    assert!(scope.tools("fixture", None, context()).await.is_err());
    assert!(!marker.exists());
}

#[tokio::test]
async fn multimodal_results_remain_structured_and_oversized_frames_pause_without_retry() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("starts");
    let peer = script(root.path());
    configure(
        root.path(),
        json!({"servers":{"fixture":{"trusted":true,"connection":{"type":"stdio","executable":"python3","args":[peer,marker]}}}}),
    );
    let registry = Registry::load(root.path()).unwrap();
    let scope = registry
        .scope(root.path().into(), Arc::new(|| "fixture".into()))
        .unwrap();
    let output = scope
        .call("fixture", "image", json!({}), context())
        .await
        .unwrap();
    assert!(output.validate());
    let ToolOutput::Mixed { parts, .. } = output else {
        panic!("image must not become base64 text")
    };
    assert_eq!(parts[0]["text"], "preview");
    assert_eq!(parts[1]["data"]["data"], "AQID");
    assert!(
        matches!(
            scope
                .call("fixture", "known-error", json!({}), context())
                .await,
            Err(Error::Tool(_))
        ),
        "definitive server failures must use the kernel failure path"
    );
    assert!(matches!(
        scope
            .call("fixture", "oversized", json!({}), context())
            .await,
        Err(Error::Paused(_))
    ));
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap().lines().count(),
        1,
        "no automatic restart or replay"
    );
    // A separate explicit operation can establish a new channel after review.
    scope.tools("fixture", None, context()).await.unwrap();
    assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 2);
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_does_not_repeat_a_call_and_shutdown_reaps_its_process_group() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("starts");
    let peer = script(root.path());
    configure(
        root.path(),
        json!({"servers":{"fixture":{"trusted":true,"connection":{"type":"stdio","executable":"python3","args":[peer,marker]}}}}),
    );
    let registry = Registry::load(root.path()).unwrap();
    let scope = registry
        .scope(root.path().into(), Arc::new(|| "fixture".into()))
        .unwrap();
    let ctx = context();
    let cancel = ctx.cancel.clone();
    let call_scope = scope.clone();
    let call =
        tokio::spawn(async move { call_scope.call("fixture", "hold", json!({}), ctx).await });
    let child_file = marker.with_extension("child");
    // The fixture writes starts.child only after the remote side effect.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !child_file.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let child: i32 = std::fs::read_to_string(child_file)
        .unwrap()
        .parse()
        .unwrap();
    cancel.cancel();
    assert!(matches!(call.await.unwrap(), Err(Error::Interrupted)));
    assert_eq!(
        std::fs::read_to_string(&marker)
            .unwrap()
            .lines()
            .filter(|l| *l == "hold-effect")
            .count(),
        1
    );
    drop(scope);
    drop(registry);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let gone = unsafe { libc::kill(child, 0) } != 0;
            let zombie = std::fs::read_to_string(format!("/proc/{child}/stat"))
                .is_ok_and(|s| s.split_whitespace().nth(2) == Some("Z"));
            if gone || zombie {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("external descendants must stop with their owned server");
}
