//! First-run Electron migration and the reusable Settings entrypoint.
use crate::localization::message as tr;
use crate::{
    popover, settings,
    state::{AppState, EngineHandle, EngineMode},
    theme::Theme,
};
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use zuse_i18n::messages as msg;
use zuse_protocol::{HarnessId, methods};

gpui::actions!(desktop_import, [OpenDesktopImport]);
pub struct Applied {
    pub open_chat: Option<String>,
    pub show_chats: bool,
}
/// What the first discovery found, for first-run onboarding.
pub struct Discovery {
    pub has_chats: bool,
    /// An interrupted import is waiting to resume in this dialog.
    pub pending: bool,
    pub previous: Option<Previous>,
}
pub struct Previous {
    pub name: String,
    pub chats: u64,
    pub projects: u64,
}
pub enum PreviousImport {
    Idle,
    Running(String),
    Failed(String),
    Done {
        chats: u64,
        open_chat: Option<String>,
    },
}
pub struct Onboarding {
    state: Entity<AppState>,
    account: Entity<crate::settings::plugins::PluginsCard>,
    engine: Option<EngineHandle>,
    destination: String,
    sources: Vec<Value>,
    selected: usize,
    visible: bool,
    manual: bool,
    review: bool,
    busy: bool,
    loading: bool,
    include_settings: bool,
    error: Option<String>,
    progress: String,
    result: Option<Value>,
    request_id: Option<String>,
    focus: FocusHandle,
    focus_pending: bool,
    task: Option<Task<()>>,
    preferences_task: Option<Task<()>>,
    embedded: bool,
    destination_has_chats: Option<bool>,
}
impl EventEmitter<Applied> for Onboarding {}
impl Onboarding {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let account =
            cx.new(|cx| crate::settings::plugins::PluginsCard::account(state.clone(), cx));
        cx.observe(&state, |this, _, cx| this.bind(cx)).detach();
        let mut this = Self {
            state,
            account,
            engine: None,
            destination: String::new(),
            sources: Vec::new(),
            selected: 0,
            visible: false,
            manual: false,
            review: false,
            busy: false,
            loading: false,
            include_settings: true,
            error: None,
            progress: String::new(),
            result: None,
            request_id: None,
            focus: cx.focus_handle(),
            focus_pending: false,
            task: None,
            preferences_task: None,
            embedded: false,
            destination_has_chats: None,
        };
        this.bind(cx);
        this
    }
    fn bind(&mut self, cx: &mut Context<Self>) {
        let engine = self.state.read(cx).engine().map(EngineHandle::controller);
        let engine = engine.filter(|e| match e.mode() {
            EngineMode::InProcess => true,
            EngineMode::Remote { url } => {
                url.starts_with("ws://127.0.0.1:") || url.starts_with("ws://localhost:")
            }
        });
        if self
            .engine
            .as_ref()
            .zip(engine.as_ref())
            .is_some_and(|(a, b)| a.same_connection(b))
        {
            return;
        }
        self.task = None;
        self.preferences_task = None;
        self.loading = false;
        self.engine = engine;
        self.destination_has_chats = None;
        if self.busy {
            self.busy = false;
            self.error = Some("The connection changed. Retry to recover the same import.".into());
        }
        if self.engine.is_some() {
            self.discover(None, cx);
        }
    }
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.manual = true;
        self.visible = true;
        self.focus_pending = true;
        window.focus(&self.focus, cx);
        if self.engine.is_none() {
            self.bind(cx);
        }
        if self.engine.is_some() && !self.busy {
            self.discover(None, cx);
        }
        cx.notify();
    }
    /// First-run onboarding owns the previous-version import while it runs,
    /// so this dialog stays closed instead of competing with it.
    pub fn set_embedded(&mut self, embedded: bool, cx: &mut Context<Self>) {
        self.embedded = embedded;
        if embedded && !self.manual {
            self.visible = false;
        }
        cx.notify();
    }
    /// `None` until this engine's first discovery reply lands.
    pub fn discovery(&self) -> Option<Discovery> {
        let has_chats = self.destination_has_chats?;
        let source = self
            .sources
            .get(self.selected)
            .filter(|_| self.request_id.is_some())
            .or_else(|| self.sources.iter().find(|s| s["importedBefore"] != true));
        Some(Discovery {
            has_chats,
            pending: self.request_id.is_some() && self.result.is_none(),
            previous: source.map(|s| Previous {
                name: s["name"].as_str().unwrap_or("Zuse").to_owned(),
                chats: s["summary"]["threads"].as_u64().unwrap_or(0),
                projects: s["summary"]["projects"].as_u64().unwrap_or(0),
            }),
        })
    }
    pub fn progress(&self) -> PreviousImport {
        if let Some(result) = &self.result {
            return PreviousImport::Done {
                chats: result["report"]["threads"].as_u64().unwrap_or(0),
                open_chat: result["openChatId"].as_str().map(str::to_owned),
            };
        }
        if self.busy {
            return PreviousImport::Running(self.progress.clone());
        }
        match &self.error {
            Some(error) => PreviousImport::Failed(error.clone()),
            None => PreviousImport::Idle,
        }
    }
    pub fn include_settings(&self) -> bool {
        self.include_settings
    }
    pub fn set_include_settings(&mut self, include: bool, cx: &mut Context<Self>) {
        if self.request_id.is_none() {
            self.include_settings = include;
            cx.notify();
        }
    }
    /// Import the detected previous-version profile without the review dialog.
    pub fn import_previous(&mut self, cx: &mut Context<Self>) {
        if self.request_id.is_none() {
            if let Some(index) = self
                .sources
                .iter()
                .position(|s| s["importedBefore"] != true)
            {
                self.selected = index;
            }
        }
        self.review = true;
        self.start(cx);
    }
    fn discover(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            self.error = Some("Connect to this computer to import Electron data.".into());
            return;
        };
        self.loading = true;
        self.error = None;
        let request_engine = engine.clone();
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            request_engine
                .client()
                .call_scoped("DiscoverDesktopImport", json!({"path":path}))
                .await
                .map_err(|e| e.to_string())
        });
        self.task=Some(cx.spawn(async move|this,cx|{
            let result=job.await.unwrap_or_else(|e|Err(e.to_string()));
            let _=this.update(cx,|this,cx|{
                if !this.engine.as_ref().is_some_and(|e|e.same_connection(&engine)){return;}
                this.loading=false;
                match result {
                    Ok(value)=>{
                        this.destination=value["destinationId"].as_str().unwrap_or_default().into();
                        this.sources=value["sources"].as_array().cloned().unwrap_or_default();this.selected=0;
                        this.destination_has_chats=Some(value["destinationHasChats"]==true);
                        let prefs=settings::current(cx);
                        let pending=prefs.desktop_import_pending.as_ref().filter(|p|p["destinationId"]==this.destination);
                        if let Some(pending)=pending {
                            if let Some(index)=this.sources.iter().position(|s|s["id"]==pending["sourceId"]){this.selected=index;}
                            this.include_settings=pending["includeSettings"].as_bool().unwrap_or(true);
                            this.request_id=pending["requestId"].as_str().map(str::to_owned);
                            this.review=true;this.visible=true;this.focus_pending=true;
                        } else if !this.embedded && !prefs.desktop_import_dismissed && value["destinationHasChats"] != true && this.sources.iter().any(|s|s["importedBefore"]!=true) {
                            this.visible=true;this.focus_pending=true;
                        }
                        if this.sources.is_empty() && value["errors"].as_array().is_some_and(|v|!v.is_empty()) {
                            this.error=Some("An Electron profile was found but could not be read. Choose another folder or start fresh.".into());
                        }
                    }
                    Err(message)=>{this.error=Some(message);}
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn browse(&mut self, cx: &mut Context<Self>) {
        if self.request_id.is_some() {
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose Electron data folder".into()),
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                if let Some(path) = paths.first() {
                    let path = path.to_string_lossy().into_owned();
                    let _ = this.update(cx, |this, cx| this.discover(Some(path), cx));
                }
            }
        }));
    }
    fn close(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.visible = false;
        self.manual = false;
        if self.result.take().is_some() {
            self.request_id = None;
            self.review = false;
        }
        settings::update(settings::SavePolicy::Immediate, cx, |s| {
            s.desktop_import_dismissed = true
        });
        cx.notify();
    }
    fn start(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.loading {
            return;
        }
        let Some(engine) = self.engine.clone() else {
            return;
        };
        let Some(source) = self.sources.get(self.selected).cloned() else {
            return;
        };
        let request_id = self
            .request_id
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        let params = json!({"sourceId":source["id"],"requestId":request_id,"includeSettings":self.include_settings});
        let pending = json!({"sourceId":source["id"],"requestId":request_id,"includeSettings":self.include_settings,"destinationId":self.destination});
        settings::update(settings::SavePolicy::Immediate, cx, |s| {
            s.desktop_import_pending = Some(pending)
        });
        settings::flush(cx);
        self.state.update(cx, |s, _| s.auto_selected = true);
        self.busy = true;
        self.error = None;
        self.progress = "Starting import…".into();
        let request_engine = engine.clone();
        let start = gpui_tokio::Tokio::spawn(cx, async move {
            request_engine
                .client()
                .forward("ImportDesktopProfile", params)
                .await
                .map_err(|e| e.to_string())
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let mut stream = match start.await.unwrap_or_else(|e| Err(e.to_string())) {
                Ok(stream) => stream,
                Err(message) => {
                    let _ = this.update(cx, |this, cx| {
                        this.busy = false;
                        this.error = Some(message);
                        cx.notify();
                    });
                    return;
                }
            };
            while let Some(frame) = stream.recv().await {
                let mut terminal = false;
                if this
                    .update(cx, |this, cx| {
                        if !this
                            .engine
                            .as_ref()
                            .is_some_and(|e| e.same_connection(&engine))
                        {
                            terminal = true;
                            return;
                        }
                        if let Some(error) = frame.err {
                            this.busy = false;
                            this.error = Some(error);
                            terminal = true;
                        }
                        if let Some(value) = frame.item.or(frame.ok) {
                            match value["kind"].as_str() {
                                Some("progress") => {
                                    this.progress = if let (Some(done), Some(total)) =
                                        (value["completed"].as_u64(), value["total"].as_u64())
                                    {
                                        format!("Importing sessions: {done} of {total}")
                                    } else {
                                        value["message"].as_str().unwrap_or("Importing…").into()
                                    };
                                }
                                Some("error") => {
                                    this.busy = false;
                                    this.error = Some(
                                        value["message"]
                                            .as_str()
                                            .unwrap_or("Import was interrupted. Retry to continue.")
                                            .into(),
                                    );
                                    terminal = true;
                                }
                                Some("complete") => {
                                    this.finish(value, cx);
                                    terminal = true;
                                }
                                _ => {}
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                    || terminal
                {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                if this.busy {
                    this.busy = false;
                    this.error = Some(
                        "The connection closed. Resume import to recover your progress.".into(),
                    );
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }
    fn finish(&mut self, value: Value, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        let request = value["requestId"].as_str().unwrap_or_default().to_owned();
        if !settings::current(cx)
            .desktop_import_applied
            .contains(&request)
        {
            if let Err(message) = apply_preferences(&self.state, &value["preferences"], cx) {
                self.busy = false;
                self.error = Some(message);
                return;
            }
        }
        self.progress = "Finishing settings…".into();
        let prefs = if settings::current(cx)
            .desktop_import_applied
            .contains(&request)
        {
            json!({})
        } else {
            value["preferences"].clone()
        };
        let request_engine = engine.clone();
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            if let Some(enabled) = prefs["providerEnabled"].as_object() {
                for (provider, enabled) in enabled {
                    if let (Some(harness), Some(enabled)) =
                        (legacy_harness(provider), enabled.as_bool())
                    {
                        request_engine
                            .client()
                            .call_scoped(
                                methods::SET_HARNESS_ENABLED,
                                json!({"harness":harness,"enabled":enabled}),
                            )
                            .await
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
            Ok::<_, String>(())
        });
        self.preferences_task = Some(cx.spawn(async move |this, cx| {
            let result = job.await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |this, cx| {
                if !this
                    .engine
                    .as_ref()
                    .is_some_and(|e| e.same_connection(&engine))
                {
                    return;
                }
                this.busy = false;
                match result {
                    Ok(()) => {
                        settings::update(settings::SavePolicy::Immediate, cx, |s| {
                            s.desktop_import_pending = None;
                            s.desktop_import_dismissed = true;
                            if !s.desktop_import_applied.contains(&request) {
                                s.desktop_import_applied.push(request);
                            }
                        });
                        this.result = Some(value);
                        crate::pickers::bump_harness_catalog(cx);
                        cx.emit(Applied {
                            open_chat: None,
                            show_chats: false,
                        });
                    }
                    Err(message) => {
                        this.error = Some(format!(
                            "Chats were imported, but settings need another try: {message}"
                        ))
                    }
                }
                cx.notify();
            });
        }));
    }
}
fn legacy_harness(value: &str) -> Option<HarnessId> {
    Some(match value {
        "claude" => HarnessId::ClaudeCode,
        "codex" => HarnessId::Codex,
        "cursor" => HarnessId::Cursor,
        "grok" => HarnessId::Grok,
        "opencode" => HarnessId::Opencode,
        "gemini" => HarnessId::Gemini,
        "kiro" => HarnessId::Kiro,
        "devin" => HarnessId::Devin,
        "hermes" => HarnessId::Hermes,
        "pi" => HarnessId::Pi,
        "muse" => HarnessId::Muse,
        "antigravity" => HarnessId::Antigravity,
        "zuse" => HarnessId::Zuse,
        _ => return None,
    })
}
fn apply_preferences(state: &Entity<AppState>, prefs: &Value, cx: &mut App) -> Result<(), String> {
    use settings::composer::{ComposerDefaults, RememberedModel};
    if prefs.as_object().is_none_or(|p| p.is_empty()) {
        return Ok(());
    }
    let mut current = settings::current(cx);
    if let Some(value) = prefs.get("appearanceMode") {
        current.appearance = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    }
    if let Some(enabled) = prefs["completionSoundEnabled"].as_bool() {
        current.sound_completion_enabled = enabled;
    }
    if let Some(value) = prefs.get("completionSoundPreset") {
        current.completion_sound_preset =
            serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    }
    if let Some(mode) = prefs["defaultRuntimeMode"].as_str() {
        current.default_permission_mode =
            serde_json::from_value(json!(if matches!(mode, "auto" | "full-access") {
                mode
            } else {
                "approval-required"
            }))
            .map_err(|e| e.to_string())?;
    }
    if let Some(value) = prefs.get("keybindings") {
        current
            .keymap
            .import_desktop_rules(settings::keybindings::import(value)?);
    }
    let state = state.read(cx);
    let dir = state
        .data_dir
        .clone()
        .ok_or("GPUI settings folder is unavailable")?;
    let mut defaults = ComposerDefaults::load(&dir);
    if let Some(models) = prefs["defaultModelByProvider"].as_object() {
        for (provider, model) in models {
            if let (Some(harness), Some(model)) = (legacy_harness(provider), model.as_str()) {
                defaults.model_by_harness.insert(
                    harness,
                    RememberedModel {
                        id: model.into(),
                        label: model.into(),
                    },
                );
            }
        }
    }
    if let Some(harness) = prefs["defaultProviderId"].as_str().and_then(legacy_harness) {
        defaults.harness = Some(harness);
        if let Some(model) = defaults.model_by_harness.get(&harness) {
            current.default_model = Some(zuse_protocol::presentation::DefaultModelSelection {
                provider: harness,
                model: model.id.clone(),
                label: model.label.clone(),
            });
        }
    }
    if let Some(worktree) = prefs["defaultAutoCreateWorktree"].as_bool() {
        for space in &state.spaces {
            defaults.workspace_by_project.insert(
                space.id.clone(),
                if worktree {
                    zuse_protocol::presentation::WorkspaceMode::Worktree
                } else {
                    zuse_protocol::presentation::WorkspaceMode::Local
                },
            );
        }
    }
    defaults.save(&dir).map_err(|e| e.to_string())?;
    let appearance = current.appearance;
    settings::replace(current, settings::SavePolicy::Immediate, cx);
    let prefs = settings::current(cx);
    crate::shell::apply_keymap(cx, &prefs.keymap, prefs.composer_send_behavior);
    crate::appearance::set_mode(appearance, cx);
    Ok(())
}
impl Render for Onboarding {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }
        if self.focus_pending {
            self.focus_pending = false;
            window.focus(&self.focus, cx);
        }
        let theme = Theme::of(cx).for_popup();
        let mut card=popover::dialog_card(&theme).id("desktop-import-dialog").role(Role::Dialog).track_focus(&self.focus)
            .w(px(520.)).max_w(window.viewport_size().width-px(32.)).max_h(window.viewport_size().height-px(32.)).overflow_y_scroll()
            .on_key_down(cx.listener(|this,event:&KeyDownEvent,window,cx|{if !this.focus.is_focused(window){return;} if event.keystroke.key=="escape"&&!this.busy{this.close(cx);cx.stop_propagation();}else if event.keystroke.key=="enter"&&!this.busy&&!this.sources.is_empty()&&this.result.is_none(){if this.review{this.start(cx);}else{this.review=true;cx.notify();}cx.stop_propagation();}}))
            .child(popover::dialog_title(&theme,if self.result.is_some(){"Your chats are ready"}else{"Bring your chats into GPUI"}))
            .child(div().mt_2().child(popover::dialog_body(&theme,"Import chats, projects, and supported settings from Electron. Your original data stays in place.")));
        if let Some(result) = &self.result {
            let report = &result["report"];
            card=card.child(div().mt_4().text_color(theme.text).child(format!("{} chats · {} sessions · {} projects",report["threads"],report["sessions"],report["projects"])))
                .child(div().mt_2().text_color(theme.text_muted).child(tr(cx, msg::native::your_account_plugins_are_available_when_you_sign_in_with_the_same_zuse_account)));
            if report["missingAttachments"]
                .as_array()
                .is_some_and(|v| !v.is_empty())
            {
                card=card.child(div().mt_2().text_color(theme.text_muted).child(format!("{} original attachment files were unavailable. Their records remain in the backup.",report["missingAttachments"].as_array().unwrap().len())));
            }
            if report["settingsRetained"]
                .as_array()
                .is_some_and(|v| !v.is_empty())
            {
                card=card.child(div().mt_2().text_color(theme.text_muted).child(tr(cx, msg::native::electron_only_preferences_were_preserved_with_the_backup_they_have_not_been_appl_1276c192)));
            }
            if result["localPlugins"].as_u64().unwrap_or(0) > 0 {
                card = card.child(div().mt_2().text_color(theme.text_muted).child(tr(cx, msg::native::local_plugin_definitions_are_backed_up_command_based_electron_plugins_are_not_ye_76dff728)));
            }
            card = card.child(
                div().mt_4().child(
                    popover::btn_primary(&theme, "Open imported chats")
                        .id("desktop-import-done")
                        .on_click(cx.listener(|this, _, _, cx| {
                            let open_chat = this
                                .result
                                .as_ref()
                                .and_then(|r| r["openChatId"].as_str())
                                .map(str::to_owned);
                            cx.emit(Applied {
                                open_chat,
                                show_chats: true,
                            });
                            this.close(cx);
                        })),
                ),
            );
        } else if self.busy {
            card = card
                .child(
                    div()
                        .mt_5()
                        .text_color(theme.text)
                        .child(self.progress.clone()),
                )
                .child(div().mt_2().text_color(theme.text_muted).child(tr(
                    cx,
                    msg::native::you_can_retry_an_interrupted_import_without_duplicating_chats,
                )));
        } else {
            if self.loading {
                card = card.child(
                    div()
                        .mt_4()
                        .text_color(theme.text_muted)
                        .child(tr(cx, msg::native::looking_for_electron_data)),
                );
            } else if self.sources.is_empty() {
                card = card.child(div().mt_4().text_color(theme.text_muted).child(tr(
                    cx,
                    msg::native::no_electron_profile_was_detected_on_this_computer,
                )));
            } else if !self.review {
                for (index, source) in self.sources.iter().enumerate() {
                    let selected = self.selected == index;
                    card = card.child(
                        div()
                            .id(("desktop-import-source", index))
                            .mt_3()
                            .p_3()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(if selected {
                                theme.text_muted
                            } else {
                                theme.border
                            })
                            .cursor_pointer()
                            .child(format!(
                                "{} (Electron)",
                                source["name"].as_str().unwrap_or("Zuse")
                            ))
                            .child(div().text_color(theme.text_muted).child(format!(
                                "{} chats · {} sessions",
                                source["summary"]["threads"], source["summary"]["sessions"]
                            )))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.request_id.is_some() {
                                    return;
                                }
                                this.selected = index;
                                this.request_id = None;
                                cx.notify();
                            })),
                    );
                }
            } else if let Some(source) = self.sources.get(self.selected) {
                card = card
                    .child(div().mt_4().text_color(theme.text).child(format!(
                        "{} chats · {} sessions · {} projects",
                        source["summary"]["threads"],
                        source["summary"]["sessions"],
                        source["summary"]["projects"]
                    )))
                    .child(
                        div()
                            .id("desktop-import-settings")
                            .mt_3()
                            .cursor_pointer()
                            .child(if self.include_settings {
                                "✓ Bring over supported settings"
                            } else {
                                "Bring over supported settings"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.request_id.is_none() {
                                    this.include_settings = !this.include_settings;
                                    cx.notify();
                                }
                            })),
                    );
                let missing = source["missingAttachments"].as_u64().unwrap_or(0);
                if missing > 0 {
                    card = card.child(div().mt_2().text_color(theme.text_muted).child(format!("{missing} original attachment files are unavailable. Their records will remain in the backup.")));
                }
                let retained = source["summary"]["settingsRetained"]
                    .as_array()
                    .map_or(0, Vec::len);
                if retained > 0 {
                    card = card.child(div().mt_2().text_color(theme.text_muted).child(format!(
                        "{retained} Electron-only preference groups will be kept in your backup."
                    )));
                }
                if source["localPlugins"].as_u64().unwrap_or(0) > 0 {
                    card = card.child(div().mt_2().text_color(theme.text_muted).child(tr(cx, msg::native::local_plugin_definitions_will_be_backed_up_command_based_electron_plugins_are_no_37441bc7)));
                }
                card = card.child(div().mt_2().text_color(theme.text_muted).child(tr(cx, msg::native::imported_settings_replace_matching_gpui_preferences_other_gpui_preferences_stay_3e190ea2)));
                if zuse_viewport::cloud_gate::enabled() {
                    card = card.child(div().mt_4().child(self.account.clone()));
                }
            }
            if let Some(error) = &self.error {
                card = card.child(div().mt_3().text_color(theme.text).child(error.clone()));
            }
            let mut buttons = popover::dialog_footer(&theme)
                .child(
                    popover::btn_ghost(
                        &theme,
                        if self.request_id.is_some() {
                            "Close for now"
                        } else {
                            "Start fresh"
                        },
                        "desktop-import-skip",
                    )
                    .id("desktop-import-skip")
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                )
                .child(
                    popover::btn_ghost(
                        &theme,
                        &tr(cx, msg::native::choose_folder),
                        "desktop-import-browse",
                    )
                    .id("desktop-import-browse")
                    .on_click(cx.listener(|this, _, _, cx| this.browse(cx))),
                );
            if !self.sources.is_empty() && !self.loading {
                buttons = buttons.child(
                    popover::btn_primary(
                        &theme,
                        if self.review {
                            if self.request_id.is_some() {
                                "Resume import"
                            } else {
                                "Import"
                            }
                        } else {
                            "Review import"
                        },
                    )
                    .id("desktop-import-primary")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.review {
                            this.start(cx)
                        } else {
                            this.review = true;
                            cx.notify();
                        }
                    })),
                );
            }
            card = card.child(buttons);
        }
        popover::modal(
            "desktop-import-modal",
            window.viewport_size(),
            card.into_any_element(),
        )
    }
}
