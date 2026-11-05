//! Browser persistence never grants execution access. Opening heads can paint
//! provisionally in the confirmed actor/API/org scope while a fresh read runs.
//! Reusing tool/history pages requires fresh workspace authorization (30s).
use futures::{FutureExt, channel::mpsc::UnboundedSender};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::HashMap};
use wasm_bindgen::prelude::*;
use zuse_protocol::methods;

mod identity;
use identity::{error_policy, opening_identity, request_identity};

#[wasm_bindgen(module = "/src/history_cache.js")]
extern "C" {
    #[wasm_bindgen(js_name=cacheGet)]
    fn get(namespace: &str, identity: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name=cachePut)]
    fn put(namespace: &str, identity: &str, workspace: &str, text: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name=cacheClear)]
    fn clear(namespace: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name=cacheClearAll)]
    fn clear_all() -> js_sys::Promise;
    #[wasm_bindgen(js_name=cacheIdentity)]
    fn identity(value: &str) -> js_sys::Promise;
}
#[derive(Clone)]
struct Pending {
    method: String,
    params: Value,
    namespace: Option<String>,
    identity: String,
    workspace: String,
}
#[derive(Default)]
struct State {
    origin: String,
    account: Option<String>,
    api: Option<String>,
    organization: Option<String>,
    pending: HashMap<u64, Pending>,
    authorized: HashMap<(String, String), f64>,
}
thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
fn namespace(state: &State, params: &Value) -> Option<String> {
    let account = state.account.as_ref()?;
    let api = state.api.as_ref()?;
    if params["expectedAccountId"]
        .as_str()
        .is_some_and(|expected| expected != account)
    {
        return None;
    }
    if params["expectedOrganizationId"].as_str() != state.organization.as_deref() {
        return None;
    }
    Some(json!([state.origin, api, account, params["expectedOrganizationId"]]).to_string())
}
fn purge(namespace: String) {
    wasm_bindgen_futures::spawn_local(async move {
        let _ = wasm_bindgen_futures::JsFuture::from(clear(&namespace)).await;
    });
}
pub(super) fn signed_out() {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.authorized.clear();
        state.account = None;
        state.api = None;
        state.organization = None;
        state.pending.clear();
    });
    wasm_bindgen_futures::spawn_local(async move {
        let _ = wasm_bindgen_futures::JsFuture::from(clear_all()).await;
    });
}
pub(super) fn opened(origin: String) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.origin = origin;
        state.pending.clear();
        state.authorized.clear();
        state.account = None;
        state.api = None;
        state.organization = None;
    });
}
pub(super) async fn outgoing(frame: &Value, events: &UnboundedSender<Value>) -> bool {
    let Some(id) = frame["id"].as_u64() else {
        return false;
    };
    if frame["cancel"] == true {
        STATE.with(|state| {
            state.borrow_mut().pending.remove(&id);
        });
        return false;
    }
    let method = frame["method"].as_str().unwrap_or_default();
    if matches!(method, methods::SIGN_OUT | methods::CLOUD_ACCOUNT_SIGN_OUT) {
        signed_out();
    }
    if !matches!(method, methods::CLOUD_CONTEXT | methods::CLOUD_HISTORY) {
        return false;
    }
    let (pending, authorized) = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let scope = namespace(&state, &frame["params"]);
        let workspace = frame["params"]["workspaceId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let pending = Pending {
            method: method.into(),
            params: frame["params"].clone(),
            namespace: scope.clone(),
            identity: request_identity(&frame["params"]),
            workspace: workspace.clone(),
        };
        let authorized = scope
            .as_ref()
            .and_then(|scope| state.authorized.get(&(scope.clone(), workspace)).copied())
            .is_some_and(|at| {
                let age = js_sys::Date::now() - at;
                age >= 0. && age < 30_000.
            });
        state.pending.insert(id, pending.clone());
        (pending, authorized)
    });
    if method == methods::CLOUD_HISTORY && pending.params["cacheOnly"] == true {
        let lookup = opening_identity(&pending.params);
        let cached = if let Some(scope) = &pending.namespace {
            let read = wasm_bindgen_futures::JsFuture::from(get(scope, &lookup));
            futures::select_biased! {value=read.fuse()=>value.ok().and_then(|v|v.as_string()).and_then(|v|serde_json::from_str::<Value>(&v).ok()),_=super::connection::delay(20).fuse()=>None}
        } else {
            None
        };
        let current = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let valid = namespace(&state, &pending.params) == pending.namespace
                && state
                    .pending
                    .get(&id)
                    .is_some_and(|p| p.identity == pending.identity);
            state.pending.remove(&id);
            valid
        });
        let head = cached
            .filter(|value| {
                current
                    && value["accountId"] == pending.params["expectedAccountId"]
                    && value["head"]["workspaceId"] == pending.workspace
                    && pending.params["sessionId"]
                        .as_str()
                        .is_none_or(|s| value["head"]["sessionId"] == s)
            })
            .map(|value| value["head"].clone())
            .unwrap_or(Value::Null);
        let _=events.unbounded_send(json!({"id":id,"ok":{"accountId":pending.params["expectedAccountId"],"organizationId":pending.params["expectedOrganizationId"],"head":head,"provisional":true}}));
        return true;
    }
    if method != methods::CLOUD_HISTORY || !authorized {
        return false;
    }
    let Some(scope) = pending.namespace.clone() else {
        return false;
    };
    let cached = wasm_bindgen_futures::JsFuture::from(get(&scope, &pending.identity));
    let result = futures::select_biased! {value=cached.fuse()=>value.ok().and_then(|value|value.as_string()),_=super::connection::delay(20).fuse()=>None};
    let Some(value) = result.and_then(|s| serde_json::from_str::<Value>(&s).ok()) else {
        return false;
    };
    let still_authorized = STATE.with(|state| {
        let state = state.borrow();
        namespace(&state, &pending.params).as_ref() == Some(&scope)
            && state
                .pending
                .get(&id)
                .is_some_and(|current| current.identity == pending.identity)
            && state
                .authorized
                .get(&(scope.clone(), pending.workspace.clone()))
                .is_some_and(|at| {
                    let age = js_sys::Date::now() - at;
                    age >= 0. && age < 30_000.
                })
    });
    if !still_authorized {
        return false;
    }
    let immutable_page =
        pending.params["beforeSequence"].is_number() || pending.params["toolBlobRef"].is_string();
    if immutable_page {
        STATE.with(|state| {
            state.borrow_mut().pending.remove(&id);
        });
        let _ = events.unbounded_send(json!({"id":id,"ok":value,"cachedHistory":true}));
        true
    } else {
        // An opening head is provisional and read-only while the same request
        // revalidates it; do not suppress the fresh server result.
        let _ = events.unbounded_send(json!({"id":id,"item":value,"cachedHistory":true}));
        false
    }
}
pub(super) fn incoming(frame: &mut Value) {
    // Account invalidations arrive on their own long-lived RPC stream, outside
    // the history request map. Invalidate keys before a provisional read can run.
    if let Some(envelope) = frame.get("item").or_else(|| frame.get("ok")) {
        let data = &envelope["data"];
        if data["cacheRevoked"] == true
            || data["revokedWorkspaceIds"]
                .as_array()
                .is_some_and(|ids| !ids.is_empty())
        {
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                if envelope["accountId"].as_str() == state.account.as_deref()
                    && envelope["organizationId"].as_str() == state.organization.as_deref()
                {
                    if let (Some(api), Some(account)) = (&state.api, &state.account) {
                        purge(json!([state.origin, api, account, state.organization]).to_string());
                    }
                    state.authorized.clear();
                    state.account = None;
                    state.api = None;
                    state.organization = None;
                    state
                        .pending
                        .retain(|_, pending| pending.method != methods::CLOUD_HISTORY);
                }
            });
        }
    }
    let Some(id) = frame["id"].as_u64() else {
        return;
    };
    let terminal = frame.get("ok").is_some() || frame.get("err").is_some() || frame["done"] == true;
    if !terminal {
        return;
    }
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(pending) = state.pending.remove(&id) else {
            return;
        };
        if pending.method == methods::CLOUD_CONTEXT {
            if let Some(error) = frame.get("err") {
                state.authorized.clear();
                let policy = error_policy(error);
                if policy.purge {
                    if let (Some(api), Some(account)) = (&state.api, &state.account) {
                        purge(json!([state.origin, api, account, state.organization]).to_string());
                    } else {
                        // A newly opened browser has not confirmed an actor
                        // yet; denial must also revoke persisted prior pages.
                        wasm_bindgen_futures::spawn_local(async move {
                            let _ = wasm_bindgen_futures::JsFuture::from(clear_all()).await;
                        });
                    }
                    state.account = None;
                    state.api = None;
                    state.organization = None;
                }
                if policy.revoked {
                    frame["cacheRevoked"] = json!(true);
                }
                return;
            }
            if frame["ok"]["accountId"].is_string() && frame["ok"]["apiOrigin"].is_string() {
                let account = frame["ok"]["accountId"].as_str().unwrap().to_owned();
                let api = frame["ok"]["apiOrigin"].as_str().unwrap().to_owned();
                let organization = frame["ok"]["organizationId"].as_str().map(str::to_owned);
                if state.account.as_ref() != Some(&account)
                    || state.api.as_ref() != Some(&api)
                    || state.organization != organization
                {
                    let old = state
                        .authorized
                        .keys()
                        .map(|(scope, _)| scope.clone())
                        .collect::<std::collections::HashSet<_>>();
                    // Even after a transport error removed fresh authorization,
                    // the previously confirmed scope may still own offline pages.
                    let mut old = old;
                    if let (Some(api), Some(account)) = (&state.api, &state.account) {
                        old.insert(
                            json!([state.origin, api, account, state.organization]).to_string(),
                        );
                    }
                    state.authorized.clear();
                    for scope in old {
                        purge(scope);
                    }
                }
                state.account = Some(account);
                state.api = Some(api);
                state.organization = organization;
                let value = json!([state.origin, state.api, state.account]).to_string();
                wasm_bindgen_futures::spawn_local(async move {
                    let _ = wasm_bindgen_futures::JsFuture::from(identity(&value)).await;
                });
            }
            return;
        }
        let Some(scope) = pending.namespace else {
            return;
        };
        // A late result from a retired account/API/org cannot revoke or
        // authorize the newly confirmed scope.
        if namespace(&state, &pending.params).as_ref() != Some(&scope) {
            return;
        }
        if frame.get("err").is_some() {
            state
                .authorized
                .retain(|(namespace, _), _| namespace != &scope);
            let policy = error_policy(&frame["err"]);
            if policy.purge {
                purge(scope);
            }
            if policy.revoked {
                frame["cacheRevoked"] = json!(true);
            }
            return;
        }
        let value = &frame["ok"];
        if value["accountId"].as_str() != state.account.as_deref()
            || value["workspace"]["workspaceId"] != pending.workspace
        {
            return;
        }
        if !(value["head"].is_object()
            || value["admission"].is_object()
            || value["toolOutput"].is_object())
        {
            return;
        }
        if namespace(&state, &pending.params).as_ref() != Some(&scope) {
            return;
        }
        state.authorized.insert(
            (scope.clone(), pending.workspace.clone()),
            js_sys::Date::now(),
        );
        let text = value.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = wasm_bindgen_futures::JsFuture::from(put(
                &scope,
                &pending.identity,
                &pending.workspace,
                &text,
            ))
            .await;
        });
    });
}
