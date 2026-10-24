use super::*;
use creation_sources::{Event, Request, SourcePicker, Target};

impl WorkspaceView {
    pub(super) fn new_creation_sources(cx: &mut Context<Self>) -> Entity<SourcePicker> {
        let picker = cx.new(SourcePicker::new);
        cx.observe(&picker, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&picker, |this, _, request: &Request, _cx| {
            let remote = this.call(request.method, request.params.clone());
            this.creation_source_requests.insert(remote, request.id);
        })
        .detach();
        cx.subscribe(&picker, |this, _, event: &Event, cx| match event {
            Event::Changed(selection) => {
                this.creation_source_checkout = match selection {
                    Some(zuse_protocol::creation_source::Resolution::Checkout {
                        outcome, ..
                    }) => this
                        .project
                        .as_ref()
                        .and_then(|p| p["path"].as_str())
                        .map(|root| {
                            (
                                root.into(),
                                outcome.worktree.path.clone(),
                                outcome.worktree.branch.clone(),
                            )
                        }),
                    _ => None,
                };
                if let Some(zuse_protocol::creation_source::Resolution::Issue { title, .. }) =
                    selection
                {
                    this.input.update(cx, |input, cx| {
                        let current = input.value(cx);
                        input.set_value(
                            if current.is_empty() {
                                title.clone()
                            } else {
                                format!("{current}\n{title}")
                            },
                            cx,
                        );
                    });
                }
                cx.notify();
            }
            Event::ReturnFocus(None) => this.input.update(cx, |i, cx| i.focus(cx)),
            Event::ReturnFocus(Some(_)) => {}
        })
        .detach();
        picker
    }
    pub(super) fn creation_source_control(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = if self.creation_workspace_key().is_some() {
            let path = self.project.as_ref()?["path"].as_str()?;
            Some(Target {
                repo_path: path.into(),
                space_id: self
                    .spaces
                    .iter()
                    .find(|s| s.path == path)
                    .map(|s| s.id.clone()),
                device_id: self
                    .spaces
                    .iter()
                    .find(|s| s.path == path)
                    .map(|s| s.device_id.clone()),
            })
        } else {
            None
        };
        let visible = target.is_some();
        self.creation_source_picker
            .update(cx, |picker, cx| picker.set_target(target, cx));
        visible.then(|| self.creation_source_picker.clone().into_any_element())
    }
    pub(super) fn receive_creation_source(
        &mut self,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((remote, local)) = frame["id"].as_u64().and_then(|id| {
            self.creation_source_requests
                .remove(&id)
                .map(|local| (id, local))
        }) else {
            return false;
        };
        self.pending.remove(&remote);
        let result = if let Some(error) = frame["err"].as_str() {
            Err(error.into())
        } else {
            frame
                .get("ok")
                .or_else(|| frame.get("item"))
                .cloned()
                .ok_or_else(|| "The source response was interrupted. Retry to recover it.".into())
        };
        self.creation_source_picker
            .update(cx, |picker, cx| picker.receive(local, result, cx));
        true
    }
    pub(super) fn selected_creation_checkout(&self) -> Option<(&str, &str)> {
        if self.selected.is_some() || self.cloud.mode {
            return None;
        }
        let (root, path, branch) = self.creation_source_checkout.as_ref()?;
        (self.project.as_ref()?["path"] == root.as_str())
            .then_some((path.as_str(), branch.as_str()))
    }
    pub(super) fn stage_creation_issue(&mut self, cx: &App) {
        if self.selected.is_some() {
            return;
        }
        if let Some(zuse_protocol::creation_source::Resolution::Issue {
            number,
            attachment_path,
            ..
        }) = self.creation_source_picker.read(cx).selection()
        {
            if !self
                .attachments
                .iter()
                .any(|a| a["path"] == *attachment_path)
            {
                self.attachments
                    .push(json!({"path":attachment_path,"fileName":format!("issue-{number}.md")}));
            }
        }
    }
}
