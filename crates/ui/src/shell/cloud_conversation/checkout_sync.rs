//! This chat's files in the user's own checkout of the repository here: the
//! switch in the computer menu, its status there and beside the header's
//! cloud icon. The local engine does the work and keeps it safe.
use super::computer::{Tone, split_field, split_main};
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
    folder: Option<Task<()>>,
    error: Option<String>,
}

/// How a status reads, with its dot.
pub(super) fn phase(status: &Value) -> (&'static str, Tone) {
    match status["state"].as_str() {
        Some("synced") => ("In sync", Tone::Running),
        Some("syncing") => ("Syncing…", Tone::Busy),
        Some("preparing") => ("Preparing…", Tone::Busy),
        Some("behind") => ("Waiting for the cloud computer", Tone::Idle),
        Some("paused") => ("Paused: edited here", Tone::Busy),
        Some("restoring") => ("Putting your folder back…", Tone::Busy),
        Some("error") => ("Sync failed", Tone::Failed),
        _ => ("Off", Tone::Idle),
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

/// The last part of a folder path, as the header shows it.
fn folder_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

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

    /// This repository's checkout on this computer, when Zuse knows one.
    pub(super) fn local_checkout(&self) -> Option<String> {
        let identity = self.computer.metadata["repositoryIdentity"]
            .as_str()
            .or_else(|| self.config["repositoryIdentity"].as_str())
            .or_else(|| self.computer.metadata["repositoryUrl"].as_str())
            .or_else(|| self.config["repositoryUrl"].as_str())?;
        matching_checkout(&self.computer.checkout.local, identity)
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
                let Ok(Some((body, idle, often))) = this.update(cx, |this, cx| {
                    if !this.authorized {
                        return None;
                    }
                    let body = this.checkout_body(cx)?;
                    let often =
                        this.computer.open || this.computer.checkout.status["enabled"] == true;
                    Some((body, this.computer.checkout.change.is_none(), often))
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
                        if this.computer.checkout.change.is_some() {
                            return;
                        }
                        if let Ok(value) = result
                            && value["accountId"] == target.subject
                            && this.computer.checkout.status != value["status"]
                        {
                            this.computer.checkout.status = value["status"].clone();
                            cx.notify();
                        }
                    });
                }
                let wait = if often { 2 } else { 15 };
                cx.background_executor()
                    .timer(Duration::from_secs(wait))
                    .await;
            }
        }));
    }

    fn change_checkout(
        &mut self,
        method: &'static str,
        enabled: bool,
        path: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.computer.checkout.change.is_some() || !self.authorized {
            return;
        }
        let Some(mut body) = self.checkout_body(cx) else {
            return;
        };
        body["enabled"] = json!(enabled);
        if let Some(path) = path {
            body["localPath"] = json!(path);
        }
        let target = self.target.clone();
        self.computer.checkout.error = None;
        self.computer.checkout.change = Some(cx.spawn(async move |this, cx| {
            let result = target.controller.client().call_scoped(method, body).await;
            let _ = this.update(cx, |this, cx| {
                this.computer.checkout.change = None;
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

    fn choose_checkout_folder(&mut self, cx: &mut Context<Self>) {
        if self.computer.checkout.change.is_some() || self.computer.checkout.folder.is_some() {
            return;
        }
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose Checkout".into()),
        });
        self.computer.checkout.folder = Some(cx.spawn(async move |this, cx| {
            let result = receiver.await;
            let _ = this.update(cx, |this, cx| {
                this.computer.checkout.folder = None;
                if let Ok(Ok(Some(paths))) = result
                    && let Some(path) = paths.first()
                {
                    this.change_checkout(
                        methods::CLOUD_CHECKOUT_SYNC_CONFIGURE,
                        true,
                        Some(path.to_string_lossy().into_owned()),
                        cx,
                    );
                }
                cx.notify();
            });
        }));
    }

    /// The small status beside the header's cloud icon while this chat is
    /// synced into a checkout here.
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

    /// The computer menu's rows: the switch, then while on its status, the
    /// folder and what can be done about a pause or failure.
    pub(super) fn render_checkout_rows(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        self.watch_checkout(cx);
        let checkout = &self.computer.checkout;
        let status = &checkout.status;
        let loaded = status["enabled"].is_boolean();
        let enabled = status["enabled"] == true;
        let busy = checkout.change.is_some() || checkout.folder.is_some();
        let known = self.local_checkout();
        let folder = status["localPath"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| known.clone());
        let mut rows = Vec::new();
        let known_path = known.clone();
        rows.push(
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
                .aria_label("Sync into your checkout")
                .aria_toggled(if enabled {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .tip("Puts this chat's changes in your own checkout here, so your dev server and builds use them. Turning it off puts the folder back exactly as it was.")
                .opacity(if busy || !loaded { 0.5 } else { 1. })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child("Sync into your checkout"),
                )
                .child(crate::settings::widgets::toggle_switch(
                    theme,
                    enabled,
                    "checkout-sync",
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if busy || !loaded {
                        return;
                    }
                    if enabled {
                        this.change_checkout(
                            methods::CLOUD_CHECKOUT_SYNC_CONFIGURE,
                            false,
                            None,
                            cx,
                        );
                    } else if let Some(path) = known_path.clone() {
                        this.change_checkout(
                            methods::CLOUD_CHECKOUT_SYNC_CONFIGURE,
                            true,
                            Some(path),
                            cx,
                        );
                    } else {
                        this.choose_checkout_folder(cx);
                    }
                }))
                .into_any_element(),
        );
        let state = status["state"].as_str().unwrap_or("off");
        if enabled || state == "restoring" {
            let (label, tone) = phase(status);
            rows.push(
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
        if let Some(path) = folder.clone() {
            let reveal = path.clone();
            rows.push(
                split_field(theme)
                    .child(
                        split_main(theme, "checkout-sync-reveal")
                            .tip("Open folder")
                            .child(
                                icon(icons::FOLDER)
                                    .size(px(14.))
                                    .flex_none()
                                    .text_color(theme.text_muted),
                            )
                            .child(div().min_w_0().truncate().child(path))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.reveal_path(std::path::Path::new(&reveal))
                            })),
                    )
                    .into_any_element(),
            );
        }
        if !enabled && known.is_none() && state != "restoring" {
            rows.push(
                popover::menu_row(theme, false, "checkout-sync-folder")
                    .id("checkout-sync-folder")
                    .opacity(if busy { 0.5 } else { 1. })
                    .text_size(crate::typography::text_xs())
                    .child(
                        icon(icons::FOLDER)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .child("Choose checkout…")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !busy {
                            this.choose_checkout_folder(cx);
                        }
                    }))
                    .into_any_element(),
            );
        }
        let action = match state {
            "paused" => Some(("Keep my edits aside and resume", icons::REFRESH)),
            "error" | "restoring" => Some(("Try again", icons::REFRESH)),
            _ => None,
        };
        if let Some((label, glyph)) = action {
            rows.push(
                popover::menu_row(theme, false, "checkout-sync-resume")
                    .id("checkout-sync-resume")
                    .opacity(if busy { 0.5 } else { 1. })
                    .text_size(crate::typography::text_xs())
                    .when(state == "paused", |row| {
                        row.tip("Your edits go to the folder's stash list (git stash list), then sync continues.")
                    })
                    .child(icon(glyph).size(px(14.)).text_color(theme.text_muted))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !busy {
                            this.change_checkout(
                                methods::CLOUD_CHECKOUT_SYNC_RESUME,
                                true,
                                None,
                                cx,
                            );
                        }
                    }))
                    .into_any_element(),
            );
        }
        let problem = checkout
            .error
            .clone()
            .or_else(|| status["error"].as_str().map(str::to_owned));
        if let Some(problem) = problem {
            rows.push(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_color(if state == "behind" {
                        theme.text_muted
                    } else {
                        theme.danger
                    })
                    .child(problem)
                    .into_any_element(),
            );
        }
        if let Some(notice) = status["notice"].as_str() {
            rows.push(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_color(theme.text_muted)
                    .child(notice.to_owned())
                    .into_any_element(),
            );
        }
        rows
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
            ("preparing", "Preparing…", Tone::Busy),
            ("behind", "Waiting for the cloud computer", Tone::Idle),
            ("paused", "Paused: edited here", Tone::Busy),
            ("restoring", "Putting your folder back…", Tone::Busy),
            ("error", "Sync failed", Tone::Failed),
            ("off", "Off", Tone::Idle),
        ] {
            assert_eq!(phase(&json!({"state":state})), (label, tone), "{state}");
        }
        assert_eq!(phase(&Value::Null), ("Off", Tone::Idle));
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
    fn the_header_names_the_folder() {
        assert_eq!(folder_name("/Users/alex/code/web"), "web");
        assert_eq!(folder_name("/"), "/");
    }
}
