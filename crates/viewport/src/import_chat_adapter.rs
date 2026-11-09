use super::*;
use import_chat::{Event, ImportMenu, Request, Target};
impl WorkspaceView {
    pub(super) fn new_import_menu(cx: &mut Context<Self>) -> Entity<ImportMenu> {
        let menu = cx.new(ImportMenu::new);
        cx.observe(&menu, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&menu, |this, _, request: &Request, _| {
            let remote = this.call(request.method, request.params.clone());
            this.import_requests.insert(remote, request.id);
        })
        .detach();
        cx.subscribe(&menu, |this, _, event: &Event, cx| match event {
            Event::ReturnFocus(None) => this.input.update(cx, |input, cx| input.focus(cx)),
            Event::ReturnFocus(Some(_)) => {}
            Event::Imported(imported, target) => {
                if this.import_target().as_ref() != Some(target) {
                    return;
                }
                if !this
                    .spaces
                    .iter()
                    .any(|space| space.id == imported.project.id)
                {
                    this.spaces.push(imported.project.clone());
                }
                let chat = serde_json::to_value(&imported.chat).unwrap();
                if !this.chats.iter().any(|row| row["id"] == imported.chat.id) {
                    this.chats.push(chat.clone());
                }
                this.select(chat, cx);
                this.cloud.mode = false;
            }
        })
        .detach();
        menu
    }
    fn import_target(&self) -> Option<Target> {
        if self.selected.is_some()
            || self.cloud_controller
            || self.cloud.guest
            || self.cloud_model_change_pending()
            || !self.connected
        {
            return None;
        }
        let device = self
            .project
            .as_ref()
            .and_then(|project| this_space(&self.spaces, project))
            .map(|space| space.device_id.clone())
            .filter(|id| Some(id) != self.device_id.as_ref());
        Some(Target {
            device,
            owner: format!("{}:{:?}", self.cloud_account["user"]["id"], self.device_id),
        })
    }
    pub(super) fn import_chat_control(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let target = self.import_target();
        let visible = target.is_some();
        self.import_picker
            .update(cx, |menu, cx| menu.set_target(target, cx));
        visible.then(|| self.import_picker.clone().into_any_element())
    }
    pub(super) fn receive_import_chat(&mut self, frame: &Value, cx: &mut Context<Self>) -> bool {
        let Some(remote) = frame["id"].as_u64() else {
            return false;
        };
        let Some(local) = self.import_requests.remove(&remote) else {
            return false;
        };
        self.pending.remove(&remote);
        let result = frame
            .get("err")
            .map(|error| {
                Err(error
                    .as_str()
                    .unwrap_or("Import was interrupted. Retry to recover it.")
                    .to_owned())
            })
            .unwrap_or_else(|| {
                frame
                    .get("ok")
                    .or_else(|| frame.get("item"))
                    .cloned()
                    .ok_or_else(|| "Import response was interrupted. Retry to recover it.".into())
            });
        self.import_picker
            .update(cx, |menu, cx| menu.receive(local, result, cx));
        true
    }
}
fn this_space<'a>(
    spaces: &'a [zuse_protocol::Space],
    project: &Value,
) -> Option<&'a zuse_protocol::Space> {
    spaces.iter().find(|space| project["path"] == space.path)
}
