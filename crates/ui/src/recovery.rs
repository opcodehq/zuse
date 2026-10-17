//! User review of interrupted native tools, bound to the selected conversation.
use crate::{
    composer::ComposerInput,
    settings::widgets,
    state::{AppState, EngineHandle},
    theme::Theme,
};
use gpui::{prelude::*, *};
use serde_json::json;
use zuse_protocol::{
    methods,
    recovery::{RecoveryAction, RecoverySnapshot, ResolveRecovery},
};

pub struct RecoveryCard {
    state: Entity<AppState>,
    engine: Option<EngineHandle>,
    subject: Option<(String, String)>,
    snapshot: Option<RecoverySnapshot>,
    selected: Option<usize>,
    result: Entity<ComposerInput>,
    error: Option<String>,
    notice: Option<String>,
    open: bool,
    epoch: u64,
    busy: bool,
    task: Option<Task<()>>,
    _watch: Subscription,
}
impl RecoveryCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let watch = cx.observe(&state, |this: &mut Self, _, cx| this.sync(cx));
        let result = cx.new(|cx| ComposerInput::new("Describe the result you inspected…", cx));
        let mut card = Self {
            state,
            engine: None,
            subject: None,
            snapshot: None,
            selected: None,
            result,
            error: None,
            notice: None,
            open: false,
            epoch: 0,
            busy: false,
            task: None,
            _watch: watch,
        };
        card.sync(cx);
        card
    }
    fn sync(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let next = state
            .selected_chat_row()
            .map(|chat| (chat.id.clone(), chat.device_id.clone()));
        let engine = state.engine().cloned();
        let same = match (&self.engine, &engine) {
            (Some(a), Some(b)) => a.same_connection(b),
            (None, None) => true,
            _ => false,
        };
        if self.subject == next && same {
            return;
        }
        self.subject = next;
        self.engine = engine;
        self.epoch = self.epoch.wrapping_add(1);
        self.task = None;
        self.busy = false;
        self.open = false;
        self.snapshot = None;
        self.selected = None;
        self.error = None;
        self.notice = None;
        self.result.update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }
    fn request(&mut self, decision: Option<RecoveryAction>, cx: &mut Context<Self>) {
        self.sync(cx);
        if self.busy {
            return;
        }
        let (Some(engine), Some((chat, device))) = (self.engine.clone(), self.subject.clone())
        else {
            return;
        };
        let (method, mut params) = if let Some(decision) = decision {
            let Some(snapshot) = &self.snapshot else {
                return;
            };
            let Some(call) = self.selected.and_then(|index| snapshot.calls.get(index)) else {
                return;
            };
            (
                methods::RESOLVE_RECOVERY,
                serde_json::to_value(ResolveRecovery {
                    chat_id: chat,
                    session_id: snapshot.session_id.clone(),
                    agent_id: call.agent_id.clone(),
                    call_id: call.call_id.clone(),
                    revision: call.revision.clone(),
                    decision,
                })
                .unwrap(),
            )
        } else {
            (methods::INSPECT_RECOVERY, json!({"chatId":chat}))
        };
        params["targetDeviceId"] = device.into();
        let epoch = self.epoch;
        self.busy = true;
        self.open = true;
        self.error = None;
        self.notice = None;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(method, params).await;
            let _ = this.update(cx, |card, cx| {
                card.sync(cx);
                if card.epoch != epoch {
                    return;
                }
                card.busy = false;
                match result {
                    Ok(value) => match serde_json::from_value(value) {
                        Ok(snapshot) => {
                            card.snapshot = Some(snapshot);
                            card.selected = None;
                            card.result.update(cx, |input, cx| input.set_text("", cx));
                            if method == methods::RESOLVE_RECOVERY {
                                card.notice =
                                    Some("Review saved. Continue in chat when ready.".into());
                            }
                        }
                        Err(_) => {
                            card.error =
                                Some("The engine returned an invalid recovery review.".into())
                        }
                    },
                    Err(error) => card.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}
impl Render for RecoveryCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_popup();
        let button = |id: SharedString, text: &'static str| {
            widgets::ghost_action(&theme)
                .id(id)
                .tab_index(0)
                .role(Role::Button)
                .child(text)
        };
        let mut view = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .text_color(theme.text)
            .text_size(px(12.))
            .child(
                button(
                    "inspect-native-recovery".into(),
                    if self.busy {
                        "Loading recovery…"
                    } else {
                        "Review interrupted tools"
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| this.request(None, cx))),
            );
        if !self.open {
            return view;
        }
        view = view.child(div().text_color(theme.text_muted).child("Inspect the tool and its effects before authorizing a retry. Saving a review does not run anything."));
        if let Some(error) = &self.error {
            view = view.child(
                div()
                    .text_color(theme.warning)
                    .child(SharedString::from(error.clone())),
            );
        }
        if let Some(notice) = &self.notice {
            view = view.child(SharedString::from(notice.clone()));
        }
        if let Some(snapshot) = &self.snapshot {
            if snapshot.calls.is_empty() {
                view = view.child("No uncertain tool outcomes remain.");
            }
            let mut calls = div()
                .id("recovery-calls")
                .max_h(px(360.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(8.));
            for (index, call) in snapshot.calls.iter().enumerate() {
                calls = calls.child(
                    button(format!("recovery-call-{index}").into(), "Inspect call")
                        .child(SharedString::from(format!(
                            "{} · {}",
                            call.agent_name, call.tool
                        )))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.busy {
                                this.selected = Some(index);
                                this.result.update(cx, |input, cx| input.set_text("", cx));
                                cx.notify();
                            }
                        })),
                );
            }
            view = view.child(calls);
            if let Some(call) = self.selected.and_then(|index| snapshot.calls.get(index)) {
                let input = serde_json::to_string_pretty(&call.input).unwrap_or_default();
                view = view
                    .child(SharedString::from(format!(
                        "{} · {}\n{}",
                        call.agent_name, call.tool, call.call_id
                    )))
                    .child(
                        div()
                            .id("recovery-input")
                            .max_h(px(260.))
                            .overflow_y_scroll()
                            .font_family("Geist Mono")
                            .child(SharedString::from(input.clone())),
                    )
                    .child(
                        button("copy-recovery-input".into(), "Copy full input").on_click(
                            move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(input.clone()))
                            },
                        ),
                    )
                    .child(
                        button(
                            "authorize-recovery-retry".into(),
                            "Authorize retry after inspection",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.request(Some(RecoveryAction::RetryAfterInspection), cx)
                        })),
                    )
                    .child(self.result.clone())
                    .child(
                        button("record-recovery-outcome".into(), "Save inspected outcome")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let output = this.result.read(cx).text().to_owned();
                                this.request(Some(RecoveryAction::RecordOutcome { output }), cx);
                            })),
                    );
            }
        }
        view
    }
}
