//! Simulator picker, setup requirements, and a bounded native image viewer.
use crate::localization::message as tr;
use crate::tooltip::TooltipExt as _;
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
use zuse_i18n::messages as msg;
use zuse_protocol::simulators::{
    AgentStatus, Device, Frame, FrameEvent, Input, Platform, Setup, SetupStatus, State,
};
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
    menu_open: bool,
    setup_agents: Option<bool>,
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
        let input = cx
            .new(|cx| InputState::new(window, cx).placeholder(tr(cx, msg::native::type_on_device)));
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
            menu_open: false,
            setup_agents: None,
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
                let preparing = this
                    .update(cx, |this, _| {
                        this.state
                            .as_ref()
                            .is_some_and(|state| state.enabled && setup_pending(state))
                    })
                    .unwrap_or(false);
                cx.background_executor()
                    .timer(if preparing {
                        Duration::from_millis(350)
                    } else {
                        Duration::from_secs(4)
                    })
                    .await;
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
        if method == methods::CONFIGURE_SIMULATORS {
            self.watch = None;
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
                if method == methods::CONFIGURE_SIMULATORS && this.active {
                    this.watch(cx);
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
    fn render_setup(
        &self,
        setup: &Setup,
        enabled: bool,
        agents: bool,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::of(cx);
        let failed = matches!(setup.status, SetupStatus::Error | SetupStatus::Unavailable)
            || setup.agent_status == AgentStatus::Error;
        let agents = if enabled {
            agents
        } else {
            self.setup_agents.unwrap_or(agents)
        };
        let checking = setup.requirements.is_empty();
        let blocked = !checking && setup.requirements.iter().any(|r| !r.ready);
        let mut card = div().flex_none().w_full().max_w(px(320.)).mx_auto().flex().flex_col().gap(px(20.))
            .child(div().flex().flex_col().items_center().gap(px(12.))
                .child(crate::icons::icon(crate::icons::SMARTPHONE).size(px(40.)).text_color(theme.text_muted))
                .child(div().text_size(px(14.)).child(if !enabled {"Set up devices"} else if failed {"Device setup needs attention"} else {"Preparing your devices"}))
                .child(div().text_center().text_color(theme.text_muted).line_height(px(20.)).child(if !enabled {
                    "Run your app beside the chat. Zuse installs and starts the device helpers on this host."
                } else if failed {"Fix the issue below, then try again."} else if agents {"Setting up Device Hub and your agent tools. You can cancel at any time."} else {"Setting up Device Hub. You can cancel at any time."})));
        if !enabled || blocked {
            let mut list = div()
                .rounded(px(8.))
                .bg(theme.muted_fill())
                .px(px(12.))
                .py(px(6.))
                .flex()
                .flex_col()
                .gap(px(4.));
            if checking {
                list = list.child(
                    div()
                        .py(px(8.))
                        .text_color(theme.text_muted)
                        .child(tr(cx, msg::native::checking_this_host)),
                );
            }
            for requirement in &setup.requirements {
                list = list.child(
                    div()
                        .py(px(5.))
                        .flex()
                        .items_start()
                        .gap(px(10.))
                        .child(
                            crate::icons::icon(if requirement.ready {
                                crate::icons::QUEUE_CHECK
                            } else {
                                crate::icons::QUEUE_CLOSE
                            })
                            .size(px(14.))
                            .text_color(if requirement.ready {
                                theme.text_muted
                            } else {
                                theme.danger
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(
                                    div()
                                        .flex()
                                        .items_start()
                                        .gap(px(8.))
                                        .child(div().flex_none().child(requirement.label.clone()))
                                        .when(requirement.ready, |row| {
                                            row.child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .text_right()
                                                    .text_size(px(11.))
                                                    .line_height(px(16.))
                                                    .text_color(theme.text_muted)
                                                    .child(requirement.detail.clone()),
                                            )
                                        }),
                                )
                                .when(!requirement.ready, |column| {
                                    column.child(
                                        div()
                                            .text_size(px(11.))
                                            .line_height(px(16.))
                                            .text_color(theme.text_muted)
                                            .child(requirement.detail.clone()),
                                    )
                                }),
                        ),
                );
            }
            card = card.child(list);
        } else {
            let mut list = div()
                .rounded(px(8.))
                .bg(theme.muted_fill())
                .px(px(12.))
                .py(px(6.))
                .flex()
                .flex_col();
            for (label, status) in setup_steps(setup, agents) {
                let (icon, color) = match status {
                    SetupStep::Done => (crate::icons::QUEUE_CHECK, theme.success),
                    SetupStep::Active => (crate::icons::CLOCK_CIRCLE, theme.text),
                    SetupStep::Failed => (crate::icons::QUEUE_CLOSE, theme.danger),
                    SetupStep::Pending => (crate::icons::CLOCK_CIRCLE, theme.text_muted),
                };
                list = list.child(
                    div()
                        .h(px(30.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(crate::icons::icon(icon).size(px(14.)).text_color(color))
                        .child(div().min_w_0().truncate().text_color(color).child(label)),
                );
            }
            card = card.child(list);
        }
        if blocked
            && setup
                .requirements
                .iter()
                .any(|r| r.label == "Node.js" && !r.ready)
        {
            card = card.child(
                self.button("simulator-get-node", "Get Node.js", cx, |_, _, cx| {
                    cx.open_url("https://nodejs.org/en/download")
                }),
            );
        }
        if !enabled {
            card = card.child(
                div()
                    .id("simulator-setup-agents")
                    .role(Role::Switch)
                    .aria_label(tr(cx, msg::native::allow_agents_to_control_devices))
                    .aria_toggled(if agents {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .tab_index(0)
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .cursor_pointer()
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(div().flex_1().min_w_0().child(tr(cx, msg::native::allow_agent_control)))
                            .child(crate::settings::widgets::toggle_switch(
                                &theme,
                                agents,
                                "simulator-setup-agents-switch",
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(theme.text_muted)
                            .child(tr(cx, msg::native::let_agents_inspect_and_interact_with_the_device_in_this_chat)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.setup_agents = Some(!agents);
                        cx.notify();
                    })),
            );
            if blocked || checking {
                card = card.child(self.button(
                    "simulator-check-again",
                    "Check again",
                    cx,
                    |this, _, cx| {
                        this.error = None;
                        this.watch(cx);
                    },
                ));
            } else {
                card = card.child(self.button(
                    "simulator-enable",
                    "Turn on devices",
                    cx,
                    move |this, _, cx| {
                        this.request(
                            methods::CONFIGURE_SIMULATORS,
                            json!({"enabled":true,"agentAccess":agents}),
                            cx,
                        )
                    },
                ));
            }
            card=card.child(div().text_size(px(11.)).line_height(px(16.)).text_center().text_color(theme.text_muted).child(if blocked {
                "Finish the host requirements, then check again. Nothing has been installed.".to_owned()
            } else if agents {
                format!("Installs Device Hub {} and agent-device {} in Zuse’s private tool directory.",setup.hub_version,setup.agent_version)
            } else {
                format!("Installs Device Hub {} in Zuse’s private tool directory.",setup.hub_version)
            }));
        } else if failed {
            let detail = if setup.agent_status == AgentStatus::Error {
                &setup.agent_detail
            } else {
                &setup.detail
            };
            card = card
                .child(
                    div()
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(theme.danger)
                        .child(detail.clone()),
                )
                .child(
                    self.button("simulator-retry", "Try again", cx, move |this, _, cx| {
                        this.request(
                            methods::CONFIGURE_SIMULATORS,
                            json!({"enabled":true,"agentAccess":agents}),
                            cx,
                        )
                    }),
                )
                .child(self.button("simulator-off", "Turn off", cx, |this, _, cx| {
                    this.request(
                        methods::CONFIGURE_SIMULATORS,
                        json!({"enabled":false,"agentAccess":false}),
                        cx,
                    )
                }));
        } else {
            let detail = if setup.agent_status == AgentStatus::Installing {
                &setup.agent_detail
            } else {
                &setup.detail
            };
            card = card
                .child(
                    div()
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_center()
                        .text_color(theme.text_muted)
                        .child(detail.clone()),
                )
                .child(
                    self.button("simulator-cancel", "Cancel setup", cx, |this, _, cx| {
                        this.request(
                            methods::CONFIGURE_SIMULATORS,
                            json!({"enabled":false,"agentAccess":false}),
                            cx,
                        )
                    }),
                );
        }
        if enabled
            && setup.status == SetupStatus::Ready
            && agents
            && matches!(
                setup.agent_status,
                AgentStatus::Installing | AgentStatus::Error
            )
        {
            card = card.child(self.button(
                "simulator-manual",
                "Continue without agent tools",
                cx,
                |this, _, cx| {
                    this.request(
                        methods::CONFIGURE_SIMULATORS,
                        json!({"enabled":true,"agentAccess":false}),
                        cx,
                    )
                },
            ));
        }
        if let Some(error) = &self.error {
            card = card.child(
                div()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(theme.danger)
                    .child(error.clone()),
            );
        }
        div()
            .id("simulator-installation")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .px(px(20.))
            .py(px(32.))
            .child(div().flex_1().min_h(px(10.)))
            .child(card)
            .child(div().flex_1().min_h(px(10.)))
            .into_any_element()
    }
    fn icon_button(
        &self,
        id: &'static str,
        label: &'static str,
        icon: &'static str,
        cx: &Context<Self>,
        click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let theme = Theme::of(cx);
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(label)
            .tab_index(0)
            .on_key_down(contain_activation)
            .size(px(26.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .cursor_pointer()
            .tip(label)
            .focus_visible(|s| s.border_1().border_color(theme.accent))
            .hover(|s| s.bg(theme.muted_fill()))
            .child(
                crate::icons::icon(icon)
                    .size(px(14.))
                    .text_color(theme.text_muted),
            )
            .when(!self.busy, |d| {
                d.on_click(cx.listener(move |this, _, window, cx| click(this, window, cx)))
            })
            .when(self.busy, |d| d.opacity(0.4))
    }
    fn button(
        &self,
        id: &'static str,
        label: &'static str,
        cx: &Context<Self>,
        click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let theme = Theme::of(cx);
        let primary = matches!(id, "simulator-enable" | "simulator-retry");
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(label)
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(5.0))
            .text_center()
            .text_color(if primary { theme.bg } else { theme.text })
            .bg(if primary {
                theme.text
            } else {
                theme.surface_raised
            })
            .cursor_pointer()
            .when(!self.busy, |d| {
                d.tab_index(0)
                    .on_key_down(contain_activation)
                    .focus_visible(|s| s.border_1().border_color(theme.accent))
                    .hover(|s| {
                        s.bg(if primary {
                            theme.text_strong
                        } else {
                            theme.surface_card
                        })
                    })
                    .on_click(cx.listener(move |this, _, window, cx| click(this, window, cx)))
            })
            .when(self.busy, |d| d.opacity(0.5))
            .child(label)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupStep {
    Done,
    Active,
    Pending,
    Failed,
}
fn setup_pending(state: &State) -> bool {
    state.setup.as_ref().is_some_and(|s| {
        s.status != SetupStatus::Ready || state.agent_access && s.agent_status != AgentStatus::Ready
    })
}
fn setup_steps(setup: &Setup, agents: bool) -> Vec<(String, SetupStep)> {
    let failed = matches!(setup.status, SetupStatus::Error | SetupStatus::Unavailable);
    let mut steps = vec![
        "Check host requirements".to_owned(),
        format!("Install Device Hub {}", setup.hub_version),
        "Start Device Hub".to_owned(),
        "Find available devices".to_owned(),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, label)| {
        (
            label,
            if index < setup.step {
                SetupStep::Done
            } else if index == setup.step && failed {
                SetupStep::Failed
            } else if index == setup.step {
                SetupStep::Active
            } else {
                SetupStep::Pending
            },
        )
    })
    .collect::<Vec<_>>();
    if agents {
        steps.push((
            format!("Install agent-device {}", setup.agent_version),
            match setup.agent_status {
                AgentStatus::Off => SetupStep::Pending,
                AgentStatus::Installing => SetupStep::Active,
                AgentStatus::Ready => SetupStep::Done,
                AgentStatus::Error => SetupStep::Failed,
            },
        ));
    }
    steps
}
fn contain_activation(event: &KeyDownEvent, _: &mut Window, cx: &mut App) {
    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
        cx.stop_propagation();
    }
}
fn selected_session(state: &State, previous: Option<&str>, picker: bool) -> Option<Device> {
    if !state.enabled || picker || setup_pending(state) {
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
            .font_family(crate::typography::effective_family_name(cx))
            .flex()
            .flex_col()
            .text_size(crate::typography::text_sm())
            .text_color(theme.text)
            .bg(theme.bg)
            .relative()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.menu_open {
                    this.menu_open = false;
                    cx.stop_propagation();
                    cx.notify();
                }
            }));
        let enabled = self.state.as_ref().is_some_and(|s| s.enabled);
        let agents = self.state.as_ref().is_some_and(|s| s.agent_access);
        if let Some(state) = &self.state {
            if let Some(setup) = &state.setup {
                if !state.enabled || setup_pending(state) {
                    return body
                        .child(self.render_setup(setup, state.enabled, state.agent_access, cx))
                        .into_any_element();
                }
            }
        }
        if self.selected.is_none() {
            body = body.child(
                div()
                    .h(px(36.))
                    .flex_none()
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().child(tr(cx, msg::simulators::devices)))
                    .child(
                        div().text_size(px(11.)).text_color(theme.text_muted).child(
                            self.state
                                .as_ref()
                                .map(|s| s.devices.len().to_string())
                                .unwrap_or_default(),
                        ),
                    )
                    .child(div().flex_1())
                    .child(self.icon_button(
                        "simulator-refresh",
                        "Refresh devices",
                        crate::icons::REFRESH,
                        cx,
                        |this, _, cx| {
                            this.error = None;
                            this.watch(cx);
                        },
                    ))
                    .child(self.icon_button(
                        "simulator-settings",
                        "Device options",
                        crate::icons::MORE_HORIZONTAL,
                        cx,
                        |this, _, cx| {
                            this.menu_open = !this.menu_open;
                            cx.notify();
                        },
                    )),
            );
        }
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
                    .child(tr(cx, msg::native::preparing_device)),
            );
        }
        let Some(state) = self.state.as_ref() else {
            return body
                .child(div().p(px(20.0)).child(tr(
                    cx,
                    msg::native::checking_simulator_tools_on_the_chat_s_host,
                )))
                .into_any_element();
        };
        if !enabled {
            let requirements = state.requirements.clone();
            body = body.child(
                div()
                    .id("simulator-setup")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(24.))
                    .py(px(40.))
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(
                        crate::icons::icon(crate::icons::SMARTPHONE)
                            .size(px(28.))
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .child(tr(cx, msg::native::your_app_right_beside_your_chat)),
                    )
                    .child(
                        div()
                            .text_color(theme.text_muted)
                            .line_height(px(20.))
                            .child(tr(cx, msg::native::run_an_ios_simulator_or_android_emulator_on_this_chat_s_host)),
                    )
                    .child(
                        self.button("simulator-enable", "Enable devices", cx, |this, _, cx| {
                            this.request(methods::CONFIGURE_SIMULATORS, json!({"enabled":true}), cx)
                        })
                        .self_start(),
                    )
                    .child(
                        div()
                            .mt(px(16.))
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .child(tr(cx, msg::native::requirements)),
                    )
                    .children(requirements.into_iter().map(|r| {
                        div()
                            .text_color(theme.text_muted)
                            .line_height(px(20.))
                            .child(r.message)
                    })),
            );
        } else if let Some(device) = self.selected.clone() {
            let mut controls = div()
                .h(px(36.))
                .flex_none()
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(2.))
                .child(self.icon_button(
                    "simulator-devices",
                    "All devices",
                    crate::icons::ARROW_LEFT,
                    cx,
                    |this, _, cx| {
                        this.picker = true;
                        this.selected = None;
                        this.frames = None;
                        this.clear_image(cx);
                        cx.notify();
                    },
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(div().truncate().child(device.name.clone()))
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(theme.text_muted)
                                .child(device.runtime.clone()),
                        ),
                );
            if device.platform == Platform::Android {
                controls = controls.child(self.icon_button(
                    "simulator-back",
                    "Back",
                    crate::icons::RESTORE,
                    cx,
                    |this, _, cx| this.send_input(Input::Back, cx),
                ));
            }
            controls = controls.child(self.icon_button(
                "simulator-home",
                "Home",
                crate::icons::DEVICE_HOME,
                cx,
                |this, _, cx| this.send_input(Input::Home, cx),
            ));
            if device.platform == Platform::Android {
                controls = controls.child(self.icon_button(
                    "simulator-recents",
                    "Recent apps",
                    crate::icons::DEVICE_RECENTS,
                    cx,
                    |this, _, cx| this.send_input(Input::Recents, cx),
                ));
            }
            controls = controls
                .child(self.icon_button(
                    "simulator-screenshot",
                    "Copy screenshot",
                    crate::icons::DEVICE_CAMERA,
                    cx,
                    |this, _, cx| this.request(methods::SIMULATOR_SCREENSHOT, json!({}), cx),
                ))
                .child(self.icon_button(
                    "simulator-options",
                    "Device options",
                    crate::icons::MORE_HORIZONTAL,
                    cx,
                    |this, _, cx| {
                        this.menu_open = !this.menu_open;
                        cx.notify();
                    },
                ));
            body = body.child(controls);
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
                let bounds = *self.bounds.borrow();
                let available_w = f32::from(bounds.size.width).max(1.);
                let available_h = f32::from(bounds.size.height).max(1.);
                let scale = (available_w / self.dimensions.0.max(1) as f32)
                    .min(available_h / self.dimensions.1.max(1) as f32);
                let width = self.dimensions.0 as f32 * scale;
                let height = self.dimensions.1 as f32 * scale;
                viewport = viewport.child(
                    div()
                        .absolute()
                        .left(px((available_w - width) / 2.))
                        .top(px((available_h - height) / 2.))
                        .w(px(width))
                        .h(px(height))
                        .rounded(px(width.min(height) * 0.12))
                        .overflow_hidden()
                        .border_1()
                        .border_color(theme.border)
                        .shadow_lg()
                        .child(
                            img(image.clone())
                                .size_full()
                                .rounded(px(width.min(height) * 0.12))
                                .object_fit(ObjectFit::Contain),
                        ),
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
                    move |bounds, window, _| {
                        if *measure.borrow() != bounds {
                            window.refresh();
                        }
                        *measure.borrow_mut() = bounds;
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
            body = body
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .px(px(20.))
                        .pt(px(8.))
                        .pb(px(16.))
                        .flex()
                        .child(viewport),
                )
                .child(
                    div()
                        .px(px(16.0))
                        .pb(px(12.0))
                        .flex()
                        .gap(px(6.0))
                        .items_center()
                        .child(
                            div()
                                .id("simulator-text")
                                .flex_1()
                                .min_w_0()
                                .h(px(28.0))
                                .px(px(8.0))
                                .flex()
                                .items_center()
                                .rounded(px(5.0))
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.surface)
                                .cursor_text()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        this.input.update(cx, |input, cx| input.focus(window, cx));
                                    }),
                                )
                                .child(TextInput::new(&self.input)),
                        )
                        .child(self.icon_button(
                            "simulator-type",
                            "Send text",
                            crate::icons::RETURN,
                            cx,
                            |this, _, cx| this.type_text(cx),
                        )),
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
                .gap(px(2.0));
            for platform in [Platform::Ios, Platform::Android] {
                let devices: Vec<_> = state
                    .devices
                    .iter()
                    .filter(|d| d.platform == platform)
                    .cloned()
                    .collect();
                let mut runtimes: Vec<_> = devices.iter().map(|d| d.runtime.clone()).collect();
                runtimes.sort();
                runtimes.dedup();
                for runtime in runtimes {
                    picker = picker.child(
                        div()
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .child(runtime.clone()),
                    );
                    for device in devices.iter().filter(|d| d.runtime == runtime) {
                        let id = device.id.clone();
                        picker = picker.child(
                            div()
                                .id(SharedString::from(format!("device-{id}")))
                                .role(Role::Button)
                                .tab_index(0)
                                .on_key_down(contain_activation)
                                .focus_visible(|s| s.border_1().border_color(theme.accent))
                                .aria_label(format!("Open {}", device.name))
                                .h(px(34.))
                                .px(px(8.))
                                .rounded(px(6.))
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.muted_fill()))
                                .child(
                                    crate::icons::icon(crate::icons::SMARTPHONE)
                                        .size(px(14.))
                                        .text_color(theme.text_muted),
                                )
                                .child(div().flex_1().child(device.name.clone()))
                                .child(div().text_size(px(11.)).text_color(theme.text_muted).child(
                                    if device.booted {
                                        "● Running"
                                    } else {
                                        "Start →"
                                    },
                                ))
                                .when(!self.busy, |row| {
                                    row.on_click(cx.listener(move |this, _, _, cx| {
                                        this.request(
                                            methods::OPEN_SIMULATOR,
                                            json!({"deviceId":id}),
                                            cx,
                                        )
                                    }))
                                }),
                        );
                    }
                }
                for r in state
                    .requirements
                    .iter()
                    .filter(|r| r.platform == platform && !r.ready)
                {
                    picker = picker.child(
                        div()
                            .py(px(8.))
                            .text_color(theme.text_muted)
                            .line_height(px(20.))
                            .child(r.message.clone()),
                    );
                }
            }
            if state.devices.is_empty() && state.requirements.iter().all(|r| r.ready) {
                picker = picker.child(div().py(px(24.)).text_color(theme.text_muted).child(tr(cx, msg::native::no_devices_found_create_a_simulator_in_xcode_or_an_avd_in_android_studio_then_refresh)));
            }
            body = body.child(picker);
        }
        if enabled && self.selected.is_none() {
            body = body.child(
                div()
                    .h(px(42.))
                    .flex_none()
                    .px(px(16.))
                    .border_t_1()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .child(if agents {
                                "Agent control enabled"
                            } else {
                                "Agents can’t control devices"
                            }),
                    )
                    .child(self.button(
                        "simulator-agents",
                        if agents { "Revoke" } else { "Allow" },
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
        }
        if self.menu_open {
            let mut menu = crate::popover::popover_card(&theme.for_popup())
                .id("simulator-menu")
                .occlude()
                .absolute()
                .top(px(34.))
                .right(px(8.))
                .w(px(210.))
                .flex()
                .flex_col()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.menu_open = false;
                    cx.stop_propagation();
                    cx.notify();
                }))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        this.menu_open = false;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }));
            let mut actions = vec![(
                "simulator-menu-agents",
                if agents {
                    "Revoke agent control"
                } else {
                    "Allow agent control"
                },
                methods::CONFIGURE_SIMULATORS,
                json!({"enabled":true,"agentAccess":!agents}),
            )];
            if self.selected.is_some() {
                actions.extend([
                    (
                        "simulator-reconnect",
                        "Reconnect",
                        methods::OPEN_SIMULATOR,
                        json!({}),
                    ),
                    (
                        "simulator-close",
                        "Disconnect",
                        methods::CLOSE_SIMULATOR,
                        json!({"shutdown":false}),
                    ),
                    (
                        "simulator-shutdown",
                        "Shut down device",
                        methods::CLOSE_SIMULATOR,
                        json!({"shutdown":true}),
                    ),
                ]);
            }
            actions.push((
                "simulator-enabled",
                if enabled {
                    "Disable devices"
                } else {
                    "Enable devices"
                },
                methods::CONFIGURE_SIMULATORS,
                json!({"enabled":!enabled,"agentAccess":false}),
            ));
            for (id, label, method, params) in actions {
                menu = menu.child(
                    crate::popover::menu_row(&theme, false, id)
                        .id(id)
                        .role(Role::Button)
                        .aria_label(label)
                        .tab_index(0)
                        .on_key_down(contain_activation)
                        .focus_visible(|s| s.border_1().border_color(theme.accent))
                        .when(id == "simulator-shutdown", |row| {
                            row.text_color(theme.danger)
                        })
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.menu_open = false;
                            if this.busy {
                                cx.notify();
                                return;
                            }
                            if method == methods::OPEN_SIMULATOR {
                                this.frames = None;
                                this.clear_image(cx);
                            }
                            this.request(method, params.clone(), cx);
                        })),
                );
            }
            body = body.child(
                gpui::deferred(crate::frost::floating_glass(
                    crate::popover::CARD_RADIUS,
                    crate::frost::MENU_BLUR,
                    menu,
                ))
                .with_priority(1),
            );
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

#[cfg(test)]
mod installation_tests {
    use super::*;
    #[test]
    fn setup_steps_follow_host_progress_and_keep_agent_install_separate() {
        for (status, step) in [
            (SetupStatus::Checking, 0),
            (SetupStatus::Installing, 1),
            (SetupStatus::Starting, 2),
            (SetupStatus::Discovering, 3),
        ] {
            let state = Setup {
                status,
                step,
                ..Default::default()
            };
            let steps = setup_steps(&state, true);
            assert_eq!(steps.len(), 5);
            assert_eq!(steps[step].1, SetupStep::Active);
            assert!(steps[..step].iter().all(|s| s.1 == SetupStep::Done));
            assert_eq!(steps[4].1, SetupStep::Pending);
        }
        let state = Setup {
            status: SetupStatus::Error,
            step: 1,
            ..Default::default()
        };
        assert_eq!(setup_steps(&state, false)[1].1, SetupStep::Failed);
        let state = Setup {
            status: SetupStatus::Ready,
            step: 4,
            agent_status: AgentStatus::Installing,
            ..Default::default()
        };
        assert!(
            setup_steps(&state, true)[..4]
                .iter()
                .all(|s| s.1 == SetupStep::Done)
        );
        assert_eq!(setup_steps(&state, true)[4].1, SetupStep::Active);
        assert!(setup_pending(&State {
            enabled: true,
            agent_access: true,
            setup: Some(state),
            ..Default::default()
        }));
    }
}

#[cfg(test)]
mod setup_render_tests {
    use super::*;
    #[gpui::test]
    fn simulator_setup_and_retry_surfaces_build_for_all_host_phases(cx: &mut gpui::TestAppContext) {
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
        for status in [
            SetupStatus::Disabled,
            SetupStatus::Checking,
            SetupStatus::Installing,
            SetupStatus::Starting,
            SetupStatus::Discovering,
            SetupStatus::Ready,
            SetupStatus::Error,
            SetupStatus::Unavailable,
        ] {
            for agent_status in [
                AgentStatus::Off,
                AgentStatus::Installing,
                AgentStatus::Ready,
                AgentStatus::Error,
            ] {
                window
                    .update(cx, |pane, window, cx| {
                        pane.state = Some(State {
                            enabled: status != SetupStatus::Disabled,
                            agent_access: true,
                            setup: Some(Setup {
                                status,
                                step: if status == SetupStatus::Ready { 4 } else { 1 },
                                agent_status,
                                requirements: vec![zuse_protocol::simulators::SetupRequirement {
                                    label: "Node.js".into(),
                                    ready: status != SetupStatus::Unavailable,
                                    detail: "fixture".into(),
                                }],
                                ..Default::default()
                            }),
                            ..Default::default()
                        });
                        // Building a GPUI tree also validates interactivity composition (including hover styles).
                        let _ = pane.render(window, cx).into_any_element();
                    })
                    .unwrap();
            }
        }
    }
}
