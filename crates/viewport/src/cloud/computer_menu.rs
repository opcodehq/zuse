//! Stable action slots keep connection changes from moving destructive controls under the pointer.
use crate::shell::{BORDER, column, control, label};
use gpui::{prelude::*, *};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Terminal,
    Files,
    Previews,
    Stop,
    Sleep,
    Resume,
    Delete,
}
impl Action {
    const ALL: [Self; 7] = [
        Self::Terminal,
        Self::Files,
        Self::Previews,
        Self::Stop,
        Self::Sleep,
        Self::Resume,
        Self::Delete,
    ];
    fn id(self) -> &'static str {
        match self {
            Self::Terminal => "computer-terminal",
            Self::Files => "computer-files",
            Self::Previews => "computer-browser",
            Self::Stop => "computer-stop-agent",
            Self::Sleep => "pause-cloud-workspace",
            Self::Resume => "computer-resume",
            Self::Delete => "computer-delete",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal",
            Self::Files => "Files",
            Self::Previews => "Previews",
            Self::Stop => "Stop agent",
            Self::Sleep => "Sleep computer",
            Self::Resume => "Resume computer",
            Self::Delete => "Delete computer…",
        }
    }
}
#[derive(Clone, Copy)]
pub(super) struct State {
    pub tools_ready: bool,
    pub tools_reason: &'static str,
    pub stop: bool,
    pub stop_reason: &'static str,
    pub sleep: bool,
    pub resume: bool,
    pub delete: bool,
}
impl State {
    fn enabled(self, action: Action) -> bool {
        match action {
            Action::Terminal | Action::Files | Action::Previews => self.tools_ready,
            Action::Stop => self.stop,
            Action::Sleep => self.sleep,
            Action::Resume => self.resume,
            Action::Delete => self.delete,
        }
    }
    fn description(self, action: Action) -> &'static str {
        match action {
            Action::Terminal | Action::Files | Action::Previews => self.tools_reason,
            Action::Stop => self.stop_reason,
            Action::Sleep => {
                if self.sleep {
                    "Preserves the computer and pauses compute."
                } else {
                    "The computer must be connected and ready to sleep."
                }
            }
            Action::Resume => {
                if self.resume {
                    "Continue using the retained computer."
                } else {
                    "Resume is available when the computer is sleeping or stopped."
                }
            }
            Action::Delete => {
                "Deletes this computer, its retained disk, stored cloud history and recovery data; verified edit access is required."
            }
        }
    }
}
/// The product and rendered regression use this exact element tree.
pub(super) fn actions(state: State, on_action: impl Fn(Action, &mut App) + 'static) -> Div {
    let dispatch: Rc<dyn Fn(Action, &mut App)> = Rc::new(on_action);
    let button = |action: Action| {
        let enabled = state.enabled(action);
        let dispatch = dispatch.clone();
        control(action.id(), action.title())
            .debug_selector(move || action.id().into())
            .w_full()
            .flex_shrink_0()
            .aria_description(state.description(action))
            .when(!enabled, |row| row.opacity(0.45).cursor_default())
            .when(enabled, |row| {
                row.on_click(move |_, _, cx| dispatch(action, cx))
            })
    };
    column()
        .w_full()
        .gap_1()
        .flex_shrink_0()
        .children(
            [Action::Terminal, Action::Files, Action::Previews]
                .into_iter()
                .map(button),
        )
        .child(
            label(if state.tools_ready {
                " "
            } else {
                state.tools_reason
            })
            .h(px(18.))
            .px_2()
            .truncate()
            .text_size(px(11.)),
        )
        .child(div().h(px(1.)).my_1().bg(rgb(BORDER)))
        .child(button(Action::Stop))
        .child(
            label(state.stop_reason)
                .h(px(18.))
                .px_2()
                .truncate()
                .text_size(px(11.)),
        )
        .child(button(Action::Sleep))
        .child(button(Action::Resume))
        .child(div().h(px(1.)).my_1().bg(rgb(BORDER)))
        .child(button(Action::Delete))
        .child(div().h(px(1.)).my_1().bg(rgb(BORDER)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::prelude::v1::test;
    struct MenuFixture {
        state: State,
        clicks: Rc<RefCell<Vec<Action>>>,
    }
    impl Render for MenuFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let clicks = self.clicks.clone();
            div()
                .w(px(320.))
                .child(actions(self.state, move |action, _| {
                    clicks.borrow_mut().push(action)
                }))
        }
    }
    #[gpui::test]
    fn computer_menu_keeps_tools_and_stop_bounds_across_connection_changes(
        cx: &mut TestAppContext,
    ) {
        let clicks = Rc::new(RefCell::new(Vec::new()));
        let ready = State {
            tools_ready: true,
            tools_reason: "",
            stop: true,
            stop_reason: "Stops this conversation and pauses its queue.",
            sleep: true,
            resume: false,
            delete: true,
        };
        let states = [
            ready,
            State {
                tools_ready: false,
                tools_reason: "Reconnecting to this computer…",
                ..ready
            },
            State {
                tools_ready: false,
                tools_reason: "Resume this computer to use its tools.",
                stop: false,
                sleep: false,
                resume: true,
                ..ready
            },
            State {
                tools_ready: false,
                tools_reason: "A computer operation is in progress.",
                stop: false,
                sleep: false,
                resume: false,
                delete: false,
                ..ready
            },
            State {
                stop: false,
                stop_reason: "Agent is not running.",
                ..ready
            },
        ];
        let (view, cx) = cx.add_window_view(|_, _| MenuFixture {
            state: ready,
            clicks: clicks.clone(),
        });
        cx.simulate_resize(size(px(400.), px(400.)));
        let mut original = None;
        for state in states {
            view.update(cx, |view, cx| {
                view.state = state;
                cx.notify();
            });
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear());
            let bounds: Vec<_> = Action::ALL
                .into_iter()
                .map(|action| {
                    cx.debug_bounds(action.id())
                        .expect("Every action keeps its visible slot")
                })
                .collect();
            if let Some(original) = &original {
                assert_eq!(
                    &bounds, original,
                    "Readiness changes must not move the action slots"
                );
            } else {
                original = Some(bounds.clone());
            }
            clicks.borrow_mut().clear();
            cx.simulate_click(bounds[0].center(), Modifiers::none());
            assert_eq!(
                &*clicks.borrow(),
                if state.tools_ready {
                    &[Action::Terminal][..]
                } else {
                    &[][..]
                },
                "Clicking Terminal must never dispatch Stop while reconnecting"
            );
            clicks.borrow_mut().clear();
            cx.simulate_click(bounds[3].center(), Modifiers::none());
            assert_eq!(
                &*clicks.borrow(),
                if state.stop {
                    &[Action::Stop][..]
                } else {
                    &[][..]
                },
                "Idle/disabled Stop must not dispatch"
            );
        }
    }
}
