//! Transcript activity uses compact disclosures with task-specific details.
use super::*;
use crate::{
    assets::icon,
    shell::{column, control, label, row},
};

impl WorkspaceView {
    pub(super) fn activity_part(
        &self,
        message_id: &str,
        part: &Value,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let reasoning = part["kind"] == "reasoning";
        let key = format!(
            "{message_id}:{}:{}",
            part["kind"].as_str().unwrap_or_default(),
            part["id"].as_str().unwrap_or_default()
        );
        let expanded = self.expanded_parts.contains(&key);
        let call = &part["call"];
        let kind = call["kind"].as_str().unwrap_or_default();
        let (name, glyph) = match kind {
            "exec" => ("Run command", "terminal"),
            "readFile" => ("Read file", "file-code"),
            "writeFile" => ("Write file", "file-code"),
            "editFile" | "applyPatch" => ("Edit file", "file-code"),
            "search" => ("Search files", "file-tree"),
            "glob" => ("Find files", "folder"),
            "webFetch" | "webSearch" => ("Search the web", "globe"),
            "todo" => ("Update plan", "check"),
            "mcp" => (call["tool"].as_str().unwrap_or("Connected tool"), "globe"),
            _ => (call["name"].as_str().unwrap_or("Tool call"), "terminal"),
        };
        let name = if reasoning { "Thought process" } else { name };
        let detail = ["command", "path", "pattern", "query", "url"]
            .into_iter()
            .find_map(|field| call[field].as_str());
        let failed = part["isError"] == true;
        let resolved = part["resolved"] == true;
        let color = if failed {
            0xe49a91
        } else if resolved || reasoning {
            0x858585
        } else {
            0xd7ad85
        };
        let toggle = key.clone();
        let mut view = column().gap_2().child(
            row()
                .id(SharedString::from(format!("expand-{key}")))
                .role(Role::Button)
                .aria_label(SharedString::from(format!(
                    "{} {name}",
                    if expanded { "Collapse" } else { "Expand" }
                )))
                .cursor_pointer()
                .rounded_md()
                .py_1()
                .gap_2()
                .text_color(rgb(0xa3a3a3))
                .hover(|d| d.bg(rgb(0x181818)))
                .child(
                    icon(if expanded {
                        "alt-arrow-down"
                    } else {
                        "alt-arrow-right"
                    })
                    .size(px(11.))
                    .text_color(rgb(0x777777)),
                )
                .child(
                    icon(if reasoning { "clock-circle" } else { glyph })
                        .size(px(14.))
                        .text_color(rgb(color)),
                )
                .child(div().text_size(px(12.)).child(name.to_owned()))
                .when_some(detail.map(str::to_owned), |d, text| {
                    d.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family("Geist Mono")
                            .text_size(px(11.))
                            .text_color(rgb(0x858585))
                            .text_ellipsis()
                            .child(text),
                    )
                })
                .child(div().flex_1())
                .when(!reasoning, |d| {
                    d.child(
                        label(if failed {
                            "Failed"
                        } else if resolved {
                            "Done"
                        } else {
                            "Running"
                        })
                        .text_size(px(10.))
                        .text_color(rgb(color)),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.expanded_parts.remove(&toggle) {
                        this.expanded_parts.insert(toggle.clone());
                    }
                    cx.notify();
                })),
        );
        if expanded {
            let mut body = column()
                .ml_3()
                .pl_4()
                .border_l_1()
                .border_color(rgb(0x303030))
                .gap_3();
            if reasoning {
                body = body.child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(22.))
                        .text_color(rgb(0xa3a3a3))
                        .child(part["text"].as_str().unwrap_or_default().to_owned()),
                );
            } else {
                let input = if kind == "exec" {
                    call["command"].as_str().unwrap_or_default().to_owned()
                } else {
                    serde_json::to_string_pretty(call.get("input").unwrap_or(call))
                        .unwrap_or_default()
                };
                let copy = input.clone();
                body = body.child(
                    column()
                        .rounded_lg()
                        .overflow_hidden()
                        .border_1()
                        .border_color(rgb(0x292929))
                        .child(
                            row()
                                .px_3()
                                .py_1()
                                .bg(rgb(0x191919))
                                .child(
                                    label(if kind == "exec" { "Command" } else { "Input" })
                                        .text_size(px(10.)),
                                )
                                .child(div().flex_1())
                                .child(
                                    control(SharedString::from(format!("copy-{key}")), "Copy")
                                        .h(px(24.))
                                        .text_size(px(10.))
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copy.clone(),
                                            ))
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .p_3()
                                .font_family("Geist Mono")
                                .text_size(px(11.))
                                .line_height(px(19.))
                                .bg(rgb(0x111111))
                                .child(input),
                        ),
                );
                if matches!(kind, "readFile" | "writeFile" | "editFile" | "applyPatch") {
                    if let Some(path) = call["path"].as_str() {
                        let path = path.to_owned();
                        body = body.child(
                            control(
                                SharedString::from(format!("open-tool-file-{key}")),
                                "Open in workspace",
                            )
                            .child(icon("alt-arrow-right").size(px(12.)))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if this.file_dirty(cx) {
                                        this.error = Some(
                                        "Save or discard your edits before opening another file."
                                            .into(),
                                    );
                                        cx.notify();
                                        return;
                                    }
                                    this.right_open = true;
                                    this.right_tab = "files".into();
                                    this.open_entry(json!({"path":path,"kind":"file"}), cx);
                                },
                            )),
                        );
                    }
                }
                body = body.child(
                    column()
                        .gap_2()
                        .child(label("Output").text_size(px(10.)))
                        .child(
                            div()
                                .text_size(px(12.))
                                .line_height(px(20.))
                                .text_color(rgb(if failed { 0xe49a91 } else { 0xa3a3a3 }))
                                .child(
                                    part["output"]
                                        .as_str()
                                        .unwrap_or(if resolved {
                                            "Tool completed."
                                        } else {
                                            "Waiting for tool output…"
                                        })
                                        .to_owned(),
                                ),
                        ),
                );
            }
            view = view.child(body);
        }
        view.into_any_element()
    }
}
