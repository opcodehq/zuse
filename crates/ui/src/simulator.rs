//! Simulator picker, setup requirements, and a bounded native image viewer.
use crate::{state::EngineHandle, theme::Theme};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui::{
    App, Bounds, ClipboardItem, Context, Div, Entity, ImageFormat, ImageSource, KeyDownEvent,
    MouseButton, MouseDownEvent, ObjectFit, Pixels, Point, Render, RenderImage, Role, SharedString,
    Stateful, Subscription, Task, Window, canvas, div, img, prelude::*, px,
};
use gpui_base::input::{Input as TextInput, InputEditorStyle, InputEvent, InputState};
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};
use zuse_protocol::simulators::{Device, Frame, FrameEvent, Input, Platform, State};
use zuse_rpc::methods;

pub struct SimulatorPane {
    #[cfg(feature = "simulator-fixture")]
    fixture: bool,
    engine: EngineHandle,
    chat_id: String,
    host_id: String,
    state: Option<State>,
    selected: Option<Device>,
    picker: bool,
    active: bool,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    image: Option<Arc<RenderImage>>,
    dimensions: (u32, u32),
    bounds: Rc<RefCell<Bounds<Pixels>>>,
    input: Entity<InputState>,
    _input_events: Subscription,
    watch: Option<Task<()>>,
    frames: Option<Task<()>>,
    action: Option<Task<()>>,
}
impl SimulatorPane {
    pub fn new(
        engine: EngineHandle,
        chat_id: String,
        host_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type on device…"));
        let events = cx.subscribe_in(&input, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.type_text(cx);
            }
        });
        Self {
            #[cfg(feature = "simulator-fixture")]
            fixture: false,
            engine,
            chat_id,
            host_id,
            state: None,
            selected: None,
            picker: false,
            active: false,
            busy: false,
            error: None,
            notice: None,
            image: None,
            dimensions: (0, 0),
            bounds: Rc::new(RefCell::new(Bounds::default())),
            input,
            _input_events: events,
            watch: None,
            frames: None,
            action: None,
        }
    }
    #[cfg(feature = "simulator-fixture")]
    pub fn fixture_load(&mut self, state: State, frame: Option<Frame>, cx: &mut Context<Self>) {
        self.fixture = true;
        self.watch = None;
        self.frames = None;
        self.active = false;
        self.state = Some(state.clone());
        self.selected = selected_session(&state, None, false);
        if let Some(frame) = frame {
            self.dimensions = (frame.width, frame.height);
            self.clear_image(cx);
            self.image = Some(decode_frame(frame).unwrap());
        } else {
            self.clear_image(cx);
        }
        cx.notify();
    }
    fn params(&self) -> Value {
        json!({"chatId":self.chat_id,"targetDeviceId":self.host_id})
    }
    fn clear_image(&mut self, cx: &mut Context<Self>) {
        if let Some(image) = self.image.take() {
            cx.defer(move |cx| ImageSource::Render(image).evict(None, cx));
        }
    }
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        #[cfg(feature = "simulator-fixture")]
        if self.fixture {
            return;
        }
        if self.active == active {
            return;
        }
        self.active = active;
        if !active {
            self.watch = None;
            self.frames = None;
            self.clear_image(cx);
            return;
        }
        self.watch(cx);
        self.start_frames(cx);
    }
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.set_active(false, cx);
        // Queue cleanup behind any boot already admitted by the host.
        let engine = self.engine.clone();
        let params = self.params();
        cx.spawn(async move |_, _| {
            let _ = engine.client().call(methods::CLOSE_SIMULATOR, params).await;
        })
        .detach();
    }
    fn watch(&mut self, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        let params = self.params();
        self.watch = Some(cx.spawn(async move |this, cx| {
            loop {
                let result = engine
                    .client()
                    .call_scoped(methods::SIMULATOR_STATE, params.clone())
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()));
                if this
                    .update(cx, |this, cx| {
                        if this.busy {
                            return;
                        }
                        match result {
                            Ok(state) => {
                                this.apply_state(state, cx);
                            }
                            Err(error) => {
                                this.error = Some(error);
                                this.frames = None;
                                this.clear_image(cx);
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor().timer(Duration::from_secs(4)).await;
            }
        }));
    }
    fn apply_state(&mut self, state: State, cx: &mut Context<Self>) {
        let previous = self.selected.as_ref().map(|d| d.id.clone());
        self.selected = selected_session(&state, previous.as_deref(), self.picker);
        self.state = Some(state);
        if self.selected.as_ref().map(|d| &d.id) != previous.as_ref() {
            self.frames = None;
            self.clear_image(cx);
        }
        self.start_frames(cx);
    }
    fn start_frames(&mut self, cx: &mut Context<Self>) {
        if !self.active || self.frames.is_some() {
            return;
        }
        let Some(device) = self.selected.as_ref() else {
            return;
        };
        let engine = self.engine.clone();
        let mut params = self.params();
        params["deviceId"] = json!(device.id);
        self.frames = Some(cx.spawn(async move |this, cx| {
            let result: Result<(), String> = async {
                let mut stream = engine
                    .client()
                    .subscribe_checked(methods::SIMULATOR_FRAMES, params)
                    .await
                    .map_err(|e| e.to_string())?;
                while let Some(value) = stream.recv().await {
                    let frame: FrameEvent =
                        serde_json::from_value(value).map_err(|e| e.to_string())?;
                    match frame {
                        FrameEvent::Frame { frame } => {
                            let dimensions = (frame.width, frame.height);
                            let image = cx
                                .background_executor()
                                .spawn(async move { decode_frame(frame) })
                                .await?;
                            this.update(cx, |this, cx| {
                                this.clear_image(cx);
                                this.image = Some(image);
                                this.dimensions = dimensions;
                                cx.notify();
                            })
                            .map_err(|e| e.to_string())?;
                        }
                        FrameEvent::Error { message } => return Err(message),
                    }
                }
                Err("Device stream disconnected. Use Reconnect to retry.".to_owned())
            }
            .await;
            if let Err(error) = result {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(error);
                    this.clear_image(cx);
                    cx.notify();
                });
            }
        }));
    }
    fn request(&mut self, method: &'static str, extra: Value, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let mut params = self.params();
        if let Some(device) = &self.selected {
            params["deviceId"] = json!(device.id);
        }
        for (key, value) in extra.as_object().into_iter().flatten() {
            params[key] = value.clone();
        }
        let opening_id = (method == methods::OPEN_SIMULATOR)
            .then(|| params["deviceId"].as_str().unwrap_or_default().to_owned());
        self.busy = true;
        self.error = None;
        self.notice = None;
        let engine = self.engine.clone();
        let state_params = self.params();
        self.action = Some(cx.spawn(async move |this, cx| {
            let result: Result<Option<State>, String> = async {
                let value = engine
                    .client()
                    .call(method, params)
                    .await
                    .map_err(|e| e.to_string())?;
                if method == methods::SIMULATOR_SCREENSHOT {
                    let frame: Frame = serde_json::from_value(value).map_err(|e| e.to_string())?;
                    let data = STANDARD.decode(frame.data).map_err(|e| e.to_string())?;
                    this.update(cx, |this, cx| {
                        let image = gpui::Image::from_bytes(ImageFormat::Png, data);
                        cx.write_to_clipboard(ClipboardItem::new_image(&image));
                        this.notice = Some("Screenshot copied".into());
                    })
                    .map_err(|e| e.to_string())?;
                }
                if matches!(
                    method,
                    methods::SIMULATOR_INPUT | methods::SIMULATOR_SCREENSHOT
                ) {
                    return Ok(None);
                }
                engine
                    .client()
                    .call_as::<State>(methods::SIMULATOR_STATE, state_params)
                    .await
                    .map(Some)
                    .map_err(|e| e.to_string())
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(None) => {}
                    Ok(Some(state)) => {
                        if let Some(id) = opening_id {
                            this.picker = false;
                            this.selected = state.sessions.iter().find(|d| d.id == id).cloned();
                        }
                        this.apply_state(state, cx);
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn send_input(&mut self, input: Input, cx: &mut Context<Self>) {
        self.request(methods::SIMULATOR_INPUT, json!({"input":input}), cx);
    }
    fn type_text(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_none() {
            return;
        }
        let text = self.input.read(cx).value().to_string();
        if !text.is_empty() {
            self.send_input(Input::Text { text }, cx);
        }
    }
    fn button(
        &self,
        id: &'static str,
        label: &'static str,
        cx: &Context<Self>,
        click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let theme = Theme::of(cx);
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(label)
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(5.0))
            .text_color(theme.text)
            .bg(theme.surface_raised)
            .cursor_pointer()
            .when(!self.busy, |d| {
                d.tab_index(0)
                    .on_key_down(contain_activation)
                    .focus_visible(|s| s.border_1().border_color(theme.accent))
                    .hover(|s| s.bg(theme.surface_card))
                    .on_click(cx.listener(move |this, _, window, cx| click(this, window, cx)))
            })
            .when(self.busy, |d| d.opacity(0.5))
            .child(label)
    }
}
fn contain_activation(event: &KeyDownEvent, _: &mut Window, cx: &mut App) {
    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
        cx.stop_propagation();
    }
}
fn selected_session(state: &State, previous: Option<&str>, picker: bool) -> Option<Device> {
    if !state.enabled || picker {
        return None;
    }
    state
        .sessions
        .iter()
        .find(|d| Some(d.id.as_str()) == previous)
        .or_else(|| state.sessions.first())
        .cloned()
}
fn decode_frame(frame: Frame) -> Result<Arc<RenderImage>, String> {
    if frame.data.len() > 12 * 1024 * 1024
        || frame.width == 0
        || frame.height == 0
        || u64::from(frame.width) * u64::from(frame.height) > 32_000_000
    {
        return Err("Device frame exceeded image limits".into());
    }
    let bytes = STANDARD.decode(frame.data).map_err(|e| e.to_string())?;
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut rgba = reader.decode().map_err(|e| e.to_string())?.to_rgba8();
    if rgba.dimensions() != (frame.width, frame.height) {
        return Err("Device frame dimensions did not match".into());
    }
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new([image::Frame::new(rgba)])))
}
/// Letterboxing is excluded from input; normalized coordinates are independent of scale.
fn tap_position(
    bounds: Bounds<Pixels>,
    dimensions: (u32, u32),
    position: Point<Pixels>,
) -> Option<(f64, f64)> {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if dimensions.0 == 0 || dimensions.1 == 0 || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let scale = (w / dimensions.0 as f32).min(h / dimensions.1 as f32);
    let iw = dimensions.0 as f32 * scale;
    let ih = dimensions.1 as f32 * scale;
    let x = (f32::from(position.x - bounds.origin.x) - (w - iw) / 2.0) / iw;
    let y = (f32::from(position.y - bounds.origin.y) - (h - ih) / 2.0) / ih;
    ((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)).then_some((x as f64, y as f64))
}
impl Render for SimulatorPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        self.input.update(cx, |input, _| {
            input.set_editor_style(InputEditorStyle {
                foreground: theme.text,
                muted_foreground: theme.text_muted,
                background: gpui::transparent_black(),
                border: theme.border,
                selection: theme.selection,
                caret: theme.caret,
                ..Default::default()
            });
        });
        let mut body = div()
            .size_full()
            .flex()
            .flex_col()
            .text_size(crate::typography::text_sm())
            .text_color(theme.text)
            .bg(theme.bg);
        let enabled = self.state.as_ref().is_some_and(|s| s.enabled);
        let agents = self.state.as_ref().is_some_and(|s| s.agent_access);
        body = body.child(
            crate::surface_chrome::toolbar(&theme)
                .child(div().flex_1().child("Simulator"))
                .child(
                    self.button("simulator-refresh", "Refresh", cx, |this, _, cx| {
                        this.error = None;
                        this.watch(cx);
                    }),
                )
                .child(self.button(
                    "simulator-enabled",
                    if enabled { "Disable" } else { "Enable" },
                    cx,
                    move |this, _, cx| {
                        this.request(
                            methods::CONFIGURE_SIMULATORS,
                            json!({"enabled":!enabled,"agentAccess":false}),
                            cx,
                        )
                    },
                )),
        );
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .p(px(12.0))
                    .text_color(theme.text_muted)
                    .child(error.clone()),
            );
        }
        if let Some(notice) = &self.notice {
            body = body.child(div().px(px(12.0)).child(notice.clone()));
        }
        if self.busy {
            body = body.child(
                div()
                    .px(px(12.0))
                    .py(px(6.0))
                    .text_color(theme.text_muted)
                    .child("Preparing device…"),
            );
        }
        let Some(state) = self.state.as_ref() else {
            return body
                .child(
                    div()
                        .p(px(20.0))
                        .child("Checking simulator tools on the chat's host…"),
                )
                .into_any_element();
        };
        if !enabled {
            let requirements = state.requirements.clone();
            return body.child(div().id("simulator-setup").flex_1().min_h_0().overflow_y_scroll().p(px(20.0)).flex().flex_col().gap(px(12.0))
                .child(div().text_size(crate::typography::text_md()).child("Run a device beside your chat"))
                .child(div().text_color(theme.text_muted).child("Enable device support, then choose an iOS Simulator or Android emulator on this chat's execution host."))
                .children(requirements.into_iter().map(|r| div().child(format!("{} · {}",if r.ready {"Ready"} else {"Setup"},r.message)))))
                .into_any_element();
        }
        body = body.child(
            div()
                .px(px(12.0))
                .py(px(8.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(self.button(
                    "simulator-agents",
                    if agents {
                        "Agent access: on"
                    } else {
                        "Agent access: off"
                    },
                    cx,
                    move |this, _, cx| {
                        this.request(
                            methods::CONFIGURE_SIMULATORS,
                            json!({"enabled":true,"agentAccess":!agents}),
                            cx,
                        )
                    },
                )),
        );
        if let Some(device) = self.selected.clone() {
            let mut controls = div()
                .px(px(12.0))
                .py(px(6.0))
                .flex()
                .flex_wrap()
                .gap(px(6.0))
                .child(
                    self.button("simulator-devices", "Devices", cx, |this, _, cx| {
                        this.picker = true;
                        this.selected = None;
                        this.frames = None;
                        this.clear_image(cx);
                        cx.notify();
                    }),
                )
                .child(self.button("simulator-home", "Home", cx, |this, _, cx| {
                    this.send_input(Input::Home, cx)
                }))
                .child(
                    self.button("simulator-screenshot", "Screenshot", cx, |this, _, cx| {
                        this.request(methods::SIMULATOR_SCREENSHOT, json!({}), cx)
                    }),
                )
                .child(
                    self.button("simulator-reconnect", "Reconnect", cx, |this, _, cx| {
                        this.frames = None;
                        this.clear_image(cx);
                        this.request(methods::OPEN_SIMULATOR, json!({}), cx);
                    }),
                )
                .child(self.button("simulator-close", "Close", cx, |this, _, cx| {
                    this.request(methods::CLOSE_SIMULATOR, json!({"shutdown":false}), cx)
                }))
                .child(
                    self.button("simulator-shutdown", "Shut down", cx, |this, _, cx| {
                        this.request(methods::CLOSE_SIMULATOR, json!({"shutdown":true}), cx)
                    }),
                );
            if device.platform == Platform::Android {
                controls = controls
                    .child(self.button("simulator-back", "Back", cx, |this, _, cx| {
                        this.send_input(Input::Back, cx)
                    }))
                    .child(
                        self.button("simulator-recents", "Recents", cx, |this, _, cx| {
                            this.send_input(Input::Recents, cx)
                        }),
                    );
            }
            body = body.child(controls).child(
                div()
                    .px(px(12.0))
                    .text_color(theme.text_muted)
                    .child(format!("{} · {}", device.name, device.runtime)),
            );
            let measure = self.bounds.clone();
            let mut viewport = div()
                .id("simulator-screen")
                .flex_1()
                .min_h_0()
                .w_full()
                .relative()
                .overflow_hidden()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        let tap =
                            tap_position(*this.bounds.borrow(), this.dimensions, event.position);
                        if this.image.is_some()
                            && let Some((x, y)) = tap
                        {
                            this.send_input(Input::Tap { x, y }, cx);
                        }
                    }),
                );
            if let Some(image) = &self.image {
                viewport = viewport.child(
                    img(image.clone())
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                );
            } else {
                viewport = viewport.child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(theme.text_muted)
                        .child(if self.error.is_some() {
                            "Device unavailable"
                        } else {
                            "Connecting to device…"
                        }),
                );
            }
            viewport = viewport.child(
                canvas(
                    move |bounds, _, _| {
                        *measure.borrow_mut() = bounds;
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
            body = body.child(viewport).child(
                div()
                    .p(px(8.0))
                    .flex()
                    .gap(px(6.0))
                    .items_center()
                    .child(
                        div()
                            .id("simulator-text")
                            .flex_1()
                            .min_w_0()
                            .h(px(32.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .rounded(px(5.0))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.input_glass_bg())
                            .cursor_text()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.input.update(cx, |input, cx| input.focus(window, cx));
                                }),
                            )
                            .child(TextInput::new(&self.input)),
                    )
                    .child(self.button("simulator-type", "Type", cx, |this, _, cx| {
                        this.type_text(cx)
                    })),
            );
        } else {
            let state = self.state.as_ref().unwrap();
            let mut picker = div()
                .id("simulator-picker")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(10.0));
            for platform in [Platform::Ios, Platform::Android] {
                picker = picker.child(div().text_color(theme.text_muted).child(
                    if platform == Platform::Ios {
                        "iOS Simulators"
                    } else {
                        "Android emulators"
                    },
                ));
                for device in state.devices.iter().filter(|d| d.platform == platform) {
                    let id = device.id.clone();
                    picker = picker.child(
                        div()
                            .id(SharedString::from(format!("device-{id}")))
                            .role(Role::Button)
                            .tab_index(0)
                            .on_key_down(contain_activation)
                            .focus_visible(|s| s.border_1().border_color(theme.accent))
                            .aria_label(format!("Open {}", device.name))
                            .p(px(12.0))
                            .rounded(px(8.0))
                            .bg(theme.surface_card)
                            .cursor_pointer()
                            .child(format!(
                                "{} · {} · {}",
                                device.name,
                                device.runtime,
                                if device.booted {
                                    "Running"
                                } else {
                                    "Shut down"
                                }
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.request(methods::OPEN_SIMULATOR, json!({"deviceId":id}), cx)
                            })),
                    );
                }
                for r in state
                    .requirements
                    .iter()
                    .filter(|r| r.platform == platform && !r.ready)
                {
                    picker =
                        picker.child(div().text_color(theme.text_muted).child(r.message.clone()));
                }
            }
            body = body.child(picker);
        }
        body.into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};
    #[test]
    fn taps_follow_letterboxing_and_resizing() {
        let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(400.0), px(400.0)));
        assert_eq!(
            tap_position(bounds, (100, 200), point(px(210.0), px(220.0))),
            Some((0.5, 0.5))
        );
        assert_eq!(
            tap_position(bounds, (100, 200), point(px(20.0), px(220.0))),
            None
        );
        assert_eq!(
            tap_position(bounds, (100, 200), point(px(110.0), px(20.0))),
            Some((0.0, 0.0))
        );
        assert_eq!(
            tap_position(bounds, (0, 0), point(px(210.0), px(220.0))),
            None
        );
    }
    fn fixture_device(id: &str) -> Device {
        Device {
            id: id.into(),
            name: id.into(),
            platform: Platform::Android,
            runtime: "Android".into(),
            booted: true,
            serial: Some("emulator-5554".into()),
        }
    }
    #[test]
    fn simulator_selection_preserves_viewer_and_respects_picker_or_revocation() {
        let state = State {
            enabled: true,
            sessions: vec![fixture_device("a"), fixture_device("b")],
            ..Default::default()
        };
        assert_eq!(selected_session(&state, Some("b"), false).unwrap().id, "b");
        assert_eq!(
            selected_session(&state, Some("removed"), false).unwrap().id,
            "a"
        );
        assert!(selected_session(&state, Some("b"), true).is_none());
        assert!(
            selected_session(
                &State {
                    enabled: false,
                    ..state
                },
                Some("b"),
                false
            )
            .is_none()
        );
    }
    #[test]
    fn simulator_decode_rejects_bad_or_inconsistent_images() {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(2, 3))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let frame = Frame {
            data: STANDARD.encode(bytes),
            mime_type: "image/png".into(),
            width: 2,
            height: 3,
        };
        assert!(decode_frame(frame.clone()).is_ok());
        assert!(
            decode_frame(Frame {
                width: 3,
                ..frame.clone()
            })
            .is_err()
        );
        assert!(
            decode_frame(Frame {
                data: STANDARD.encode(b"bad"),
                ..frame
            })
            .is_err()
        );
    }
    #[gpui::test]
    fn simulator_hiding_releases_poll_and_frames(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
        });
        let (out, _requests) = tokio::sync::mpsc::channel(16);
        let (_replies, inbound) = tokio::sync::mpsc::channel(16);
        let engine = EngineHandle::from_test_client(zuse_rpc::RpcClient::new(out, inbound));
        let window = cx.add_window(|window, cx| {
            SimulatorPane::new(engine, "chat".into(), "host".into(), window, cx)
        });
        window
            .update(cx, |pane, _, cx| {
                pane.set_active(true, cx);
                assert!(pane.watch.is_some());
                pane.apply_state(
                    State {
                        enabled: true,
                        sessions: vec![fixture_device("a")],
                        ..Default::default()
                    },
                    cx,
                );
                assert!(pane.frames.is_some());
                pane.set_active(false, cx);
                assert!(pane.watch.is_none());
                assert!(pane.frames.is_none());
                assert!(pane.image.is_none());
            })
            .unwrap();
    }
}
