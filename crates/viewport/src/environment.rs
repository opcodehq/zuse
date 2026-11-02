//! Client-local environment never changes engine ownership or call lifetime.
use super::*;
use zuse_protocol::presentation::motion;

pub(super) struct Environment {
    system_reduced: bool,
    pub visible: bool,
    focused: bool,
}
impl Default for Environment {
    fn default() -> Self {
        Self {
            system_reduced: false,
            visible: true,
            focused: true,
        }
    }
}
impl WorkspaceView {
    pub(super) fn receive_environment(&mut self, frame: &Value, cx: &mut Context<Self>) -> bool {
        let Some(value) = frame.get("environment") else {
            return false;
        };
        let (Some(reduced), Some(visible), Some(focused)) = (
            value["reduced"].as_bool(),
            value["visible"].as_bool(),
            value["focused"].as_bool(),
        ) else {
            return true;
        };
        self.environment = Environment {
            system_reduced: reduced,
            visible,
            focused,
        };
        self.sync_motion(cx);
        cx.notify();
        true
    }
    pub(super) fn sync_motion(&self, cx: &mut App) {
        cx.set_reduce_motion(motion::resolve(
            self.preferences.reduce_motion,
            self.environment.system_reduced,
            self.preferences.pause_animations_in_background,
            self.environment.visible && self.environment.focused,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use motion::ReduceMotion;
    use std::prelude::v1::test;
    #[gpui::test]
    fn environment_and_saved_overrides_share_one_motion_policy(cx: &mut TestAppContext) {
        cx.update(initialize);
        let (requests, mut rx) = futures::channel::mpsc::unbounded();
        let (_, events) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(Transport { requests, events }, window, cx)
        });
        window
            .update(cx, |view, _, cx| {
                view.receive(
                    json!({"environment":{"reduced":true,"visible":true,"focused":true}}),
                    cx,
                );
                assert!(cx.reduce_motion());
                view.preferences.reduce_motion = ReduceMotion::Off;
                view.sync_motion(cx);
                assert!(!cx.reduce_motion());
                view.preferences.pause_animations_in_background = true;
                view.receive(
                    json!({"environment":{"reduced":true,"visible":false,"focused":false}}),
                    cx,
                );
                assert!(cx.reduce_motion());
                assert!(!view.environment.visible);
                view.receive(
                    json!({"environment":{"reduced":true,"visible":true,"focused":true}}),
                    cx,
                );
                assert!(!cx.reduce_motion());
                view.preferences.reduce_motion = ReduceMotion::On;
                view.sync_motion(cx);
                assert!(cx.reduce_motion());
                view.receive(json!({"environment":{"reduced":false}}), cx);
                assert!(view.environment.visible);
                view.save_preferences();
            })
            .unwrap();
        while let Ok(frame) = rx.try_recv() {
            if frame["ui"] == "savePreferences" {
                assert_eq!(frame["value"]["reduceMotion"], "on");
                assert_eq!(frame["value"]["pauseAnimationsInBackground"], true);
            }
            assert_ne!(frame["method"], methods::QUEUE_MESSAGE);
            assert_ne!(frame["method"], methods::QUEUE_COMMAND);
        }
    }
}
