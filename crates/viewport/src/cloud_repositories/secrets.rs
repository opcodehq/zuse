//! Secret values are write-only; setup recipes contain opaque selected references.
use super::*;
mod sheet;

/// What the Variables overlay shows: the add sheet, an edit dialog or a
/// delete confirmation. Secrets are keyed by id, plain variables by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Overlay {
    Add {
        one_by_one: bool,
        secret: bool,
    },
    EditSecret {
        id: String,
        name: String,
        revision: i64,
        replace: bool,
    },
    EditVariable {
        name: String,
    },
    DeleteSecret {
        id: String,
        name: String,
        revision: i64,
    },
    DeleteVariable {
        name: String,
    },
}
pub(super) struct SecretsView {
    project: Option<String>,
    rows: Vec<Value>,
    name: Entity<TextInput>,
    value: Entity<TextInput>,
    /// Plain-text value for variables (secrets use the masked `value`).
    plain: Entity<TextInput>,
    paste: Entity<TextInput>,
    overlay: Option<Overlay>,
    revealed: std::collections::BTreeSet<String>,
    pub error: Option<String>,
    notice: Option<String>,
}
impl SecretsView {
    pub fn new(window: &mut Window, cx: &mut Context<RepositoryPanel>) -> Self {
        let name = cx.new(|cx| TextInput::new("NAME, such as NPM_TOKEN", window, cx));
        let value = cx.new(|cx| TextInput::password("Value", window, cx));
        let plain = cx.new(|cx| TextInput::new("Value", window, cx));
        let paste = cx.new(|cx| {
            TextInput::script(
                "Paste your .env here\n\nDATABASE_URL=postgres://…\nNPM_TOKEN=npm_…",
                window,
                cx,
            )
        });
        for input in [&name, &value, &plain, &paste] {
            cx.observe(input, |_, _, cx| cx.notify()).detach();
        }
        Self {
            project: None,
            rows: vec![],
            name,
            value,
            plain,
            paste,
            overlay: None,
            revealed: Default::default(),
            error: None,
            notice: None,
        }
    }
    pub fn reset(&mut self, cx: &mut Context<RepositoryPanel>) {
        self.project = None;
        self.rows.clear();
        self.overlay = None;
        self.revealed.clear();
        self.error = None;
        self.notice = None;
        for input in [&self.name, &self.value, &self.plain, &self.paste] {
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
            self.secrets.notice = Some(
                if kind == "secret-revoke" {
                    "Secret deleted. Running computers keep any value they already received."
                } else {
                    "Secret saved. New computers for this repository will receive it."
                }
                .into(),
            );
        } else {
            self.secrets.error = Some(
                "The result could not be verified. Reload secret metadata before retrying.".into(),
            );
        }
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
}
