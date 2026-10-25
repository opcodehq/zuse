//! Main-pane archive navigation keeps the active conversation and draft intact.
use super::*;
impl Shell {
    pub(super) fn open_project_archives(
        &mut self,
        project: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.read(cx).spaces.iter().any(|s| s.id == project) {
            return;
        }
        if self.archive_project.as_deref() != Some(&project) {
            if let Some(page) = self
                .archive_project
                .as_ref()
                .and_then(|id| self.project_archives.get(id))
                .cloned()
            {
                page.update(cx, |page, cx| page.on_hidden(cx));
            }
        }
        if !self.project_archives.contains_key(&project) {
            let state = self.state.clone();
            let id = project.clone();
            let page =
                cx.new(|cx| crate::project_archive::ProjectArchive::new(state, id, window, cx));
            let scope = project.clone();
            let subscription = cx.subscribe(&page, move |this, _, event, cx| {
                let crate::project_archive::Event::Restored(id) = event else {return;};
                if matches!(this.route, Route::Archives)
                    && this.archive_project.as_ref() == Some(&scope)
                {
                    this.open_chat(id.clone(), cx);
                }
            });
            self.project_archive_events
                .insert(project.clone(), subscription);
            self.project_archives.insert(project.clone(), page);
        }
        self.command_palette = None;
        self.set_voice_stage_open(false, cx);
        self.close_space_menu(cx);
        self.close_chat_menu(cx);
        window.focus(&self.unfocused, cx);
        self.archive_project = Some(project.clone());
        self.set_route(Route::Archives, cx);
        if let Some(page) = self.project_archives.get(&project).cloned() {
            page.update(cx, |page, cx| page.on_visible(cx));
        }
        self.nav.push(NavEntry::Archives(project));
        cx.notify();
    }
}
