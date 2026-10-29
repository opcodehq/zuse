use super::tray::{TrayPill, TrayTone, tray_text_action};
use super::*;
use zuse_protocol::project_creation::*;

impl Composer {
    /// The selected project's creation state as a tray row: what is
    /// happening and the one next action, like the composer's other notices.
    pub(super) fn project_creation_row(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<TrayPill> {
        let op = self.state.read(cx).selected_project_creation()?.clone();
        if op.status == ProjectCreationStatus::Ready
            && op.publish_error.is_none()
            && op.warning.is_none()
        {
            return None;
        }
        let name = &op.request.name;
        let running = op.status == ProjectCreationStatus::Running;
        let (tone, title, subtitle) = match op.status {
            ProjectCreationStatus::Running => (
                TrayTone::Default,
                format!("Setting up {name}"),
                format!(
                    "{}{}",
                    op.stage,
                    op.percent.map(|p| format!(" {p}%")).unwrap_or_default()
                ),
            ),
            ProjectCreationStatus::Cancelled => (
                TrayTone::Default,
                format!("Setup of {name} was cancelled"),
                op.error.clone().unwrap_or_default(),
            ),
            ProjectCreationStatus::Interrupted => (
                TrayTone::Warning,
                format!("Setup of {name} was interrupted"),
                op.error.clone().unwrap_or_default(),
            ),
            ProjectCreationStatus::Failed => (
                TrayTone::Danger,
                format!("Couldn't create {name}"),
                op.error.clone().unwrap_or_default(),
            ),
            ProjectCreationStatus::Ready if op.publish_error.is_some() => (
                TrayTone::Warning,
                "Not published to GitHub".to_owned(),
                op.publish_error.clone().unwrap_or_default(),
            ),
            ProjectCreationStatus::Ready => (
                TrayTone::Warning,
                format!("{name} is ready"),
                op.warning.clone().unwrap_or_default(),
            ),
        };
        let mut pill = TrayPill::new("composer-project-creation", title).tone(tone);
        pill = if running {
            pill.icon_element(crate::loaders::ring_spinner(
                "composer-project-spinner",
                12.0,
                1.5,
                theme.hairline(0.12),
                theme.accent,
            ))
        } else if tone == TrayTone::Danger {
            pill.glyph(crate::icons::DANGER_TRIANGLE)
        } else {
            pill.glyph(crate::icons::INFO_CIRCLE)
        };
        if !subtitle.is_empty() {
            pill = pill.subtitle(subtitle);
        }
        let retry = matches!(
            op.status,
            ProjectCreationStatus::Failed
                | ProjectCreationStatus::Cancelled
                | ProjectCreationStatus::Interrupted
        ) || op.publish_error.is_some();
        if !(running || retry) {
            return Some(pill);
        }
        let (label, method) = if running {
            ("Cancel", CANCEL)
        } else {
            ("Retry", RETRY)
        };
        Some(pill.action(
            tray_text_action("composer-project-action", label, None, theme).on_click(cx.listener(
                move |this, _, _, cx| {
                    let Some(engine) = this.state.read(cx).engine().cloned() else {
                        return;
                    };
                    let mut params = serde_json::json!({"id": op.id});
                    if this.state.read(cx).local_device_id.as_deref() != Some(op.device_id.as_str())
                    {
                        params["targetDeviceId"] = op.device_id.clone().into();
                    }
                    cx.spawn(async move |this, cx| {
                        let result = engine.client().call(method, params).await;
                        this.update(cx, |composer, cx| {
                            if let Err(error) = result {
                                composer.failure = Some(error.to_string().into());
                                composer.failure_key =
                                    composer.state.read(cx).selected_chat.clone();
                            }
                            cx.notify();
                        })
                        .ok();
                    })
                    .detach();
                },
            )),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn pending_project_keeps_draft_and_publishing_does_not_block_send(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.selected_space = Some("project".into());
            state.spaces.push(serde_json::from_value(serde_json::json!({"id":"project","deviceId":"host","path":"/projects/new","createdAt":"2026-01-01T00:00:00Z"})).unwrap());
            state.project_creations.insert("host".into(),vec![ProjectCreation {
                id:"operation".into(), device_id:"host".into(), destination:"/projects/new".into(),
                request:StartProjectCreation { id:"operation".into(),name:"new".into(),parent:"/projects".into(),source:ProjectSource::Create { template:ProjectTemplate::Empty,publish:true } },
                stage:"Scaffolding".into(),status:ProjectCreationStatus::Running,percent:None,files_ready:false,error:None,warning:None,publish_error:None,
            }]);
            state
        });
        let composer = cx.new(|cx| Composer::new(state.clone(), cx));
        composer.update(cx, |composer, cx| {
            composer
                .input
                .update(cx, |input, cx| input.set_text("Build a dashboard", cx));
            assert!(composer.send_blocked(cx));
            composer.on_submit(cx);
            assert_eq!(composer.input.read(cx).text(), "Build a dashboard");
            assert!(composer.failure.is_none());
        });
        state.update(cx, |state, cx| {
            state.project_creations.get_mut("host").unwrap()[0].files_ready = true;
            cx.notify();
        });
        composer.update(cx, |composer, cx| {
            assert!(!composer.send_blocked(cx));
            assert_eq!(composer.input.read(cx).text(), "Build a dashboard");
        });
    }
}
