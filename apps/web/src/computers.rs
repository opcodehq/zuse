//! Paired computers this browser has reached. Each link is a device credential:
//! it stays in this origin's storage, and the host can revoke it at any time.
use futures::channel::mpsc;
use serde_json::{Value, json};
use web_sys::Storage;
use zuse_remote::PairedTarget;

const SAVED: &str = "zuse.paired-computers.v1";
pub(super) const SELECTED: &str = "zuse.paired-target.v1";
const ACCOUNT_SELECTED: &str = "zuse.account-target.v1";
const MAX_SAVED: usize = 16;

fn saved_storage() -> Option<Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}
fn tab_storage() -> Option<Storage> {
    web_sys::window()?.session_storage().ok().flatten()
}
/// Newest first. Entries that no longer parse are dropped on the next write.
fn load() -> Vec<PairedTarget> {
    saved_storage()
        .and_then(|storage| storage.get_item(SAVED).ok().flatten())
        .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
        .unwrap_or_default()
        .iter()
        .filter_map(|link| PairedTarget::parse_link(link).ok())
        .collect()
}
fn save(targets: &[PairedTarget]) {
    let links: Vec<String> = targets
        .iter()
        .take(MAX_SAVED)
        .filter_map(|target| target.export_link().ok())
        .collect();
    if let (Some(storage), Ok(text)) = (saved_storage(), serde_json::to_string(&links)) {
        let _ = storage.set_item(SAVED, &text);
    }
}
fn id(target: &PairedTarget) -> &str {
    target.invitation().id.as_str()
}
fn selected() -> Option<PairedTarget> {
    let link = tab_storage()?.get_item(SELECTED).ok().flatten()?;
    PairedTarget::parse_link(&link).ok()
}
fn select(target: &PairedTarget) {
    if let (Some(storage), Ok(link)) = (tab_storage(), target.export_link()) {
        let _ = storage.set_item(SELECTED, &link);
    }
}

/// Saved only after a connection works, so a bad link never becomes the
/// computer a new tab tries first.
pub(super) fn remember(target: &PairedTarget) {
    let mut targets = load();
    targets.retain(|saved| id(saved) != id(target));
    targets.insert(0, target.clone());
    save(&targets);
}
/// The computer a tab with no link or owner session should reopen.
pub(super) fn resume() -> Option<PairedTarget> {
    let target = load().into_iter().next()?;
    select(&target);
    Some(target)
}
pub(super) fn forget_selected() {
    if let Some(target) = selected() {
        forget(id(&target));
    }
}
fn forget(pair: &str) {
    let mut targets = load();
    targets.retain(|saved| id(saved) != pair);
    save(&targets);
}
pub(super) fn publish(events: &mpsc::UnboundedSender<Value>) {
    let current = selected();
    let computers: Vec<Value> = load()
        .iter()
        .map(|target| {
            json!({
                "id":id(target),
                "origin":target.origin(),
                "current":current.as_ref().is_some_and(|c| id(c) == id(target)),
            })
        })
        .collect();
    let _ = events.unbounded_send(json!({"savedComputers":computers}));
}
fn open(target: &PairedTarget) {
    select(target);
    if let Some(storage) = tab_storage() {
        let _ = storage.remove_item(ACCOUNT_SELECTED);
    }
    let _ = web_sys::window().unwrap().location().reload();
}
pub(super) fn action(frame: &Value, events: &mpsc::UnboundedSender<Value>) -> bool {
    match frame["ui"].as_str() {
        Some("listSavedComputers") => publish(events),
        Some("forgetSavedComputer") => {
            forget(frame["id"].as_str().unwrap_or_default());
            publish(events);
        }
        Some("openSavedComputer") => {
            let wanted = frame["id"].as_str().unwrap_or_default();
            if let Some(target) = load().iter().find(|target| id(target) == wanted) {
                open(target);
            }
        }
        Some("addSavedComputer") => {
            if let Ok(target) = PairedTarget::parse_link(frame["link"].as_str().unwrap_or_default())
            {
                open(&target);
            }
        }
        _ => return false,
    }
    true
}
