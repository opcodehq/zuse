//! Device-local browser tabs. GPUI owns chrome; the native host owns pages.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
use linux as native;
#[cfg(target_os = "macos")]
use macos as native;
#[cfg(windows)]
mod windows_webview;
#[cfg(windows)]
use windows_webview as native;
pub mod model;
pub(crate) mod profile;
mod recent;
mod view;

use crate::composer::{ComposerInput, ComposerInputEvent};
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Subscription, Window,
};
use model::{PageState, Presentation};
gpui::actions!(
    browser,
    [Reload, FocusAddress, NewTab, CloseTab, Back, Forward]
);

pub(crate) fn bind_keys(cx: &mut App, keymap: &crate::settings::KeymapConfig) {
    use crate::settings::{ShortcutId, platform_combo};
    // Existing customized app chords win. In particular old settings may
    // already assign mod-shift-r to the right pane.
    let available = |combo: &str, own: Option<ShortcutId>| {
        !combo.is_empty()
            && !ShortcutId::ALL.iter().any(|id| {
                Some(*id) != own
                    && gpui::Keystroke::parse(&platform_combo(keymap.get(*id))).ok()
                        == gpui::Keystroke::parse(&platform_combo(combo)).ok()
            })
    };
    macro_rules! bind {
        ($combo:literal, $action:ident) => {
            if available($combo, None) {
                cx.bind_keys([gpui::KeyBinding::new(
                    &platform_combo($combo),
                    $action,
                    Some("Browser"),
                )]);
            }
        };
    }
    bind!("mod-l", FocusAddress);
    bind!("mod-t", NewTab);
    bind!("mod-w", CloseTab);
    bind!("mod-[", Back);
    bind!("mod-]", Forward);
    let reload = keymap.get(ShortcutId::BrowserReload);
    if available(reload, Some(ShortcutId::BrowserReload))
        && gpui::Keystroke::parse(&platform_combo(reload)).is_ok()
    {
        cx.bind_keys([gpui::KeyBinding::new(
            &platform_combo(reload),
            Reload,
            Some("Browser"),
        )]);
    }
}

/// Only application commands leave an embedded page. Text-editing shortcuts
/// stay in its inputs so Enter, Shift+Enter and Save keep their browser meaning.
#[cfg(any(target_os = "macos", windows, test))]
fn forwarded_shortcuts(keymap: &crate::settings::KeymapConfig) -> Vec<String> {
    use crate::settings::{ShortcutId, platform_combo};
    let desktop = keymap
        .desktop_bindings()
        .into_iter()
        .filter(|rule| rule.command.context().is_none())
        .filter(|rule| {
            rule.when.as_deref().is_none_or(|when| {
                crate::settings::keybindings::evaluate_when(when).unwrap_or(false)
            })
        })
        .map(|rule| rule.key);
    let native = ShortcutId::ALL
        .into_iter()
        .filter(|id| !matches!(id, ShortcutId::Desktop(_)) && id.key_context().is_none())
        .map(|id| keymap.get(id).to_owned());
    desktop
        .chain(native)
        .filter(|combo| !combo.is_empty())
        .map(|combo| platform_combo(&combo))
        .filter(|combo| gpui::Keystroke::parse(combo).is_ok())
        .collect()
}

#[cfg(test)]
mod keyboard_forwarding_tests {
    use super::*;
    #[test]
    fn browser_keeps_text_keys_and_forwards_every_application_override() {
        let mut keymap = crate::settings::KeymapConfig::default();
        keymap.import_desktop_rules(
            crate::settings::keybindings::import(&serde_json::json!({"schemaVersion":1,"rules":[
                {"key":"mod+shift+7","command":"new-tab"},
                {"key":"mod+shift+8","command":"new-tab"}
            ]}))
            .unwrap(),
        );
        let keys = forwarded_shortcuts(&keymap);
        for key in ["enter", "shift-enter", "mod-enter", "mod-s", "shift-tab"] {
            assert!(
                !keys.contains(&crate::settings::platform_combo(key)),
                "{key}"
            );
        }
        for key in ["mod-shift-7", "mod-shift-8", "mod-n"] {
            assert!(
                keys.contains(&crate::settings::platform_combo(key)),
                "{key}"
            );
        }
    }
}

#[derive(Clone, Debug)]
pub enum BrowserEvent {
    Changed,
    NewTab(Option<String>),
    Close,
    ElementSelected(crate::context_items::ContextItem),
}

/// A profile's website data, allocated on first navigation. Default contexts are
/// ephemeral, for unresolved identities and isolated fixtures.
#[derive(Clone, Default)]
pub struct BrowserContext {
    recent: std::rc::Rc<std::cell::RefCell<recent::Recent>>,
    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    data: native::BrowserData,
}

impl BrowserContext {
    pub(crate) fn for_profile(profile: profile::BrowserProfile) -> Self {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            Self {
                recent: recent::shared(profile.root.clone()),
                data: native::BrowserData::for_profile(profile),
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = profile;
            Self::default()
        }
    }
}

pub struct BrowserSurface {
    context: BrowserContext,
    address: Entity<ComposerInput>,
    focus: FocusHandle,
    pub page: PageState,
    pub favicon: Option<std::sync::Arc<gpui::Image>>,
    address_edited: bool,
    validation: Option<String>,
    toolbar_menu: crate::popover::Popup<()>,
    #[cfg(target_os = "macos")]
    appearance_menu_open: bool,
    #[cfg(target_os = "macos")]
    appearance_menu_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    #[cfg(target_os = "macos")]
    appearance_hover: crate::popover::HoverIntent<()>,
    hovered_toolbar_button: Option<&'static str>,
    #[cfg(target_os = "macos")]
    screenshot: Option<(std::path::PathBuf, Vec<u8>)>,
    #[cfg(target_os = "macos")]
    selecting_element: bool,
    #[cfg(target_os = "macos")]
    selection_generation: u64,
    remote: bool,
    previews: zuse_protocol::PreviewSnapshot,
    previews_loading: bool,
    previews_task: Option<gpui::Task<()>>,
    #[cfg(feature = "browser-fixture")]
    fixture_preview_open: std::rc::Rc<std::cell::Cell<Option<gpui::Point<gpui::Pixels>>>>,
    presentation: Presentation,
    #[cfg(target_os = "macos")]
    resize_inset: gpui::Pixels,
    #[cfg(any(target_os = "macos", windows))]
    right_occlusion: gpui::Pixels,
    _input_sub: Subscription,
    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    native: Option<native::NativePage>,
    #[cfg(target_os = "linux")]
    native_startup: Option<gpui::Task<()>>,
    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    native_tx: tokio::sync::mpsc::Sender<native::NativeEvent>,
    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    _native_task: gpui::Task<()>,
    #[cfg(any(target_os = "macos", windows))]
    favicon_task: Option<gpui::Task<()>>,
    #[cfg(any(target_os = "macos", windows))]
    favicon_generation: u64,
    /// The page's pointer cursor (Windows: GPUI owns the cursor).
    #[cfg(windows)]
    page_cursor: gpui::CursorStyle,
}

impl EventEmitter<BrowserEvent> for BrowserSurface {}
impl Focusable for BrowserSurface {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl BrowserSurface {
    pub fn new(
        context: BrowserContext,
        remote: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let address = cx.new(|cx| {
            ComposerInput::with_context("Search or enter a website", "PaletteSearch", cx)
                .with_text_metrics(crate::typography::TEXT_XS, 16.0)
                .with_single_line()
                .with_accessibility_role(gpui::Role::TextInput)
        });
        let input_sub = cx.subscribe(&address, |this, _, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                this.address_edited =
                    this.address.read(cx).text() != this.page.url.as_deref().unwrap_or_default();
                this.validation = None;
                cx.notify();
            }
        });
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        let (native_tx, mut events) = tokio::sync::mpsc::channel(64);
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        let native_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this
                    .update_in(cx, |this, window, cx| {
                        this.on_native_event(event, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
                // An animated offscreen page may produce updates faster than
                // a redraw finishes. Tokio's cooperative budget is not driven
                // by GPUI's executor: explicitly return control to its event loop.
                #[cfg(target_os = "linux")]
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1))
                    .await;
            }
        });
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        let _ = window;
        Self {
            context,
            address,
            focus: cx.focus_handle(),
            page: PageState::default(),
            favicon: None,
            address_edited: false,
            validation: None,
            toolbar_menu: Default::default(),
            #[cfg(target_os = "macos")]
            appearance_menu_open: false,
            #[cfg(target_os = "macos")]
            appearance_menu_bounds: None,
            #[cfg(target_os = "macos")]
            appearance_hover: Default::default(),
            hovered_toolbar_button: None,
            #[cfg(target_os = "macos")]
            screenshot: None,
            #[cfg(target_os = "macos")]
            selecting_element: false,
            #[cfg(target_os = "macos")]
            selection_generation: 0,
            remote,
            previews: zuse_protocol::PreviewSnapshot::default(),
            previews_loading: true,
            previews_task: None,
            #[cfg(feature = "browser-fixture")]
            fixture_preview_open: Default::default(),
            presentation: Presentation::Hidden,
            #[cfg(target_os = "macos")]
            resize_inset: gpui::px(0.0),
            #[cfg(any(target_os = "macos", windows))]
            right_occlusion: gpui::px(0.0),
            _input_sub: input_sub,
            #[cfg(any(target_os = "macos", target_os = "linux", windows))]
            native: None,
            #[cfg(target_os = "linux")]
            native_startup: None,
            #[cfg(any(target_os = "macos", target_os = "linux", windows))]
            native_tx,
            #[cfg(any(target_os = "macos", target_os = "linux", windows))]
            _native_task: native_task,
            #[cfg(any(target_os = "macos", windows))]
            favicon_task: None,
            #[cfg(any(target_os = "macos", windows))]
            favicon_generation: 0,
            #[cfg(windows)]
            page_cursor: gpui::CursorStyle::Arrow,
        }
    }

    pub fn title(&self) -> gpui::SharedString {
        self.page.label().into()
    }
    pub fn set_remote(&mut self, remote: bool) {
        self.remote = remote;
    }
    pub fn set_shortcuts(&mut self, keymap: &crate::settings::KeymapConfig) {
        #[cfg(any(target_os = "macos", windows))]
        if let Some(native) = &self.native {
            native.set_shortcuts(forwarded_shortcuts(keymap));
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        let _ = keymap;
    }

    pub fn focus_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(any(target_os = "macos", windows))]
        if let Some(native) = &self.native {
            native.focus_chrome();
        }
        window.focus(&self.address.focus_handle(cx), cx);
        window.dispatch_action(Box::new(crate::composer::SelectAll), cx);
    }

    /// Reserve the overlapping part of the shell divider for GPUI hit testing.
    #[cfg(target_os = "macos")]
    pub fn set_resize_inset(&mut self, inset: gpui::Pixels, cx: &mut Context<Self>) {
        if self.resize_inset != inset {
            self.resize_inset = inset;
            cx.notify();
        }
    }

    /// Crop a GPUI overlay out of both native painting and native hit testing.
    #[cfg(any(target_os = "macos", windows))]
    pub fn set_right_occlusion(&mut self, width: gpui::Pixels, cx: &mut Context<Self>) {
        if self.right_occlusion != width {
            self.right_occlusion = width;
            cx.notify();
        }
    }

    pub fn set_presentation(&mut self, presentation: Presentation, cx: &mut Context<Self>) {
        if self.presentation == presentation {
            return;
        }
        self.presentation = presentation;
        #[cfg(target_os = "macos")]
        if presentation != Presentation::Live {
            self.cancel_selection();
        }
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        if let Some(native) = &mut self.native {
            native.present(presentation);
        }
        cx.notify();
    }

    #[cfg(feature = "browser-fixture")]
    pub fn fixture_previews(&self) -> zuse_protocol::PreviewSnapshot {
        self.previews.clone()
    }
    #[cfg(feature = "browser-fixture")]
    pub fn fixture_preview_open_position(&self) -> Option<gpui::Point<gpui::Pixels>> {
        self.fixture_preview_open.get()
    }

    /// The daemon resolves the session's current cwd for every update, so a
    /// checkout change cannot leave this tab discovering the previous project.
    pub fn watch_previews(
        &mut self,
        handle: crate::state::EngineHandle,
        chat_id: String,
        cx: &mut Context<Self>,
    ) {
        self.previews_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let subscription = handle
                    .client()
                    .subscribe(
                        zuse_rpc::methods::WATCH_PREVIEWS,
                        serde_json::json!({"chatId": chat_id}),
                    )
                    .await;
                if let Ok(mut updates) = subscription {
                    while let Some(value) = updates.recv().await {
                        if let Ok(snapshot) =
                            serde_json::from_value::<zuse_protocol::PreviewSnapshot>(value)
                        {
                            if this
                                .update(cx, |this, cx| {
                                    #[cfg(target_os = "macos")]
                                    for service in &snapshot.services {
                                        this.context
                                            .data
                                            .register_preview(&service.url(snapshot.proxy_port));
                                    }
                                    this.previews = snapshot;
                                    this.previews_loading = false;
                                    cx.notify();
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
                if this
                    .update(cx, |this, cx| {
                        this.previews.services.clear();
                        this.previews.error = Some("Connecting to preview discovery…".into());
                        this.previews_loading = false;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
            }
        }));
    }

    pub fn navigate(&mut self, input: &str, window: &mut Window, cx: &mut Context<Self>) {
        let url = match model::normalize_address(input) {
            Ok(url) => url,
            Err(message) => {
                self.validation = Some(message.into());
                cx.notify();
                return;
            }
        };
        #[cfg(target_os = "macos")]
        self.cancel_selection();
        self.validation = None;
        self.address
            .update(cx, |input, cx| input.set_text(url.clone(), cx));
        self.address_edited = false;
        self.page.url = Some(url.clone());
        self.page.title.clear();
        self.page.error = None;
        self.clear_favicon(cx);
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        {
            #[cfg(any(target_os = "macos", windows))]
            {
                self.favicon_generation += 1;
                self.favicon_task = None;
            }
            let result = if let Some(native) = &self.native {
                native.load(&url)
            } else {
                #[cfg(any(target_os = "macos", windows))]
                {
                    native::NativePage::new(window, &self.context.data, self.native_tx.clone())
                        .map(|mut native| {
                            native.present(self.presentation);
                            self.native = Some(native);
                        })
                        .and_then(|_| self.native.as_ref().unwrap().load(&url))
                }
                #[cfg(target_os = "linux")]
                {
                    self.start_linux_page(window, cx)
                }
            };
            self.page.loading = result.is_ok();
            if let Err(error) = result {
                self.page.error = Some(format!("Could not open this page: {error}"));
            }
            window.focus(&self.focus, cx);
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        {
            let _ = window;
            cx.open_url(&url);
        }
        cx.emit(BrowserEvent::Changed);
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.address.read(cx).text().to_owned();
        match model::address_bar_input(&input) {
            Ok(url) => self.navigate(&url, window, cx),
            Err(message) => {
                self.validation = Some(message.into());
                cx.notify();
            }
        }
    }

    fn close_toolbar_menu(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            self.appearance_menu_open = false;
            self.appearance_menu_bounds = None;
            self.appearance_hover.reset();
        }
        if self.toolbar_menu.begin_close() {
            crate::popover::reap_popup(cx, |this: &mut Self| &mut this.toolbar_menu);
            cx.notify();
        }
    }

    #[cfg(target_os = "macos")]
    fn cancel_selection(&mut self) {
        if self.selecting_element {
            self.selecting_element = false;
            self.selection_generation = self.selection_generation.wrapping_add(1);
            if let Some(native) = &self.native {
                native.cancel_selection();
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn toggle_select_element(&mut self, cx: &mut Context<Self>) {
        if self.selecting_element {
            self.cancel_selection();
        } else if let (Some(native), Some(url)) = (&self.native, &self.page.url) {
            self.selection_generation = self.selection_generation.wrapping_add(1);
            self.selecting_element = true;
            native.select_element(url.clone(), self.selection_generation);
        }
        cx.notify();
    }

    #[cfg(target_os = "macos")]
    fn toggle_devtools(&mut self, cx: &mut Context<Self>) {
        if let Some(native) = &self.native {
            if let Err(message) = native.toggle_devtools() {
                crate::toast::info(cx, message);
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn save_screenshot(&mut self, cx: &mut Context<Self>) {
        self.close_toolbar_menu(cx);
        if let Some(native) = &self.native {
            native.save_screenshot(self.native_tx.clone());
        }
    }

    #[cfg(target_os = "macos")]
    fn confirm_clear_data(&mut self, cookies: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.close_toolbar_menu(cx);
        let (title, detail) = if cookies {
            (
                "Clear cookies?",
                "This clears cookies for all tabs in this browser profile and may sign you out of websites.",
            )
        } else {
            (
                "Clear cache?",
                "This clears cached website files for all tabs in this browser profile.",
            )
        };
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            title,
            Some(detail),
            &["Clear", "Cancel"],
            cx,
        );
        let data = self.context.data.clone();
        let tx = self.native_tx.clone();
        cx.spawn(async move |_, _| {
            if matches!(answer.await, Ok(0)) {
                data.clear_data(cookies, tx);
            }
        })
        .detach();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        if let Some(native) = &self.native {
            if self.page.error.is_some() {
                if let Some(url) = &self.page.url {
                    let _ = native.load(url);
                }
            } else {
                native.reload();
            }
            self.page.error = None;
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        self.open_external(cx);
        cx.notify();
    }

    fn open_external(&self, cx: &mut Context<Self>) {
        if let Some(url) = self
            .page
            .url
            .as_deref()
            .filter(|url| model::allowed_navigation(url))
        {
            cx.open_url(url);
        }
    }

    fn history(&mut self, forward: bool) {
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        if let Some(native) = &self.native {
            native.history(forward);
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        let _ = forward;
    }

    /// Explicitly close even if an async callback temporarily retains an entity.
    fn clear_favicon(&mut self, cx: &mut Context<Self>) {
        if let Some(image) = self.favicon.take() {
            cx.defer(move |cx| gpui::ImageSource::Image(image).evict(None, cx));
        }
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.set_presentation(Presentation::Hidden, cx);
        self.clear_favicon(cx);
        #[cfg(target_os = "linux")]
        {
            // Cancels delivery; any helper still starting finishes/cleans up on
            // its startup thread and cannot reattach to this closed tab.
            self.native_startup = None;
        }
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        {
            if let Some(native) = &mut self.native {
                native.present(Presentation::Hidden);
            }
            #[cfg(target_os = "linux")]
            if let Some(image) = self.native.as_mut().and_then(|native| native.image.take()) {
                cx.defer(move |cx| gpui::ImageSource::Render(image).evict(None, cx));
            }
            self.native = None;
            #[cfg(any(target_os = "macos", windows))]
            {
                self.favicon_task = None;
                self.favicon_generation += 1;
            }
        }
    }
}

#[cfg(any(target_os = "macos", windows))]
impl BrowserSurface {
    fn on_native_event(
        &mut self,
        event: native::NativeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(native) = &mut self.native else {
            return;
        };
        match event {
            native::NativeEvent::Changed | native::NativeEvent::Finished => {
                let finished = matches!(event, native::NativeEvent::Finished);
                let mut page = native.state();
                // A failed provisional request has no committed WebKit URL.
                if page.url.is_none() {
                    page.url = self.page.url.clone();
                }
                if page.error.is_some() {
                    page.loading = false;
                }
                if page.url != self.page.url || (!self.page.loading && page.loading) {
                    if let Some(image) = self.favicon.take() {
                        cx.defer(move |cx| gpui::ImageSource::Image(image).evict(None, cx));
                    }
                    self.favicon_generation += 1;
                    self.favicon_task = None;
                }
                if !self.address.focus_handle(cx).is_focused(window) {
                    if let Some(url) = &page.url {
                        if self.address.read(cx).text() != url {
                            self.address
                                .update(cx, |input, cx| input.set_text(url.clone(), cx));
                        }
                    }
                    self.address_edited = false;
                }
                native.present(self.presentation);
                if finished && let Some(url) = &page.url {
                    native.discover_favicon(url.clone());
                    if page.error.is_none() {
                        self.context.recent.borrow_mut().record(url, &page.title);
                    }
                }
                #[cfg(target_os = "macos")]
                if self.selecting_element
                    && (page.url != self.page.url || (!self.page.loading && page.loading))
                {
                    self.selecting_element = false;
                    self.selection_generation = self.selection_generation.wrapping_add(1);
                    native.cancel_selection();
                }
                if page != self.page {
                    self.page = page;
                    cx.emit(BrowserEvent::Changed);
                }
                cx.notify();
            }
            #[cfg(target_os = "macos")]
            native::NativeEvent::ElementSelected {
                page,
                generation,
                result,
            } => {
                if !self.selecting_element || generation != self.selection_generation {
                    return;
                }
                self.selecting_element = false;
                if self.page.url.as_deref() == Some(&page) {
                    match result {
                        Ok(Some(details)) => {
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&details) {
                                let selector = value["selector"].as_str().unwrap_or("element");
                                let text = value["text"].as_str().unwrap_or("");
                                let tag = value["tag"].as_str().unwrap_or("element");
                                let summary: String = text.chars().take(60).collect();
                                let label = if summary.is_empty() {
                                    tag.to_string()
                                } else if text.chars().count() > 60 {
                                    format!("{tag} · {summary}…")
                                } else {
                                    format!("{tag} · {summary}")
                                };
                                let mut body = format!(
                                    "Selected browser element from {page}:\nElement: {selector}"
                                );
                                if !text.is_empty() {
                                    body.push_str(&format!("\nText: {text}"));
                                }
                                let context = crate::context_items::ContextItem::new(
                                    format!("browser-element:{}", uuid::Uuid::new_v4()),
                                    label,
                                    body,
                                );
                                cx.emit(BrowserEvent::ElementSelected(context));
                            }
                        }
                        Err(error) => {
                            crate::toast::error(cx, format!("Could not select element: {error}"))
                        }
                        _ => {}
                    }
                }
                cx.notify();
            }
            #[cfg(target_os = "macos")]
            native::NativeEvent::InspectorUnavailable => crate::toast::info(
                cx,
                "Make the browser panel wider or taller to open developer tools inside it.",
            ),
            #[cfg(target_os = "macos")]
            native::NativeEvent::DataCleared(message) => crate::toast::success(cx, message),
            #[cfg(target_os = "macos")]
            native::NativeEvent::ScreenshotSaved(result) => match result {
                Ok(screenshot) => {
                    self.screenshot = Some(screenshot);
                    cx.notify();
                }
                Err(error) => {
                    crate::toast::error(cx, format!("Could not save screenshot: {error}"))
                }
            },
            native::NativeEvent::NewTab(url) => {
                if self.presentation == Presentation::Live {
                    cx.emit(BrowserEvent::NewTab(Some(url)));
                }
            }
            native::NativeEvent::Key(key) => {
                if self.presentation == Presentation::Live {
                    window.focus(&self.focus, cx);
                    window.defer(cx, move |window, cx| {
                        window.dispatch_keystroke(key, cx);
                    });
                }
            }
            #[cfg(windows)]
            native::NativeEvent::Cursor(style) => {
                if self.page_cursor != style {
                    self.page_cursor = style;
                    cx.notify();
                }
            }
            native::NativeEvent::Favicon { page, url } => {
                if self.page.url.as_deref() != Some(&page) || !model::allowed_navigation(&url) {
                    return;
                }
                let generation = self.favicon_generation;
                // reqwest's system proxy has no loopback exemption; a local dev
                // server's favicon must be fetched directly.
                let direct = url::Url::parse(&url).is_ok_and(|parsed| model::loopback(&parsed));
                let download = gpui_tokio::Tokio::spawn(cx, async move {
                    // Keep separate pools so loopback requests never use a proxy.
                    static DIRECT: std::sync::OnceLock<Option<reqwest::Client>> =
                        std::sync::OnceLock::new();
                    static PROXIED: std::sync::OnceLock<Option<reqwest::Client>> =
                        std::sync::OnceLock::new();
                    let pool = if direct { &DIRECT } else { &PROXIED };
                    let client = pool
                        .get_or_init(|| {
                            let builder = reqwest::Client::builder()
                                .timeout(std::time::Duration::from_secs(5))
                                .redirect(reqwest::redirect::Policy::limited(3));
                            let builder = if direct { builder.no_proxy() } else { builder };
                            builder.build().ok()
                        })
                        .as_ref()?;
                    let mut response =
                        client.get(url).send().await.ok()?.error_for_status().ok()?;
                    if response.content_length().is_some_and(|n| n > 1024 * 1024) {
                        return None;
                    }
                    let mut bytes = Vec::new();
                    while let Some(chunk) = response.chunk().await.ok()? {
                        if bytes.len() + chunk.len() > 1024 * 1024 {
                            return None;
                        }
                        bytes.extend_from_slice(&chunk);
                    }
                    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                        .with_guessed_format()
                        .ok()?;
                    let mut limits = image::Limits::default();
                    limits.max_image_width = Some(1024);
                    limits.max_image_height = Some(1024);
                    limits.max_alloc = Some(8 * 1024 * 1024);
                    reader.limits(limits);
                    let icon = reader.decode().ok()?.thumbnail(32, 32);
                    let mut png = std::io::Cursor::new(Vec::new());
                    icon.write_to(&mut png, image::ImageFormat::Png).ok()?;
                    Some(png.into_inner())
                });
                self.favicon_task = Some(cx.spawn(async move |this, cx| {
                    let Ok(Some(bytes)) = download.await else {
                        return;
                    };
                    let _ = this.update(cx, |this, cx| {
                        if this.favicon_generation == generation
                            && this.page.url.as_deref() == Some(&page)
                        {
                            this.context
                                .recent
                                .borrow_mut()
                                .set_icon(&page, bytes.clone());
                            this.favicon = Some(std::sync::Arc::new(gpui::Image::from_bytes(
                                gpui::ImageFormat::Png,
                                bytes,
                            )));
                            cx.emit(BrowserEvent::Changed);
                            cx.notify();
                        }
                    });
                }));
            }
        }
    }
}

#[cfg(feature = "browser-fixture")]
impl BrowserContext {
    pub fn fixture_persistence_supported() -> bool {
        #[cfg(target_os = "macos")]
        {
            native::persistent_profiles_supported()
        }
        #[cfg(not(target_os = "macos"))]
        {
            cfg!(target_os = "linux")
        }
    }

    /// Named WK stores live outside the fixture's temporary data directory.
    /// Remove only the store derived from this fixture installation and identity.
    pub fn fixture_remove_profile(
        data_dir: &std::path::Path,
        device: &str,
        completion: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        #[cfg(target_os = "macos")]
        {
            use wry::WebViewExtDarwin;
            if native::persistent_profiles_supported() {
                let profile = profile::BrowserProfile::for_workspace(
                    data_dir,
                    Some(zuse_protocol::WorkspaceScope::Local),
                    None,
                    Some(device),
                )
                .unwrap();
                eprintln!(
                    "Storage cleanup: invoke; main_thread={}",
                    objc2::MainThreadMarker::new().is_some()
                );
                // A cleanup-only process has not constructed a WebKit store yet.
                // Initialize its main run loop before removal dispatches back from the
                // WebsiteDataStoreIO queue. An ephemeral store never opens user data.
                let mtm = objc2::MainThreadMarker::new()
                    .expect("profile cleanup requires the main thread");
                let initialization =
                    unsafe { objc2_web_kit::WKWebsiteDataStore::nonPersistentDataStore(mtm) };
                drop(initialization);
                wry::WebView::remove_data_store(&profile.data_store_identifier(), move |result| {
                    eprintln!(
                        "Storage cleanup: callback; main_thread={}; result={result:?}",
                        objc2::MainThreadMarker::new().is_some()
                    );
                    completion(result.map_err(|error| error.to_string()));
                });
                eprintln!("Storage cleanup: invocation returned");
            } else {
                completion(Ok(()));
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (data_dir, device);
            completion(Ok(()));
        }
    }
}

#[cfg(feature = "browser-fixture")]
impl BrowserSurface {
    pub fn fixture_history(&mut self, forward: bool) {
        self.history(forward);
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_move_cursor(&self, x: f64, y: f64) {
        self.native.as_ref().unwrap().fixture_move_cursor(x, y);
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_origin(&self) -> (f32, f32) {
        self.native.as_ref().unwrap().fixture_origin()
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_overlay_visible(&self) -> bool {
        self.native.as_ref().unwrap().fixture_overlay_visible()
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_visibility_changes(&self) -> u64 {
        self.native.as_ref().unwrap().fixture_visibility_changes()
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_focus(&self) {
        self.native.as_ref().unwrap().fixture_focus();
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_focused(&self) -> bool {
        self.native.as_ref().unwrap().fixture_focused()
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_overlay_at(&self, x: f64, y: f64) -> bool {
        self.native.as_ref().unwrap().fixture_overlay_at(x, y)
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_click(&self, x: f64, y: f64) {
        self.native.as_ref().unwrap().fixture_click(x, y);
    }
    pub fn fixture_native_visible(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.native
                .as_ref()
                .is_some_and(|native| native.fixture_visible())
        }
        #[cfg(target_os = "linux")]
        {
            self.presentation != Presentation::Hidden
                && self.native.as_ref().is_some_and(|n| n.image.is_some())
        }
        #[cfg(windows)]
        {
            self.presentation != Presentation::Hidden && self.native.is_some()
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        {
            false
        }
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_backdrop_layers(&self) -> String {
        self.native.as_ref().unwrap().fixture_backdrop_layers()
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_page_hit(&self, x: f64, y: f64) -> bool {
        self.native.as_ref().unwrap().fixture_page_hit(x, y)
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_backdrops(&self) -> Vec<(f64, f64, f64, f64)> {
        self.native.as_ref().unwrap().fixture_backdrops()
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_geometry(&self) -> (f32, f32, f32) {
        self.native.as_ref().unwrap().fixture_geometry()
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_cookies(&self, completion: impl FnOnce(Vec<String>) + 'static) {
        self.native
            .as_ref()
            .expect("storage page must be attached")
            .fixture_cookies(completion);
    }

    pub fn fixture_eval(&self, script: &str) {
        #[cfg(target_os = "macos")]
        if let Some(native) = &self.native {
            native.fixture_eval(script);
        }
        #[cfg(target_os = "linux")]
        if let Some(native) = &self.native {
            native.evaluate(script);
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let _ = script;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    #[gpui::test]
    fn browser_options_and_appearance_menu_render_and_accept_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            crate::composer::init(cx, Default::default());
            cx.set_global(crate::theme::Theme::default());
            crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
        });
        let (browser, cx) = cx.add_window_view(|window, cx| {
            BrowserSurface::new(BrowserContext::default(), false, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear());
        let options = cx.debug_bounds("browser-options").unwrap().center();
        cx.simulate_click(options, gpui::Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        let appearance = cx
            .debug_bounds("browser-appearance-hover")
            .unwrap()
            .center();
        cx.simulate_mouse_move(appearance, None, gpui::Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        browser.read_with(cx, |browser, _| assert!(browser.appearance_menu_open));
        let screenshot = cx.debug_bounds("browser-screenshot").unwrap().center();
        cx.simulate_mouse_move(screenshot, None, gpui::Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        browser.read_with(cx, |browser, _| assert!(!browser.appearance_menu_open));
        cx.simulate_mouse_move(appearance, None, gpui::Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        let dark = cx.debug_bounds("browser-appearance-dark").unwrap().center();
        cx.simulate_mouse_move(dark, None, gpui::Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        browser.read_with(cx, |browser, _| assert!(browser.appearance_menu_open));
        cx.simulate_click(dark, gpui::Modifiers::default());
        cx.update(|_, cx| {
            assert_eq!(
                crate::settings::current(cx).browser_appearance,
                crate::appearance::AppearanceMode::Dark
            );
        });
    }

    #[gpui::test]
    fn address_enter_rejects_unsupported_schemes_and_escape_restores_url(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_base::init(cx);
            crate::composer::init(cx, Default::default());
            cx.set_global(crate::theme::Theme::default());
            bind_keys(cx, &crate::settings::KeymapConfig::default());
        });
        let window = cx.add_window(|window, cx| {
            BrowserSurface::new(BrowserContext::default(), false, window, cx)
        });
        window
            .update(cx, |browser, window, cx| {
                browser.page.url = Some("https://example.com/".into());
                browser
                    .address
                    .update(cx, |input, cx| input.set_text("javascript:alert(1)", cx));
                browser.focus_address(window, cx);
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "enter");
        window
            .update(cx, |browser, _, _| {
                assert!(
                    browser.validation.is_some(),
                    "Enter did not reach browser address handling"
                );
                assert_eq!(browser.page.url.as_deref(), Some("https://example.com/"));
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "escape");
        window
            .update(cx, |browser, window, cx| {
                assert!(browser.validation.is_none());
                assert_eq!(browser.address.read(cx).text(), "https://example.com/");
                assert!(browser.focus.is_focused(window));
                window.blur();
            })
            .unwrap();
    }

    #[gpui::test]
    fn customized_app_shortcuts_win_over_browser_defaults(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let mut config = crate::settings::KeymapConfig::default();
            // The browser address fallback is available after the application
            // focus-composer command gives up its default chord.
            config.set(
                crate::settings::ShortcutId::Desktop(
                    crate::settings::keybindings::Command::FocusComposer,
                ),
                String::new(),
            );
            bind_keys(cx, &config);
            assert_eq!(
                cx.key_bindings()
                    .borrow()
                    .bindings_for_action(&FocusAddress)
                    .count(),
                1
            );
            assert_eq!(
                cx.key_bindings()
                    .borrow()
                    .bindings_for_action(&Reload)
                    .count(),
                1
            );
            cx.clear_key_bindings();
            config.toggle_sidebar = crate::settings::platform_combo("mod-l");
            config.toggle_changes = "mod-shift-r".into();
            bind_keys(cx, &config);
            assert_eq!(
                cx.key_bindings()
                    .borrow()
                    .bindings_for_action(&FocusAddress)
                    .count(),
                0
            );
            assert_eq!(
                cx.key_bindings()
                    .borrow()
                    .bindings_for_action(&Reload)
                    .count(),
                0
            );
            assert_eq!(
                cx.key_bindings()
                    .borrow()
                    .bindings_for_action(&Back)
                    .count(),
                1
            );
        });
    }
}
