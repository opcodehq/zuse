//! First-run onboarding: agents, account, MCP servers, chats from the previous
//! version of Zuse and other agent apps, then a few defaults. Every step drives
//! an existing entity (sign-in and MCP import through the shared plugins card,
//! the previous-version import through the desktop import job) so Settings and
//! onboarding never disagree.
use crate::localization::message as tr;
use crate::{
    desktop_import::{self, PreviousImport},
    icons, popover,
    settings::{self, plugins::PluginsCard, widgets},
    state::{AppState, EngineHandle},
    theme::Theme,
};
use gpui::{prelude::*, *};
use serde_json::json;
use std::collections::BTreeSet;
use zuse_engine::registry::{HarnessDescriptor, descriptor_enabled};
use zuse_i18n::{LanguagePreference, Locale, messages as msg};
use zuse_protocol::{HarnessId, external_chat as external, methods, presentation::PermissionMode};

pub struct Finished {
    pub add_project: bool,
    pub open_chat: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Welcome,
    Agents,
    SignIn,
    Plugins,
    Chats,
    Personalize,
    Launch,
}

const COLUMN: f32 = 520.0;

/// A row in the Chats step: the previous version of Zuse, or one agent app.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Source {
    Previous,
    App(HarnessId),
}

pub struct Wizard {
    state: Entity<AppState>,
    desktop: Entity<desktop_import::Onboarding>,
    plugins: Entity<PluginsCard>,
    decided: bool,
    visible: bool,
    step: Step,
    /// Frozen when the Agents step is left, so signing in later doesn't
    /// renumber the steps already shown.
    ask_sign_in: Option<bool>,
    skipped_sign_in: bool,
    harnesses: Option<Result<Vec<HarnessDescriptor>, String>>,
    harness_task: Option<Task<()>>,
    harness_error: Option<String>,
    rows: Option<Result<Vec<external::Row>, String>>,
    picked: BTreeSet<String>,
    /// External rows already copied in, so their app row reads as done.
    done: BTreeSet<String>,
    take_previous: bool,
    /// Other-app chats wait for the previous-version import, which needs
    /// the workspace to itself.
    chats_after_previous: bool,
    expanded: Option<Source>,
    importing: Option<(usize, usize)>,
    imported: Vec<external::Imported>,
    failed: usize,
    chats_task: Option<Task<()>>,
    language_open: bool,
    focus: FocusHandle,
    focus_pending: bool,
}

impl EventEmitter<Finished> for Wizard {}

impl Wizard {
    pub fn new(
        state: Entity<AppState>,
        desktop: Entity<desktop_import::Onboarding>,
        cx: &mut Context<Self>,
    ) -> Self {
        let plugins = PluginsCard::shared(state.clone(), cx);
        let embedded = !settings::current(cx).onboarding_completed;
        desktop.update(cx, |desktop, cx| desktop.set_embedded(embedded, cx));
        cx.observe(&desktop, |this, _, cx| {
            this.decide(cx);
            this.previous_changed(cx);
        })
        .detach();
        cx.observe(&state, |this, _, cx| this.decide(cx)).detach();
        cx.observe(&plugins, |this, _, cx| this.account_changed(cx))
            .detach();
        let mut this = Self {
            state,
            desktop,
            plugins,
            decided: !embedded,
            visible: false,
            step: Step::Welcome,
            ask_sign_in: None,
            skipped_sign_in: false,
            harnesses: None,
            harness_task: None,
            harness_error: None,
            rows: None,
            picked: BTreeSet::new(),
            done: BTreeSet::new(),
            take_previous: true,
            chats_after_previous: false,
            expanded: None,
            importing: None,
            imported: Vec::new(),
            failed: 0,
            chats_task: None,
            language_open: false,
            focus: cx.focus_handle(),
            focus_pending: false,
        };
        this.decide(cx);
        this
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Show onboarding only on a fresh workspace. Anyone who already has chats
    /// (or an interrupted import to resume) is marked done without seeing it.
    fn decide(&mut self, cx: &mut Context<Self>) {
        if self.decided {
            return;
        }
        let Some(discovery) = self.desktop.read(cx).discovery() else {
            return;
        };
        self.decided = true;
        if discovery.has_chats || discovery.pending || !self.state.read(cx).spaces.is_empty() {
            self.finish_silently(cx);
            return;
        }
        self.visible = true;
        self.focus_pending = true;
        self.load_harnesses(cx);
        cx.notify();
    }

    fn finish_silently(&mut self, cx: &mut Context<Self>) {
        settings::update(settings::SavePolicy::Immediate, cx, |s| {
            s.onboarding_completed = true
        });
        self.desktop
            .update(cx, |desktop, cx| desktop.set_embedded(false, cx));
    }

    fn account_changed(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        let (signed_in, ready) = {
            let plugins = self.plugins.read(cx);
            (plugins.is_signed_in(), plugins.workspace_ready())
        };
        if self.step == Step::SignIn && signed_in {
            self.next(cx);
            return;
        }
        if self.step == Step::Plugins && signed_in && ready {
            self.plugins.update(cx, |p, cx| p.begin_import(cx));
        }
        cx.notify();
    }

    fn previous_changed(&mut self, cx: &mut Context<Self>) {
        if self.chats_after_previous
            && matches!(
                self.desktop.read(cx).progress(),
                PreviousImport::Done { .. } | PreviousImport::Failed(_)
            )
        {
            self.chats_after_previous = false;
            self.import_chats(cx);
        }
    }

    /// Chats the previous version would bring over if imported now.
    fn previous_pending(&self, cx: &App) -> Option<u64> {
        let desktop = self.desktop.read(cx);
        if !self.take_previous
            || !matches!(
                desktop.progress(),
                PreviousImport::Idle | PreviousImport::Failed(_)
            )
        {
            return None;
        }
        desktop.discovery()?.previous.map(|p| p.chats)
    }

    fn start_imports(&mut self, cx: &mut Context<Self>) {
        // The workspace watch can publish copied chats before each reply; keep
        // startup restoration from navigating while onboarding is open.
        self.state.update(cx, |state, _| state.auto_selected = true);
        if self.previous_pending(cx).is_some() {
            self.desktop.update(cx, |d, cx| d.import_previous(cx));
            // A refused start leaves the job idle; don't wait on it forever.
            self.chats_after_previous = !self.picked.is_empty()
                && matches!(self.desktop.read(cx).progress(), PreviousImport::Running(_));
            if !self.chats_after_previous {
                self.import_chats(cx);
            }
        } else {
            self.import_chats(cx);
        }
        cx.notify();
    }

    fn engine(&self, cx: &App) -> Option<EngineHandle> {
        self.state.read(cx).engine().map(EngineHandle::controller)
    }

    fn steps(&self, cx: &App) -> Vec<Step> {
        sequence(
            self.ask_sign_in
                .unwrap_or_else(|| !self.plugins.read(cx).is_signed_in()),
            self.skipped_sign_in,
        )
    }

    fn go(&mut self, step: Step, cx: &mut Context<Self>) {
        if self.step == Step::Agents && step != Step::Welcome {
            self.ask_sign_in
                .get_or_insert_with(|| !self.plugins.read(cx).is_signed_in());
        }
        self.step = step;
        self.language_open = false;
        self.focus_pending = true;
        match step {
            Step::Plugins => {
                let ready = {
                    let p = self.plugins.read(cx);
                    p.is_signed_in() && p.workspace_ready()
                };
                if ready {
                    self.plugins.update(cx, |p, cx| p.begin_import(cx));
                }
            }
            Step::Chats if self.rows.is_none() => self.load_chats(cx),
            _ => {}
        }
        cx.notify();
    }

    fn next(&mut self, cx: &mut Context<Self>) {
        let steps = self.steps(cx);
        let index = steps.iter().position(|s| *s == self.step).unwrap_or(0);
        match steps.get(index + 1) {
            Some(step) => self.go(*step, cx),
            None => self.complete(false, None, cx),
        }
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        let steps = self.steps(cx);
        if let Some(index) = steps
            .iter()
            .position(|s| *s == self.step)
            .filter(|i| *i > 0)
        {
            self.go(steps[index - 1], cx);
        }
    }

    fn complete(&mut self, add_project: bool, open_chat: Option<String>, cx: &mut Context<Self>) {
        if self.busy(cx) {
            return;
        }
        self.visible = false;
        self.finish_silently(cx);
        cx.emit(Finished {
            add_project,
            open_chat,
        });
        cx.notify();
    }

    fn busy(&self, cx: &App) -> bool {
        self.importing.is_some()
            || self.chats_after_previous
            || matches!(self.desktop.read(cx).progress(), PreviousImport::Running(_))
    }

    fn load_harnesses(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            engine
                .client()
                .call_scoped(methods::LIST_HARNESSES, json!({}))
                .await
                .map_err(|e| e.to_string())
        });
        self.harness_task = Some(cx.spawn(async move |this, cx| {
            let result = job.await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.harnesses = Some(result.and_then(|v| {
                    serde_json::from_value::<Vec<HarnessDescriptor>>(v).map_err(|e| e.to_string())
                }));
                cx.notify();
            });
        }));
    }

    fn set_harness(&mut self, harness: HarnessId, enabled: bool, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        if let Some(Ok(list)) = self.harnesses.as_mut() {
            if let Some(row) = list.iter_mut().find(|d| d.id == harness) {
                row.enabled = Some(enabled);
            }
        }
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            engine
                .client()
                .call_scoped(
                    methods::SET_HARNESS_ENABLED,
                    json!({"harness":harness,"enabled":enabled}),
                )
                .await
                .map_err(|e| e.to_string())
        });
        self.harness_task = Some(cx.spawn(async move |this, cx| {
            let result = job.await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |this, cx| {
                match result.and_then(|v| {
                    serde_json::from_value::<Vec<HarnessDescriptor>>(v).map_err(|e| e.to_string())
                }) {
                    Ok(list) => {
                        this.harness_error = None;
                        this.harnesses = Some(Ok(list));
                    }
                    // The switch moved optimistically; reload the engine's
                    // real state instead of leaving it wrong.
                    Err(error) => {
                        this.harness_error = Some(error);
                        this.load_harnesses(cx);
                    }
                }
                crate::pickers::bump_harness_catalog(cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn load_chats(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            self.rows = Some(Err("Connect to this computer to import its chats.".into()));
            return;
        };
        self.rows = None;
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            engine
                .client()
                .call_scoped(external::LIST, json!({"targetDeviceId":null}))
                .await
                .map_err(|e| e.to_string())
        });
        self.chats_task = Some(cx.spawn(async move |this, cx| {
            let result = job.await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.rows = Some(result.and_then(|v| {
                    serde_json::from_value::<Vec<external::Row>>(v)
                        .map(|rows| rows.into_iter().filter(external::Row::importable).collect())
                        .map_err(|_| "Couldn't read chats from other agent apps.".into())
                }));
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn import_chats(&mut self, cx: &mut Context<Self>) {
        if self.importing.is_some() || self.picked.is_empty() {
            return;
        }
        let Some(engine) = self.engine(cx) else {
            return;
        };
        let ids: Vec<String> = self.picked.iter().cloned().collect();
        let total = ids.len();
        self.importing = Some((0, total));
        self.failed = 0;
        self.state.update(cx, |state, _| state.auto_selected = true);
        self.chats_task = Some(cx.spawn(async move |this, cx| {
            for (done, id) in ids.into_iter().enumerate() {
                let request = external::ImportRequest {
                    id: id.clone(),
                    request_id: uuid::Uuid::new_v4().to_string(),
                };
                let mut params = serde_json::to_value(request).unwrap_or_default();
                params["targetDeviceId"] = json!(null);
                let engine = engine.clone();
                let result = gpui_tokio::Tokio::spawn(cx, async move {
                    engine
                        .client()
                        .call_scoped(external::IMPORT, params)
                        .await
                        .map_err(|e| e.to_string())
                });
                let result = result.await.unwrap_or_else(|e| Err(e.to_string()));
                let alive = this.update(cx, |this, cx| {
                    match result
                        .ok()
                        .and_then(|v| serde_json::from_value::<external::Imported>(v).ok())
                    {
                        Some(imported) => {
                            this.picked.remove(&id);
                            this.done.insert(id.clone());
                            this.imported.push(imported);
                        }
                        None => this.failed += 1,
                    }
                    this.importing = Some((done + 1, total));
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.importing = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The footer's main action for the current step: label, enabled, action.
    fn primary(&self, cx: &App) -> (String, bool) {
        let label = |f: fn(&zuse_i18n::Localizer) -> String| tr(cx, f);
        match self.step {
            Step::Welcome => (label(msg::onboarding::flow_get_started), true),
            Step::SignIn => {
                let p = self.plugins.read(cx);
                if p.is_signed_in() {
                    (label(msg::onboarding::flow_continue), true)
                } else {
                    (label(msg::onboarding::flow_signin_action), !p.signing_in())
                }
            }
            Step::Plugins => match self.plugins.read(cx).import_summary() {
                Some(s) if s.submitted.is_none() && !s.loading && !s.selected.is_empty() => (
                    tr(cx, |l| {
                        msg::onboarding::flow_mcp_import(l, s.selected.len())
                    }),
                    true,
                ),
                Some(s) if s.submitted.is_some() && s.pending > 0 => {
                    (label(msg::onboarding::flow_mcp_adding), false)
                }
                _ => (label(msg::onboarding::flow_continue), true),
            },
            Step::Chats => {
                let count = self.previous_pending(cx).unwrap_or(0) as usize + self.picked.len();
                if let Some((done, total)) = self.importing {
                    (
                        tr(cx, |l| {
                            msg::onboarding::flow_chats_importing(l, done, total)
                        }),
                        false,
                    )
                } else if self.busy(cx) {
                    (label(msg::onboarding::flow_source_importing), false)
                } else if count > 0 {
                    (
                        tr(cx, |l| msg::onboarding::flow_chats_import(l, count)),
                        true,
                    )
                } else {
                    (label(msg::onboarding::flow_continue), true)
                }
            }
            Step::Launch => (
                if self.state.read(cx).spaces.is_empty() && self.imported.is_empty() {
                    label(msg::onboarding::flow_launch_project)
                } else {
                    label(msg::onboarding::flow_launch_open)
                },
                true,
            ),
            Step::Agents | Step::Personalize => (label(msg::onboarding::flow_continue), true),
        }
    }

    fn run_primary(&mut self, cx: &mut Context<Self>) {
        if !self.primary(cx).1 {
            return;
        }
        match self.step {
            Step::SignIn if !self.plugins.read(cx).is_signed_in() => {
                self.plugins.update(cx, |p, cx| p.begin_sign_in(cx));
            }
            Step::Plugins => {
                let run =
                    self.plugins.read(cx).import_summary().is_some_and(|s| {
                        s.submitted.is_none() && !s.loading && !s.selected.is_empty()
                    });
                if run {
                    self.plugins.update(cx, |p, cx| p.import_selected(cx));
                } else {
                    self.next(cx);
                }
            }
            Step::Chats if self.previous_pending(cx).is_some() || !self.picked.is_empty() => {
                self.start_imports(cx)
            }
            Step::Launch => {
                let add = self.state.read(cx).spaces.is_empty() && self.imported.is_empty();
                self.complete(add, None, cx);
            }
            _ => self.next(cx),
        }
    }

    fn skip(&mut self, cx: &mut Context<Self>) {
        match self.step {
            Step::Welcome => self.complete(false, None, cx),
            Step::SignIn => {
                self.skipped_sign_in = true;
                self.next(cx);
            }
            _ => self.next(cx),
        }
    }
}

// Rendering
impl Wizard {
    fn heading(&self, theme: &Theme, title: String, body: String) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(24.))
                    .line_height(px(30.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_strong)
                    .child(title),
            )
            .child(
                div()
                    .text_size(crate::typography::text_md())
                    .line_height(px(21.))
                    .text_color(theme.text_muted)
                    .child(body),
            )
    }

    fn point(theme: &Theme, icon_path: &'static str, text: String) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .size(px(28.))
                    .flex_none()
                    .rounded(px(8.))
                    .bg(theme.text.opacity(0.05))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        icons::icon(icon_path)
                            .size(px(15.))
                            .text_color(theme.text_muted),
                    ),
            )
            .child(
                div()
                    .text_size(crate::typography::text_md())
                    .text_color(theme.text)
                    .child(text),
            )
    }

    /// A list frame that scrolls past `max` and fades its last rows so the
    /// overflow reads as scrollable.
    fn list(
        theme: &Theme,
        id: &'static str,
        max: f32,
        overflow: bool,
        rows: Vec<AnyElement>,
    ) -> AnyElement {
        div()
            .relative()
            .w_full()
            .rounded(px(12.))
            .border_1()
            .border_color(theme.text.opacity(0.07))
            .bg(theme.text.opacity(0.03))
            .shadow_sm()
            .overflow_hidden()
            .child(
                div()
                    .id(id)
                    .max_h(px(max))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .when(overflow, |el| {
                el.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(28.))
                        .bg(linear_gradient(
                            180.,
                            linear_color_stop(theme.bg.opacity(0.), 0.),
                            linear_color_stop(theme.bg.opacity(0.85), 1.),
                        )),
                )
            })
            .into_any_element()
    }

    fn row_text(theme: &Theme, title: String, meta: String) -> Div {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .truncate()
                    .text_size(crate::typography::text_sm())
                    .text_color(theme.text)
                    .child(title),
            )
            .when(!meta.is_empty(), |el| {
                el.child(
                    div()
                        .truncate()
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .child(meta),
                )
            })
    }

    fn note(theme: &Theme, text: String) -> Div {
        div()
            .text_size(crate::typography::text_sm())
            .text_color(theme.text_muted)
            .child(text)
    }

    fn render_welcome(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let returning = self
            .desktop
            .read(cx)
            .discovery()
            .is_some_and(|d| d.previous.is_some());
        div()
            .flex()
            .flex_col()
            .gap(px(28.))
            .child(motion_logo(theme))
            .child(self.heading(
                theme,
                tr(
                    cx,
                    if returning {
                        msg::onboarding::flow_welcome_back_title
                    } else {
                        msg::onboarding::flow_welcome_title
                    },
                ),
                tr(
                    cx,
                    if returning {
                        msg::onboarding::flow_welcome_back_body
                    } else {
                        msg::onboarding::flow_welcome_body
                    },
                ),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(Self::point(
                        theme,
                        icons::BOT,
                        tr(cx, msg::onboarding::flow_welcome_point_agents),
                    ))
                    .child(Self::point(
                        theme,
                        icons::WORKTREE,
                        tr(cx, msg::onboarding::flow_welcome_point_parallel),
                    ))
                    .child(Self::point(
                        theme,
                        icons::SMARTPHONE,
                        tr(cx, msg::onboarding::flow_welcome_point_phone),
                    )),
            )
            .into_any_element()
    }

    fn render_agents(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().flex().flex_col().gap(px(16.)).child(self.heading(
            theme,
            tr(cx, msg::onboarding::flow_agents_title),
            tr(cx, msg::onboarding::flow_agents_body),
        ));
        match &self.harnesses {
            None => {
                body = body.child(Self::note(
                    theme,
                    tr(cx, msg::onboarding::flow_agents_loading),
                ))
            }
            Some(Err(error)) => body = body.child(Self::note(theme, error.clone())),
            Some(Ok(list)) => {
                let visible = crate::pickers::visible_harnesses(list);
                let installed: Vec<_> = visible.iter().filter(|d| d.installed).cloned().collect();
                let missing = visible.len() - installed.len();
                if installed.is_empty() {
                    body = body.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_color(theme.text)
                                    .child(tr(cx, msg::onboarding::flow_agents_none)),
                            )
                            .child(Self::note(
                                theme,
                                tr(cx, msg::onboarding::flow_agents_none_body),
                            )),
                    );
                } else {
                    let ready = installed.iter().filter(|d| descriptor_enabled(d)).count();
                    body = body.child(Self::note(
                        theme,
                        tr(cx, |l| msg::onboarding::flow_agents_ready(l, ready)),
                    ));
                    let overflow = installed.len() > 6;
                    let rows =
                        installed
                            .iter()
                            .enumerate()
                            .map(|(index, d)| {
                                let on = descriptor_enabled(d);
                                let id = d.id;
                                let (icon_path, tint) = crate::pickers::harness_brand_icon(d.id);
                                div()
                                    .id(("onboarding-agent", index))
                                    .flex()
                                    .items_center()
                                    .gap(px(12.))
                                    .px(px(14.))
                                    .py(px(10.))
                                    .when(index > 0, |el| {
                                        el.border_t_1().border_color(theme.text.opacity(0.05))
                                    })
                                    .cursor_pointer()
                                    .role(Role::Switch)
                                    .aria_label(SharedString::from(d.name.clone()))
                                    .aria_toggled(if on { Toggled::True } else { Toggled::False })
                                    .tab_index(0)
                                    .child(
                                        icons::icon(icon_path)
                                            .size(px(18.))
                                            .text_color(tint.unwrap_or(theme.text)),
                                    )
                                    .child(Self::row_text(theme, d.name.clone(), String::new()))
                                    .child(widgets::toggle_switch(
                                        theme,
                                        on,
                                        format!("onboarding-agent-{index}"),
                                    ))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_harness(id, !on, cx)
                                    }))
                                    .into_any_element()
                            })
                            .collect();
                    body = body.child(Self::list(theme, "onboarding-agents", 300., overflow, rows));
                }
                if missing > 0 {
                    body = body.child(Self::note(
                        theme,
                        tr(cx, |l| msg::onboarding::flow_agents_more(l, missing)),
                    ));
                }
            }
        }
        if let Some(error) = &self.harness_error {
            body = body.child(
                div()
                    .text_size(crate::typography::text_sm())
                    .text_color(theme.danger)
                    .child(error.clone()),
            );
        }
        body.into_any_element()
    }

    fn render_sign_in(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let plugins = self.plugins.read(cx);
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(24.))
            .child(self.heading(
                theme,
                tr(cx, msg::onboarding::flow_signin_title),
                tr(cx, msg::onboarding::flow_signin_body),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(Self::point(
                        theme,
                        icons::DITHER_PLUG,
                        tr(cx, msg::onboarding::flow_signin_point_mcp),
                    ))
                    .child(Self::point(
                        theme,
                        icons::CLOUD,
                        tr(cx, msg::onboarding::flow_signin_point_cloud),
                    ))
                    .child(Self::point(
                        theme,
                        icons::SMARTPHONE,
                        tr(cx, msg::onboarding::flow_signin_point_phone),
                    )),
            );
        if plugins.is_signed_in() {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_color(theme.text)
                    .child(
                        icons::icon(icons::CHECK_CIRCLE)
                            .size(px(16.))
                            .text_color(theme.success),
                    )
                    .child(tr(cx, msg::onboarding::flow_signin_done)),
            );
        } else if plugins.signing_in() {
            body = body.child(Self::note(
                theme,
                tr(cx, msg::onboarding::flow_signin_waiting),
            ));
        }
        if let Some(error) = plugins.account_error() {
            body = body.child(
                div()
                    .text_size(crate::typography::text_sm())
                    .text_color(theme.danger)
                    .child(error),
            );
        }
        body.into_any_element()
    }

    fn render_plugins(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().flex().flex_col().gap(px(16.)).child(self.heading(
            theme,
            tr(cx, msg::onboarding::flow_mcp_title),
            tr(cx, msg::onboarding::flow_mcp_body),
        ));
        let summary = self.plugins.read(cx).import_summary();
        let Some(summary) = summary else {
            return body
                .child(Self::note(theme, tr(cx, msg::onboarding::flow_mcp_waiting)))
                .into_any_element();
        };
        if summary.loading {
            body = body.child(Self::note(theme, tr(cx, msg::onboarding::flow_mcp_loading)));
        } else if let Some(error) = &summary.error {
            body = body.child(Self::note(theme, error.clone()));
        } else if summary.servers.is_empty() {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_color(theme.text)
                            .child(tr(cx, msg::onboarding::flow_mcp_none)),
                    )
                    .child(Self::note(
                        theme,
                        tr(cx, msg::onboarding::flow_mcp_none_body),
                    )),
            );
        } else {
            let locked = summary.submitted.is_some();
            let overflow = summary.servers.len() > 6;
            let rows = summary
                .servers
                .iter()
                .enumerate()
                .map(|(index, (name, source, target, env))| {
                    let mut meta = vec![source.clone(), home_relative(target)];
                    if *env > 0 {
                        meta.push(tr(cx, |l| msg::onboarding::flow_mcp_env(l, *env)));
                    }
                    let on = summary.selected.contains(&index);
                    div()
                        .id(("onboarding-mcp", index))
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .px(px(14.))
                        .py(px(10.))
                        .when(index > 0, |el| {
                            el.border_t_1().border_color(theme.text.opacity(0.05))
                        })
                        .cursor_pointer()
                        .role(Role::Switch)
                        .aria_label(SharedString::from(name.clone()))
                        .aria_toggled(if on { Toggled::True } else { Toggled::False })
                        .tab_index(0)
                        .when(locked, |el| el.cursor_default().opacity(0.7))
                        .child(Self::row_text(theme, name.clone(), meta.join(" · ")))
                        .child(widgets::toggle_switch(
                            theme,
                            on,
                            format!("onboarding-mcp-{index}"),
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.plugins
                                .update(cx, |p, cx| p.toggle_imported_server(index, cx))
                        }))
                        .into_any_element()
                })
                .collect();
            body = body.child(Self::list(
                theme,
                "onboarding-mcp-list",
                312.,
                overflow,
                rows,
            ));
            if let Some((_, skipped)) = summary.submitted {
                let added = summary.added();
                body = body.child(Self::note(
                    theme,
                    if summary.pending > 0 {
                        tr(cx, msg::onboarding::flow_mcp_adding)
                    } else {
                        tr(cx, |l| msg::onboarding::flow_mcp_added(l, added))
                    },
                ));
                let failed = skipped + summary.failed;
                if summary.pending == 0 && failed > 0 {
                    body = body.child(Self::note(
                        theme,
                        tr(cx, |l| msg::onboarding::flow_mcp_skipped(l, failed)),
                    ));
                }
                if let Some(error) = self.plugins.read(cx).account_error() {
                    body = body.child(
                        div()
                            .text_size(crate::typography::text_sm())
                            .text_color(theme.danger)
                            .child(error),
                    );
                }
            }
        }
        body.into_any_element()
    }

    /// One source in the Chats list: icon, name and a one-line summary, an
    /// optional chevron to pick individual chats, and a switch or status.
    #[allow(clippy::too_many_arguments)]
    fn source_row(
        theme: &Theme,
        source: Source,
        first: bool,
        art: AnyElement,
        title: String,
        meta: String,
        meta_color: Hsla,
        trailing: AnyElement,
        expanded: Option<bool>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let key = match source {
            Source::Previous => "previous".to_owned(),
            Source::App(id) => format!("{id:?}"),
        };
        div()
            .id(SharedString::from(format!("onboarding-source-{key}")))
            .flex()
            .items_center()
            .gap(px(12.))
            .h(px(52.))
            .px(px(12.))
            .when(!first, |el| {
                el.border_t_1().border_color(theme.text.opacity(0.05))
            })
            .child(
                div()
                    .size(px(28.))
                    .flex_none()
                    .rounded(px(7.))
                    .bg(theme.text.opacity(0.05))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(art),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(crate::typography::text_sm())
                            .text_color(theme.text)
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(crate::typography::text_xs())
                            .text_color(meta_color)
                            .child(meta),
                    ),
            )
            .when_some(expanded, |el, open| {
                el.child(
                    div()
                        .id(SharedString::from(format!("onboarding-expand-{key}")))
                        .size(px(24.))
                        .flex_none()
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.element_hover))
                        .role(Role::Button)
                        .aria_expanded(open)
                        .tab_index(0)
                        .child(
                            icons::icon(if open {
                                icons::ALT_ARROW_UP
                            } else {
                                icons::ALT_ARROW_DOWN
                            })
                            .size(px(13.))
                            .text_color(theme.text_muted),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.expanded = if open { None } else { Some(source) };
                            cx.notify();
                        })),
                )
            })
            .child(div().flex_none().child(trailing))
    }

    fn status(theme: &Theme, done: bool, text: String) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(crate::typography::text_xs())
            .text_color(if done { theme.text } else { theme.text_muted })
            .when(done, |el| {
                el.child(
                    icons::icon(icons::CHECK)
                        .size(px(12.))
                        .text_color(theme.success),
                )
            })
            .child(text)
            .into_any_element()
    }

    /// A slim row inside an expanded source: picked rows carry a check, the
    /// rest are dimmed. No checkbox frame.
    fn pick_row(
        theme: &Theme,
        id: ElementId,
        on: bool,
        title: String,
        aside: Option<String>,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(32.))
            .pl(px(52.))
            .pr(px(14.))
            .cursor_pointer()
            .hover(|s| s.bg(theme.element_hover))
            .role(Role::CheckBox)
            .aria_toggled(if on { Toggled::True } else { Toggled::False })
            .tab_index(0)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::text_sm())
                    .text_color(if on { theme.text } else { theme.text_muted })
                    .child(title),
            )
            .when_some(aside, |el, aside| {
                el.child(
                    div()
                        .flex_none()
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .child(aside),
                )
            })
            .child(div().size(px(14.)).flex_none().when(on, |el| {
                el.child(
                    icons::icon(icons::CHECK)
                        .size(px(14.))
                        .text_color(theme.accent),
                )
            }))
    }

    fn render_chats(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().flex().flex_col().gap(px(16.)).child(self.heading(
            theme,
            tr(cx, msg::onboarding::flow_chats_title),
            tr(cx, msg::onboarding::flow_chats_body),
        ));
        let busy = self.busy(cx);
        let mut list: Vec<AnyElement> = Vec::new();

        // The previous version of Zuse comes first: it's everything at once.
        let (progress, previous, include) = {
            let desktop = self.desktop.read(cx);
            (
                desktop.progress(),
                desktop.discovery().and_then(|d| d.previous),
                desktop.include_settings(),
            )
        };
        if previous.is_some() || !matches!(progress, PreviousImport::Idle) {
            let summary = previous
                .as_ref()
                .map(|p| {
                    tr(cx, |l| {
                        msg::onboarding::flow_previous_summary(l, p.chats, p.projects)
                    })
                })
                .unwrap_or_default();
            let (meta, meta_color, trailing, open) = match &progress {
                PreviousImport::Done { chats, .. } => (
                    summary,
                    theme.text_muted,
                    Self::status(
                        theme,
                        true,
                        tr(cx, |l| msg::onboarding::flow_chats_imported(l, *chats)),
                    ),
                    None,
                ),
                PreviousImport::Running(message) => (
                    message.clone(),
                    theme.text_muted,
                    Self::status(theme, false, tr(cx, msg::onboarding::flow_source_importing)),
                    None,
                ),
                PreviousImport::Idle | PreviousImport::Failed(_) => {
                    let on = self.take_previous;
                    let (meta, color) = match &progress {
                        PreviousImport::Failed(error) => (error.clone(), theme.danger),
                        _ => (summary, theme.text_muted),
                    };
                    (
                        meta,
                        color,
                        widgets::toggle_switch(theme, on, "onboarding-previous").into_any_element(),
                        Some(self.expanded == Some(Source::Previous)),
                    )
                }
            };
            let interactive = open.is_some() && !busy;
            list.push(
                Self::source_row(
                    theme,
                    Source::Previous,
                    true,
                    icons::icon(icons::ZUSE_LOGO)
                        .w(px(13.))
                        .h(px(15.))
                        .text_color(theme.text)
                        .into_any_element(),
                    tr(cx, msg::onboarding::flow_previous_title),
                    meta,
                    meta_color,
                    trailing,
                    open,
                    cx,
                )
                .when(interactive, |el| {
                    el.cursor_pointer()
                        .role(Role::Switch)
                        .aria_toggled(if self.take_previous {
                            Toggled::True
                        } else {
                            Toggled::False
                        })
                        .tab_index(0)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.take_previous = !this.take_previous;
                            cx.notify();
                        }))
                })
                .into_any_element(),
            );
            if open == Some(true) {
                list.push(
                    Self::pick_row(
                        theme,
                        "onboarding-previous-settings".into(),
                        include,
                        tr(cx, msg::onboarding::flow_previous_settings),
                        None,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.desktop
                            .update(cx, |d, cx| d.set_include_settings(!include, cx))
                    }))
                    .into_any_element(),
                );
            }
        }

        // Then one row per agent app, busiest first.
        let rows = match &self.rows {
            Some(Ok(rows)) => rows.as_slice(),
            _ => &[],
        };
        let mut apps: Vec<(HarnessId, Vec<&external::Row>)> = Vec::new();
        for row in rows {
            match apps.iter_mut().find(|(id, _)| *id == row.provider) {
                Some((_, group)) => group.push(row),
                None => apps.push((row.provider, vec![row])),
            }
        }
        apps.sort_by_key(|(_, group)| std::cmp::Reverse(group.len()));
        for (provider, group) in &apps {
            let provider = *provider;
            let source = Source::App(provider);
            let open_rows: Vec<&external::Row> = group
                .iter()
                .copied()
                .filter(|r| !self.done.contains(&r.id))
                .collect();
            let imported = group.len() - open_rows.len();
            let picked = open_rows
                .iter()
                .filter(|r| self.picked.contains(&r.id))
                .count();
            let projects = group
                .iter()
                .map(|r| r.project_path.as_str())
                .collect::<BTreeSet<_>>()
                .len();
            let all_ids: Vec<String> = open_rows.iter().map(|r| r.id.clone()).collect();
            let on = picked > 0;
            let running = self.importing.is_some() && picked > 0;
            let (meta, trailing, open) = if open_rows.is_empty() {
                (
                    tr(cx, |l| {
                        msg::onboarding::flow_previous_summary(l, group.len(), projects)
                    }),
                    Self::status(
                        theme,
                        true,
                        tr(cx, |l| msg::onboarding::flow_chats_imported(l, imported)),
                    ),
                    None,
                )
            } else if running {
                (
                    tr(cx, |l| {
                        msg::onboarding::flow_source_picked(l, picked, open_rows.len())
                    }),
                    Self::status(theme, false, tr(cx, msg::onboarding::flow_source_importing)),
                    None,
                )
            } else {
                (
                    if on && picked < open_rows.len() {
                        tr(cx, |l| {
                            msg::onboarding::flow_source_picked(l, picked, open_rows.len())
                        })
                    } else {
                        tr(cx, |l| {
                            msg::onboarding::flow_previous_summary(l, open_rows.len(), projects)
                        })
                    },
                    widgets::toggle_switch(theme, on, format!("onboarding-app-{provider:?}"))
                        .into_any_element(),
                    Some(self.expanded == Some(source)),
                )
            };
            let (icon_path, tint) = crate::pickers::harness_brand_icon(provider);
            let interactive = open.is_some() && !busy;
            let first = list.is_empty();
            list.push(
                Self::source_row(
                    theme,
                    source,
                    first,
                    icons::icon(icon_path)
                        .size(px(15.))
                        .text_color(tint.unwrap_or(theme.text))
                        .into_any_element(),
                    crate::pickers::harness_display_name(provider).to_owned(),
                    meta,
                    theme.text_muted,
                    trailing,
                    open,
                    cx,
                )
                .when(interactive, |el| {
                    el.cursor_pointer()
                        .role(Role::Switch)
                        .aria_toggled(if on { Toggled::True } else { Toggled::False })
                        .tab_index(0)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if on {
                                for id in &all_ids {
                                    this.picked.remove(id);
                                }
                            } else {
                                this.picked.extend(all_ids.iter().cloned());
                            }
                            cx.notify();
                        }))
                })
                .into_any_element(),
            );
            if open == Some(true) {
                for (index, row) in open_rows.iter().enumerate() {
                    let id = row.id.clone();
                    list.push(
                        Self::pick_row(
                            theme,
                            SharedString::from(format!("onboarding-chat-{provider:?}-{index}"))
                                .into(),
                            self.picked.contains(&row.id),
                            row.title.clone(),
                            Some(relative_time(row.updated_at)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.busy(cx) {
                                return;
                            }
                            if !this.picked.remove(&id) {
                                this.picked.insert(id.clone());
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                    );
                }
            }
        }

        let loading = self.rows.is_none();
        if !list.is_empty() {
            let overflow = list.len() > 9;
            body = body.child(Self::list(
                theme,
                "onboarding-sources",
                452.,
                overflow,
                list,
            ));
        }
        if loading {
            body = body.child(Self::note(
                theme,
                tr(cx, msg::onboarding::flow_chats_loading),
            ));
        } else if let Some(Err(error)) = &self.rows {
            body = body.child(Self::note(theme, error.clone()));
        } else if apps.is_empty() && previous.is_none() && matches!(progress, PreviousImport::Idle)
        {
            body = body.child(Self::note(theme, tr(cx, msg::onboarding::flow_chats_none)));
        }
        if !apps.is_empty() {
            body = body.child(Self::note(theme, tr(cx, msg::onboarding::flow_chats_hint)));
        }
        if self.failed > 0 {
            body = body.child(
                div()
                    .text_size(crate::typography::text_sm())
                    .text_color(theme.danger)
                    .child(tr(cx, |l| {
                        msg::onboarding::flow_chats_failed(l, self.failed)
                    })),
            );
        }
        body.into_any_element()
    }

    fn render_personalize(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let prefs = settings::current(cx);
        let label = |text: String| {
            div()
                .text_size(crate::typography::text_sm())
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(text)
        };
        let appearance = div().flex().gap(px(8.)).children(
            crate::appearance::AppearanceMode::ALL
                .into_iter()
                .map(|mode| {
                    let on = prefs.appearance == mode;
                    div()
                        .id(SharedString::from(format!(
                            "onboarding-appearance-{}",
                            mode.label()
                        )))
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(8.))
                        .py(px(10.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(if on { theme.accent } else { theme.border })
                        .when(on, |el| el.bg(theme.accent_wash))
                        .cursor_pointer()
                        .role(Role::RadioButton)
                        .aria_toggled(if on { Toggled::True } else { Toggled::False })
                        .tab_index(0)
                        .text_size(crate::typography::text_sm())
                        .text_color(if on {
                            theme.text_strong
                        } else {
                            theme.text_muted
                        })
                        .child(icons::icon(mode.icon()).size(px(15.)).text_color(if on {
                            theme.text_strong
                        } else {
                            theme.text_muted
                        }))
                        .child(mode.localized_label(cx))
                        .on_click(move |_, _, cx| crate::appearance::set_mode(mode, cx))
                }),
        );
        let access = [
            (
                PermissionMode::ApprovalRequired,
                msg::onboarding::flow_access_ask as fn(&zuse_i18n::Localizer) -> String,
                msg::onboarding::flow_access_ask_body as fn(&zuse_i18n::Localizer) -> String,
            ),
            (
                PermissionMode::Auto,
                msg::onboarding::flow_access_auto,
                msg::onboarding::flow_access_auto_body,
            ),
            (
                PermissionMode::FullAccess,
                msg::onboarding::flow_access_full,
                msg::onboarding::flow_access_full_body,
            ),
        ];
        let rows = access
            .into_iter()
            .enumerate()
            .map(|(index, (mode, title, body))| {
                let on = prefs.default_permission_mode == mode;
                div()
                    .id(("onboarding-access", index))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(10.))
                    .when(index > 0, |el| {
                        el.border_t_1().border_color(theme.text.opacity(0.05))
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.element_hover))
                    .role(Role::RadioButton)
                    .aria_toggled(if on { Toggled::True } else { Toggled::False })
                    .tab_index(0)
                    .child(popover::radio_dot(theme, on))
                    .child(Self::row_text(theme, tr(cx, title), tr(cx, body)))
                    .on_click(move |_, _, cx| {
                        settings::update(settings::SavePolicy::Immediate, cx, |s| {
                            s.default_permission_mode = mode
                        });
                        cx.refresh_windows();
                    })
                    .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(px(24.))
            .child(self.heading(
                theme,
                tr(cx, msg::onboarding::flow_personal_title),
                tr(cx, msg::onboarding::flow_personal_body),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(label(tr(cx, msg::onboarding::flow_appearance)))
                    .child(appearance),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(label(tr(cx, msg::onboarding::flow_access)))
                    .child(Self::list(
                        theme,
                        "onboarding-access-list",
                        400.,
                        false,
                        rows,
                    )),
            )
            .into_any_element()
    }

    fn render_launch(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let previous = match self.desktop.read(cx).progress() {
            PreviousImport::Done { chats, open_chat } => Some((chats, open_chat)),
            _ => None,
        };
        let chats = self.imported.len() as u64 + previous.as_ref().map_or(0, |p| p.0);
        let servers = self
            .plugins
            .read(cx)
            .import_summary()
            .filter(|s| s.pending == 0)
            .map_or(0, |s| s.added());
        let open_chat = self
            .imported
            .first()
            .map(|i| i.chat.id.clone())
            .or_else(|| previous.and_then(|p| p.1));
        let mut points = div().flex().flex_col().gap(px(12.));
        if chats > 0 {
            points = points.child(Self::point(
                theme,
                icons::CHAT_ROUND_LINE,
                tr(cx, |l| msg::onboarding::flow_launch_chats(l, chats)),
            ));
        }
        if servers > 0 {
            points = points.child(Self::point(
                theme,
                icons::DITHER_PLUG,
                tr(cx, |l| msg::onboarding::flow_launch_servers(l, servers)),
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(24.))
            .child(motion_logo(theme))
            .child(self.heading(
                theme,
                tr(cx, msg::onboarding::flow_launch_title),
                tr(cx, msg::onboarding::flow_launch_body),
            ))
            .child(points)
            .when_some(open_chat.filter(|_| chats > 0), |el, id| {
                el.child(
                    div().child(
                        popover::btn_ghost(
                            theme,
                            &tr(cx, msg::onboarding::flow_launch_open_chats),
                            "onboarding-open-chats",
                        )
                        .id("onboarding-open-chats")
                        .border_1()
                        .border_color(theme.border)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.complete(false, Some(id.clone()), cx)
                        })),
                    ),
                )
            })
            .into_any_element()
    }

    fn render_language(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let current = settings::current(cx).language;
        let name = match current {
            LanguagePreference::System => tr(cx, msg::onboarding::flow_system_language),
            LanguagePreference::Explicit(locale) => locale.name().to_owned(),
        };
        let pill = div()
            .id("onboarding-language")
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(30.))
            .px(px(8.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|s| s.bg(theme.element_hover))
            .role(Role::Button)
            .aria_label(tr(cx, msg::onboarding::flow_language))
            .aria_expanded(self.language_open)
            .tab_index(0)
            .text_size(crate::typography::text_sm())
            .text_color(theme.text_muted)
            .child(
                icons::icon(icons::GLOBE)
                    .size(px(13.))
                    .text_color(theme.text_muted),
            )
            .child(name)
            .child(
                icons::icon(if self.language_open {
                    icons::ALT_ARROW_UP
                } else {
                    icons::ALT_ARROW_DOWN
                })
                .size(px(11.))
                .text_color(theme.text_muted),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.language_open = !this.language_open;
                cx.notify();
            }));
        let mut host = div().relative().child(pill);
        if self.language_open {
            let options = std::iter::once(LanguagePreference::System)
                .chain(Locale::ALL.into_iter().map(LanguagePreference::Explicit))
                .enumerate()
                .map(|(index, pref)| {
                    let on = pref == current;
                    let label = match pref {
                        LanguagePreference::System => tr(cx, msg::onboarding::flow_system_language),
                        LanguagePreference::Explicit(locale) => locale.name().to_owned(),
                    };
                    popover::menu_row(theme, on, format!("onboarding-language-{index}"))
                        .id(("onboarding-language-option", index))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.language_open = false;
                            crate::localization::set(pref, cx);
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            host = host.child(
                deferred(
                    anchored().anchor(Anchor::BottomLeft).child(
                        popover::popover_card(theme)
                            .mb(px(6.))
                            .w(px(200.))
                            .p(px(4.))
                            .occlude()
                            .children(options),
                    ),
                )
                .priority(3),
            );
        }
        host.into_any_element()
    }
}

/// Sign-in comes right before MCP servers, which need an account; skipping
/// it drops that step too so nothing asks again.
fn sequence(ask_sign_in: bool, skipped_sign_in: bool) -> Vec<Step> {
    let mut steps = vec![Step::Welcome, Step::Agents];
    if ask_sign_in {
        steps.push(Step::SignIn);
    }
    if !skipped_sign_in {
        steps.push(Step::Plugins);
    }
    steps.extend([Step::Chats, Step::Personalize, Step::Launch]);
    steps
}

fn motion_logo(theme: &Theme) -> impl IntoElement {
    crate::motion::settle_down(
        "onboarding-logo",
        icons::icon(icons::ZUSE_LOGO)
            .w(px(30.5))
            .h(px(35.))
            .text_color(theme.text_strong),
    )
}

/// `~/…` reads faster than a full home path in a narrow row.
fn home_relative(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match path.strip_prefix(home.trim_end_matches('/')) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
        _ => path.to_owned(),
    }
}

fn relative_time(ms: i64) -> String {
    let now = chrono::Utc::now().timestamp_millis();
    let minutes = (now - ms).max(0) / 60_000;
    match minutes {
        0 => "now".into(),
        1..=59 => format!("{minutes}m"),
        60..=1439 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1440),
    }
}

impl Render for Wizard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }
        if self.focus_pending {
            self.focus_pending = false;
            window.focus(&self.focus, cx);
        }
        let theme = Theme::of(cx).clone();
        let viewport = window.viewport_size();
        let steps = self.steps(cx);
        let index = steps.iter().position(|s| *s == self.step).unwrap_or(0);
        let total = steps.len();
        let content = match self.step {
            Step::Welcome => self.render_welcome(&theme, cx),
            Step::Agents => self.render_agents(&theme, cx),
            Step::SignIn => self.render_sign_in(&theme, cx),
            Step::Plugins => self.render_plugins(&theme, cx),
            Step::Chats => self.render_chats(&theme, cx),
            Step::Personalize => self.render_personalize(&theme, cx),
            Step::Launch => self.render_launch(&theme, cx),
        };
        let (primary, enabled) = self.primary(cx);
        let busy = self.busy(cx);
        let skip = match self.step {
            Step::Welcome => Some(tr(cx, msg::onboarding::flow_skip_setup)),
            Step::SignIn if !self.plugins.read(cx).is_signed_in() => {
                Some(tr(cx, msg::onboarding::flow_signin_skip))
            }
            Step::Launch => None,
            _ if busy => None,
            _ => Some(tr(cx, msg::onboarding::flow_skip)),
        };
        let progress = (index + 1) as f32 / total as f32;
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(28.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .w(px(120.))
                            .h(px(3.))
                            .rounded_full()
                            .bg(theme.text.opacity(0.08))
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(theme.accent)
                                    .w(px(120. * progress)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::text_xs())
                            .text_color(theme.text_muted)
                            .child(tr(cx, |l| msg::onboarding::flow_step(l, index + 1, total))),
                    ),
            );
        let footer = div()
            .flex()
            .items_center()
            .justify_between()
            .pt(px(20.))
            .child(
                div()
                    .when(self.step == Step::Welcome, |el| {
                        el.child(self.render_language(&theme, cx))
                    })
                    .when(index > 0 && !busy, |el| {
                        el.child(
                            div()
                                .id("onboarding-back")
                                .size(px(30.))
                                .rounded(px(8.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.element_hover))
                                .role(Role::Button)
                                .aria_label(tr(cx, msg::onboarding::flow_back))
                                .tab_index(0)
                                .child(
                                    icons::icon(icons::ARROW_LEFT)
                                        .size(px(15.))
                                        .text_color(theme.text_muted),
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .when_some(skip, |el, label| {
                        el.child(
                            popover::btn_ghost(&theme, &label, "onboarding-skip")
                                .id("onboarding-skip")
                                .on_click(cx.listener(|this, _, _, cx| this.skip(cx))),
                        )
                    })
                    .child(
                        popover::btn_primary(&theme, &primary)
                            .id("onboarding-primary")
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .when(!enabled, |el| el.opacity(0.5).cursor_default())
                            .when(enabled, |el| {
                                el.child(
                                    div().relative().child(
                                        popover::kbd_hint(&theme, "↵")
                                            .text_color(gpui::white().opacity(0.6)),
                                    ),
                                )
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.run_primary(cx))),
                    ),
            );
        let page = div()
            .w(px(COLUMN))
            .max_w(viewport.width - px(48.))
            .h_full()
            .flex()
            .flex_col()
            .pt(px(56.))
            .pb(px(32.))
            .child(header)
            .child(
                div()
                    .id("onboarding-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .pt(px((f32::from(viewport.height) * 0.12).clamp(24., 120.)))
                    .pb(px(24.))
                    .child(crate::motion::fade_quick(
                        SharedString::from(format!("onboarding-step-{:?}", self.step)),
                        div().w_full().child(content),
                    )),
            )
            .child(footer);
        // Frosted over the app behind it where the theme has glass; nearly
        // solid otherwise so the empty shell doesn't read through sharply.
        let veil = if theme.floating_glass() { 0.74 } else { 0.94 };
        let glow = div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(360.))
            .bg(linear_gradient(
                180.,
                linear_color_stop(theme.accent.opacity(0.07), 0.),
                linear_color_stop(theme.accent.opacity(0.), 1.),
            ));
        let surface = div()
            .id("onboarding")
            .track_focus(&self.focus)
            .key_context("Onboarding")
            .role(Role::Dialog)
            .occlude()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(theme.bg.opacity(veil))
            .text_color(theme.text)
            .flex()
            .justify_center()
            .child(glow)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "enter" => {
                        this.run_primary(cx);
                        cx.stop_propagation();
                    }
                    "escape" if this.language_open => {
                        this.language_open = false;
                        cx.notify();
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(page);
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(crate::frost::floating_glass(0., 40., surface))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn sign_in_precedes_plugins_and_skipping_it_drops_them() {
        use Step::*;
        assert_eq!(
            sequence(true, false),
            [Welcome, Agents, SignIn, Plugins, Chats, Personalize, Launch]
        );
        assert_eq!(
            sequence(true, true),
            [Welcome, Agents, SignIn, Chats, Personalize, Launch]
        );
        assert_eq!(
            sequence(false, false),
            [Welcome, Agents, Plugins, Chats, Personalize, Launch]
        );
    }

    #[test]
    fn paths_and_times_stay_short() {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            assert_eq!(home_relative(&format!("{home}/code/app")), "~/code/app");
        }
        assert_eq!(home_relative("/srv/app"), "/srv/app");
        assert_eq!(
            home_relative(&format!("{home}2/app")),
            format!("{home}2/app")
        );
        let now = chrono::Utc::now().timestamp_millis();
        assert_eq!(relative_time(now - 5 * 60_000), "5m");
        assert_eq!(relative_time(now - 3 * 3_600_000), "3h");
        assert_eq!(relative_time(now - 2 * 86_400_000), "2d");
    }
}
