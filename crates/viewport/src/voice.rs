//! Browser call ownership and host signaling. Media stays in the platform adapter.
use super::*;
use crate::localization::message as tr;
use std::{collections::HashSet, time::Duration};
use web_time::Instant;
use zuse_i18n::messages as msg;
use zuse_protocol::voice::{VoiceEvent, VoicePhase, remote as wire};
use zuse_voice_session::VoiceView;
#[derive(Clone, Copy, PartialEq)]
enum Step {
    Capabilities,
    Media,
    Preparing,
    Owner,
    Offer,
    Negotiating,
    Answer,
    Confirming,
    Activating,
    Active,
}
#[derive(Clone, Copy)]
enum Request {
    Capabilities,
    Prepare,
    Negotiate,
    Confirm,
    Report(u64),
}
struct Call {
    key: wire::AttemptKey,
    host: String,
    config: Value,
    step: Step,
    deadline: Instant,
    lease: Option<wire::Lease>,
    negotiation: Option<wire::AttemptKey>,
    owner: Option<u64>,
    requests: HashMap<u64, Request>,
    report_at: Instant,
    report_deadline: Option<Instant>,
    sequence: u64,
    view: VoiceView,
    orb: Entity<super::voice_orb::VoiceOrb>,
}
#[derive(Default)]
pub(super) struct Voice {
    call: Option<Call>,
    pub notice: Option<String>,
    pub stage_open: bool,
    cleanup: HashSet<u64>,
    timer: Option<Task<()>>,
}
impl WorkspaceView {
    fn voice_media(&self, action: &str, extra: Value) {
        let Some(call) = &self.voice.call else {
            return;
        };
        let _ = self.requests.unbounded_send(
            json!({"ui":"voiceMedia","action":action,"attempt":call.key.expose(),"value":extra}),
        );
    }
    fn voice_request(&mut self, kind: Request, method: &str, payload: Value) {
        let Some(call) = &self.voice.call else {
            return;
        };
        let id = self.call(
            method,
            json!({"targetDeviceId":call.host,"payload":payload}),
        );
        self.voice.call.as_mut().unwrap().requests.insert(id, kind);
    }
    pub(super) fn start_browser_voice(&mut self, cx: &mut Context<Self>) {
        if self.voice.call.is_some() || !self.connected {
            return;
        }
        let Some(host) = self
            .selected
            .as_ref()
            .and_then(|c| c["deviceId"].as_str())
            .map(str::to_owned)
            .or_else(|| self.device_id.clone())
        else {
            return;
        };
        let config = if self.composer_provider().is_some_and(|p| p["id"] == "codex") {
            json!({"harness":"codex","model":self.model,"reasoning":self.reasoning,"modelOptions":self.model_options,"sandbox":self.sandbox})
        } else {
            json!({"harness":"codex","model":null,"reasoning":null,"sandbox":"workspace-write"})
        };
        let now = Instant::now();
        self.voice.notice = None;
        self.voice.stage_open = true;
        self.voice.call = Some(Call {
            key: wire::AttemptKey::new(),
            host,
            config,
            step: Step::Capabilities,
            deadline: now + Duration::from_secs(8),
            lease: None,
            negotiation: None,
            owner: None,
            requests: HashMap::new(),
            report_at: now,
            report_deadline: None,
            sequence: 0,
            view: VoiceView::new(),
            orb: cx.new(super::voice_orb::VoiceOrb::new),
        });
        self.voice_request(
            Request::Capabilities,
            methods::VOICE_CAPABILITIES_V2,
            json!({}),
        );
        let key = self.voice.call.as_ref().unwrap().key.clone();
        self.voice.timer = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this
                    .update(cx, |view, cx| {
                        if view.voice.call.as_ref().is_none_or(|call| call.key != key) {
                            return false;
                        }
                        view.voice_tick(cx);
                        true
                    })
                    .unwrap_or(false)
                    == false
                {
                    break;
                }
            }
        }));
        cx.notify();
    }
    pub(super) fn end_browser_voice(&mut self, reason: Option<String>, cx: &mut Context<Self>) {
        let Some(call) = self.voice.call.take() else {
            return;
        };
        // Stop local tracks synchronously in the adapter before waiting for host cleanup.
        let _ = self
            .requests
            .unbounded_send(json!({"ui":"voiceMedia","action":"stop","attempt":call.key.expose()}));
        self.voice.timer = None;
        for id in call.requests.into_keys().chain(call.owner) {
            self.cancel(id);
        }
        if self.connected {
            if let Some(lease) = call.lease {
                let id = self.call(
                    methods::STOP_VOICE_V2,
                    json!({"targetDeviceId":call.host,"payload":lease}),
                );
                self.voice.cleanup.insert(id);
            }
            let id = self.call(
                methods::CANCEL_VOICE_ATTEMPT_V2,
                json!({"targetDeviceId":call.host,"payload":{"attemptKey":call.key}}),
            );
            self.voice.cleanup.insert(id);
        }
        self.voice.notice = reason;
        cx.notify();
    }
    fn voice_tick(&mut self, cx: &mut Context<Self>) {
        let Some(call) = &mut self.voice.call else {
            return;
        };
        let now = Instant::now();
        if (call.step != Step::Active && now >= call.deadline)
            || call.report_deadline.is_some_and(|d| now >= d)
        {
            self.end_browser_voice(
                Some("Voice connection timed out. Start a fresh call to try again.".into()),
                cx,
            );
            return;
        }
        if call.lease.is_some()
            && call.owner.is_some()
            && call.step != Step::Owner
            && call.report_deadline.is_none()
            && now >= call.report_at
        {
            call.sequence += 1;
            let sequence = call.sequence;
            let payload = json!({"lease":call.lease,"sequence":sequence,"muted":call.view.muted(),"state":"ready"});
            call.report_deadline = Some(now + Duration::from_secs(10));
            self.voice_request(
                Request::Report(sequence),
                methods::REPORT_VOICE_MEDIA_V2,
                payload,
            );
        }
    }
    pub(super) fn mute_browser_voice(&mut self, cx: &mut Context<Self>) {
        let Some(call) = &mut self.voice.call else {
            return;
        };
        let muted = !call.view.muted();
        call.view.set_muted(muted);
        self.voice_media("mute", json!({"muted":muted}));
        cx.notify();
    }
    pub(super) fn receive_voice_media(&mut self, frame: &Value, cx: &mut Context<Self>) -> bool {
        let Some(event) = frame.get("voiceMedia") else {
            return false;
        };
        let Some(call) = &mut self.voice.call else {
            return true;
        };
        if event["attempt"].as_str() != Some(call.key.expose()) {
            return true;
        }
        if (call.step != Step::Active && Instant::now() >= call.deadline)
            || call
                .report_deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.end_browser_voice(
                Some("Voice connection timed out. Start a fresh call to try again.".into()),
                cx,
            );
            return true;
        }
        if event["kind"] == "error" {
            self.end_browser_voice(
                Some(
                    event["message"]
                        .as_str()
                        .unwrap_or("Voice media failed.")
                        .into(),
                ),
                cx,
            );
            return true;
        }
        match (call.step, event["kind"].as_str().unwrap_or("")) {
            (Step::Media, "prepared") => {
                call.step = Step::Preparing;
                call.deadline = Instant::now() + Duration::from_secs(65);
                let payload = json!({"attemptKey":call.key,"config":call.config,"voice":null});
                self.voice_request(Request::Prepare, methods::PREPARE_VOICE_V2, payload);
            }
            (Step::Offer, "offer") => {
                let Ok(offer) = serde_json::from_value::<wire::Sdp>(event["sdp"].clone()) else {
                    self.end_browser_voice(
                        Some("The browser produced an invalid voice offer.".into()),
                        cx,
                    );
                    return true;
                };
                let negotiation = wire::AttemptKey::new();
                call.negotiation = Some(negotiation.clone());
                call.step = Step::Negotiating;
                call.deadline = Instant::now() + Duration::from_secs(95);
                let payload = json!({"lease":call.lease,"negotiationId":negotiation,"offer":offer});
                self.voice_request(Request::Negotiate, methods::NEGOTIATE_VOICE_V2, payload);
            }
            (Step::Answer, "ready") => {
                call.step = Step::Confirming;
                call.deadline = Instant::now() + Duration::from_secs(8);
                let payload = json!({"lease":call.lease,"negotiationId":call.negotiation,"muted":call.view.muted()});
                self.voice_request(Request::Confirm, methods::CONFIRM_VOICE_MEDIA_V2, payload);
            }
            (Step::Activating, "active") => {
                call.step = Step::Active;
                call.view.phase = VoicePhase::Active;
            }
            (Step::Active, "levels") => {
                if let Some(lease) = &call.lease {
                    let event = VoiceEvent::Levels {
                        generation: lease.voice.generation,
                        microphone: event["microphone"]
                            .as_u64()
                            .unwrap_or(0)
                            .min(u16::MAX as u64) as u16,
                        speaker: event["speaker"].as_u64().unwrap_or(0).min(u16::MAX as u64) as u16,
                    };
                    call.view.reduce(event, Instant::now());
                }
            }
            _ => {}
        }
        cx.notify();
        true
    }
    pub(super) fn receive_voice_rpc(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.voice.cleanup.remove(&id) {
            self.pending.remove(&id);
            return true;
        }
        let Some(call) = &mut self.voice.call else {
            return false;
        };
        if call.owner != Some(id) && !call.requests.contains_key(&id) {
            return false;
        }
        if (call.step != Step::Active && Instant::now() >= call.deadline)
            || call
                .report_deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.end_browser_voice(
                Some("Voice connection timed out. Start a fresh call to try again.".into()),
                cx,
            );
            return true;
        }
        if call.owner == Some(id) {
            if frame.get("err").is_some() || frame["done"] == true {
                self.end_browser_voice(
                    Some("Voice ownership ended. Start a fresh call to reconnect.".into()),
                    cx,
                );
                return true;
            }
            if let Some(item) = frame.get("item") {
                let Ok(event) = serde_json::from_value::<VoiceEvent>(item.clone()) else {
                    self.end_browser_voice(Some("Invalid voice event from the host.".into()), cx);
                    return true;
                };
                let Some(lease) = &call.lease else {
                    return true;
                };
                let valid = match &event {
                    VoiceEvent::Snapshot { snapshot } => {
                        snapshot.session_id == lease.voice.session_id
                            && snapshot.generation == lease.voice.generation
                    }
                    VoiceEvent::Final { transcript } => {
                        transcript.session_id == lease.voice.session_id
                    }
                    VoiceEvent::Partial { generation, .. }
                    | VoiceEvent::Closed { generation, .. }
                    | VoiceEvent::InvalidatePlayout { generation, .. } => {
                        *generation == lease.voice.generation
                    }
                    _ => {
                        self.end_browser_voice(
                            Some("The host sent unsupported voice media.".into()),
                            cx,
                        );
                        return true;
                    }
                };
                if !valid {
                    return true;
                }
                if let VoiceEvent::Closed { reason, .. } = &event {
                    self.end_browser_voice(reason.map(|r| voice_error(&format!("{r:?}"))), cx);
                    return true;
                }
                if let VoiceEvent::Snapshot { snapshot } = &event {
                    if matches!(snapshot.phase, VoicePhase::Closed | VoicePhase::Failed) {
                        let reason = snapshot
                            .reason
                            .map(|reason| voice_error(&format!("{reason:?}")));
                        self.end_browser_voice(reason, cx);
                        return true;
                    }
                }
                let snapshot = matches!(&event, VoiceEvent::Snapshot { .. });
                call.view.reduce(event, Instant::now());
                if snapshot && call.step == Step::Owner {
                    call.step = Step::Offer;
                    call.deadline = Instant::now() + Duration::from_secs(30);
                    self.voice_media("offer", json!({}));
                }
            }
            cx.notify();
            return true;
        }
        let Some(kind) = call.requests.get(&id).copied() else {
            return false;
        };
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        call.requests.remove(&id);
        self.pending.remove(&id);
        let Some(value) = frame.get("ok") else {
            let error = voice_error(frame["err"].as_str().unwrap_or_default());
            self.end_browser_voice(Some(error), cx);
            return true;
        };
        let mut invalid = false;
        match kind {
            Request::Capabilities => {
                if !serde_json::from_value::<wire::Capabilities>(value.clone())
                    .is_ok_and(|v| v.client_webrtc && v.protocol == wire::CAPABILITY)
                {
                    invalid = true;
                } else {
                    call.step = Step::Media;
                    call.deadline = Instant::now() + Duration::from_secs(60);
                    self.voice_media("prepare", json!({}));
                }
            }
            Request::Prepare => {
                if let Ok(prepared) = serde_json::from_value::<wire::Prepared>(value.clone()) {
                    if prepared.lease.host_device_id != call.host {
                        invalid = true;
                    } else {
                        call.lease = Some(prepared.lease.clone());
                        call.step = Step::Owner;
                        call.deadline = Instant::now() + Duration::from_secs(8);
                        let host = call.host.clone();
                        let id = self.call(
                            methods::OWN_VOICE_V2,
                            json!({"targetDeviceId":host,"payload":prepared.lease}),
                        );
                        self.voice.call.as_mut().unwrap().owner = Some(id);
                    }
                } else {
                    invalid = true;
                }
            }
            Request::Negotiate => {
                if let Ok(reply) = serde_json::from_value::<wire::Negotiated>(value.clone()) {
                    if call.negotiation.as_ref() != Some(&reply.negotiation_id) {
                        invalid = true;
                    } else {
                        call.step = Step::Answer;
                        call.deadline = Instant::now() + Duration::from_secs(30);
                        self.voice_media("answer", json!({"sdp":reply.answer}));
                    }
                } else {
                    invalid = true;
                }
            }
            Request::Confirm => {
                if let Ok(snapshot) =
                    serde_json::from_value::<zuse_protocol::voice::VoiceSnapshot>(value.clone())
                {
                    if call.lease.as_ref().is_none_or(|lease| {
                        snapshot.session_id != lease.voice.session_id
                            || snapshot.generation != lease.voice.generation
                            || snapshot.phase != VoicePhase::Active
                    }) {
                        invalid = true;
                    } else {
                        call.view
                            .reduce(VoiceEvent::Snapshot { snapshot }, Instant::now());
                        call.step = Step::Activating;
                        call.deadline = Instant::now() + Duration::from_secs(10);
                        let muted = call.view.muted();
                        self.voice_media("activate", json!({"muted":muted}));
                    }
                } else {
                    invalid = true;
                }
            }
            Request::Report(sequence) => {
                if !serde_json::from_value::<wire::Ack>(value.clone())
                    .is_ok_and(|ack| ack.sequence == sequence)
                {
                    invalid = true;
                } else {
                    call.report_deadline = None;
                    call.report_at = Instant::now() + Duration::from_secs(wire::HEARTBEAT_SECS);
                    self.voice_media("lease", json!({}));
                }
            }
        }
        if invalid {
            self.end_browser_voice(
                Some("The host returned an invalid voice response.".into()),
                cx,
            );
        }
        cx.notify();
        true
    }
    pub(super) fn disconnect_browser_voice(&mut self, cx: &mut Context<Self>) {
        self.voice.cleanup.clear();
        self.end_browser_voice(
            Some(
                "Voice stopped after the connection changed. Start a fresh call to reconnect."
                    .into(),
            ),
            cx,
        );
    }
    pub(super) fn toggle_browser_voice_stage(&mut self, cx: &mut Context<Self>) {
        self.voice.stage_open = !self.voice.stage_open;
        cx.notify();
    }
    pub(super) fn sync_browser_voice_visibility(&mut self, cx: &mut Context<Self>) {
        if let Some(call) = &self.voice.call {
            let visible = self.environment.visible
                && self.voice.stage_open
                && !self.settings
                && !self.project_picker;
            call.orb.update(cx, |orb, cx| {
                if orb.visible != visible {
                    orb.visible = visible;
                    cx.notify();
                }
            });
        }
    }
    pub(super) fn browser_voice_replaces_composer(&self) -> bool {
        self.browser_voice_live() && self.voice.stage_open && !self.settings && !self.project_picker
    }
    pub(super) fn browser_voice_stage(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::shell::{column, control, label, row};
        let Some(call) = &self.voice.call else {
            return div().into_any_element();
        };
        let reduced = cx.reduce_motion();
        call.orb.update(cx, |orb, cx| {
            if orb.update(
                call.view.orb_state(),
                call.view.microphone_level(),
                call.view.speaker_level(),
                reduced,
            ) {
                cx.notify();
            }
        });
        column()
            .w_full()
            .items_center()
            .gap_4()
            .py_6()
            .child(label(if call.step == Step::Active {
                if call.view.muted() {
                    "Microphone muted"
                } else if call.view.playing() {
                    "Speaking"
                } else {
                    "Listening"
                }
            } else {
                "Connecting voice…"
            }))
            .child(call.orb.clone())
            .child(
                div().max_w(px(640.)).px_4().text_size(px(18.)).child(
                    call.view
                        .caption()
                        .chars()
                        .rev()
                        .take(160)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect::<String>(),
                ),
            )
            .child(
                row()
                    .gap_3()
                    .child(
                        control(
                            "voice-mute",
                            if call.view.muted() { "Unmute" } else { "Mute" },
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.mute_browser_voice(cx))),
                    )
                    .child(
                        control("voice-end", tr(cx, msg::native::end_call)).on_click(
                            cx.listener(|this, _, _, cx| this.end_browser_voice(None, cx)),
                        ),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn browser_voice_live(&self) -> bool {
        self.voice.call.is_some()
    }
}

fn voice_error(error: &str) -> String {
    if error.contains("ChatgptRequired") {"Sign in to Codex with ChatGPT on the execution computer to use voice."}
    else if error.contains("NativeRuntimeUnavailable") {"Install or update Codex with its official installer on the execution computer to use voice."}
    else if error.contains("Busy") {"Another window already owns the voice session."}
    else if error.contains("Unsupported") {"Update Codex on the execution computer to use voice."}
    else if error.contains("InvalidLease") {"Voice ownership expired. Start a fresh call."}
    else {"Voice could not connect. Check Codex and its account on the execution computer, then start a fresh call."}.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    fn setup(cx: &mut TestAppContext) -> (WindowHandle<WorkspaceView>, UnboundedReceiver<Value>) {
        cx.update(initialize);
        let (requests, rx) = futures::channel::mpsc::unbounded();
        let (_, events) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(Transport { requests, events }, window, cx)
        });
        window
            .update(cx, |view, _, _| {
                view.connected = true;
                view.device_id = Some("local".into());
                view.selected = Some(json!({"id":"chat","deviceId":"host"}));
            })
            .unwrap();
        (window, rx)
    }
    fn request(view: &WorkspaceView, kind: fn(&Request) -> bool) -> u64 {
        *view
            .voice
            .call
            .as_ref()
            .unwrap()
            .requests
            .iter()
            .find(|(_, r)| kind(r))
            .unwrap()
            .0
    }
    fn snapshot() -> Value {
        json!({"sessionId":"session","chatId":"voice-chat","generation":7,"phase":"active","muted":false,"playing":false,"work":"idle"})
    }
    fn prepared() -> Value {
        json!({"lease":{"hostDeviceId":"host","voice":{"sessionId":"session","generation":7,"token":"private-test-lease"}},"chatId":"voice-chat","voices":[],"heartbeatSeconds":5,"leaseSeconds":15})
    }
    fn capabilities(view: &mut WorkspaceView, cx: &mut Context<WorkspaceView>) -> String {
        view.start_browser_voice(cx);
        let id = request(view, |r| matches!(r, Request::Capabilities));
        view.receive(
            json!({"id":id,"ok":{"protocol":wire::CAPABILITY,"clientWebrtc":true,"voices":[]}}),
            cx,
        );
        view.voice.call.as_ref().unwrap().key.expose().to_owned()
    }
    #[gpui::test]
    fn voice_orders_media_and_ownership_before_unmute_and_keeps_routing_scoped(
        cx: &mut TestAppContext,
    ) {
        let (window, mut requests) = setup(cx);
        window
            .update(cx, |view, _, cx| {
                let key = capabilities(view, cx);
                assert!(
                    view.voice.call.as_ref().unwrap().requests.is_empty(),
                    "permission must precede host preparation"
                );
                view.receive(json!({"voiceMedia":{"attempt":key,"kind":"prepared"}}), cx);
                let id = request(view, |r| matches!(r, Request::Prepare));
                view.receive(json!({"id":id,"ok":prepared()}), cx);
                let owner = view.voice.call.as_ref().unwrap().owner.unwrap();
                let mut stale = snapshot();
                stale["generation"] = json!(6);
                view.receive(
                    json!({"id":owner,"item":{"type":"snapshot","snapshot":stale}}),
                    cx,
                );
                assert!(view.voice.call.as_ref().unwrap().step == Step::Owner);
                view.receive(
                    json!({"id":owner,"item":{"type":"snapshot","snapshot":snapshot()}}),
                    cx,
                );
                view.receive(
                    json!({"voiceMedia":{"attempt":key,"kind":"offer","sdp":"offer"}}),
                    cx,
                );
                let id = request(view, |r| matches!(r, Request::Negotiate));
                let negotiation = view
                    .voice
                    .call
                    .as_ref()
                    .unwrap()
                    .negotiation
                    .clone()
                    .unwrap();
                view.receive(
                    json!({"id":id,"ok":{"negotiationId":negotiation,"answer":"answer"}}),
                    cx,
                );
                view.mute_browser_voice(cx);
                view.receive(json!({"voiceMedia":{"attempt":key,"kind":"ready"}}), cx);
                let id = request(view, |r| matches!(r, Request::Confirm));
                view.receive(json!({"id":id,"ok":snapshot()}), cx);
                view.receive(json!({"voiceMedia":{"attempt":key,"kind":"active"}}), cx);
                assert!(view.voice.call.as_ref().unwrap().step == Step::Active);
                assert!(view.voice.call.as_ref().unwrap().view.muted());
                view.voice_tick(cx);
                let id = request(view, |r| matches!(r, Request::Report(1)));
                view.receive(json!({"id":id,"ok":{"sequence":1}}), cx);
                view.toggle_browser_voice_stage(cx);
                assert!(!view.browser_voice_replaces_composer());
                assert!(view.browser_voice_live());
                view.end_browser_voice(None, cx);
            })
            .unwrap();
        let mut media = vec![];
        let mut methods_seen = vec![];
        while let Ok(frame) = requests.try_recv() {
            if frame["ui"] == "voiceMedia" {
                media.push(frame.clone());
            }
            if frame["method"].is_string() {
                assert_eq!(frame["params"]["targetDeviceId"], "host");
                methods_seen.push(frame.clone());
            }
        }
        let activation = media.iter().find(|f| f["action"] == "activate").unwrap();
        assert_eq!(activation["value"]["muted"], true);
        assert_eq!(
            media.iter().filter(|f| f["action"] == "activate").count(),
            1
        );
        assert_eq!(media.last().unwrap()["action"], "stop");
        assert!(
            methods_seen
                .iter()
                .any(|f| f["method"] == methods::STOP_VOICE_V2)
        );
        assert!(
            methods_seen
                .iter()
                .any(|f| f["method"] == methods::CANCEL_VOICE_ATTEMPT_V2)
        );
        assert!(!methods_seen.iter().any(|f| {
            [
                methods::QUEUE_COMMAND,
                methods::QUEUE_MESSAGE,
                methods::MUTATE,
            ]
            .contains(&f["method"].as_str().unwrap())
        }));
    }
    #[gpui::test]
    fn cancelled_permission_and_lost_prepare_cannot_activate_a_later_call(cx: &mut TestAppContext) {
        let (window, mut requests) = setup(cx);
        window
            .update(cx, |view, _, cx| {
                let old = capabilities(view, cx);
                view.end_browser_voice(None, cx);
                let current = capabilities(view, cx);
                view.receive(json!({"voiceMedia":{"attempt":old,"kind":"prepared"}}), cx);
                assert!(view.voice.call.as_ref().unwrap().requests.is_empty());
                view.receive(
                    json!({"voiceMedia":{"attempt":current,"kind":"prepared"}}),
                    cx,
                );
                let stale = request(view, |r| matches!(r, Request::Prepare));
                view.end_browser_voice(None, cx);
                let next = capabilities(view, cx);
                view.receive(json!({"id":stale,"ok":prepared()}), cx);
                assert!(view.voice.call.as_ref().unwrap().lease.is_none());
                view.receive(
                    json!({"voiceMedia":{"attempt":current,"kind":"active"}}),
                    cx,
                );
                assert!(view.voice.call.as_ref().unwrap().step == Step::Media);
                assert_eq!(view.voice.call.as_ref().unwrap().key.expose(), next);
                view.connected = false;
                view.disconnect_browser_voice(cx);
                assert!(!view.browser_voice_live());
            })
            .unwrap();
        let mut activations = 0;
        let mut owners = 0;
        while let Ok(frame) = requests.try_recv() {
            if frame["ui"] == "voiceMedia" && frame["action"] == "activate" {
                activations += 1;
            }
            if frame["method"] == methods::OWN_VOICE_V2 {
                owners += 1;
            }
        }
        assert_eq!(activations, 0);
        assert_eq!(owners, 0);
    }
    #[gpui::test]
    fn expired_voice_reply_cannot_unmute_or_consume_an_unrelated_receipt(cx: &mut TestAppContext) {
        let (window, mut requests) = setup(cx);
        window
            .update(cx, |view, _, cx| {
                let key = capabilities(view, cx);
                view.receive(json!({"voiceMedia":{"attempt":key,"kind":"prepared"}}), cx);
                let id = request(view, |r| matches!(r, Request::Prepare));
                view.voice.call.as_mut().unwrap().deadline =
                    Instant::now() - Duration::from_secs(1);
                assert!(!view.receive_voice_rpc(9999, &json!({"ok":true}), cx));
                assert!(view.browser_voice_live());
                view.receive(json!({"id":id,"ok":prepared()}), cx);
                assert!(!view.browser_voice_live());
            })
            .unwrap();
        while let Ok(frame) = requests.try_recv() {
            assert_ne!(frame["method"], methods::OWN_VOICE_V2);
            assert!(frame["action"] != "activate");
        }
    }
}
