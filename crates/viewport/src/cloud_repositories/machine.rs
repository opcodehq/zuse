//! The account's machine: what every cloud computer has before a repository
//! is checked out. Saved once, built into the account's image.
use super::*;
use zuse_protocol::cloud_computers::*;

pub(super) struct MachineView {
    version: Option<MachineVersion>,
    setup: Entity<TextInput>,
    instructions: Entity<TextInput>,
    pub error: Option<String>,
    pub(super) notice: Option<String>,
    /// Refreshes the status while a build is waiting or running and the
    /// account feed is disconnected.
    pub(super) poll: Option<Task<()>>,
    /// Fallback reads since the last change, for the backoff.
    polls: u32,
    /// "Configure with an agent" opened the repository picker because the
    /// account had no repository: start the setup chat once one is added.
    pub(super) setup_after_add: bool,
}

/// Asks the host to open a setup chat: a cloud chat on `repository` whose
/// agent configures the account's machine image.
#[derive(Clone, Debug)]
pub struct StartSetupChat {
    pub repository: super::FirstChatRepository,
}

impl StartSetupChat {
    pub(super) fn on(project: &CloudProject) -> Self {
        Self {
            repository: super::FirstChatRepository {
                project_id: project.project_id.clone(),
                repository_identity: project.repository_identity.clone(),
                repository_url: project.repository_url.clone(),
                display_name: project.display_name.clone(),
                default_branch: project.default_branch.clone(),
            },
        }
    }
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
            polls: 0,
            setup_after_add: false,
        }
    }
    pub(super) fn stop_polling(&mut self) {
        self.poll = None;
        self.polls = 0;
    }
    fn waiting(&self) -> bool {
        self.version
            .as_ref()
            .is_some_and(|v| matches!(v.image_state.as_str(), "queued" | "building"))
    }
    pub fn reset(&mut self, cx: &mut Context<RepositoryPanel>) {
        self.version = None;
        self.error = None;
        self.notice = None;
        self.stop_polling();
        for input in [&self.setup, &self.instructions] {
            input.update(cx, |input, cx| input.set_value(String::new(), cx));
        }
    }
    pub(super) fn version(&self) -> Option<&MachineVersion> {
        self.version.as_ref()
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
            "Standard machine",
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
    /// "Configure with an agent": a setup chat on the account's first
    /// repository. A chat needs one, so without any the picker opens first
    /// and the chat starts once a repository is added.
    pub(super) fn configure_with_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_manage() {
            return;
        }
        if let Some(project) = self.projects.first() {
            cx.emit(StartSetupChat::on(project));
            return;
        }
        if !self.github_connected() {
            cx.emit(OpenPage(CloudPage::Repositories));
            return;
        }
        self.machine.setup_after_add = true;
        self.open_palette(window, cx);
    }
    /// The button: dithered, like every primary action here.
    fn configure_button(&self, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        use crate::cloud_settings::kit;
        let enabled = self.can_manage() && !busy;
        kit::primary_button(
            "cloud-machine-configure-agent",
            "Configure with an agent",
            enabled,
        )
        .when(enabled, |b| {
            b.on_click(cx.listener(|this, _, window, cx| this.configure_with_agent(window, cx)))
        })
        .into_any_element()
    }
    pub(super) fn rebuild_machine(&mut self, cx: &mut Context<Self>) {
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
                // A status read (a build hint, the image backstop) never
                // replaces what someone is typing; a save shows the saved text.
                let editing = !saved && self.machine_dirty(cx);
                if !editing {
                    self.machine.setup.update(cx, |input, cx| {
                        input.set_value(version.recipe.setup_script.clone(), cx)
                    });
                    self.machine.instructions.update(cx, |input, cx| {
                        input.set_value(version.recipe.instructions.clone(), cx)
                    });
                }
                if saved {
                    self.machine.notice = Some(saved_notice(&version.recipe).into());
                }
                let changed = self.machine.version.as_ref().is_none_or(|v| {
                    v.image_state != version.image_state || v.revision != version.revision
                });
                if changed {
                    self.machine.polls = 0;
                }
                self.machine.version = Some(version);
                self.machine.error = None;
                self.machine.poll = None;
                self.schedule_machine_poll(cx);
                self.schedule_image_check(cx);
            }
            Err(_) => {
                self.machine.error =
                    Some("Machine settings could not be verified. Reload before saving.".into())
            }
        }
        cx.notify();
    }
    /// Polls a waiting build only while the account feed is disconnected.
    pub(super) fn schedule_machine_poll(&mut self, cx: &mut Context<Self>) {
        if self.account_feed_live || !self.machine.waiting() {
            self.machine.stop_polling();
            return;
        }
        if self.machine.poll.is_some() {
            return;
        }
        let delay = fallback_delay(std::time::Duration::from_secs(10), self.machine.polls);
        self.machine.polls = self.machine.polls.saturating_add(1);
        self.machine.poll = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                this.machine.poll = None;
                if !this.pending.values().any(|p| p.kind.starts_with("machine")) {
                    this.load_machine(cx);
                }
            });
        }));
    }
    pub(super) fn save_machine(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn machine_dirty(&self, cx: &App) -> bool {
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
    /// The machine's size: the default every plan starts chats on, and the
    /// larger Cloud Max size. Choosing a size per chat isn't offered yet, so
    /// the larger option shows what unlocks it instead of a control.
    fn machine_size(&self) -> Div {
        use super::plan::{DEFAULT_MACHINE, LARGE_MACHINE, SLEEP_AFTER_MINUTES, Tier};
        use crate::cloud_settings::kit::{self, Choice, Tone};
        let tier = self.plan_tier.map(|(tier, _)| tier);
        let large_note: SharedString = match tier {
            Some(Tier::CloudMax) => "Coming soon".into(),
            _ => "Cloud Max".into(),
        };
        let cards = kit::choice_cards(vec![
            Choice {
                title: DEFAULT_MACHINE.into(),
                detail: "Default".into(),
                selected: true,
                locked: None,
            },
            Choice {
                title: LARGE_MACHINE.into(),
                detail: "Uses minutes twice as fast".into(),
                selected: false,
                locked: Some(large_note),
            },
        ]);
        let help = match tier {
            Some(Tier::CloudMax) => {
                "Your plan includes machines up to 8 vCPU · 32 GB. Choosing a larger size for a chat is coming soon."
            }
            _ => {
                "Cloud Max runs machines up to 8 vCPU · 32 GB. Change plans in Plan & usage → Billing."
            }
        };
        kit::section(
            "Your cloud computer",
            help,
            None,
            column().child(div().p(px(kit::INSET)).child(cards)).child(
                s::row(false)
                    .flex_nowrap()
                    .child(kit::icon_tile("clock-circle", 28.))
                    .child(div().flex_1().child(kit::title_with_tip(
                        "Sleeps when quiet",
                        "A sleeping computer uses no minutes and wakes as soon as you reply.",
                    )))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(palette::rgb(MUTED))
                            .child(format!("After {SLEEP_AFTER_MINUTES} min")),
                    )
                    .child(kit::pill("Automatic", Tone::Neutral)),
            ),
        )
    }

    fn machine_image_card(
        &self,
        version: &MachineVersion,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        use super::form;
        use crate::cloud_settings::kit;
        let (label, detail, _) = image_copy(&version.image_state);
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
            // Ready and standard images need no sentence; the pill says it.
            ("ready" | "none", _) => SharedString::default(),
            _ => detail.into(),
        };
        let built = match build {
            Some(b) if b.state == "ready" => b
                .built_at
                .map(|at| format!("Version {} · built {}", b.recipe_revision, day(at)))
                .unwrap_or_default(),
            _ => format!("Version {}", version.revision),
        };
        let mut body = column().child(kit::row_item(
            true,
            Some(kit::icon_tile("dither-monitor", 28.).into_any_element()),
            "Machine image",
            detail,
            Some(
                row()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(palette::rgb(MUTED))
                            .child(built),
                    )
                    .child(kit::pill(label, image_tone(&version.image_state)))
                    .into_any_element(),
            ),
        ));
        if version.image_state == "building" {
            body = body.child(div().px(px(kit::INSET)).pb(px(12.)).child(kit::meter(
                build_fraction(build.and_then(|b| b.phase.as_deref()).unwrap_or_default()),
                6.,
            )));
        }
        if let Some(error) = version.image_error.as_deref() {
            body = body.child(
                div()
                    .px(px(kit::INSET))
                    .pb(px(12.))
                    .child(form::message(error_copy(error), true)),
            );
        }
        if let Some(log) = build
            .and_then(|b| b.log_excerpt.clone())
            .filter(|l| !l.is_empty())
        {
            body = body.child(
                div()
                    .mx(px(kit::INSET))
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
            let line = match good.built_at {
                Some(at) => format!(
                    "New computers use your last working image: version {}, built {}.",
                    good.recipe_revision,
                    day(at)
                ),
                None => "New computers use your last working image.".into(),
            };
            body = body.child(div().px(px(kit::INSET)).pb(px(12.)).child(kit::help(line)));
        }
        // It builds by itself; a manual rebuild stays in the "…" menu.
        let rebuild = (version.image_state != "none" && self.can_manage()).then(|| {
            self.overflow_menu(
                "cloud-machine-more".into(),
                "More image actions".into(),
                busy || version.image_state == "building",
                vec![super::presentation::MenuItem {
                    id: "cloud-machine-rebuild".into(),
                    glyph: "refresh",
                    label: "Rebuild now".into(),
                    danger: false,
                    run: std::rc::Rc::new(
                        |this: &mut RepositoryPanel,
                         _: &mut Window,
                         cx: &mut Context<RepositoryPanel>| {
                            this.rebuild_machine(cx)
                        },
                    ),
                }],
                cx,
            )
        });
        let actions = row()
            .items_center()
            .gap(px(8.))
            .child(self.configure_button(busy, cx))
            .children(rebuild)
            .into_any_element();
        kit::section(
            "Image",
            "Your setup script and repositories, built into the image every new computer boots from. It rebuilds by itself when they change. Configure with an agent to have one install tools and pick repositories for you.",
            Some(actions),
            body,
        )
    }

    /// The setup script alone, with its save bar: the onboarding step.
    pub(super) fn render_machine_setup(&self, _cx: &mut Context<Self>) -> Div {
        use super::form;
        use crate::cloud_settings::kit;
        let busy = self.pending.values().any(|p| p.kind.starts_with("machine"));
        let mut root = column().gap(px(kit::SECTION_GAP));
        if self.machine.version.is_none() {
            return root.child(kit::section(
                "Setup script",
                if busy {
                    "Loading your machine…"
                } else {
                    "Your machine couldn't be loaded. Skip this step and add a script later from Settings → Machine."
                },
                None,
                div(),
            ));
        }
        root = root.child(kit::section(
            "Setup script",
            "Runs once with sudo when your image builds: apt packages, runtimes, global CLIs. Leave it empty if your repositories need nothing extra.",
            None,
            div()
                .p(px(kit::INSET))
                .child(form::script_box(self.machine.setup.clone())),
        ));
        if let Some(notice) = &self.machine.notice {
            root = root.child(form::message(notice, false));
        }
        if let Some(error) = &self.machine.error {
            root = root.child(form::message(error, true));
        }
        // Onboarding's Continue saves these edits ("Save and continue").
        let _ = busy;
        root
    }

    pub(super) fn render_machine(&self, cx: &mut Context<Self>) -> Div {
        use super::form;
        use crate::cloud_settings::kit;
        let busy = self.pending.values().any(|p| p.kind.starts_with("machine"));
        let mut root = column()
            .gap(px(kit::SECTION_GAP))
            .child(self.machine_size());
        let Some(version) = &self.machine.version else {
            root = root.child(kit::section(
                "Image",
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
        root = root
            .child(self.machine_image_card(version, busy, cx))
            .child(kit::section(
                "Setup script",
                "Runs once with sudo when your image is built. Install what every repository needs: apt packages, runtimes, global CLIs.",
                None,
                div().p(px(kit::INSET)).child(form::script_box(self.machine.setup.clone())),
            ))
            .child(kit::section(
                "Instructions",
                "Given to every agent in every new chat, alongside the repository's own AGENTS.md. Applies as soon as you save; no image build needed.",
                None,
                div().p(px(kit::INSET)).child(form::script_box(self.machine.instructions.clone())),
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
                    s::row(i == 0)
                        .child(kit::pill(kind, kit::Tone::Neutral))
                        .child(form::mono(name)),
                );
            }
            root = root.child(kit::section(
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
            self.viewport,
            self.machine_dirty(cx) && self.can_manage(),
            busy,
            |this, cx| this.discard_machine(cx),
            |this, cx| this.save_machine(cx),
            cx,
        ))
    }
}

/// The pill tone for a machine image state.
pub(super) fn image_tone(state: &str) -> crate::cloud_settings::kit::Tone {
    use crate::cloud_settings::kit::Tone;
    match state {
        "ready" => Tone::Ready,
        "queued" | "building" => Tone::Progress,
        "failed" => Tone::Error,
        "none" => Tone::Neutral,
        _ => Tone::Attention,
    }
}

/// How far along a running build is, by phase, for its dithered meter.
pub(super) fn build_fraction(phase: &str) -> f32 {
    match phase {
        "allocate" => 0.1,
        "setup" => 0.35,
        "sanitize" => 0.6,
        "capture" | "retire-builder" => 0.75,
        "verify" | "retire-verifier" => 0.88,
        "publish" => 0.96,
        _ => 0.05,
    }
}

#[cfg(test)]
mod tests {
    use super::{build_fraction, image_copy, image_tone, saved_notice};
    use crate::cloud_settings::kit::Tone;

    #[test]
    fn image_states_have_tones_and_builds_move_forward() {
        assert_eq!(image_tone("ready"), Tone::Ready);
        assert_eq!(image_tone("building"), Tone::Progress);
        assert_eq!(image_tone("failed"), Tone::Error);
        assert_eq!(image_tone("none"), Tone::Neutral);
        assert_eq!(image_copy("none").0, "Standard machine");
        let phases = [
            "allocate", "setup", "sanitize", "capture", "verify", "publish",
        ];
        let fractions: Vec<f32> = phases.iter().map(|p| build_fraction(p)).collect();
        assert!(fractions.windows(2).all(|w| w[0] < w[1]));
        assert!(fractions.iter().all(|f| (0.0..1.0).contains(f)));
    }
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
