//! Zapper avatars in the shell: the shared renderer, and choosing a picture
//! of your own for a Zapper's settings page.
use super::*;
use zuse_protocol::presentation::zapper_mascots::Mood;
use zuse_protocol::zappers::Zapper;
pub(super) use zuse_viewport::zapper_avatar::ZapperFace;

/// A Zapper's profile picture, `size` logical pixels across.
pub(super) fn zapper_mascot(face: &ZapperFace, size: f32) -> gpui::Div {
    zuse_viewport::zapper_avatar::zapper_face(face, size)
}

/// What a saved Zapper looks like: its picture, else its robot.
pub(super) fn zapper_avatar(zapper: &Zapper) -> ZapperFace {
    ZapperFace::of(zapper)
}

impl Shell {
    /// Ask for a picture, then crop and shrink it off the main thread. The
    /// result lands on the open settings page; saving the page stores it with
    /// the Zapper on its home device, which every client reads it from.
    pub(super) fn upload_zapper_picture(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Use image".into()),
        });
        cx.spawn(async move |this, cx| {
            let Some(path) = paths
                .await
                .ok()
                .and_then(Result::ok)
                .flatten()
                .and_then(|paths| paths.into_iter().next())
            else {
                return;
            };
            let prepared = cx
                .background_executor()
                .spawn(async move {
                    let bytes =
                        std::fs::read(&path).map_err(|_| "Zuse couldn't open that file.")?;
                    zuse_viewport::zapper_avatar::prepare_image(&bytes)
                })
                .await;
            let _ = this.update(cx, |this, cx| match prepared {
                Ok(url) => {
                    if let Some(form) = this.zapper_ui.page.as_mut() {
                        form.image = Some(url);
                    }
                    cx.notify();
                }
                Err(text) => this.show_notice(crate::toast::ToastKind::Error, text, cx),
            });
        })
        .detach();
    }
}

impl Shell {
    /// Open a Zapper's settings page in the isolated native fixture.
    #[doc(hidden)]
    pub fn fixture_open_zapper_editor(&mut self, zapper_id: &str, cx: &mut Context<Self>) {
        let zapper = self
            .state
            .read(cx)
            .zappers
            .iter()
            .find(|zapper| zapper.id == zapper_id)
            .cloned();
        if let Some(zapper) = zapper {
            self.open_zapper_editor(zapper, cx);
        }
    }
}

impl Shell {
    /// A Zapper's face as the shell shows it, with its mood: what the
    /// coordinator and its threads are doing right now.
    pub(super) fn zapper_face(&self, zapper: &Zapper, cx: &App) -> ZapperFace {
        let state = self.state.read(cx);
        let now = chrono::Utc::now();
        let status = |chat: &str| {
            state
                .chats
                .iter()
                .find(|c| c.id == chat)
                .map(|row| state.display_status_for(row, now))
        };
        let workers: Vec<_> = zapper
            .workers
            .iter()
            .filter_map(|w| status(&w.chat_id))
            .collect();
        ZapperFace::of(zapper).with_mood(mood_of(
            zapper,
            status(&zapper.coordinator_chat_id),
            &workers,
        ))
    }
}

/// What a Zapper's mascot should show: needing you beats working, the
/// Zapper's own work beats its threads', and nothing running is rest.
pub(super) fn mood_of(
    zapper: &Zapper,
    coordinator: Option<zuse_protocol::ChatIndicator>,
    workers: &[zuse_protocol::ChatIndicator],
) -> Mood {
    use zuse_protocol::ChatIndicator as C;
    use zuse_protocol::zappers::{ZapperIssueStatus, ZapperStatus};
    if zapper.status != ZapperStatus::Active {
        return Mood::Resting;
    }
    let asks = zapper.decisions.iter().any(|d| d.answer.is_none())
        || zapper
            .issues
            .iter()
            .any(|i| i.details.status == ZapperIssueStatus::NeedsYou)
        || coordinator == Some(C::AwaitingInput)
        || workers.contains(&C::AwaitingInput);
    if asks {
        Mood::NeedsYou
    } else if coordinator == Some(C::Working) {
        Mood::Working
    } else if workers.contains(&C::Working) {
        Mood::Delegating
    } else {
        Mood::Resting
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::rust_2024::test;
    use zuse_protocol::ChatIndicator as C;

    fn zapper(status: &str, decisions: &str) -> Zapper {
        serde_json::from_str(&format!(
            r#"{{"id": "z", "revision": 1, "status": "{status}",
            "config": {{"name": "Ops", "purpose": "", "homeDeviceId": "d", "repositories": [],
                "coordinatorModel": {{"harness": "claude-code", "model": null}},
                "workerDeviceId": "d", "maxConcurrentWorkers": 4, "maxFollowups": 3}},
            "coordinatorChatId": "c", "previousCoordinatorChats": [],
            "workers": [], "decisions": {decisions}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn mascot_mood_follows_what_the_zapper_is_doing() {
        let active = zapper("active", "[]");
        assert_eq!(mood_of(&active, None, &[]), Mood::Resting);
        assert_eq!(
            mood_of(&active, Some(C::Idle), &[C::Completed]),
            Mood::Resting
        );
        assert_eq!(mood_of(&active, Some(C::Working), &[]), Mood::Working);
        assert_eq!(
            mood_of(&active, Some(C::Idle), &[C::Idle, C::Working]),
            Mood::Delegating
        );
        // The Zapper's own work shows over its threads'.
        assert_eq!(
            mood_of(&active, Some(C::Working), &[C::Working]),
            Mood::Working
        );
        // Needing you shows over everything.
        assert_eq!(
            mood_of(&active, Some(C::Working), &[C::AwaitingInput]),
            Mood::NeedsYou
        );
        assert_eq!(
            mood_of(&active, Some(C::AwaitingInput), &[]),
            Mood::NeedsYou
        );
        let asking = zapper(
            "active",
            r#"[{"id": "q", "workerChatId": null, "question": "Ship it?", "answer": null}]"#,
        );
        assert_eq!(mood_of(&asking, Some(C::Idle), &[]), Mood::NeedsYou);
        // A paused Zapper rests, whatever its chats last said.
        assert_eq!(
            mood_of(&zapper("paused", "[]"), Some(C::Working), &[]),
            Mood::Resting
        );
    }
}
