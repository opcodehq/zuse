//! Paged tool results with bounded retention and virtualized output/diff rows.
use super::*;
use crate::shell::{column, control, label, row};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct ToolResultView {
    page: Option<ResultPage>,
    pages: std::collections::BTreeMap<usize, ResultPage>,
    history: Vec<usize>,
    busy: bool,
    requested_offset: usize,
    error: Option<String>,
}
#[derive(Clone)]
struct ResultPage {
    text: String,
    offset: usize,
    next: usize,
    total: usize,
    lines: Arc<Vec<ResultLine>>,
    path: Option<String>,
    scroll: UniformListScrollHandle,
}
pub(super) struct ResultLine {
    pub(super) text: String,
    pub(super) kind: i8,
}
pub(super) fn diff_lines(old: &str, new: &str) -> Vec<ResultLine> {
    similar::TextDiff::configure()
        .timeout(std::time::Duration::from_millis(100))
        .diff_lines(old, new)
        .iter_all_changes()
        .map(|change| ResultLine {
            text: change.value().trim_end_matches(['\r', '\n']).to_owned(),
            kind: match change.tag() {
                similar::ChangeTag::Delete => -1,
                similar::ChangeTag::Insert => 1,
                _ => 0,
            },
        })
        .collect()
}
fn result_lines(text: &str, diff: bool) -> (Vec<ResultLine>, Option<String>) {
    if diff {
        if let Ok(diff) = serde_json::from_str::<zuse_protocol::ToolDiff>(text) {
            let old = diff.old_text.as_deref().unwrap_or_default();
            let lines = diff_lines(old, &diff.new_text);
            return (lines, Some(diff.path));
        }
    }
    (
        text.lines()
            .map(|line| ResultLine {
                text: line.into(),
                kind: 0,
            })
            .collect(),
        None,
    )
}
impl WorkspaceView {
    pub(super) fn interrupt_tool_results(&mut self) {
        self.tool_result_requests.clear();
        for view in self.tool_results.values_mut() {
            if view.busy {
                view.busy = false;
                view.error = Some("Read interrupted. Retry when connected.".into());
            }
        }
    }
    pub(super) fn reset_tool_results(&mut self) {
        for id in std::mem::take(&mut self.tool_result_requests).into_keys() {
            self.cancel(id);
        }
        self.tool_results.clear();
    }
    fn read_tool_result(&mut self, reference: String, offset: usize, cx: &mut Context<Self>) {
        if let Some(view) = self.tool_results.get_mut(&reference) {
            if let Some(page) = view.pages.get(&offset).cloned() {
                if let Some(previous) = &view.page {
                    if offset > previous.offset {
                        view.history.push(previous.offset);
                    } else if view.history.last() == Some(&offset) {
                        view.history.pop();
                    }
                }
                view.page = Some(page);
                view.error = None;
                cx.notify();
                return;
            }
        }
        if self
            .tool_results
            .get(&reference)
            .is_some_and(|view| view.busy)
        {
            return;
        }
        if self.tool_results.len() >= 4 && !self.tool_results.contains_key(&reference) {
            let old = self
                .tool_results
                .iter()
                .find(|(_, v)| !v.busy)
                .map(|(key, _)| key.clone());
            let Some(old) = old else {
                return;
            };
            self.tool_results.remove(&old);
        }
        let view = self.tool_results.entry(reference.clone()).or_default();
        view.busy = true;
        view.requested_offset = offset;
        view.error = None;
        let source = reference.split_once('/').map(|(chat, _)| chat);
        let target = self
            .chats
            .iter()
            .find(|chat| chat["id"].as_str() == source)
            .or(self.selected.as_ref())
            .map(|chat| chat["deviceId"].clone());
        let id = self.call(
            methods::FETCH_TOOL_BLOB,
            json!({"blobRef":reference,"offset":offset,"limit":65536,"targetDeviceId":target}),
        );
        self.tool_result_requests.insert(id, (reference, offset));
        cx.notify();
    }
    pub(super) fn receive_tool_result(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((reference, offset)) = self.tool_result_requests.get(&id).cloned() else {
            return false;
        };
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        self.tool_result_requests.remove(&id);
        self.pending.remove(&id);
        let view = self.tool_results.entry(reference.clone()).or_default();
        view.busy = false;
        let result = (|| {
            if let Some(error) = frame["err"].as_str() {
                return Err(error.to_owned());
            }
            let value = &frame["ok"];
            let text = value["text"].as_str().ok_or("Missing tool output text")?;
            let next = value["nextOffset"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or("Missing tool output cursor")?;
            let total = value["totalBytes"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or("Missing tool output size")?;
            let done = value["done"]
                .as_bool()
                .ok_or("Missing tool output completion state")?;
            if text.len() > 65536
                || next != offset.saturating_add(text.len())
                || next > total
                || done != (next == total)
                || (!done && next == offset)
            {
                return Err("Invalid tool output page".into());
            }
            let (lines, path) =
                result_lines(text, reference.ends_with(".diff") && offset == 0 && done);
            Ok(ResultPage {
                text: text.into(),
                offset,
                next,
                total,
                lines: Arc::new(lines),
                path,
                scroll: UniformListScrollHandle::new(),
            })
        })();
        match result {
            Ok(page) => {
                if let Some(previous) = &view.page {
                    if offset > previous.offset {
                        view.history.push(previous.offset);
                    } else if view.history.last() == Some(&offset) {
                        view.history.pop();
                    }
                }
                view.pages.insert(offset, page.clone());
                while view.pages.len() > 4 {
                    let Some(old) = view.pages.keys().copied().find(|key| *key != offset) else {
                        break;
                    };
                    view.pages.remove(&old);
                }
                view.page = Some(page);
                view.error = None;
            }
            Err(error) => view.error = Some(error),
        }
        cx.notify();
        true
    }
    pub(super) fn tool_result_control(
        &self,
        reference: &str,
        name: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let reference = reference.to_owned();
        let mut result = column().gap_2();
        match self.tool_results.get(&reference) {
            Some(view) => {
                if let Some(page) = &view.page {
                    let lines = page.lines.clone();
                    let visible_height = (lines.len().clamp(1, 11) as f32) * 20.;
                    let text = page.text.clone();
                    let mut toolbar = row()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .bg(rgb(0x1a1a1a))
                        .child(
                            label(format!(
                                "{} · {}–{} / {} bytes",
                                name,
                                if page.total == 0 { 0 } else { page.offset + 1 },
                                page.next,
                                page.total
                            ))
                            .text_size(px(10.)),
                        )
                        .child(div().flex_1())
                        .child(
                            control(
                                SharedString::from(format!("copy-result-{reference}")),
                                "Copy page",
                            )
                            .text_size(px(10.))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
                            }),
                        );
                    if let Some(previous) = view.history.last().copied() {
                        let reference = reference.clone();
                        toolbar = toolbar.child(
                            control(
                                SharedString::from(format!("previous-result-{reference}")),
                                "Previous",
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.read_tool_result(reference.clone(), previous, cx)
                                },
                            )),
                        );
                    }
                    if page.next < page.total {
                        let reference = reference.clone();
                        let next = page.next;
                        toolbar = toolbar.child(
                            control(
                                SharedString::from(format!("next-result-{reference}")),
                                "Next",
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.read_tool_result(reference.clone(), next, cx)
                                },
                            )),
                        );
                    }
                    result = result.child(
                        column()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(0x292929))
                            .overflow_hidden()
                            .child(toolbar)
                            .when_some(page.path.clone(), |d, path| {
                                d.child(label(path).px_3().py_2())
                            })
                            .child(
                                uniform_list(
                                    SharedString::from(format!(
                                        "result-lines-{reference}-{}",
                                        page.offset
                                    )),
                                    lines.len(),
                                    move |range, _, _| {
                                        range
                                            .map(|index| {
                                                let line = &lines[index];
                                                row()
                                                    .h(px(20.))
                                                    .gap_2()
                                                    .px_2()
                                                    .bg(rgb(match line.kind {
                                                        -1 => 0x291719,
                                                        1 => 0x14291e,
                                                        _ => 0x111111,
                                                    }))
                                                    .font_family("Geist Mono")
                                                    .text_size(px(11.))
                                                    .child(
                                                        div()
                                                            .w(px(32.))
                                                            .flex_shrink_0()
                                                            .text_color(rgb(0x666666))
                                                            .child((index + 1).to_string()),
                                                    )
                                                    .child(
                                                        div()
                                                            .w(px(10.))
                                                            .flex_shrink_0()
                                                            .text_color(rgb(match line.kind {
                                                                -1 => 0xe5a0a0,
                                                                1 => 0x8ccca4,
                                                                _ => 0x777777,
                                                            }))
                                                            .child(match line.kind {
                                                                -1 => "−",
                                                                1 => "+",
                                                                _ => " ",
                                                            }),
                                                    )
                                                    .child(
                                                        div()
                                                            .whitespace_nowrap()
                                                            .child(line.text.clone()),
                                                    )
                                            })
                                            .collect::<Vec<_>>()
                                    },
                                )
                                .track_scroll(&page.scroll)
                                .h(px(visible_height))
                                .w_full()
                                .with_horizontal_sizing_behavior(
                                    ListHorizontalSizingBehavior::Unconstrained,
                                ),
                            ),
                    );
                }
                if view.busy {
                    result = result.child(label("Loading tool details…"));
                }
                if let Some(error) = &view.error {
                    let reference = reference.clone();
                    let offset = view.requested_offset;
                    result = result.child(label(error.clone())).child(
                        control(
                            SharedString::from(format!("retry-result-{reference}")),
                            "Retry read",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.read_tool_result(reference.clone(), offset, cx)
                        })),
                    );
                }
            }
            None => {
                let id = SharedString::from(format!("load-result-{reference}"));
                result = result.child(control(id, name).on_click(cx.listener(
                    move |this, _, _, cx| this.read_tool_result(reference.clone(), 0, cx),
                )));
            }
        }
        result.into_any_element()
    }
}
