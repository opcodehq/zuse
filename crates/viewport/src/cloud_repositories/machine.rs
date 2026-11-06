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
}

impl MachineView {
    pub fn new(window: &mut Window, cx: &mut Context<RepositoryPanel>) -> Self {
        let setup = cx.new(|cx| {
            TextInput::multiline(
                "For example: curl -fsSL https://bun.sh/install | bash && npm i -g pnpm",
                window,
                cx,
            )
        });
        let instructions = cx.new(|cx| {
            TextInput::multiline(
                "How you like agents to work, in every repository",
                window,
                cx,
            )
        });
        for input in [&setup, &instructions] {
            cx.observe(input, |_, _, cx| cx.notify()).detach();
        }
        Self { version: None, setup, instructions, error: None, notice: None }
    }
    pub fn reset(&mut self, cx: &mut Context<RepositoryPanel>) {
        self.version = None;
        self.error = None;
        self.notice = None;
        for input in [&self.setup, &self.instructions] {
            input.update(cx, |input, cx| input.set_value(String::new(), cx));
        }
    }
    /// A customized machine the guide can mark as done.
    pub(super) fn customized(&self) -> bool {
        self.version.as_ref().is_some_and(|v| !v.recipe.is_empty())
    }
}

fn image_copy(state: &str) -> (&'static str, &'static str, bool) {
    match state {
        "none" => ("Shared base", "Computers start from Zuse's standard machine.", false),
        "queued" => ("Waiting to build", "Your machine image will be built from these settings. Until then, computers use the standard machine.", true),
        "building" => ("Building", "Computers keep using your previous image until this build is verified.", true),
        "ready" => ("Ready", "New computers start from your machine image.", false),
        "failed" => ("Build failed", "Computers keep using your last working image. Fix the setup script and save again.", true),
        _ => ("Unknown", "", true),
    }
}

impl RepositoryPanel {
    pub(super) fn load_machine(&mut self, cx: &mut Context<Self>) {
        self.call("machine-read", methods::CLOUD_MACHINE, json!({}), cx);
    }
    pub(super) fn receive_machine(&mut self, data: Value, cx: &mut Context<Self>) {
        match serde_json::from_value::<MachineVersion>(data) {
            Ok(version) => {
                let saved = self.machine.version.as_ref().is_some_and(|v| v.revision < version.revision);
                self.machine.setup.update(cx, |input, cx| input.set_value(version.recipe.setup_script.clone(), cx));
                self.machine
                    .instructions
                    .update(cx, |input, cx| input.set_value(version.recipe.instructions.clone(), cx));
                if saved {
                    self.machine.notice = Some(if version.recipe.is_empty() {
                        "Saved. Computers use the standard machine.".into()
                    } else {
                        "Saved. Your machine image will be rebuilt; running computers are unchanged.".into()
                    });
                }
                self.machine.version = Some(version);
                self.machine.error = None;
            }
            Err(_) => {
                self.machine.error = Some("Machine settings could not be verified. Reload before saving.".into())
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
                "machine_recipe_text_too_large" => "Keep the setup script and instructions under 64 KiB each.".into(),
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
    pub(super) fn render_machine(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.pending.values().any(|p| p.kind.starts_with("machine"));
        let mut body = column().gap(px(10.)).p(px(12.));
        match &self.machine.version {
            None => {
                body = body.child(s::description_text(if busy {
                    "Loading your machine…"
                } else {
                    "Your machine settings are unavailable. Refresh to try again."
                }));
            }
            Some(version) => {
                let (label, detail, warning) = image_copy(&version.image_state);
                body = body
                    .child(
                        row()
                            .items_center()
                            .gap(px(8.))
                            .child(s::title_text("Machine image"))
                            .child(cs::badge(label, warning, !warning && version.image_state == "none")),
                    )
                    .child(s::description_text(detail))
                    .when_some(version.image_error.clone(), |body, error| {
                        body.child(
                            div()
                                .text_size(px(12.))
                                .text_color(s::destructive_text())
                                .child(SharedString::from(error)),
                        )
                    })
                    .child(s::title_text("Setup script"))
                    .child(s::description_text(
                        "Runs once when your image is built, with sudo. Install tools and packages every repository should have: apt packages, bun, pnpm, uv, global CLIs.",
                    ))
                    .child(self.machine.setup.clone())
                    .child(s::title_text("Instructions"))
                    .child(s::description_text(
                        "Added for every agent, in every repository, alongside each repository's own AGENTS.md.",
                    ))
                    .child(self.machine.instructions.clone());
                if !version.recipe.skills.is_empty() || !version.recipe.files.is_empty() {
                    body = body.child(s::description_text(format!(
                        "Also included: {} skill{} and {} file{}.",
                        version.recipe.skills.len(),
                        if version.recipe.skills.len() == 1 { "" } else { "s" },
                        version.recipe.files.len(),
                        if version.recipe.files.len() == 1 { "" } else { "s" },
                    )));
                }
                body = body
                    .child(
                        row().gap(px(8.)).child(
                            cs::button("save-cloud-machine", "Save machine", true, busy || !self.can_manage(), busy)
                                .on_click(cx.listener(|this, _, _, cx| this.save_machine(cx))),
                        ),
                    )
                    .child(s::description_text(format!(
                        "Machine version {} · No secrets: use project secrets for tokens.",
                        version.revision
                    )));
            }
        }
        if let Some(notice) = &self.machine.notice {
            body = body.child(s::description_text(notice.clone()));
        }
        if let Some(error) = &self.machine.error {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(s::destructive_text())
                    .child(SharedString::from(error.clone())),
            );
        }
        cs::group(
            "Your machine",
            Some("One image for all your repositories. Tasks still check out only the repository they work on."),
            None,
            body,
        )
    }
}
