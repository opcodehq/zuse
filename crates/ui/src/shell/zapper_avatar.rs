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
            state.device_online(&zapper.config.home_device_id, now),
            status(&zapper.coordinator_chat_id),
            &workers,
        ))
    }
}

/// What a Zapper's mascot should show, from the same state the rest of the
/// shell reads. Chat statuses arrive already staleness-gated, so a session
/// nobody refreshed never reads as working. In order:
///
/// 1. Not active (paused, stopped, archived): resting.
/// 2. Its home device offline: offline, whatever its chats last said.
/// 3. An open decision, an issue waiting on you, or any of its chats
///    waiting for input: needs you.
/// 4. Its own last turn failed and nobody has looked: blocked.
/// 5. Its own turn running: working. Thinking and tool use look the same,
///    because the session status doesn't tell them apart.
/// 6. Only its threads running: delegating.
/// 7. Its own turn finished and not seen yet: done.
/// 8. Otherwise, including a coordinator chat not synced yet: awake.
///
/// A message just sent already reads as working (see
/// `AppState::display_status_for`), so the mascot greets it with the hop
/// that starts work. There is no separate loading mood on purpose: nothing
/// marks a Zapper as starting, and a thread's "Starting" label only means no
/// data has arrived yet, which a stuck thread looks the same as.
pub(super) fn mood_of(
    zapper: &Zapper,
    home_online: bool,
    coordinator: Option<zuse_protocol::ChatIndicator>,
    workers: &[zuse_protocol::ChatIndicator],
) -> Mood {
    use zuse_protocol::ChatIndicator as C;
    use zuse_protocol::zappers::{ZapperIssueStatus, ZapperStatus};
    if zapper.status != ZapperStatus::Active {
        return Mood::Resting;
    }
    if !home_online {
        return Mood::Offline;
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
    } else if coordinator == Some(C::Errored) {
        Mood::Blocked
    } else if coordinator == Some(C::Working) {
        Mood::Working
    } else if workers.contains(&C::Working) {
        Mood::Delegating
    } else if coordinator == Some(C::Completed) {
        Mood::Done
    } else {
        Mood::Awake
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
        let mood = |coordinator, workers: &[C]| mood_of(&active, true, coordinator, workers);
        // Nothing running, or nothing known yet: awake, never busy.
        assert_eq!(mood(None, &[]), Mood::Awake);
        assert_eq!(mood(Some(C::Idle), &[C::Idle, C::Completed]), Mood::Awake);
        assert_eq!(mood(Some(C::Working), &[]), Mood::Working);
        assert_eq!(
            mood(Some(C::Idle), &[C::Idle, C::Working]),
            Mood::Delegating
        );
        // The Zapper's own work shows over its threads'.
        assert_eq!(mood(Some(C::Working), &[C::Working]), Mood::Working);
        // Its finished turn shows until you look, but any work beats it.
        assert_eq!(mood(Some(C::Completed), &[]), Mood::Done);
        assert_eq!(mood(Some(C::Completed), &[C::Working]), Mood::Delegating);
        // A failed turn shows over threads still working.
        assert_eq!(mood(Some(C::Errored), &[C::Working]), Mood::Blocked);
        // Needing you shows over everything else.
        assert_eq!(mood(Some(C::Working), &[C::AwaitingInput]), Mood::NeedsYou);
        assert_eq!(mood(Some(C::AwaitingInput), &[]), Mood::NeedsYou);
        assert_eq!(mood(Some(C::Errored), &[C::AwaitingInput]), Mood::NeedsYou);
        let asking = zapper(
            "active",
            r#"[{"id": "q", "workerChatId": null, "question": "Ship it?", "answer": null}]"#,
        );
        assert_eq!(mood_of(&asking, true, Some(C::Idle), &[]), Mood::NeedsYou);
        // A paused Zapper rests, whatever its chats last said.
        assert_eq!(
            mood_of(&zapper("paused", "[]"), true, Some(C::Working), &[]),
            Mood::Resting
        );
        // An offline home device beats every cached status: nothing it last
        // reported is live.
        assert_eq!(
            mood_of(&active, false, Some(C::Working), &[C::Working]),
            Mood::Offline
        );
        assert_eq!(
            mood_of(&asking, false, Some(C::AwaitingInput), &[]),
            Mood::Offline
        );
        assert_eq!(
            mood_of(&zapper("paused", "[]"), false, None, &[]),
            Mood::Resting
        );
    }
}
