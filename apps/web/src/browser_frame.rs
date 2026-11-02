//! Isolated browser surface adapter; GPUI owns its controls and lifetime.
use futures::channel::mpsc::UnboundedSender;
use serde_json::Value;
use wasm_bindgen::{closure::Closure, prelude::*};
#[wasm_bindgen(module = "/src/browser_frame.js")]
extern "C" {
    #[wasm_bindgen(js_name = installBrowserFrame)]
    fn install_frame(callback: &js_sys::Function, local: bool);
    #[wasm_bindgen(js_name = browserFrameCommand)]
    fn command(value: &str);
}
thread_local! {static CALLBACK: std::cell::RefCell<Option<Closure<dyn FnMut(String)>>> = const {std::cell::RefCell::new(None)};}
pub(super) fn install(events: UnboundedSender<Value>, local: bool) {
    let callback = Closure::<dyn FnMut(String)>::new(move |serialized: String| {
        if let Ok(value) = serde_json::from_str(&serialized) {
            let _ = events.unbounded_send(value);
        }
    });
    install_frame(callback.as_ref().unchecked_ref(), local);
    CALLBACK.with(|slot| *slot.borrow_mut() = Some(callback));
}
pub(super) fn handle(frame: &Value) -> bool {
    if frame["ui"] != "browserFrame" {
        return false;
    }
    command(&frame.to_string());
    true
}
