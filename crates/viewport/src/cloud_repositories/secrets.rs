//! Secret values are write-only; setup recipes contain opaque selected references.
use super::*;
pub(super) struct SecretsView {
    project: Option<String>,
    rows: Vec<Value>,
    name: Entity<TextInput>,
    value: Entity<TextInput>,
    pub error: Option<String>,
    notice: Option<String>,
}
impl SecretsView {
    pub fn new(window: &mut Window, cx: &mut Context<RepositoryPanel>) -> Self {
        let name = cx.new(|cx| TextInput::new("Environment name, such as NPM_TOKEN", window, cx));
        let value = cx.new(|cx| TextInput::password("Secret value", window, cx));
        for input in [&name, &value] {
            cx.observe(input, |_, _, cx| cx.notify()).detach();
        }
        Self {
            project: None,
            rows: vec![],
            name,
            value,
            error: None,
            notice: None,
        }
    }
    pub fn reset(&mut self, cx: &mut Context<RepositoryPanel>) {
        self.project = None;
        self.rows.clear();
        self.error = None;
        self.notice = None;
        for input in [&self.name, &self.value] {
            input.update(cx, |input, cx| input.set_value(String::new(), cx));
        }
    }
}
impl RepositoryPanel {
    pub(super) fn load_workload_secrets(&mut self, project: String, cx: &mut Context<Self>) {
        if self.secrets.project.as_ref() != Some(&project) {
            self.secrets.reset(cx);
            self.secrets.project = Some(project.clone());
        }
        self.call(
            "secret-list",
            methods::CLOUD_PROJECT_SECRETS,
            json!({"projectId":project}),
            cx,
        );
    }
    pub(super) fn receive_workload_secrets(
        &mut self,
        kind: &str,
        data: Value,
        cx: &mut Context<Self>,
    ) {
        self.secrets.error = None;
        if kind == "secret-list" {
            if let Some(rows) = data["secrets"].as_array() {
                self.secrets.rows = rows.clone();
            } else {
                self.secrets.error = Some(
                    "Secret metadata could not be verified. Reload before changing it.".into(),
                );
            }
        } else if let Some(id) = data["secretId"].as_str() {
            let mut metadata = self
                .secrets
                .rows
                .iter()
                .find(|row| row["secretId"].as_str() == Some(id))
                .cloned()
                .unwrap_or_else(|| json!({}));
            if let (Some(metadata), Some(data)) = (metadata.as_object_mut(), data.as_object()) {
                metadata.extend(data.clone());
            }
            self.secrets
                .rows
                .retain(|row| row["secretId"].as_str() != Some(id));
            self.secrets.rows.push(metadata);
            self.secrets.notice=Some(if kind=="secret-revoke"{"Secret revoked. Running programs may retain values already delivered; future grants are blocked."}else{"Secret saved. Select it below and save the environment to include it in future tasks."}.into());
        } else {
            self.secrets.error = Some(
                "The result could not be verified. Reload secret metadata before retrying.".into(),
            );
        }
        cx.notify();
    }
    fn save_workload_secret(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.secrets.project.clone() else {
            return;
        };
        let name = self.secrets.name.read(cx).value(cx).trim().to_owned();
        let value = self.secrets.value.read(cx).value(cx);
        if name.is_empty() || value.is_empty() {
            self.secrets.error = Some("Enter an environment name and secret value.".into());
            cx.notify();
            return;
        }
        let revision = self
            .secrets
            .rows
            .iter()
            .find(|row| row["name"] == name)
            .and_then(|row| row["revision"].as_i64())
            .unwrap_or(0);
        self.call(
            "secret-save",
            methods::CLOUD_PROJECT_SECRET_SAVE,
            json!({"projectId":project,"name":name,"value":value,"expectedRevision":revision}),
            cx,
        );
        self.secrets
            .value
            .update(cx, |input, cx| input.set_value(String::new(), cx));
        self.secrets.error = None;
        self.secrets.notice = None;
        cx.notify();
    }
    fn revoke_workload_secret(&mut self, id: String, revision: i64, cx: &mut Context<Self>) {
        let Some(project) = self.secrets.project.clone() else {
            return;
        };
        self.call(
            "secret-revoke",
            methods::CLOUD_PROJECT_SECRET_REVOKE,
            json!({"projectId":project,"secretId":id,"expectedRevision":revision}),
            cx,
        );
        cx.notify();
    }
    pub(super) fn render_workload_secrets(&self, cx: &mut Context<Self>) -> Div {
        let busy = self
            .pending
            .values()
            .any(|pending| pending.kind.starts_with("secret-"));
        let mut body = column().gap(px(8.));
        body=body.child(s::description_text("Encrypted values are delivered only to the selected task computer. Recipes store references; rotating a value does not rebuild an image."));
        for item in &self.secrets.rows {
            let (Some(id), Some(name), Some(revision)) = (
                item["secretId"].as_str(),
                item["name"].as_str(),
                item["revision"].as_i64(),
            ) else {
                continue;
            };
            let revoked = item["revoked"] == true;
            let selected = self.environment.secret_selected(id);
            let id = id.to_owned();
            let revoke = id.clone();
            let mut actions = row().gap(px(6.));
            if !revoked || selected {
                actions = actions.child(
                    cs::button(
                        SharedString::from(format!("select-secret-{id}")),
                        if selected { "Selected" } else { "Use in setup" },
                        selected,
                        busy || self.environment.repository_managed(),
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.environment.select_secret(id.clone(), !selected);
                        cx.notify();
                    })),
                );
            }
            if self.can_manage() && !revoked {
                actions = actions.child(
                    cs::button(
                        SharedString::from(format!("revoke-secret-{revoke}")),
                        "Revoke",
                        false,
                        busy,
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.revoke_workload_secret(revoke.clone(), revision, cx)
                    })),
                );
            }
            body = body.child(cs::setting_row(
                false,
                name.to_owned(),
                format!(
                    "{} · version {revision}",
                    if revoked { "Revoked" } else { "Stored" }
                ),
                Some(actions.into_any_element()),
            ));
        }
        if self.can_manage() {
            body = body
                .child(self.secrets.name.clone())
                .child(self.secrets.value.clone())
                .child(
                    cs::button("save-workload-secret", "Save secret", true, busy, busy)
                        .on_click(cx.listener(|this, _, _, cx| this.save_workload_secret(cx))),
                );
        }
        if let Some(project) = self.secrets.project.clone() {
            body = body.child(
                cs::button(
                    "reload-workload-secrets",
                    "Reload secrets",
                    false,
                    busy,
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.load_workload_secrets(project.clone(), cx)
                })),
            );
        }
        if let Some(error) = &self.secrets.error {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(s::destructive_text())
                    .child(error.clone()),
            );
        }
        if let Some(notice) = &self.secrets.notice {
            body = body.child(s::description_text(notice.clone()));
        }
        cs::group(
            "Workload secrets",
            Some("Project credentials for setup commands, services, terminals and agent tools."),
            None,
            body,
        )
    }
}
