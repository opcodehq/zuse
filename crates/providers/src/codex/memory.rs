//! Strict internal retained-wake inspection. Only owned, idle native sessions
//! without remote MCP or child agents may participate in socket preservation.
use super::*;
use sha2::{Digest, Sha256};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
#[derive(Default)]
pub struct ManagedMemorySessions {
    sessions: Mutex<HashMap<u32, Live>>,
}
#[derive(Clone)]
struct Live {
    client: RpcClient,
    thread: String,
    idle: Arc<AtomicBool>,
    turn: crate::TurnControl,
}
#[derive(Clone, PartialEq, Eq)]
pub struct IdleNativeProcess {
    pub pid: u32,
    pub thread_id: String,
    pub config_digest: String,
}
pub(super) struct Registration {
    registry: Arc<ManagedMemorySessions>,
    pid: u32,
    pub idle: Arc<AtomicBool>,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.registry
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.pid);
    }
}
fn failure() -> HarnessError {
    HarnessError::Protocol("codex_memory_transport_recovery_required".into())
}
fn no_remote_mcp(config: &Value) -> bool {
    let Some(config) = config.as_object() else {
        return false;
    };
    match config.get("mcp_servers") {
        None | Some(Value::Null) => true,
        Some(Value::Object(servers)) => servers.values().all(|server| {
            server["enabled"] == false
                || (server["command"].as_str().is_some_and(|s| !s.is_empty())
                    && server.get("url").is_none_or(Value::is_null))
        }),
        _ => false,
    }
}
fn native_vendor_only(config: &Value) -> bool {
    if !no_remote_mcp(config) {
        return false;
    }
    if config
        .get("model_provider")
        .is_some_and(|v| !v.is_null() && v != "openai")
    {
        return false;
    }
    if config
        .get("model_providers")
        .is_some_and(|v| !v.is_null() && !v.as_object().is_some_and(|v| v.is_empty()))
    {
        return false;
    }
    if config.get("chatgpt_base_url").is_some_and(|v| {
        !v.is_null()
            && v != "https://chatgpt.com/backend-api"
            && v != "https://chatgpt.com/backend-api/"
    }) {
        return false;
    }
    // Installed remote app connectors can share the native process's public
    // sockets. Absence is allowed; explicitly enabled/ambiguous app entries
    // require localized recovery rather than guessing which socket is model I/O.
    if config.get("apps").is_some_and(|apps| {
        !apps.is_null()
            && !apps
                .as_object()
                .is_some_and(|entries| entries.values().all(|entry| entry["enabled"] == false))
    }) {
        return false;
    }
    [
        "proxy",
        "proxy_url",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "openai_base_url",
    ]
    .iter()
    .all(|key| config.get(key).is_none_or(|v| v.is_null() || v == ""))
}
impl ManagedMemorySessions {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub(super) fn register(
        self: &Arc<Self>,
        pid: u32,
        client: RpcClient,
        thread: String,
        turn: crate::TurnControl,
    ) -> Registration {
        let idle = Arc::new(AtomicBool::new(false));
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                pid,
                Live {
                    client,
                    thread,
                    idle: idle.clone(),
                    turn,
                },
            );
        Registration {
            registry: self.clone(),
            pid,
            idle,
        }
    }
    pub async fn inspect_idle(&self) -> Result<Vec<IdleNativeProcess>, HarnessError> {
        let sessions: Vec<_> = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(pid, live)| (*pid, live.clone()))
            .collect();
        if sessions.len() > 16 {
            return Err(failure());
        }
        let mut result = Vec::with_capacity(sessions.len());
        for (pid, live) in sessions {
            if !live.idle.load(Ordering::Acquire) || live.turn.background_live() {
                return Err(failure());
            }
            let check = async {
                let thread = live
                    .client
                    .request(
                        "thread/read",
                        json!({"threadId":live.thread,"includeTurns":false}),
                    )
                    .await?;
                if thread["thread"]["id"] != live.thread
                    || thread["thread"]["status"]["type"] != "idle"
                {
                    return Err(failure());
                }
                let config = live
                    .client
                    .request("config/read", json!({"includeLayers":false}))
                    .await?;
                if !native_vendor_only(&config["config"]) {
                    return Err(failure());
                }
                let encoded = zeroize::Zeroizing::new(
                    serde_json::to_vec(&config["config"]).map_err(|_| failure())?,
                );
                let digest = Sha256::digest(&encoded)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>();
                if !live.idle.load(Ordering::Acquire) || live.turn.background_live() {
                    return Err(failure());
                }
                Ok(IdleNativeProcess {
                    pid,
                    thread_id: live.thread.clone(),
                    config_digest: digest,
                })
            };
            result.push(
                tokio::time::timeout(Duration::from_secs(3), check)
                    .await
                    .map_err(|_| failure())??,
            );
        }
        result.sort_by_key(|entry| entry.pid);
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_mcp_cannot_be_mistaken_for_a_vendor_socket() {
        assert!(no_remote_mcp(&json!({})));
        assert!(no_remote_mcp(
            &json!({"mcp_servers":{"local":{"command":"/trusted/mcp"}}})
        ));
        assert!(!no_remote_mcp(
            &json!({"mcp_servers":{"remote":{"url":"https://remote.invalid/mcp"}}})
        ));
        assert!(!no_remote_mcp(&json!({"mcp_servers":{"ambiguous":{}}})));
        assert!(!no_remote_mcp(&json!({"mcp_servers":[]})));
        assert!(!no_remote_mcp(&Value::Null));
        assert!(native_vendor_only(&json!({"model_provider":"openai"})));
        assert!(!native_vendor_only(&json!({"model_provider":"custom"})));
        assert!(!native_vendor_only(
            &json!({"model_providers":{"openai":{"base_url":"https://other.invalid"}}})
        ));
        assert!(!native_vendor_only(
            &json!({"apps":{"remote":{"enabled":true}}})
        ));
        assert!(!native_vendor_only(
            &json!({"chatgpt_base_url":"https://other.invalid"})
        ));
        assert!(!native_vendor_only(
            &json!({"https_proxy":"http://proxy.invalid"})
        ));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn owned_idle_inspection_is_bound_to_thread_config_and_registration_lifetime() {
        let mut child = tokio::process::Command::new("python3")
            .args([
                "-u",
                "-c",
                r#"import sys,json
reads=0
for line in sys.stdin:
 r=json.loads(line)
 if 'id' not in r: continue
 if r['method']=='thread/read': result={'thread':{'id':'owned-thread','status':{'type':'idle'}}}
 elif r['method']=='config/read':
  reads+=1
  result={'config':{'model_provider':'openai','model':'first' if reads==1 else 'changed'}}
 else: result={}
 print(json.dumps({'id':r['id'],'result':result}),flush=True)
"#,
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        let (client, _incoming) =
            RpcClient::new(child.stdin.take().unwrap(), child.stdout.take().unwrap());
        let registry = ManagedMemorySessions::new();
        let registration = registry.register(
            pid,
            client,
            "owned-thread".into(),
            crate::TurnControl::default(),
        );
        assert!(
            registry.inspect_idle().await.is_err(),
            "not-yet-idle session must fail before native reads"
        );
        registration.idle.store(true, Ordering::Release);
        let before = registry.inspect_idle().await.unwrap();
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].pid, pid);
        assert_eq!(before[0].thread_id, "owned-thread");
        let after = registry.inspect_idle().await.unwrap();
        assert!(
            before != after,
            "effective config changes must invalidate retained snapshot binding"
        );
        registration.idle.store(false, Ordering::Release);
        assert!(registry.inspect_idle().await.is_err());
        drop(registration);
        assert!(registry.inspect_idle().await.unwrap().is_empty());
        child.kill().await.unwrap();
        let _ = child.wait().await;
    }
}
