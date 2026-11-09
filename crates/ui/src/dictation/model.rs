use crate::localization::message as tr;
use crate::{
    popover,
    settings::{self, widgets},
    theme::Theme,
};
use gpui::{App, Context, Entity, Global, Task, Window, div, prelude::*, px};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use zuse_i18n::messages as msg;
struct VoiceGlobal {
    card: Entity<VoiceCard>,
}
impl Global for VoiceGlobal {}
pub(crate) fn init(_root: PathBuf, cx: &mut App) {
    let card = cx.new(|_| VoiceCard {
        scroll: widgets::PageScroll::default(),
        inputs: Inputs::default(),
        input_select: widgets::SelectState::default(),
        shortcut: None,
    });
    cx.set_global(VoiceGlobal { card });
}
pub(crate) fn enabled(cx: &App) -> bool {
    settings::current(cx).dictation_enabled
}
pub(crate) fn card(cx: &mut App) -> Entity<VoiceCard> {
    if !cx.has_global::<VoiceGlobal>() {
        init(PathBuf::new(), cx);
    }
    cx.global::<VoiceGlobal>().card.clone()
}
pub(crate) struct VoiceCard {
    scroll: widgets::PageScroll,
    inputs: Inputs,
    input_select: widgets::SelectState,
    shortcut: Option<Entity<settings::shortcuts::ShortcutField>>,
}
/// Microphones as last enumerated, refreshed off the UI thread while the page
/// is visible so newly connected devices appear.
#[derive(Default)]
struct Inputs {
    devices: Vec<zuse_voice::InputDevice>,
    default: Option<String>,
    checked: Option<Instant>,
    task: Option<Task<()>>,
}
const INPUT_REFRESH: Duration = Duration::from_secs(3);
impl VoiceCard {
    fn primary(&mut self, cx: &mut Context<Self>) {
        settings::update(settings::SavePolicy::Immediate, cx, |s| {
            s.dictation_enabled = !s.dictation_enabled
        });
        cx.refresh_windows();
        cx.notify();
    }
}
impl popover::ScrollRailHost for VoiceCard {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }
    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}
impl VoiceCard {
    fn refresh_inputs(&mut self, cx: &mut Context<Self>) {
        if self.inputs.task.is_some()
            || self
                .inputs
                .checked
                .is_some_and(|at| at.elapsed() < INPUT_REFRESH)
        {
            return;
        }
        let scan = cx.background_executor().spawn(async {
            (
                zuse_voice::input_devices(),
                zuse_voice::default_input_device(),
            )
        });
        self.inputs.task = Some(cx.spawn(async move |this, cx| {
            let (devices, default) = scan.await;
            this.update(cx, |this, cx| {
                let changed = this.inputs.devices != devices || this.inputs.default != default;
                this.inputs.devices = devices;
                this.inputs.default = default;
                this.inputs.checked = Some(Instant::now());
                if changed {
                    cx.notify();
                }
            })
            .ok();
            // A static settings page otherwise has no reason to render again,
            // so checking the age only in render misses hot-plugged devices.
            // Invalidate once when the scan expires. Only a visible card starts
            // the next scan, so leaving Settings does not keep polling devices.
            cx.background_executor().timer(INPUT_REFRESH).await;
            this.update(cx, |this, cx| {
                this.inputs.task = None;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Options for the microphone select and the index of the current one. A
    /// saved device that is unplugged stays listed so the choice is visible;
    /// recording falls back to the system default meanwhile.
    fn input_options(&self, saved: Option<&str>) -> (Vec<widgets::SelectOption>, usize) {
        let default_name = self.inputs.default.as_ref().and_then(|id| {
            self.inputs
                .devices
                .iter()
                .find(|d| &d.id == id)
                .map(|d| d.name.clone())
        });
        let mut options = vec![match default_name {
            Some(name) => widgets::SelectOption::new("System default").detail(name),
            None => widgets::SelectOption::new("System default"),
        }];
        options.extend(
            self.inputs
                .devices
                .iter()
                .map(|d| widgets::SelectOption::new(d.name.clone())),
        );
        let selected = match saved {
            None => 0,
            Some(id) => match self.inputs.devices.iter().position(|d| d.id == id) {
                Some(ix) => ix + 1,
                None => {
                    options.push(widgets::SelectOption::new("Disconnected microphone"));
                    options.len() - 1
                }
            },
        };
        (options, selected)
    }
}
impl Render for VoiceCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let enabled = settings::current(cx).dictation_enabled;
        let on = enabled;
        let weak = cx.entity().downgrade();
        let switch = widgets::toggle_switch(&theme, on, "voice-dictation")
            .id("voice-enable")
            .cursor_pointer()
            .tab_index(0)
            .role(gpui::Role::Switch)
            .aria_label(tr(cx, msg::native::dictation))
            .aria_toggled(if on {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
            .on_click(cx.listener(|this, _, _, cx| this.primary(cx)))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    this.primary(cx);
                }
            }))
            .on_a11y_action(gpui::AccessibleAction::Click, move |_, _, cx| {
                weak.update(cx, |this, cx| this.primary(cx)).ok();
            });
        let dictation_row = widgets::card_row(&theme, true)
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .flex()
                    .flex_col()
                    .child(widgets::row_title(&theme, tr(cx, msg::native::dictation)))
                    .child(widgets::meta_line(
                        &theme,
                        vec!["Uses your Codex sign-in".into_any_element()],
                    )),
            )
            .child(switch);
        let microphone_row = enabled.then(|| {
            self.refresh_inputs(cx);
            let saved = settings::current(cx).dictation_input.clone();
            let (options, selected) = self.input_options(saved.as_deref());
            let ids: Vec<Option<String>> = std::iter::once(None)
                .chain(self.inputs.devices.iter().map(|d| Some(d.id.clone())))
                .chain(std::iter::once(saved))
                .collect();
            let control = widgets::select(
                "voice-microphone",
                "Microphone",
                &theme,
                |card: &mut Self| &mut card.input_select,
            )
            .options(options, selected)
            .width(200.0)
            .on_select(move |_, ix, _, cx| {
                let input = ids.get(ix).cloned().flatten();
                settings::update(settings::SavePolicy::Immediate, cx, |s| {
                    s.dictation_input = input
                });
                cx.notify();
            })
            .render(&self.input_select, cx);
            widgets::card_row(&theme, false)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(160.0))
                        .child(widgets::row_title(&theme, tr(cx, msg::native::microphone))),
                )
                .child(control)
        });
        // Always visible so the hold shortcut can be rebound.
        let shortcut_row = {
            let field = self
                .shortcut
                .get_or_insert_with(|| {
                    cx.new(|cx| {
                        settings::shortcuts::ShortcutField::new(
                            settings::ShortcutId::ToggleDictation,
                            cx,
                        )
                    })
                })
                .clone();
            widgets::card_row(&theme, false)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(160.0))
                        .flex()
                        .flex_col()
                        .child(widgets::row_title(&theme, tr(cx, msg::native::shortcut)))
                        .child(widgets::meta_line(
                            &theme,
                            vec!["Hold to talk, release to transcribe".into_any_element()],
                        )),
                )
                .child(field)
        };
        let scrollbar = popover::rail(self, "voice-page-scrollbar", &theme, cx);
        div()
            .id("voice-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.scroll.set_list_hovered(*hovered) {
                    cx.notify();
                }
            }))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("voice-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::settings_page_column(window)
                                .child(widgets::section_title(
                                    &theme,
                                    &tr(cx, msg::native::voice),
                                    &tr(cx, msg::native::dictate_prompts_using_your_codex_account),
                                ))
                                .child(
                                    widgets::section_stack()

                                        .child(
                                            widgets::group(&theme, tr(cx, msg::native::dictation), tr(cx, msg::native::hold_the_microphone_beside_the_model_to_record_release_to_transcribe_into_your_d_b5fd35ed)).child(
                                                widgets::section_card(&theme)
                                                    .mt_0()
                                                    .child(dictation_row)
                                                    .children(microphone_row)
                                                    .child(shortcut_row),
                                            ),
                                        )

                                ),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn microphone_choice_survives_disconnection(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| settings::init(settings::UiSettings::default(), dir.path(), cx));
        let card = cx.update(card);
        card.update(cx, |card, _| {
            card.inputs.devices = vec![
                zuse_voice::InputDevice {
                    id: "coreaudio:built-in".into(),
                    name: "MacBook Pro Microphone".into(),
                },
                zuse_voice::InputDevice {
                    id: "coreaudio:usb".into(),
                    name: "USB Microphone".into(),
                },
            ];
            let (options, selected) = card.input_options(None);
            assert_eq!((options.len(), selected), (3, 0));
            assert_eq!(card.input_options(Some("coreaudio:usb")).1, 2);
            let (options, selected) = card.input_options(Some("coreaudio:gone"));
            assert_eq!((options.len(), selected), (4, 3));
        });
    }
    #[gpui::test]
    fn dictation_toggle_needs_no_model_download(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| settings::init(settings::UiSettings::default(), dir.path(), cx));
        cx.update(|cx| assert!(!enabled(cx)));
        let card = cx.update(card);
        card.update(cx, |card, cx| {
            assert!(!settings::current(cx).dictation_enabled);
            card.primary(cx);
        });
        cx.update(|cx| assert!(enabled(cx)));
        card.update(cx, |card, cx| {
            card.primary(cx);
        });
        cx.update(|cx| assert!(!enabled(cx)));
    }
}
