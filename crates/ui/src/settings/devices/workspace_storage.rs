//! Device workspace storage controls; account sign-in lives in General.
use super::*;
use crate::localization::message as tr;
use gpui::{EventEmitter, KeyDownEvent, MouseButton};
use zuse_i18n::messages as msg;
use zuse_viewport::settings_style as s;

#[derive(Clone, PartialEq)]
pub(crate) struct WorkspaceStorage {
    pub status: SharedString,
    pub detail: SharedString,
    pub action: Option<&'static str>,
    pub enabled: bool,
}

pub(crate) enum StorageEvent {
    Action,
}
impl EventEmitter<StorageEvent> for DevicesPage {}

impl DevicesPage {
    pub(crate) fn set_workspace_storage(
        &mut self,
        value: WorkspaceStorage,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_storage.as_ref() != Some(&value) {
            self.workspace_storage = Some(value);
            cx.notify();
        }
    }

    pub(super) fn render_workspace_storage(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let storage = self.workspace_storage.as_ref()?;
        let enabled = storage.enabled;
        let mut row = s::setting_row(true)
            .items_center()
            .child(s::details(storage.status.clone(), storage.detail.clone()));
        if let Some(label) = storage.action {
            row = row.child(
                s::button("workspace-sync-action", label)
                    .debug_selector(|| "workspace-sync-action".into())
                    .track_focus(&self.storage_focus.clone().tab_stop(enabled))
                    .tab_index(if enabled { 0 } else { -1 })
                    .when(!enabled, |view| view.opacity(0.5).cursor_default())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if enabled {
                                window.focus(&this.storage_focus, cx);
                            }
                        }),
                    )
                    .on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            if enabled {
                                cx.emit(StorageEvent::Action);
                            }
                            cx.stop_propagation();
                        }
                    }))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        if enabled {
                            cx.emit(StorageEvent::Action);
                        }
                    })),
            );
        }
        let frame = s::frame()
            .child(s::header(tr(cx, msg::native::workspace_sync), ""))
            .child(s::card().child(row));
        Some(frame.into_any_element())
    }
}
