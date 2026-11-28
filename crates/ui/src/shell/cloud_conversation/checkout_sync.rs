//! This chat's files in a folder of its own on this computer: the switch in
//! the computer menu, its status there and beside the header's cloud icon,
//! and the folder's split button. The local engine makes the folder (a
//! worktree of the user's clone, or a fresh clone) and keeps it in sync; the
//! user's own checkout is never touched.
use super::computer::{Tone, nested_chevron, split_field, split_main};
use super::open_menu::{self, Kind};
use super::*;
use std::time::Duration;

/// The last status, the poll that keeps it current and a pending change.
#[derive(Default)]
pub(super) struct Checkout {
    pub(super) status: Value,
    poll: Option<Task<()>>,
    change: Option<Task<()>>,
    /// Checkouts on this computer: folder and remote.
    local: Vec<(String, String)>,
    error: Option<String>,
    /// Why the status couldn't be read, until a read succeeds.
    read_error: Option<String>,
    /// Advanced by every change, so a status read that started before one
    /// never replaces its result.
    revision: u64,
}

impl Checkout {
    /// No change is on its way.
    pub(super) fn change_idle(&self) -> bool {
        self.change.is_none()
    }
}

/// How a status reads, with its dot.
pub(super) fn phase(status: &Value) -> (&'static str, Tone) {
    match status["state"].as_str() {
        Some("synced") => ("In sync", Tone::Running),
        Some("syncing") => ("Syncing…", Tone::Busy),
        Some("preparing") => ("Preparing the folder…", Tone::Busy),
        Some("behind") => ("Waiting for the cloud computer", Tone::Idle),
        Some("paused") => ("Paused: edited in the folder", Tone::Busy),
        Some("restoring") => ("Putting your checkout back…", Tone::Busy),
        Some("error") if status["updateRequired"] == true => ("Needs an update", Tone::Failed),
        Some("error") => ("Sync failed", Tone::Failed),
        _ => ("Off", Tone::Idle),
    }
}

/// What can be done about the status, one row each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Remedy {
    /// Paused: throw away the folder's edits and sync again.
    DiscardAndResume,
    /// Paused: stop syncing and keep the folder as it is.
    KeepEditing,
    /// The cloud computer is too old to sync: update (recreate) it.
    UpdateComputer,
    TryAgain,
}

impl Remedy {
    fn label(self) -> &'static str {
        match self {
            Self::DiscardAndResume => "Discard my edits and resume",
            Self::KeepEditing => "Keep editing (stop sync)",
            Self::UpdateComputer => "Update computer",
            Self::TryAgain => "Try again",
        }
    }

    fn tip(self) -> Option<&'static str> {
        match self {
            Self::DiscardAndResume => Some(
                "Throws away what changed in the folder here, then picks up the cloud computer's files.",
            ),
            Self::KeepEditing => Some("Stops sync. The folder and your edits stay."),
            Self::UpdateComputer => {
                Some("Moves this computer to the latest version. Its files are kept.")
            }
            Self::TryAgain => None,
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::DiscardAndResume | Self::TryAgain | Self::UpdateComputer => icons::REFRESH,
            Self::KeepEditing => icons::STOP,
        }
    }
}

/// What the computer menu shows for a status.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Rows {
    /// The status line with its dot.
    pub(super) status: bool,
    /// The folder's split button.
    pub(super) folder: bool,
    pub(super) remedies: Vec<Remedy>,
}

/// The message shown when the cloud computer must be updated to sync.
pub(super) const UPDATE_REQUIRED: &str = "Update this computer to sync.";

pub(super) fn rows(status: &Value) -> Rows {
    let state = status["state"].as_str().unwrap_or("off");
    let remedies = match state {
        "paused" => vec![Remedy::DiscardAndResume, Remedy::KeepEditing],
        "error" if status["updateRequired"] == true => vec![Remedy::UpdateComputer],
        "error" | "restoring" => vec![Remedy::TryAgain],
        _ => Vec::new(),
    };
    Rows {
        status: status["enabled"] == true || state == "restoring",
        folder: status["localPath"].is_string(),
        remedies,
    }
}

/// The checkout of `repository` among this computer's (folder, remote).
fn matching_checkout(checkouts: &[(String, String)], repository: &str) -> Option<String> {
    let repository = crate::cloud_destination::normalize(repository);
    checkouts
        .iter()
        .find(|(_, remote)| crate::cloud_destination::normalize(remote) == repository)
        .map(|(path, _)| path.clone())
}

/// Whether the status's reason reads as plain information (muted) rather
/// than a failure (red): waiting for the cloud computer, paused by edits
/// made in the folder, or an update to do, unless a request of ours failed.
fn quiet(status: &Value, request_failed: bool) -> bool {
    let update = status["state"] == "error" && status["updateRequired"] == true;
    (matches!(status["state"].as_str(), Some("behind" | "paused")) || update) && !request_failed
}

/// The last part of a folder path, as the header shows it.
fn folder_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

/// How many characters of the folder's path its button shows.
const FOLDER_PATH_CHARS: usize = 32;

impl CloudConversation {
    fn checkout_body(&self, cx: &App) -> Option<Value> {
        let chat = self.state.read(cx).selected_chat.clone()?;
        Some(json!({"expectedAccountId":self.target.subject,
            "workspaceId":self.target.workspace,"chatId":chat}))
    }

    /// The checkouts on this computer (folder, remote), as the shell knows
    /// its projects.
    pub(in crate::shell) fn set_local_checkouts(&mut self, checkouts: Vec<(String, String)>) {
        self.computer.checkout.local = checkouts;
    }

    /// This repository's checkout on this computer, when Zuse knows one:
    /// the engine makes the chat's folder from it instead of cloning.
    pub(super) fn local_checkout(&self) -> Option<String> {
        let identity = self.computer.metadata["repositoryIdentity"]
            .as_str()
            .or_else(|| self.config["repositoryIdentity"].as_str())
            .or_else(|| self.computer.metadata["repositoryUrl"].as_str())
            .or_else(|| self.config["repositoryUrl"].as_str())?;
        matching_checkout(&self.computer.checkout.local, identity)
    }

    /// The chat's folder here, synced or kept after sync stopped.
    pub(super) fn checkout_folder(&self) -> Option<String> {
        self.computer.checkout.status["localPath"]
            .as_str()
            .map(str::to_owned)
    }

    /// Whether the kept folder can be deleted: sync is off and it exists.
    pub(super) fn checkout_deletable(&self) -> bool {
        let status = &self.computer.checkout.status;
        status["enabled"] != true
            && status["state"] != "restoring"
            && status["localPath"].is_string()
    }

    /// The app the folder's button opens it in.
    pub(super) fn folder_app(&self, cx: &App) -> Option<crate::shell::open_in::OpenTarget> {
        open_menu::default_target(
            &self.computer.open_targets,
            crate::settings::current(cx).synced_folder_app.as_deref(),
        )
    }

    /// Make `id` the app the folder's button opens it in.
    pub(super) fn remember_folder_app(&mut self, id: &str, cx: &mut Context<Self>) {
        if crate::settings::current(cx).synced_folder_app.as_deref() != Some(id) {
            let id = id.to_owned();
            crate::settings::update(crate::settings::SavePolicy::Debounced, cx, |settings| {
                settings.synced_folder_app = Some(id)
            });
        }
    }

    /// Stop sync if on and delete the chat's folder.
    pub(super) fn delete_checkout_folder(&mut self, cx: &mut Context<Self>) {
        self.change_checkout(methods::CLOUD_CHECKOUT_SYNC_DELETE_FOLDER, json!({}), cx);
    }

    /// Keep the status current: often while synced or while the menu shows
    /// it, rarely otherwise.
    pub(super) fn watch_checkout(&mut self, cx: &mut Context<Self>) {
        if self.computer.checkout.poll.is_some() || !self.authorized {
            return;
        }
        let target = self.target.clone();
        self.computer.checkout.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(Some((body, idle, often, revision))) = this.update(cx, |this, cx| {
                    if !this.authorized {
                        return None;
                    }
                    let body = this.checkout_body(cx)?;
                    let checkout = &this.computer.checkout;
                    let often = this.computer.open || checkout.status["enabled"] == true;
                    Some((body, checkout.change.is_none(), often, checkout.revision))
                }) else {
                    let _ = this.update(cx, |this, _| this.computer.checkout.poll = None);
                    return;
                };
                if idle {
                    let result = target
                        .controller
                        .client()
                        .call_scoped(methods::CLOUD_CHECKOUT_SYNC_STATUS, body)
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        let checkout = &mut this.computer.checkout;
                        if checkout.change.is_some() || checkout.revision != revision {
                            return;
                        }
                        match result {
                            Ok(value) if value["accountId"] == target.subject => {
                                checkout.read_error = None;
                                if checkout.status != value["status"] {
                                    checkout.status = value["status"].clone();
                                }
                            }
                            Ok(_) => {}
                            Err(error) => checkout.read_error = Some(error.to_string()),
                        }
                        cx.notify();
                    });
                }
                let wait = if often { 2 } else { 15 };
                cx.background_executor()
                    .timer(Duration::from_secs(wait))
                    .await;
            }
        }));
    }

    /// Call `method` with the chat's scope plus `extra`, and show the status
    /// it answers with.
    fn change_checkout(&mut self, method: &'static str, extra: Value, cx: &mut Context<Self>) {
        if self.computer.checkout.change.is_some() || !self.authorized {
            return;
        }
        let Some(mut body) = self.checkout_body(cx) else {
            return;
        };
        if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            body.extend(extra.clone());
        }
        let target = self.target.clone();
        self.computer.checkout.error = None;
        self.computer.checkout.revision += 1;
        self.computer.checkout.change = Some(cx.spawn(async move |this, cx| {
            let result = target.controller.client().call_scoped(method, body).await;
            let _ = this.update(cx, |this, cx| {
                this.computer.checkout.change = None;
                this.computer.checkout.revision += 1;
                match result {
                    Ok(value) if value["accountId"] == target.subject => {
                        this.computer.checkout.status = value["status"].clone()
                    }
                    Ok(_) => {}
                    Err(error) => this.computer.checkout.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Turn sync on (from the user's checkout here when Zuse knows it, else
    /// the engine clones) or off (the folder stays).
    fn set_checkout_sync(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let mut extra = json!({"enabled": enabled});
        if enabled && let Some(source) = self.local_checkout() {
            extra["sourcePath"] = json!(source);
        }
        self.change_checkout(methods::CLOUD_CHECKOUT_SYNC_CONFIGURE, extra, cx);
    }

    fn run_remedy(&mut self, remedy: Remedy, cx: &mut Context<Self>) {
        match remedy {
            Remedy::DiscardAndResume | Remedy::TryAgain => {
                self.change_checkout(methods::CLOUD_CHECKOUT_SYNC_RESUME, json!({}), cx)
            }
            Remedy::KeepEditing => self.set_checkout_sync(false, cx),
            Remedy::UpdateComputer => {
                if let Some(action) = self.update_computer_action() {
                    self.confirm_computer_action(action, cx);
                }
            }
        }
    }

    /// The small status beside the header's cloud icon while this chat is
    /// synced into its folder here.
    pub(super) fn render_checkout_badge(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.watch_checkout(cx);
        let status = &self.computer.checkout.status;
        if status["enabled"] != true && status["state"] != "restoring" {
            return None;
        }
        let theme = Theme::of(cx).clone();
        let path = status["localPath"].as_str().unwrap_or_default().to_owned();
        let (label, tone) = phase(status);
        let tip = match status["error"].as_str() {
            Some(why) => format!("{label}: {why}"),
            None => format!("{label} · {path}"),
        };
        Some(
            div()
                .id("checkout-sync-badge")
                .h(px(22.))
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(5.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(crate::typography::text_xs())
                .text_color(theme.text_muted)
                .hover(|el| el.bg(theme.element_hover))
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(format!("Synced to {path}: {label}")))
                .tip(tip)
                .child(
                    div()
                        .flex_none()
                        .size(px(6.))
                        .rounded_full()
                        .bg(tone.color(&theme)),
                )
                .child(div().max_w(px(120.)).truncate().child(folder_name(&path)))
                .on_click(cx.listener(|this, _, _, cx| {
                    if !this.computer.open {
                        this.toggle_computer(cx);
                    }
                }))
                .into_any_element(),
        )
    }

    /// The folder's split button: its app and path open it there; the
    /// chevron lists every app.
    fn render_checkout_folder(
        &mut self,
        path: String,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let app = self.folder_app(cx);
        let glyph = match &app {
            Some(app) => crate::shell::open_in::app_icon(
                app.id,
                app.icon.as_ref(),
                crate::shell::open_in::fallback_glyph(app.id),
                16.,
                theme,
            ),
            None => icon(icons::FOLDER)
                .size(px(14.))
                .flex_none()
                .text_color(theme.text_muted)
                .into_any_element(),
        };
        let home = std::env::var("HOME").ok();
        let shown = open_menu::middle_truncate(
            &open_menu::tilde(&path, home.as_deref()),
            FOLDER_PATH_CHARS,
        );
        let tip = match &app {
            Some(app) => format!("Open in {} · {path}", app.label),
            None => format!("Open folder · {path}"),
        };
        let menu = self.render_nested(Kind::Folder, theme, cx);
        let open = self.nested_open(Kind::Folder);
        split_field(theme)
            .child(
                split_main(theme, "checkout-sync-open")
                    .role(gpui::Role::Button)
                    .aria_label(SharedString::from(tip.clone()))
                    .tip(tip)
                    .child(glyph)
                    .child(div().min_w_0().truncate().child(shown))
                    .on_click(cx.listener(move |_, _, _, cx| match app.clone() {
                        Some(app) => cx.emit(CloudConversationEvent::OpenPathIn(app, path.clone())),
                        None => cx.reveal_path(std::path::Path::new(&path)),
                    })),
            )
            .child(
                nested_chevron(theme, "checkout-sync-open-more", Kind::Folder, open, cx)
                    .aria_label("Open the folder in…")
                    .when(!open, |chevron| chevron.tip("Open in…"))
                    .children(menu),
            )
            .into_any_element()
    }

    /// The computer menu's rows: the switch, then its status, the folder,
    /// and what can be done about a pause or failure.
    pub(super) fn render_checkout_rows(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        self.watch_checkout(cx);
        let checkout = &self.computer.checkout;
        let status = checkout.status.clone();
        let loaded = status["enabled"].is_boolean();
        let enabled = status["enabled"] == true;
        let busy = checkout.change.is_some();
        let request_failed = checkout.error.is_some() || checkout.read_error.is_some();
        let shape = rows(&status);
        let mut out = Vec::new();
        out.push(
            div()
                .id("checkout-sync-toggle")
                .mt(px(4.))
                .px(px(8.))
                .h(px(28.))
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .role(gpui::Role::Switch)
                .aria_label("Sync to a local folder")
                .aria_toggled(if enabled {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .tip("This chat gets its own folder on this computer, kept in sync with the cloud computer. Your own checkout is never touched.")
                .opacity(if busy || !loaded { 0.5 } else { 1. })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child("Sync to a local folder"),
                )
                .child(crate::settings::widgets::toggle_switch(
                    theme,
                    enabled,
                    "checkout-sync",
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !busy && loaded {
                        this.set_checkout_sync(!enabled, cx);
                    }
                }))
                .into_any_element(),
        );
        if shape.status {
            let (label, tone) = phase(&status);
            out.push(
                div()
                    .px(px(8.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_color(theme.text_muted)
                    .child(
                        div()
                            .flex_none()
                            .size(px(6.))
                            .rounded_full()
                            .bg(tone.color(theme)),
                    )
                    .child(div().min_w_0().truncate().child(if busy {
                        "Updating…"
                    } else {
                        label
                    }))
                    .into_any_element(),
            );
        }
        if shape.folder
            && let Some(path) = self.checkout_folder()
        {
            out.push(self.render_checkout_folder(path, theme, cx));
        }
        for remedy in shape.remedies {
            if remedy == Remedy::UpdateComputer && self.update_computer_action().is_none() {
                // Nothing this menu can run (no permission, or a recreation
                // already underway): the message below says what's needed.
                continue;
            }
            let available = !busy;
            let id = format!("checkout-sync-{remedy:?}");
            out.push(
                popover::menu_row(theme, false, id.clone())
                    .id(SharedString::from(id))
                    .role(gpui::Role::Button)
                    .opacity(if available { 1. } else { 0.5 })
                    .text_size(crate::typography::text_xs())
                    .when_some(remedy.tip(), |row, tip| row.tip(tip))
                    .child(
                        icon(remedy.glyph())
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .child(remedy.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if available {
                            this.run_remedy(remedy, cx);
                        }
                    }))
                    .into_any_element(),
            );
        }
        let checkout = &self.computer.checkout;
        let update = status["state"] == "error" && status["updateRequired"] == true;
        let problem = checkout
            .error
            .clone()
            .or_else(|| checkout.read_error.clone())
            .or_else(|| update.then(|| UPDATE_REQUIRED.to_owned()))
            .or_else(|| status["error"].as_str().map(str::to_owned));
        if let Some(problem) = problem {
            out.push(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_color(if quiet(&status, request_failed) {
                        theme.text_muted
                    } else {
                        theme.danger
                    })
                    .child(problem)
                    .into_any_element(),
            );
        }
        if let Some(notice) = status["notice"].as_str() {
            out.push(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_color(theme.text_muted)
                    .child(notice.to_owned())
                    .into_any_element(),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_state_reads_as_a_short_label_with_its_tone() {
        for (state, label, tone) in [
            ("synced", "In sync", Tone::Running),
            ("syncing", "Syncing…", Tone::Busy),
            ("preparing", "Preparing the folder…", Tone::Busy),
            ("behind", "Waiting for the cloud computer", Tone::Idle),
            ("paused", "Paused: edited in the folder", Tone::Busy),
            ("restoring", "Putting your checkout back…", Tone::Busy),
            ("error", "Sync failed", Tone::Failed),
            ("off", "Off", Tone::Idle),
        ] {
            assert_eq!(phase(&json!({"state":state})), (label, tone), "{state}");
        }
        assert_eq!(phase(&Value::Null), ("Off", Tone::Idle));
        assert_eq!(
            phase(&json!({"state":"error","updateRequired":true})),
            ("Needs an update", Tone::Failed)
        );
    }

    #[test]
    fn each_state_shows_its_rows() {
        let folder = "/Users/alex/zuse/cloud/opcodehq-zuse/8f3k2c1a";
        let shape = |status: Value| rows(&status);
        // Off with nothing kept: the switch alone.
        assert_eq!(
            shape(json!({"enabled":false,"state":"off","localPath":null})),
            Rows {
                status: false,
                folder: false,
                remedies: vec![]
            }
        );
        // Off with the folder kept: the folder, no status line.
        assert_eq!(
            shape(json!({"enabled":false,"state":"off","localPath":folder})),
            Rows {
                status: false,
                folder: true,
                remedies: vec![]
            }
        );
        assert_eq!(
            shape(json!({"enabled":true,"state":"synced","localPath":folder})),
            Rows {
                status: true,
                folder: true,
                remedies: vec![]
            }
        );
        assert_eq!(
            shape(json!({"enabled":true,"state":"paused","localPath":folder})).remedies,
            [Remedy::DiscardAndResume, Remedy::KeepEditing]
        );
        assert_eq!(
            shape(json!({"enabled":true,"state":"error","updateRequired":true})).remedies,
            [Remedy::UpdateComputer]
        );
        assert_eq!(
            shape(json!({"enabled":true,"state":"error","updateRequired":false})).remedies,
            [Remedy::TryAgain]
        );
        // An old checkout going back shows its status even though sync is off.
        let restoring =
            shape(json!({"enabled":false,"state":"restoring","localPath":"/Users/alex/code/web"}));
        assert!(restoring.status);
        assert_eq!(restoring.remedies, [Remedy::TryAgain]);
    }

    #[test]
    fn remedies_say_what_they_do() {
        assert_eq!(
            Remedy::DiscardAndResume.label(),
            "Discard my edits and resume"
        );
        assert_eq!(Remedy::KeepEditing.label(), "Keep editing (stop sync)");
        assert_eq!(Remedy::TryAgain.label(), "Try again");
    }

    #[test]
    fn the_chats_repository_finds_its_checkout_in_any_spelling() {
        let checkouts = vec![
            (
                "/Users/alex/code/site".to_owned(),
                "github.com/example/site".to_owned(),
            ),
            (
                "/Users/alex/code/web".to_owned(),
                "git@github.com:Example/Web.git".to_owned(),
            ),
        ];
        assert_eq!(
            matching_checkout(&checkouts, "github.com/example/web").as_deref(),
            Some("/Users/alex/code/web")
        );
        assert_eq!(
            matching_checkout(&checkouts, "github.com/example/other"),
            None
        );
    }

    #[test]
    fn only_failures_read_in_red() {
        assert!(quiet(&json!({"state":"paused"}), false));
        assert!(quiet(&json!({"state":"behind"}), false));
        assert!(quiet(
            &json!({"state":"error","updateRequired":true}),
            false
        ));
        assert!(!quiet(&json!({"state":"error"}), false));
        assert!(!quiet(&json!({"state":"paused"}), true));
    }

    #[test]
    fn the_header_names_the_folder() {
        assert_eq!(
            folder_name("/Users/alex/zuse/cloud/web/8f3k2c1a"),
            "8f3k2c1a"
        );
        assert_eq!(folder_name("/"), "/");
    }
}
