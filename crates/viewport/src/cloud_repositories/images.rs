//! Reference account-image readiness and build history for both GPUI targets.
use super::*;
mod presentation;
gpui::actions!(cloud_image_dialog, [CancelImageDelete, ConfirmImageDelete]);
pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", CancelImageDelete, Some("CloudImageDialog")),
        KeyBinding::new("enter", ConfirmImageDelete, Some("CloudImageDialog")),
        KeyBinding::new("space", ConfirmImageDelete, Some("CloudImageDialog")),
    ]);
}
use zuse_protocol::cloud_projects::{CloudAccountImage, CloudImageBuild, ImageBuildMode};
#[derive(Default)]
pub(super) struct ImageView {
    value: Option<CloudAccountImage>,
    confirm_delete: Option<String>,
    confirm_focus: Option<[FocusHandle; 3]>,
    delete_focus: Option<FocusHandle>,
    error: Option<String>,
    build_key: Option<(String, String)>,
    poll: Option<Task<()>>,
    failures: u8,
    expanded: BTreeSet<String>,
    settings: BTreeSet<String>,
    history_open: bool,
    history_focus: Option<FocusHandle>,
    history_trigger: Option<FocusHandle>,
    history_handles: BTreeMap<String, FocusHandle>,
    history_order: Vec<FocusHandle>,
    previous_open: bool,
    history_page: usize,
}
impl ImageView {
    pub(super) fn has_storage(&self) -> bool {
        self.value.as_ref().is_some_and(|i| i.storage.is_some())
    }
    pub(super) fn interrupted(&mut self) {
        self.poll = None;
        self.failures = 3;
        self.error =
            Some("Account connection interrupted. Refresh to check the image build.".into());
    }
}
fn phase(value: Option<&str>) -> &'static str {
    match value.unwrap_or("queued") {
        "allocate" => "Starting the image builder…",
        "prepare" => "Preparing repositories and agents…",
        "capture" | "sanitizing" => "Preparing the snapshot…",
        "verify" => "Verifying the saved image…",
        "cleanup" => "Cleaning up temporary machines…",
        "promote" => "Making the image available…",
        "retire-images" => "Removing the superseded image…",
        _ => "Waiting for the image builder…",
    }
}
fn running(state: &str) -> bool {
    matches!(state, "queued" | "building" | "sanitizing")
}
fn duration(build: &CloudImageBuild) -> String {
    let seconds = (build.updated_at - build.created_at).max(0) / 1000;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}
fn build_title(build: &CloudImageBuild) -> &'static str {
    match build.mode {
        ImageBuildMode::Update => "Image update",
        ImageBuildMode::Rebuild => "Clean rebuild",
    }
}
impl RepositoryPanel {
    fn image_pending(&self) -> bool {
        self.pending
            .values()
            .any(|p| matches!(p.kind, "image" | "image-build" | "image-delete"))
    }
    pub(super) fn refresh_image(&mut self, cx: &mut Context<Self>) {
        if self.owner.is_none() || self.image_pending() {
            return;
        }
        self.image.poll = None;
        self.call("image", methods::CLOUD_IMAGE, json!({}), cx);
        cx.notify();
    }
    fn build_image(&mut self, mode: &str, cx: &mut Context<Self>) {
        if self.image_pending()
            || !self.can_manage()
            || self.projects.is_empty()
            || self
                .image
                .value
                .as_ref()
                .is_none_or(|v| v.state == "building" || v.state == "auth-broken")
        {
            return;
        }
        let (mode, key) = self
            .image
            .build_key
            .get_or_insert_with(|| (mode.to_owned(), uuid::Uuid::new_v4().to_string()))
            .clone();
        self.image.error = None;
        self.image.poll = None;
        self.call(
            "image-build",
            methods::CLOUD_IMAGE_BUILD,
            json!({"mode":mode,"providerId":"boxd","idempotencyKey":key}),
            cx,
        );
        cx.notify();
    }
    pub(super) fn receive_image(
        &mut self,
        pending: Pending,
        result: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        let parsed = result.and_then(|value| {
            if value["accountId"].as_str() != Some(pending.owner.0.as_str())
                || value["organizationId"].as_str() != pending.owner.1.as_deref()
            {
                return Err("Your account changed. Refresh cloud images.".into());
            }
            serde_json::from_value::<CloudAccountImage>(value["data"].clone())
                .map_err(|_| "Image status could not be verified. Refresh and try again.".into())
        });
        match parsed {
            Ok(image) => {
                self.image.failures = 0;
                self.image.error = None;
                if pending.kind == "image-build" {
                    self.image.build_key = None;
                }
                // Keep an explicitly opened attempt stable while status polling changes timestamps.
                self.image
                    .expanded
                    .retain(|id| image.builds.iter().any(|b| &b.build_id == id));
                self.image
                    .settings
                    .retain(|id| image.builds.iter().any(|b| &b.build_id == id));
                if self.image.history_open
                    && self
                        .image
                        .value
                        .as_ref()
                        .and_then(|v| v.builds.first())
                        .map(|b| &b.build_id)
                        != image.builds.first().map(|b| &b.build_id)
                {
                    if let Some(build) = image.builds.first() {
                        self.image.expanded.insert(build.build_id.clone());
                    }
                }
                self.image.value = Some(image);
            }
            Err(error) => {
                self.image.failures = self.image.failures.saturating_add(1);
                self.image.error = Some(error);
            }
        }
        if self.image.value.as_ref().is_some_and(|v| {
            v.state == "building" || v.storage.as_ref().is_some_and(|s| s.state == "deleting")
        }) && self.image.failures < 3
        {
            self.image.poll = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.image.poll = None;
                    this.refresh_image(cx);
                });
            }));
        }
        cx.notify();
    }
    fn dismiss_image_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.image.confirm_delete = None;
        if let Some(f) = &self.image.delete_focus {
            window.focus(f, cx);
        }
        cx.notify();
    }
    fn delete_image(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.image_pending()
            || !self.can_manage()
            || self
                .image
                .value
                .as_ref()
                .and_then(|i| i.storage.as_ref())
                .is_none_or(|s| s.snapshot_id != id)
        {
            return;
        }
        self.image.error = None;
        self.image.poll = None;
        self.call(
            "image-delete",
            methods::CLOUD_IMAGE_DELETE,
            json!({"snapshotId":id}),
            cx,
        );
        cx.notify();
    }
}
