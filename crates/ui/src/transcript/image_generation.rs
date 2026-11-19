//! Image tools remain visible outside the work accordion, including while
//! generating. Result pixels use the existing device-aware attachment viewer.
use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedAsset {
    owner: String,
    path: String,
    name: String,
    mime: String,
}

pub(super) fn is_generation_call(call: &ToolCall) -> bool {
    let name = match call {
        ToolCall::Unknown { name, .. } => name.as_str(),
        ToolCall::Mcp { tool, .. } => tool.as_str(),
        _ => return false,
    };
    let name = name
        .rsplit("__")
        .next()
        .unwrap_or(name)
        .rsplit(['.', '/', ':'])
        .next()
        .unwrap_or(name);
    let name: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect();
    matches!(
        name.as_str(),
        "generateimage"
            | "generateimages"
            | "generatedimage"
            | "generatedimages"
            | "generatingimage"
            | "imagegeneration"
            | "imagegenerationcall"
            | "imagegen"
            | "createimage"
            | "editimage"
            | "imagine"
    )
}

pub(super) fn owns_image(call: &str, image: &str) -> bool {
    image.strip_prefix(call).is_some_and(|suffix| {
        suffix == ":image"
            || suffix
                .strip_prefix(":image:")
                .is_some_and(|index| !index.is_empty() && index.bytes().all(|c| c.is_ascii_digit()))
    })
}

pub(super) fn assets(entry: &SessionMessageEntry, call: &str) -> Vec<GeneratedAsset> {
    let mut assets = Vec::new();
    for part in &entry.parts {
        if let MessagePart::Image {
            id,
            path,
            name,
            mime_type,
        } = part
        {
            if owns_image(call, id)
                && !assets.iter().any(|a: &GeneratedAsset| a.path == *path)
                && assets.len() < 4
            {
                assets.push(GeneratedAsset {
                    owner: entry.device_id.clone(),
                    path: path.clone(),
                    name: name.clone(),
                    mime: mime_type.clone(),
                });
            }
        }
    }
    assets
}

fn prompt(call: &ToolCall) -> Option<String> {
    let input = match call {
        ToolCall::Unknown { input, .. } | ToolCall::Mcp { input, .. } => input.as_ref()?,
        _ => return None,
    };
    ["prompt", "revised_prompt", "description"]
        .iter()
        .find_map(|key| input.get(key)?.as_str())
        .map(|s| s.chars().take(1000).collect())
}

pub(super) fn display_call(call: &ToolCall) -> ToolCall {
    let input = prompt(call).map(|prompt| serde_json::json!({"prompt": prompt}));
    match call {
        ToolCall::Unknown { name, .. } => ToolCall::Unknown {
            name: name.clone(),
            input,
        },
        ToolCall::Mcp { server, tool, .. } => ToolCall::Mcp {
            server: server.clone(),
            tool: tool.clone(),
            input,
        },
        _ => call.clone(),
    }
}

impl Transcript {
    pub(super) fn render_image_generation(
        &mut self,
        row: &SharedString,
        tool: &ToolItem,
        assets: &[GeneratedAsset],
        streaming: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let running = !tool.resolved && streaming;
        let interrupted = !tool.resolved && !streaming;
        let failed = tool.is_error;
        let title = if failed {
            "Image generation failed"
        } else if interrupted {
            "Image generation interrupted"
        } else if running {
            "Generating image…"
        } else if assets.len() > 1 {
            "Images generated"
        } else if !assets.is_empty() || !tool.images.is_empty() {
            "Image generated"
        } else {
            "Image generation finished"
        };
        let mut card = div()
            .id(SharedString::from(format!("{row}-generation-card")))
            .debug_selector(|| "image-generation-card".into())
            .w(px(536.0))
            .max_w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(12.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(theme.border)
            .bg(crate::theme::wash(0.025))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        crate::icons::icon(crate::icons::FILE_IMAGE)
                            .size(px(16.0))
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .text_color(if failed {
                                theme.danger_muted
                            } else {
                                theme.text
                            })
                            .child(title),
                    )
                    .when(running, |el| {
                        el.child(crate::loaders::working_mark(
                            format!("{row}-image-progress"),
                            16.0,
                            theme.text_muted,
                        ))
                    }),
            );
        if let Some(prompt) = prompt(&tool.call).filter(|s| !s.is_empty()) {
            card = card.child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .child(prompt),
            );
        }
        for (index, asset) in assets.iter().enumerate() {
            let preview = self.render_generated_image(
                &format!("{row}-result-{index}").into(),
                &asset.owner,
                &asset.path,
                &asset.name,
                &asset.mime,
                cx,
            );
            card = card.child(div().w_full().flex().justify_center().child(preview));
        }
        if assets.is_empty() && !tool.images.is_empty() {
            if let Some(Some(images)) = self
                .hold_tool_images(
                    row,
                    std::slice::from_ref(tool),
                    &[true],
                    &[FoldState::default()],
                    cx,
                )
                .into_iter()
                .next()
            {
                card = card.child(self.render_tool_image_strip(
                    &format!("{row}#d0").into(),
                    images,
                    false,
                    &theme,
                    cx,
                ));
            }
        } else if assets.is_empty() {
            let message = if failed {
                "The image tool reported an error. Check the conversation error for details."
            } else if interrupted {
                "The turn ended before an image was returned."
            } else if running {
                "Creating your image"
            } else {
                "No image was attached to this result."
            };
            card = card.child(
                div()
                    .h(px(if running { 180.0 } else { 64.0 }))
                    .w_full()
                    .rounded(px(8.0))
                    .bg(crate::theme::wash(0.035))
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(16.0))
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .child(message),
            );
        }
        card.into_any_element()
    }
}

#[cfg(feature = "browser-fixture")]
impl Transcript {
    pub(crate) fn fixture_image_generation(&mut self, status: &str, cx: &mut Context<Self>) {
        let ready = status == "ready";
        let failed = status == "failed";
        let interrupted = status == "interrupted";
        let now = chrono::Utc::now().timestamp_millis();
        let prompt =
            "Design a calm mobile daily planner screen with a green accent and clear typography.";
        let mut parts = vec![
            serde_json::json!({"kind":"tool","id":"generation-demo","call":{"kind":"unknown","name":if ready {"Generate image"} else {"generate_image"},"input":{"prompt":prompt}},"resolved":ready || failed,"isError":failed}),
        ];
        if ready {
            let path = "/fixture/generated-planner.png";
            crate::attachments::store_loaded_for(
                &crate::attachments::AttachmentKey::new("local", path, Some("image/png")),
                "planner.png".into(),
                Arc::new(gpui::Image::from_bytes(
                    gpui::ImageFormat::Png,
                    include_bytes!("../../examples/simulator-sample.png").to_vec(),
                )),
            );
            parts.push(serde_json::json!({"kind":"image","id":"generation-demo:image","path":path,"name":"planner.png","mimeType":"image/png"}));
            parts.push(serde_json::json!({"kind":"text","id":"reply","text":"Created the planner screen. Click the image to open the full preview."}));
        }
        if failed {
            parts.push(serde_json::json!({"kind":"error","id":"error","message":"Image generation usage limit exceeded"}));
        }
        let entries = vec![
            serde_json::from_value(serde_json::json!({"id":"image-user","role":"user","deviceId":"local","createdAt":now-10_000,"status":"complete","parts":[{"kind":"text","id":"prompt","text":prompt}]})).unwrap(),
            serde_json::from_value(serde_json::json!({"id":"image-response","role":"assistant","deviceId":"local","createdAt":now,"status":if ready || failed {"complete"} else if interrupted {"aborted"} else {"streaming"},"parts":parts})).unwrap(),
        ];
        self.state.update(cx, |state, cx| {
            state.apply_transcript(entries);
            cx.notify();
        });
        self.sync(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_generation_names_and_result_ownership_are_precise() {
        for name in [
            "Generate image",
            "generate_image",
            "functions.image_gen__imagegen",
            "imageGeneration",
        ] {
            assert!(is_generation_call(&ToolCall::Unknown {
                name: name.into(),
                input: None
            }));
        }
        assert!(!is_generation_call(&ToolCall::Unknown {
            name: "view_image".into(),
            input: None
        }));
        assert!(owns_image("call:image", "call:image:image"));
        assert!(!owns_image("call", "call:image:image"));
        assert!(owns_image("call", "call:image:2"));
        assert!(!owns_image("call", "call:image:other"));
    }
}
