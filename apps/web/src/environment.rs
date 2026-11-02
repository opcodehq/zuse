//! Browser presentation signals share the application's existing event channel.
use futures::channel::mpsc::UnboundedSender;
use serde_json::Value;
use wasm_bindgen::{closure::Closure, prelude::*};
#[wasm_bindgen(module = "/src/environment.js")]
extern "C" {
    #[wasm_bindgen(js_name = observeEnvironment)]
    fn observe(callback: &js_sys::Function);
}
thread_local! {static CALLBACK: std::cell::RefCell<Option<Closure<dyn FnMut(String)>>> = const {std::cell::RefCell::new(None)};}
pub(super) fn install(events: UnboundedSender<Value>) {
    let callback = Closure::<dyn FnMut(String)>::new(move |serialized: String| {
        if let Ok(value) = serde_json::from_str(&serialized) {
            let _ = events.unbounded_send(value);
        }
    });
    observe(callback.as_ref().unchecked_ref());
    CALLBACK.with(|slot| *slot.borrow_mut() = Some(callback));
}
