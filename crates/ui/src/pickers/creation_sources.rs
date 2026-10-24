use super::*;
use zuse_viewport::creation_sources::{Event, Request, SourcePicker, Target};

pub(crate) struct CreationIssueTitle(pub String);
impl gpui::EventEmitter<CreationIssueTitle> for Pickers {}

impl Pickers {
    pub(super) fn new_creation_sources(cx: &mut Context<Self>) -> Entity<SourcePicker> {
        let picker = cx.new(SourcePicker::new);
        cx.observe(&picker, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&picker, |this, picker, event: &Request, cx| {
            let Some(engine) = this.state.read(cx).engine().map(|e| e.controller()) else {
                picker.update(cx, |p, cx| {
                    p.receive(
                        event.id,
                        Err("Connect to this project's computer to choose a source.".into()),
                        cx,
                    )
                });
                return;
            };
            let Request { id, method, params } = event.clone();
            let picker = picker.clone();
            let job = gpui_tokio::Tokio::spawn(cx, async move {
                engine
                    .client()
                    .call_scoped(method, params)
                    .await
                    .map_err(|e| e.to_string())
            });
            let task = cx.spawn(async move |this, cx| {
                let result = job.await.unwrap_or_else(|e| Err(e.to_string()));
                let _ = this.update(cx, |this, cx| {
                    this.source_tasks.remove(&id);
                    picker.update(cx, |p, cx| p.receive(id, result, cx));
                });
            });
            this.source_tasks.insert(id, task);
        })
        .detach();
        cx.subscribe(&picker, |_, _, event: &Event, cx| match event {
            Event::ReturnFocus(pointer) => cx.emit(ReturnComposerFocus { pointer: *pointer }),
            Event::Changed(Some(zuse_protocol::creation_source::Resolution::Issue {
                title,
                ..
            })) => cx.emit(CreationIssueTitle(title.clone())),
            _ => cx.notify(),
        })
        .detach();
        picker
    }
    pub(super) fn creation_source_target(&self, cx: &App) -> Option<Target> {
        let state = self.state.read(cx);
        if state.selected_chat.is_some() {
            return None;
        }
        let space = state.selected_space_row().filter(|s| s.git_detected)?;
        let cloud = state.cloud_project.is_some();
        if let Some(project) = state.cloud_project.as_ref() {
            if !space.remote_repository.as_deref().is_some_and(|identity| {
                project["repositoryIdentity"]
                    .as_str()
                    .is_some_and(|remote| identity.eq_ignore_ascii_case(remote))
            }) {
                return None;
            }
        }
        Some(Target {
            repo_path: space.path.clone(),
            space_id: Some(space.id.clone()),
            device_id: Some(space.device_id.clone()),
            cloud,
        })
    }
    pub(crate) fn source_cloud_ref(
        &self,
        default_branch: &str,
        cx: &App,
    ) -> (String, Option<String>) {
        self.source_picker.read(cx).cloud_ref(default_branch)
    }
    pub(crate) fn preparing_source(&self, cx: &App) -> bool {
        self.source_picker.read(cx).is_preparing()
    }
    pub(crate) fn source_issue(&self, cx: &App) -> Option<(u64, String)> {
        match self.source_picker.read(cx).selection()? {
            zuse_protocol::creation_source::Resolution::Issue {
                number, markdown, ..
            } => Some((*number, markdown.clone())),
            _ => None,
        }
    }
}
