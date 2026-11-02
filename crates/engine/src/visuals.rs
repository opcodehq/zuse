//! Each subscriber owns a bounded, thread-confined Luau runtime. Disconnect
//! drops the subscription and retires the runtime, including its opaque lease.
use futures::{StreamExt, stream::BoxStream};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use tokio::sync::{Semaphore, oneshot};
use zuse_protocol::visual_session as wire;
use zuse_rpc::RpcError;
use zuse_visual::{HandlerArg, HandlerId, Node, NodeKind, Visual};

#[derive(Clone)]
pub(crate) struct Visuals {
    inner: Arc<Inner>,
}
struct Inner {
    slots: Arc<Semaphore>,
    closed: AtomicBool,
    sessions: Mutex<HashMap<String, mpsc::SyncSender<Command>>>,
}
enum Command {
    Control(wire::Control, oneshot::Sender<Result<Value, String>>),
    Draw(wire::Draw, oneshot::Sender<Result<Value, String>>),
}
struct Registration {
    inner: Arc<Inner>,
    lease: String,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.inner.sessions.lock().unwrap().remove(&self.lease);
    }
}
impl Default for Visuals {
    fn default() -> Self {
        Self {
            inner: Arc::new(Inner {
                slots: Arc::new(Semaphore::new(8)),
                closed: AtomicBool::new(false),
                sessions: Mutex::new(HashMap::new()),
            }),
        }
    }
}
impl Visuals {
    pub fn shutdown(&self) {
        let mut sessions = self.inner.sessions.lock().unwrap();
        self.inner.closed.store(true, Ordering::Release);
        self.inner.slots.close();
        sessions.clear();
    }
    pub fn open(
        &self,
        script: String,
        id: String,
        assets: zuse_visual::Assets,
    ) -> Result<BoxStream<'static, Value>, RpcError> {
        let permit = self.inner.slots.clone().try_acquire_owned().map_err(|_| {
            RpcError::Failed("Close another visual before opening this one.".into())
        })?;
        let lease = uuid::Uuid::new_v4().to_string();
        let (commands, receiver) = mpsc::sync_channel(4);
        let (frames, stream) = tokio::sync::mpsc::channel(2);
        {
            let mut sessions = self.inner.sessions.lock().unwrap();
            if self.inner.closed.load(Ordering::Acquire) {
                return Err(RpcError::Closed);
            }
            sessions.insert(lease.clone(), commands);
        }
        let registration = Registration {
            inner: self.inner.clone(),
            lease: lease.clone(),
        };
        std::thread::Builder::new()
            .name("visual-runtime".into())
            .spawn(move || {
                let (_permit, _registration) = (permit, registration);
                let run = || -> Result<(), String> {
                    let mut runtime = Visual::load(&script, zuse_visual::seed_for(&id))
                        .map_err(|e| e.to_string())?
                        .with_assets(assets);
                    let mut tree = runtime.render().map_err(|e| e.to_string())?;
                    let mut revision = 0;
                    let publish = |tree: &Node, revision| -> Result<(), String> {
                        let frame = wire::Frame {
                            version: wire::VERSION,
                            lease: lease.clone(),
                            revision,
                            tree: serde_json::to_value(tree).map_err(|e| e.to_string())?,
                        };
                        let value = serde_json::to_value(frame).map_err(|e| e.to_string())?;
                        bounded(&value)?;
                        if _registration.inner.closed.load(Ordering::Acquire) {
                            return Err("Visual host stopped".into());
                        }
                        frames
                            .try_send(value)
                            .map_err(|_| "Visual closed".to_owned())
                    };
                    publish(&tree, revision)?;
                    loop {
                        if frames.is_closed() || _registration.inner.closed.load(Ordering::Acquire)
                        {
                            break;
                        }
                        let command = match receiver.recv_timeout(Duration::from_millis(250)) {
                            Ok(command) => command,
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(_) => break,
                        };
                        if _registration.inner.closed.load(Ordering::Acquire) {
                            break;
                        }
                        match command {
                            Command::Control(p, reply) => {
                                if reply.is_closed() {
                                    continue;
                                }
                                if p.revision != revision {
                                    let _ = reply.send(Err(
                                        "Visual changed; use its current controls.".into(),
                                    ));
                                    continue;
                                }
                                if !allowed(&tree, p.handler, &p.argument) {
                                    let _ = reply.send(Err("Invalid visual control.".into()));
                                    continue;
                                }
                                let argument = match p.argument {
                                    wire::Argument::None => HandlerArg::None,
                                    wire::Argument::Text(s) => HandlerArg::Text(s),
                                    wire::Argument::Bool(b) => HandlerArg::Bool(b),
                                    wire::Argument::Number(n) => HandlerArg::Number(n),
                                    wire::Argument::Point([x, y]) => HandlerArg::Point(x, y),
                                };
                                // A failed callback may already have changed Lua state. Retire
                                // the lease instead of accepting another click on an old tree.
                                if let Err(error) = runtime.call(HandlerId(p.handler), argument) {
                                    let error = error.to_string();
                                    let _ = reply.send(Err(error.clone()));
                                    return Err(error);
                                }
                                tree = match runtime.render() {
                                    Ok(tree) => tree,
                                    Err(error) => {
                                        let error = error.to_string();
                                        let _ = reply.send(Err(error.clone()));
                                        return Err(error);
                                    }
                                };
                                revision += 1;
                                publish(&tree, revision)?;
                                let _ = reply.send(Ok(json!({"revision":revision})));
                            }
                            Command::Draw(p, reply) => {
                                if reply.is_closed() {
                                    continue;
                                }
                                let result = if p.revision != revision {
                                    Err("Visual changed.".into())
                                } else if !canvas(&tree, p.handler) || !valid_draw(&p) {
                                    Err("Invalid visual drawing request.".into())
                                } else {
                                    runtime
                                        .draw_canvas(
                                            HandlerId(p.handler),
                                            p.width,
                                            p.height,
                                            p.time,
                                            p.pointer.map(|v| zuse_visual::Pointer {
                                                x: v.x,
                                                y: v.y,
                                                down: v.down,
                                            }),
                                        )
                                        .map_err(|e| e.to_string())
                                        .and_then(|commands| {
                                            let value =
                                                json!({"revision":revision,"commands":commands});
                                            bounded(&value)?;
                                            Ok(value)
                                        })
                                };
                                let _ = reply.send(result);
                            }
                        }
                    }
                    Ok(())
                };
                if let Err(error) = run() {
                    let _ = frames.try_send(json!({"error":error}));
                }
            })
            .map_err(|e| RpcError::Failed(e.to_string()))?;
        Ok(futures::stream::unfold(stream, |mut stream| async {
            let frame = stream.recv().await?;
            Some((frame, stream))
        })
        .boxed())
    }
    async fn send(
        &self,
        lease: &str,
        command: impl FnOnce(oneshot::Sender<Result<Value, String>>) -> Command,
    ) -> Result<Value, RpcError> {
        let sender = self
            .inner
            .sessions
            .lock()
            .unwrap()
            .get(lease)
            .cloned()
            .ok_or_else(|| {
                RpcError::Failed("Visual session ended. Reopen it to start again.".into())
            })?;
        let (tx, rx) = oneshot::channel();
        sender
            .try_send(command(tx))
            .map_err(|_| RpcError::Failed("Visual is busy or closed.".into()))?;
        tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .map_err(|_| {
                RpcError::Failed("Visual response timed out. The action may have completed.".into())
            })?
            .map_err(|_| RpcError::Failed("Visual session ended.".into()))?
            .map_err(RpcError::Failed)
    }
    pub async fn control(&self, p: wire::Control) -> Result<Value, RpcError> {
        let lease = p.lease.clone();
        self.send(&lease, |reply| Command::Control(p, reply)).await
    }
    pub async fn draw(&self, p: wire::Draw) -> Result<Value, RpcError> {
        let lease = p.lease.clone();
        self.send(&lease, |reply| Command::Draw(p, reply)).await
    }
}
fn bounded(value: &Value) -> Result<(), String> {
    if serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > wire::MAX_FRAME_BYTES {
        Err("Visual output exceeds the display limit.".into())
    } else {
        Ok(())
    }
}
fn allowed(node: &Node, id: u32, arg: &wire::Argument) -> bool {
    let matches = match (&node.kind, arg) {
        (NodeKind::Button { on_click, .. }, wire::Argument::None) => on_click.0 == id,
        (NodeKind::Stack(s), wire::Argument::None) => s.on_click.is_some_and(|h| h.0 == id),
        (
            NodeKind::Tabs {
                options, on_change, ..
            },
            wire::Argument::Text(value),
        ) => on_change.0 == id && options.contains(value),
        (NodeKind::Toggle { on_change, .. }, wire::Argument::Bool(_)) => on_change.0 == id,
        (
            NodeKind::Slider {
                on_change,
                min,
                max,
                ..
            },
            wire::Argument::Number(n),
        ) => on_change.0 == id && n.is_finite() && n >= min && n <= max,
        (NodeKind::Canvas(c), wire::Argument::Point([x, y])) => {
            c.on_click.is_some_and(|h| h.0 == id)
                && x.is_finite()
                && y.is_finite()
                && *x >= 0.
                && *y >= 0.
        }
        _ => false,
    };
    matches || zuse_visual::children(node).any(|child| allowed(child, id, arg))
}
fn canvas(node: &Node, id: u32) -> bool {
    matches!(&node.kind,NodeKind::Canvas(c) if c.draw.0==id)
        || zuse_visual::children(node).any(|child| canvas(child, id))
}
fn valid_draw(p: &wire::Draw) -> bool {
    p.width.is_finite()
        && p.height.is_finite()
        && p.width >= 1.
        && p.width <= 2048.
        && p.height >= 1.
        && p.height <= 720.
        && p.time.is_finite()
        && (0. ..=86400.).contains(&p.time)
        && p.pointer.as_ref().is_none_or(|v| {
            v.x.is_finite()
                && v.y.is_finite()
                && v.x >= 0.
                && v.x <= p.width
                && v.y >= 0.
                && v.y <= p.height
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    const SCRIPT: &str = r#"local n = 0
return function()
 return ui.column {
  ui.text { tostring(n) },
  ui.button { label = "Increment", on_click = function() n += 1 end },
  ui.canvas {height=80, draw=function(g) g:text(10,20,tostring(n)) end},
 }
end"#;
    async fn frame(stream: &mut BoxStream<'static, Value>) -> wire::Frame {
        serde_json::from_value(
            tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap()
    }
    fn control(frame: &wire::Frame) -> wire::Control {
        wire::Control {
            lease: frame.lease.clone(),
            revision: frame.revision,
            handler: frame.tree["kind"]["Stack"]["children"][1]["kind"]["Button"]["on_click"]
                .as_u64()
                .unwrap() as u32,
            argument: wire::Argument::None,
            target_device_id: "host".into(),
        }
    }
    #[tokio::test]
    async fn sessions_isolate_state_reject_stale_controls_and_retire_on_owner_drop() {
        let manager = Visuals::default();
        let mut one = manager
            .open(SCRIPT.into(), "one".into(), Default::default())
            .unwrap();
        let mut two = manager
            .open(SCRIPT.into(), "one".into(), Default::default())
            .unwrap();
        let original = frame(&mut one).await;
        let other = frame(&mut two).await;
        assert_ne!(original.lease, other.lease);
        let action = control(&original);
        manager.control(action.clone()).await.unwrap();
        let next = frame(&mut one).await;
        assert_eq!(next.revision, 1);
        assert_eq!(
            next.tree["kind"]["Stack"]["children"][0]["kind"]["Text"]["spans"][0]["text"],
            "1"
        );
        assert!(manager.control(action.clone()).await.is_err());
        let mut invalid = control(&next);
        invalid.argument = wire::Argument::Text("invalid".into());
        assert!(manager.control(invalid).await.is_err());
        manager.control(control(&other)).await.unwrap();
        let independent = frame(&mut two).await;
        assert_eq!(
            independent.tree["kind"]["Stack"]["children"][0]["kind"]["Text"]["spans"][0]["text"],
            "1"
        );
        let handler = next.tree["kind"]["Stack"]["children"][2]["kind"]["Canvas"]["draw"]
            .as_u64()
            .unwrap() as u32;
        let drawing = manager
            .draw(wire::Draw {
                lease: next.lease.clone(),
                revision: next.revision,
                handler,
                width: 300.,
                height: 80.,
                time: 0.,
                pointer: None,
                target_device_id: "host".into(),
            })
            .await
            .unwrap();
        assert_eq!(drawing["commands"][0]["Text"]["text"], "1");
        drop(one);
        drop(two);
        for _ in 0..30 {
            if manager.inner.sessions.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(manager.inner.sessions.lock().unwrap().is_empty());
        assert_eq!(manager.inner.slots.available_permits(), 8);
        assert!(manager.control(action).await.is_err());
        let other_service = manager.clone();
        let mut open = manager
            .open(SCRIPT.into(), "shutdown".into(), Default::default())
            .unwrap();
        let snapshot = frame(&mut open).await;
        other_service.shutdown();
        assert!(other_service.control(control(&snapshot)).await.is_err());
        assert!(
            manager
                .open(SCRIPT.into(), "after-stop".into(), Default::default())
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(2), open.next())
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn callback_failure_retires_mutated_state_and_active_runtime_budget_is_bounded() {
        let manager = Visuals::default();
        let mut streams = vec![];
        for i in 0..8 {
            streams.push(
                manager
                    .open(SCRIPT.into(), i.to_string(), Default::default())
                    .unwrap(),
            );
        }
        assert!(
            manager
                .open(SCRIPT.into(), "overflow".into(), Default::default())
                .is_err()
        );
        drop(streams);
        for _ in 0..30 {
            if manager.inner.slots.available_permits() == 8 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let script = SCRIPT.replace("n += 1", "n += 1; error('callback failed')");
        let mut stream = manager
            .open(script, "failed".into(), Default::default())
            .unwrap();
        let first = frame(&mut stream).await;
        let action = control(&first);
        assert!(manager.control(action.clone()).await.is_err());
        assert!(stream.next().await.unwrap()["error"].is_string());
        assert!(stream.next().await.is_none());
        assert!(manager.control(action).await.is_err());
    }
}
