//! A controller tab grants access; saved destinations contain public metadata only.
use crate::{connection::delay, socket::Socket};
use futures::{
    FutureExt, StreamExt,
    channel::{mpsc, oneshot},
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
use zuse_protocol::{managed_connection::Bootstrap, methods};

const SAVED: &str = "zuse.account-target.v1";
pub(super) use zuse_protocol::managed_connection::ComputerTarget as Target;
thread_local! {static CURRENT:RefCell<Option<Target>>=const{RefCell::new(None)};}
pub(super) fn selected() -> Result<Option<Target>, String> {
    let target = read_selected()?;
    CURRENT.with(|current| *current.borrow_mut() = target.clone());
    Ok(target)
}
fn read_selected() -> Result<Option<Target>, String> {
    let window = web_sys::window().unwrap();
    let storage = window.session_storage().ok().flatten();
    let hash = window
        .location()
        .hash()
        .map_err(|_| "Could not read computer link")?;
    if hash.starts_with("#token=") || hash.starts_with("#pair=") {
        if let Some(storage) = storage {
            let _ = storage.remove_item(SAVED);
        }
        return Ok(None);
    }
    let raw = if let Some(raw) = hash.strip_prefix("#computer=") {
        if let Some(storage) = &storage {
            let _ = storage.remove_item(SAVED);
        }
        if raw.len() > 24576 {
            return Err("Invalid computer link".into());
        }
        let raw = js_sys::decode_uri_component(raw)
            .map_err(|_| "Invalid computer link")?
            .as_string()
            .ok_or("Invalid computer link")?;
        let value: Value = serde_json::from_str(&raw).map_err(|_| "Invalid computer link")?;
        let target = Target::parse(&value)?;
        // Keep the explicit public target in the URL when storage is unavailable.
        // Reloading must never select the local engine as an accidental fallback.
        if storage
            .as_ref()
            .is_some_and(|storage| storage.set_item(SAVED, &target.value().to_string()).is_ok())
        {
            let clean = format!(
                "{}{}",
                window.location().pathname().unwrap_or_else(|_| "/".into()),
                window.location().search().unwrap_or_default()
            );
            if let Ok(history) = window.history() {
                let _ =
                    history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&clean));
            }
        }
        return Ok(Some(target));
    } else {
        storage.and_then(|s| s.get_item(SAVED).ok().flatten())
    };
    raw.map(|s| {
        serde_json::from_str::<Value>(&s)
            .map_err(|_| "Invalid saved computer".to_owned())
            .and_then(|v| Target::parse(&v))
    })
    .transpose()
}
pub(super) fn action(frame: &Value, events: &mpsc::UnboundedSender<Value>) -> bool {
    let window = web_sys::window().unwrap();
    if frame["ui"] == "returnToController" {
        let parent =
            CURRENT.with(|current| current.borrow().as_ref().and_then(|target| target.parent()));
        if let Ok(Some(storage)) = window.session_storage() {
            let _ = storage.remove_item(SAVED);
        }
        let hash = parent
            .map(|target| {
                format!(
                    "#computer={}",
                    js_sys::encode_uri_component(&target.value().to_string())
                )
            })
            .unwrap_or_default();
        let clean = format!(
            "{}{}{}",
            window.location().pathname().unwrap_or_else(|_| "/".into()),
            window.location().search().unwrap_or_default(),
            hash
        );
        if let Ok(history) = window.history() {
            let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&clean));
        }
        let _ = window.location().reload();
        return true;
    }
    if frame["ui"] != "openAccountComputer" {
        return false;
    }
    let result = (|| {
        let value = json!({"environmentId":frame["environmentId"],"expectedSubject":frame["expectedSubject"],"label":frame["label"]});
        let next = Target::parse(&value)?;
        let target = CURRENT.with(|current| match current.borrow().as_ref() {
            Some(parent) => parent.append(next),
            None => Ok(next),
        })?;
        let location = window.location();
        let url = format!(
            "{}{}{}#computer={}",
            location.origin().map_err(|_| "Invalid controller origin")?,
            location.pathname().unwrap_or_else(|_| "/".into()),
            location.search().unwrap_or_default(),
            js_sys::encode_uri_component(&target.value().to_string())
        );
        let child = window
            .open_with_url_and_target(&url, "_blank")
            .map_err(|_| "Could not open computer tab")?
            .ok_or("Allow popups to open this computer in a new tab")?;
        // A same-origin tab inherits this tab's controller pairing, if any.
        // Retire the opener immediately; no capability is passed in the URL.
        if js_sys::Reflect::set(
            child.as_ref(),
            &"opener".into(),
            &wasm_bindgen::JsValue::NULL,
        ) != Ok(true)
        {
            let _ = child.close();
            return Err("Could not isolate computer tab".into());
        }
        Ok::<_, String>(())
    })();
    if let Err(error) = result {
        let _ = events.unbounded_send(json!({"accountConnectionError":error}));
    }
    true
}
struct Request {
    method: &'static str,
    params: Value,
    reply: oneshot::Sender<Result<Value, String>>,
}
struct Pending {
    reply: oneshot::Sender<Result<Value, String>>,
    until: f64,
}
#[derive(Clone)]
pub(super) struct Controller {
    requests: Rc<RefCell<mpsc::Sender<Request>>>,
    closed: Rc<Cell<bool>>,
    blocked: Rc<Cell<bool>>,
    pub target: Target,
}
struct Retire(Rc<Cell<bool>>);
impl Drop for Retire {
    fn drop(&mut self) {
        self.0.set(true);
    }
}
impl Controller {
    pub fn is_closed(&self) -> bool {
        self.closed.get()
    }
    pub fn is_blocked(&self) -> bool {
        self.blocked.get()
    }
    pub async fn open(
        target: Target,
        url: &str,
        paired: Option<&zuse_remote::PairedTarget>,
    ) -> Result<Self, String> {
        let mut hops = target.controllers.clone();
        let mut terminal = target;
        terminal.controllers.clear();
        hops.push(terminal);
        let mut socket = Socket::connect(url, paired.map(|p| p.invitation())).await?;
        let count = hops.len();
        for (index, target) in hops.into_iter().enumerate() {
            let controller = Self::from_socket(target, socket).await?;
            if index + 1 == count {
                return Ok(controller);
            }
            socket = Socket::connect_account(controller.clone()).await?;
            socket
                .verify_environment(&controller.target.environment)
                .await?;
            controller
                .remember(
                    socket
                        .account_origin()
                        .as_deref()
                        .ok_or("Computer endpoint unavailable")?,
                )
                .await;
        }
        unreachable!("A connection path always includes its terminal computer")
    }
    pub async fn remember(&self, origin: &str) {
        let request=self.call(methods::REMEMBER_ACCOUNT_ROUTE,json!({"expectedSubject":self.target.subject,"environmentId":self.target.environment,"label":self.target.label,"origin":origin}));
        futures::select_biased! {_=request.fuse()=>{},_=delay(2000).fuse()=>{}}
    }
    async fn from_socket(target: Target, mut socket: Socket) -> Result<Self, String> {
        socket.send(
            &json!({"id":1,"method":methods::CLOUD_ACCOUNT_STATUS,"params":{}}).to_string(),
        )?;
        let first = futures::select_biased! {value=socket.receive().fuse()=>value?,_=delay(10000).fuse()=>return Err("Controller account status timed out".into())};
        let first: Value =
            serde_json::from_str(&first).map_err(|_| "Invalid controller account status")?;
        let user = first
            .get("item")
            .or_else(|| first.get("ok"))
            .ok_or("Sign in on the controller before selecting this computer")?;
        if first["id"] != 1 || user["user"]["id"].as_str() != Some(target.subject.as_str()) {
            return Err("The controller account changed. Select the computer again.".into());
        }
        let (requests, mut incoming) = mpsc::channel::<Request>(16);
        let closed = Rc::new(Cell::new(false));
        let retire = Retire(closed.clone());
        let blocked = Rc::new(Cell::new(false));
        let block = blocked.clone();
        let done = closed.clone();
        let subject = target.subject.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let _retire = retire;
            let mut next = 2u64;
            let mut pending: HashMap<u64, Pending> = HashMap::new();
            loop {
                if done.get() {
                    return;
                }
                enum Next {
                    Frame(Result<String, String>),
                    Request(Option<Request>),
                    Tick,
                }
                let event = futures::select! {frame=socket.receive().fuse()=>Next::Frame(frame),request=incoming.next().fuse()=>Next::Request(request),_=delay(1000).fuse()=>Next::Tick};
                match event {
                    Next::Frame(Ok(text)) => {
                        for line in text.lines() {
                            let Ok(frame) = serde_json::from_str::<Value>(line) else {
                                return;
                            };
                            if frame["id"] == 1 {
                                if frame["done"] == true || frame.get("err").is_some() {
                                    return;
                                }
                                let user = frame.get("item").or_else(|| frame.get("ok"));
                                if user.is_some_and(|u| {
                                    u["user"]["id"].as_str() != Some(subject.as_str())
                                }) {
                                    block.set(true);
                                    return;
                                }
                            } else if let Some(id) = frame["id"].as_u64() {
                                if let Some(call) = pending.remove(&id) {
                                    let result = frame.get("ok").cloned().ok_or_else(|| {
                                        frame["err"]
                                            .as_str()
                                            .unwrap_or("Controller request failed")
                                            .to_owned()
                                    });
                                    let _ = call.reply.send(result);
                                }
                            }
                        }
                    }
                    Next::Frame(Err(_)) => {
                        if socket.account_dependency_blocked() {
                            block.set(true);
                        }
                        return;
                    }
                    Next::Request(None) => return,
                    Next::Request(Some(call)) => {
                        if pending.len() >= 64 {
                            let _ = call
                                .reply
                                .send(Err("Too many account requests. Try again shortly.".into()));
                            continue;
                        }
                        let id = next;
                        next = next.wrapping_add(1).max(2);
                        if socket
                            .send(
                                &json!({"id":id,"method":call.method,"params":call.params})
                                    .to_string(),
                            )
                            .is_err()
                        {
                            return;
                        }
                        pending.insert(
                            id,
                            Pending {
                                reply: call.reply,
                                until: js_sys::Date::now() + 60000.,
                            },
                        );
                    }
                    Next::Tick => {
                        let expired: Vec<_> = pending
                            .iter()
                            .filter(|(_, p)| {
                                p.reply.is_canceled() || p.until <= js_sys::Date::now()
                            })
                            .map(|(id, _)| *id)
                            .collect();
                        for id in expired {
                            if let Some(call) = pending.remove(&id) {
                                let _ = call.reply.send(Err("Account request timed out".into()));
                            }
                            if socket
                                .send(&json!({"id":id,"cancel":true}).to_string())
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
            }
        });
        Ok(Self {
            requests: Rc::new(RefCell::new(requests)),
            closed,
            blocked,
            target,
        })
    }
    pub async fn call(&self, method: &'static str, params: Value) -> Result<Value, String> {
        if self.is_closed() {
            return Err("The controller account connection closed".into());
        }
        let (reply, wait) = oneshot::channel();
        self.requests
            .borrow_mut()
            .try_send(Request {
                method,
                params,
                reply,
            })
            .map_err(|_| "Account request queue is busy or closed")?;
        futures::select_biased! {value=wait.fuse()=>value.map_err(|_|"The controller account connection closed".to_owned())?,_=delay(60000).fuse()=>Err("Account request timed out".into())}
    }
    pub async fn grant(&self) -> Result<Bootstrap, String> {
        let value=self.call(methods::BOOTSTRAP_ACCOUNT_CONNECTION,json!({"environmentId":self.target.environment,"expectedSubject":self.target.subject})).await.map_err(|error|{
            if error==zuse_protocol::managed_connection::REJECTED {self.blocked.set(true);self.closed.set(true);"The computer rejected account access. Check access and app versions before selecting it again.".into()}else{error}
        })?;
        let grant: Bootstrap =
            serde_json::from_value(value).map_err(|_| "Invalid account authorization response")?;
        if grant.environment_id != self.target.environment {
            return Err("The selected computer identity changed".into());
        }
        Ok(grant)
    }
}
