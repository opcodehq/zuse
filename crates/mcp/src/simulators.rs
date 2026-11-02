//! Device tools shared by MCP and the native harness application-tool bridge.
use crate::tools::ToolDef;
use serde_json::{Value, json};
pub fn definitions() -> Vec<ToolDef> {
    let target = json!({"type":"string","description":"Exact device id returned by device_list."});
    let input = json!({"oneOf":[
        {"type":"object","properties":{"type":{"const":"tap"},"x":{"type":"number","minimum":0,"maximum":1},"y":{"type":"number","minimum":0,"maximum":1}},"required":["type","x","y"],"additionalProperties":false},
        {"type":"object","properties":{"type":{"const":"text"},"text":{"type":"string","maxLength":4096}},"required":["type","text"],"additionalProperties":false},
        {"type":"object","properties":{"type":{"enum":["home","back","recents"]}},"required":["type"],"additionalProperties":false}
    ]});
    [
        ("device_list","List iOS simulators and Android emulators on this chat's execution host. Requires device support and agent access enabled in its Simulator panel.",json!({}),json!([])),
        ("device_open","Boot and open a simulator or emulator in this chat's Simulator panel. Waits for boot readiness. Device actions are blocked in plan mode.",json!({"deviceId":target}),json!(["deviceId"])),
        ("device_close","Close this chat's device session. Set shutdown to power off; otherwise the device keeps running.",json!({"deviceId":target,"shutdown":{"type":"boolean"}}),json!(["deviceId"])),
        ("device_screenshot","Inspect a PNG screenshot of a device opened in this chat. Returns image content with pixel dimensions.",json!({"deviceId":target}),json!(["deviceId"])),
        ("device_input","Tap, type, or press Home on an opened device. Tap x/y are normalized [0,1] coordinates within the screenshot. Back and Recents are Android-only. iOS requires idb and supports ASCII typing; Android typing supports printable ASCII without percent signs.",json!({"deviceId":target,"input":input}),json!(["deviceId","input"]))
    ].into_iter().map(|(name,description,properties,required)| ToolDef { name,description,input_schema:json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}) }).collect()
}
/// Keep screenshot bytes out of the text context; expose actual model image content.
pub fn image_content(name: &str, value: &Value) -> Option<Value> {
    if name != "device_screenshot" {
        return None;
    }
    Some(json!({"type":"image","data":value.get("data")?.as_str()?,"mimeType":"image/png"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn screenshot_content_is_an_image_only_for_the_screenshot_tool() {
        let value = json!({"data":"cG5n","width":100,"height":200});
        assert_eq!(
            image_content("device_screenshot", &value).unwrap()["mimeType"],
            "image/png"
        );
        assert!(image_content("device_list", &value).is_none());
        assert!(image_content("device_screenshot", &json!({})).is_none());
    }
}
