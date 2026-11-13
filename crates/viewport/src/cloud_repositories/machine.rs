//! The account's machine: what every cloud computer has before a repository
//! is checked out. Saved once, built into the account's image.
use super::*;
use zuse_protocol::cloud_computers::*;

pub(super) struct MachineView {
    version: Option<MachineVersion>,
    setup: Entity<TextInput>,
    instructions: Entity<TextInput>,
    pub error: Option<String>,
    notice: Option<String>,
    /// Refreshes the status while a build is waiting or running.
    poll: Option<Task<()>>,
}

impl MachineView {
    pub fn new(window: &mut Window, cx: &mut Context<RepositoryPanel>) -> Self {
        let setup = cx.new(|cx| {
            TextInput::script(
                "curl -fsSL https://bun.sh/install | bash\nnpm i -g pnpm\nsudo apt-get install -y ripgrep",
                window,
                cx,
            )
        });
        let instructions = cx.new(|cx| {
            TextInput::script(
                "How you like agents to work, in every repository. For example: keep commits small and run the tests you touched.",
                window,
                cx,
            )
            .proportional()
        });
        for input in [&setup, &instructions] {
            cx.observe(input, |_, _, cx| cx.notify()).detach();
        }
        Self {
            version: None,
            setup,
            instructions,
            error: None,
            notice: None,
            poll: None,
        }
    }
    pub fn reset(&mut self, cx: &mut Context<RepositoryPanel>) {
        self.version = None;
        self.error = None;
        self.notice = None;
        self.poll = None;
        for input in [&self.setup, &self.instructions] {
            input.update(cx, |input, cx| input.set_value(String::new(), cx));
        }
    }
    pub(super) fn image_state(&self) -> Option<&str> {
        self.version.as_ref().map(|v| v.image_state.as_str())
    }
    /// A customized machine the guide can mark as done.
    pub(super) fn customized(&self) -> bool {
        self.version.as_ref().is_some_and(|v| !v.recipe.is_empty())
    }
}

pub(super) fn image_copy(state: &str) -> (&'static str, &'static str, bool) {
    match state {
        "none" => (
            "Shared base",
            "Computers start from Zuse's standard machine.",
            false,
        ),
        "queued" => (
            "Waiting to build",
            "Your machine image will be built from these settings. Until then, computers use the standard machine.",
            true,
        ),
        "building" => (
            "Building",
            "Computers keep using your previous image until this build is verified.",
            true,
        ),
        "ready" => (
            "Ready",
            "New computers start from your machine image.",
            false,
        ),
        "failed" => (
            "Build failed",
            "Computers keep using your last working image. Fix the setup script and save again.",
            true,
        ),
        _ => ("Unknown", "", true),
    }
}

/// What a successful save changes, and when: instructions, skills and files
/// reach new chats right away; the setup script only runs in a built image.
pub(super) fn saved_notice(recipe: &MachineRecipe) -> &'static str {
    let delivered = recipe.delivery(0).is_some();
    let scripted = !recipe.setup_script.trim().is_empty();
    match (delivered, scripted) {
        (false, false) => "Saved. Computers use the standard machine.",
        (true, false) => "Saved. New chats get your instructions, skills and files.",
        (false, true) => "Saved. Your setup script runs once your machine image is built.",
        (true, true) => {
            "Saved. New chats get your instructions, skills and files now; your setup script runs once your machine image is built."
        }
    }
}

/// What a running build is doing now.
pub(super) fn phase_copy(phase: &str) -> &'static str {
    match phase {
        "allocate" => "Starting a build machine",
        "setup" => "Running your setup script",
        "sanitize" => "Removing credentials and history",
        "capture" | "retire-builder" => "Saving the image",
        "verify" | "retire-verifier" => "Checking the image boots",
        "publish" => "Publishing",
        _ => "Working",
    }
}

/// Owner-facing text for a build error code.
pub(super) fn error_copy(code: &str) -> &'static str {
    match code {
        "user_image_setup_failed" => "The setup script exited with an error. Its output is below.",
        "user_image_setup_timeout" => {
            "The setup script ran past its time limit. Make it faster or raise the limit."
        }
        "user_image_setup_interrupted" => {
            "The build machine stopped while the script ran. Rebuild to try again."
        }
        "user_image_runtime_files_changed" => {
            "The setup script changed Zuse's own runtime files. Leave /usr/local/bin/zuse-cloud-runtime and /usr/local/lib/zuse alone."
        }
        "user_image_runtime_started" => {
            "The setup script started Zuse's computer service. Don't start or restart zuse-runtime-supervisor."
        }
        "user_image_processes_running" => "Processes from the setup script would not stop.",
        "user_image_verification_failed" | "user_image_fresh_boot_rejected" => {
            "The built image didn't pass its boot check, so it was not used."
        }
        "user_image_build_deadline" => "The build took too long and was stopped.",
        "user_image_managed_base_retired" => {
            "Zuse's standard machine was updated during the build. A new build will start."
        }
        "user_image_managed_base_unavailable" | "user_image_provider_unavailable" => {
            "Waiting for build capacity."
        }
        "cloud_billing_period_missing" | "cloud_billing_hold" | "shared_credit_exhausted" => {
            "Builds use cloud compute. Check your plan to build this image."
        }
        _ => "The build didn't finish. Rebuild to try again.",
    }
}

fn day(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%b %-d, %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

impl RepositoryPanel {
    pub(super) fn load_machine(&mut self, cx: &mut Context<Self>) {
        self.call("machine-read", methods::CLOUD_MACHINE, json!({}), cx);
    }
    fn rebuild_machine(&mut self, cx: &mut Context<Self>) {
        if self.pending.values().any(|p| p.kind.starts_with("machine")) {
            return;
        }
        self.machine.error = None;
        self.machine.notice =
            Some("Rebuild queued. Running computers keep the image they started with.".into());
        self.call(
            "machine-rebuild",
            methods::CLOUD_MACHINE_REBUILD,
            json!({}),
            cx,
        );
        cx.notify();
    }
    pub(super) fn receive_machine(&mut self, data: Value, cx: &mut Context<Self>) {
        match serde_json::from_value::<MachineVersion>(data) {
            Ok(version) => {
                let saved = self
                    .machine
                    .version
                    .as_ref()
                    .is_some_and(|v| v.revision < version.revision);
                self.machine.setup.update(cx, |input, cx| {
                    input.set_value(version.recipe.setup_script.clone(), cx)
                });
                self.machine.instructions.update(cx, |input, cx| {
                    input.set_value(version.recipe.instructions.clone(), cx)
                });
                if saved {
                    self.machine.notice = Some(saved_notice(&version.recipe).into());
                }
                let waiting = matches!(version.image_state.as_str(), "queued" | "building");
                self.machine.version = Some(version);
                self.machine.error = None;
                self.machine.poll = waiting.then(|| {
                    cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(10))
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            this.machine.poll = None;
                            if !this.pending.values().any(|p| p.kind.starts_with("machine")) {
                                this.load_machine(cx);
                            }
                        });
                    })
                });
            }
            Err(_) => {
                self.machine.error =
                    Some("Machine settings could not be verified. Reload before saving.".into())
            }
        }
        cx.notify();
    }
    fn save_machine(&mut self, cx: &mut Context<Self>) {
        let Some(version) = self.machine.version.as_ref() else {
            return;
        };
        if self.pending.values().any(|p| p.kind.starts_with("machine")) {
            return;
        }
        let mut recipe = version.recipe.clone();
        recipe.setup_script = self.machine.setup.read(cx).value(cx);
        recipe.instructions = self.machine.instructions.read(cx).value(cx);
        if let Err(code) = recipe.validate() {
            self.machine.error = Some(match code {
                "machine_recipe_text_too_large" => {
                    "Keep the setup script and instructions under 64 KiB each.".into()
                }
                _ => "Check the machine settings and try again.".into(),
            });
            cx.notify();
            return;
        }
        self.machine.error = None;
        self.machine.notice = None;
        self.call(
            "machine-save",
            methods::CLOUD_MACHINE_SAVE,
            json!({"expectedRevision":version.revision,"recipe":recipe}),
            cx,
        );
        cx.notify();
    }
    fn machine_dirty(&self, cx: &App) -> bool {
        self.machine.version.as_ref().is_some_and(|v| {
            self.machine.setup.read(cx).value(cx).trim() != v.recipe.setup_script.trim()
                || self.machine.instructions.read(cx).value(cx).trim()
                    != v.recipe.instructions.trim()
        })
    }
    fn discard_machine(&mut self, cx: &mut Context<Self>) {
        let Some(version) = self.machine.version.clone() else {
            return;
        };
        self.machine.setup.update(cx, |input, cx| {
            input.set_value(version.recipe.setup_script.clone(), cx)
        });
        self.machine.instructions.update(cx, |input, cx| {
            input.set_value(version.recipe.instructions.clone(), cx)
        });
        self.machine.error = None;
        self.machine.notice = None;
        cx.notify();
    }
    pub(super) fn render_machine(&self, cx: &mut Context<Self>) -> Div {
        use super::form;
        let busy = self.pending.values().any(|p| p.kind.starts_with("machine"));
        let mut root = column().gap(px(16.));
        let Some(version) = &self.machine.version else {
            root = root.child(form::section(
                "Machine image",
                if busy {
                    "Loading your machine…"
                } else {
                    "Your machine couldn't be loaded. Refresh to try again."
                },
                None,
                div(),
            ));
            if let Some(error) = &self.machine.error {
                root = root.child(form::message(error, true));
            }
            return root;
        };
        let (label, detail, warning) = image_copy(&version.image_state);
        let build = version.image.as_ref();
        let detail: SharedString = match (version.image_state.as_str(), build) {
            ("building", Some(b)) => format!(
                "{}. Computers keep using your previous image until this build is verified.",
                phase_copy(b.phase.as_deref().unwrap_or_default())
            )
            .into(),
            ("queued", Some(b)) if b.error.is_some() => {
                error_copy(b.error.as_deref().unwrap_or_default()).into()
            }
            _ => detail.into(),
        };
        let built = match build {
            Some(b) if b.state == "ready" => b
                .built_at
                .map(|at| format!("Version {} · built {}", b.recipe_revision, day(at)))
                .unwrap_or_default(),
            _ => format!("Version {}", version.revision),
        };
        let status = row()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .py(px(12.))
            .child(
                div()
                    .flex_none()
                    .size(px(32.))
                    .rounded(px(8.))
                    .bg(s::frame_bg())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("cloud").size(px(16.)).text_color(palette::rgb(TEXT))),
            )
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        row()
                            .items_center()
                            .gap(px(8.))
                            .child(s::title_text("Your machine image"))
                            .child(cs::badge(
                                label,
                                warning,
                                !warning && version.image_state == "none",
                            )),
                    )
                    .child(s::description_text(detail)),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(palette::rgb(MUTED))
                    .child(built),
            );
        let mut image = column().child(status);
        if let Some(error) = version.image_error.as_deref() {
            image = image.child(
                div()
                    .px(px(12.))
                    .pb(px(12.))
                    .child(form::message(error_copy(error), true)),
            );
        }
        if let Some(log) = build
            .and_then(|b| b.log_excerpt.clone())
            .filter(|l| !l.is_empty())
        {
            image = image.child(
                div()
                    .mx(px(12.))
                    .mb(px(12.))
                    .p(px(10.))
                    .max_h(px(180.))
                    .overflow_hidden()
                    .rounded(px(6.))
                    .bg(s::frame_bg())
                    .child(form::mono(log)),
            );
        }
        if let Some(good) = &version.last_good_image {
            let line = match (good.built_at, good.base_release_id.as_deref()) {
                (Some(at), Some(base)) => format!(
                    "New computers use your last working image: version {}, built {}, on {base}.",
                    good.recipe_revision,
                    day(at)
                ),
                (Some(at), None) => format!(
                    "New computers use your last working image, built {}.",
                    day(at)
                ),
                _ => "New computers use your last working image.".into(),
            };
            image = image.child(
                div()
                    .px(px(12.))
                    .pb(px(12.))
                    .child(s::description_text(line)),
            );
        } else if let Some(base) = build
            .filter(|b| b.state == "ready")
            .and_then(|b| b.base_release_id.clone())
        {
            image = image.child(
                div()
                    .px(px(12.))
                    .pb(px(12.))
                    .child(s::description_text(format!("Built on {base}."))),
            );
        }
        if version.image_state != "none" && self.can_manage() {
            image = image.child(
                row().px(px(12.)).pb(px(12.)).child(
                    cs::button(
                        "cloud-machine-rebuild",
                        "Rebuild now",
                        false,
                        busy || version.image_state == "building",
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.rebuild_machine(cx))),
                ),
            );
        }
        root = root
            .child(s::frame().child(s::card().child(image)))
            .child(form::section(
                "Setup script",
                "Runs once with sudo when your image is built. Install what every repository needs: apt packages, runtimes, global CLIs.",
                None,
                div().p(px(12.)).child(form::script_box(self.machine.setup.clone())),
            ))
            .child(form::section(
                "Instructions",
                "Given to every agent in every new chat, alongside the repository's own AGENTS.md. Applies as soon as you save; no image build needed.",
                None,
                div().p(px(12.)).child(form::script_box(self.machine.instructions.clone())),
            ));
        let recipe = &version.recipe;
        if !recipe.skills.is_empty() || !recipe.files.is_empty() {
            let mut rows = column();
            let items = recipe
                .skills
                .iter()
                .map(|k| ("Skill", k.name.clone()))
                .chain(recipe.files.iter().map(|f| ("File", f.path.clone())));
            for (i, (kind, name)) in items.enumerate() {
                rows = rows.child(
                    row()
                        .items_center()
                        .gap(px(10.))
                        .px(px(12.))
                        .py(px(9.))
                        .when(i > 0, |v| v.border_t_1().border_color(palette::rgb(BORDER)))
                        .child(cs::badge(kind, false, true))
                        .child(form::mono(name)),
                );
            }
            root = root.child(form::section(
                "Also on your machine",
                "Imported skills and files, copied into every new chat's computer as soon as you save.",
                None,
                rows,
            ));
        }
        if let Some(notice) = &self.machine.notice {
            root = root.child(form::message(notice, false));
        }
        if let Some(error) = &self.machine.error {
            root = root.child(form::message(error, true));
        }
        root.child(form::save_bar(
            "cloud-machine",
            "Instructions, skills and files apply to new chats. Setup script changes wait for the image build.".into(),
            self.machine_dirty(cx) && self.can_manage(),
            busy,
            |this, cx| this.discard_machine(cx),
            |this, cx| this.save_machine(cx),
            cx,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::saved_notice;
    use zuse_protocol::cloud_computers::MachineRecipe;

    #[test]
    fn saved_notice_says_what_applies_now_and_what_waits_for_the_image() {
        let mut recipe = MachineRecipe::default();
        assert_eq!(
            saved_notice(&recipe),
            "Saved. Computers use the standard machine."
        );
        recipe.instructions = "Keep commits small.".into();
        assert!(saved_notice(&recipe).contains("New chats get"));
        assert!(!saved_notice(&recipe).contains("image"));
        recipe.setup_script = "npm i -g pnpm".into();
        assert!(saved_notice(&recipe).contains("New chats get"));
        assert!(saved_notice(&recipe).contains("once your machine image is built"));
        recipe.instructions.clear();
        assert!(!saved_notice(&recipe).contains("New chats"));
    }
}
