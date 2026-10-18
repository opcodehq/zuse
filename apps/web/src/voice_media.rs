//! Local microphone/WebRTC adapter. Signaling is emitted to the GPUI call owner.
use futures::channel::mpsc::UnboundedSender;
use serde_json::Value;
use wasm_bindgen::{closure::Closure, prelude::*};
#[wasm_bindgen(module = "/src/voice_media.js")]
extern "C" {
    #[wasm_bindgen(js_name = installVoiceMedia)]
    fn install_media(callback: &js_sys::Function);
    #[wasm_bindgen(js_name = voiceMediaCommand)]
    fn command(value: &str);
}
thread_local! {static CALLBACK: std::cell::RefCell<Option<Closure<dyn FnMut(String)>>> = const {std::cell::RefCell::new(None)};}
pub(super) fn install(events: UnboundedSender<Value>) {
    let callback = Closure::<dyn FnMut(String)>::new(move |serialized: String| {
        if let Ok(value) = serde_json::from_str::<Value>(&serialized) {
            let _ = events.unbounded_send(value);
        }
    });
    install_media(callback.as_ref().unchecked_ref());
    CALLBACK.with(|slot| *slot.borrow_mut() = Some(callback));
}
pub(super) fn handle(frame: &Value) -> bool {
    if frame["ui"] != "voiceMedia" {
        return false;
    }
    command(&frame.to_string());
    true
}
