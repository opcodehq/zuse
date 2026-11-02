use super::*;
use zuse_viewport::import_chat::{Event, ImportMenu, Request, Target};

impl Pickers {
    pub(super) fn new_import_menu(cx: &mut Context<Self>) -> Entity<ImportMenu> {
        let menu = cx.new(ImportMenu::new);
        cx.observe(&menu, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&menu, |this, menu, request: &Request, cx| {
            if request.method == zuse_protocol::external_chat::IMPORT {
                // The workspace watch can publish the copied chat before the
                // import reply. Keep startup restoration on the landing page
                // until this explicit navigation is acknowledged.
                this.state.update(cx, |state, _| state.auto_selected = true);
            }
            let Some(engine) = this
                .state
                .read(cx)
                .engine()
                .map(|engine| engine.controller())
            else {
                menu.update(cx, |menu, cx| {
                    menu.receive(
                        request.id,
                        Err("Connect to this computer to import its chats.".into()),
                        cx,
                    )
                });
                return;
            };
            let request = request.clone();
            let menu = menu.clone();
            let id = request.id;
            let task = gpui_tokio::Tokio::spawn(cx, async move {
                engine
                    .client()
                    .call_scoped(request.method, request.params)
                    .await
                    .map_err(|e| e.to_string())
            });
            this.import_tasks.insert(
                id,
                cx.spawn(async move |this, cx| {
                    let result = task.await.unwrap_or_else(|e| Err(e.to_string()));
                    let _ = this.update(cx, |this, cx| {
                        this.import_tasks.remove(&id);
                        menu.update(cx, |menu, cx| menu.receive(id, result, cx));
                    });
                }),
            );
        })
        .detach();
        cx.subscribe(&menu, |this, _, event: &Event, cx| match event {
            Event::ReturnFocus(pointer) => cx.emit(ReturnComposerFocus { pointer: *pointer }),
            Event::Imported(imported, target) => {
                if this.import_target(cx).as_ref() != Some(target) {
                    return;
                }
                this.leave_cloud(cx);
                this.state.update(cx, |state, cx| {
                    if !state
                        .spaces
                        .iter()
                        .any(|space| space.id == imported.project.id)
                    {
                        let mut spaces = state.spaces.clone();
                        spaces.push(imported.project.clone());
                        state.apply_spaces(spaces);
                    }
                    if !state.chats.iter().any(|chat| chat.id == imported.chat.id) {
                        let mut chats = state.chats.clone();
                        chats.push(imported.chat.clone());
                        state.apply_chats(chats);
                    }
                    state.select_space(Some(imported.project.id.clone()), cx);
                    state.select_chat(Some(imported.chat.id.clone()), cx);
                });
            }
        })
        .detach();
        menu
    }
    fn import_target(&self, cx: &App) -> Option<Target> {
        let state = self.state.read(cx);
        if self.is_settings_model()
            || state.selected_chat.is_some()
            || self.cloud_launch_frozen()
            || state
                .engine()
                .is_some_and(|engine| engine.cloud_workspace().is_some())
        {
            return None;
        }
        Some(Target {
            device: self.space_target(cx),
            owner: format!(
                "{:?}:{:?}",
                state.auth_user().map(|user| &user.id),
                state.local_device_id
            ),
        })
    }
    pub(super) fn render_import_chat(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = self.import_target(cx);
        let visible = target.is_some();
        self.import_picker
            .update(cx, |menu, cx| menu.set_target(target, cx));
        visible.then(|| self.import_picker.clone().into_any_element())
    }
    pub(crate) fn importing_chat(&self, cx: &App) -> bool {
        self.import_picker.read(cx).busy()
    }
}
