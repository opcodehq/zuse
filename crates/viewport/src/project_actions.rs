//! Project commands are edited and executed on the selected thread's engine.
use super::*;
use crate::{
    settings_style as s,
    shell::{BORDER, MUTED, column, row},
};
use zuse_protocol::{
    ProjectActionDraft, ProjectActionIcon, ProjectActionRun, ProjectActionsSnapshot,
};
#[derive(Clone, PartialEq)]
struct Scope {
    chat: String,
    host: String,
    space: String,
    terminal: String,
}
#[derive(Clone, Copy, PartialEq)]
enum Operation {
    Read,
    Save,
    Delete,
    Run,
}
struct Request {
    id: u64,
    scope: Scope,
    operation: Operation,
}
struct Editor {
    scope: Scope,
    id: Option<String>,
    name: Entity<input::TextInput>,
    command: Entity<input::TextInput>,
    icon: ProjectActionIcon,
    setup: bool,
    confirm_delete: bool,
}
#[derive(Default)]
pub(super) struct ProjectActionsView {
    snapshot: Option<ProjectActionsSnapshot>,
    request: Option<Request>,
    editor: Option<Editor>,
    confirmed: bool,
    error: Option<String>,
}
impl WorkspaceView {
    fn project_action_scope(&self) -> Option<Scope> {
        let chat = self.selected.as_ref()?;
        Some(Scope {
            chat: chat["id"].as_str()?.into(),
            host: chat["deviceId"].as_str()?.into(),
            space: chat["spaceId"].as_str()?.into(),
            terminal: self.terminal_scope(),
        })
    }
    pub(super) fn project_action_navigation_blocked(&mut self, cx: &mut Context<Self>) -> bool {
        if self.project_actions.editor.is_some()
            || self
                .project_actions
                .request
                .as_ref()
                .is_some_and(|request| request.operation != Operation::Read)
        {
            self.error = Some("Save or cancel the project action edit and wait for any action request before switching threads.".into());
            cx.notify();
            true
        } else {
            false
        }
    }
    pub(super) fn reset_project_actions(&mut self, cx: &mut Context<Self>) {
        if let Some(request) = self.project_actions.request.take() {
            if request.operation == Operation::Read {
                self.cancel(request.id);
            } else {
                self.pending.remove(&request.id);
                self.connection_notice = Some("A project action may have completed on its original computer. Check that computer before trying again.".into());
            }
        }
        let editor = self.project_actions.editor.take();
        if let Some(editor) = &editor {
            editor
                .name
                .update(cx, |input, cx| input.set_readonly(false, cx));
            editor
                .command
                .update(cx, |input, cx| input.set_readonly(false, cx));
        }
        self.project_actions = ProjectActionsView {
            editor,
            ..Default::default()
        };
    }
    pub(super) fn interrupt_project_actions(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.project_actions;
        if state
            .request
            .take()
            .is_some_and(|request| request.operation != Operation::Read)
        {
            state.error = Some("The project action may have completed before disconnection. Refresh its saved state and check running terminals before retrying.".into());
        }
        state.confirmed = false;
        if let Some(editor) = &state.editor {
            editor
                .name
                .update(cx, |input, cx| input.set_readonly(false, cx));
            editor
                .command
                .update(cx, |input, cx| input.set_readonly(false, cx));
        }
    }
    pub(super) fn refresh_project_actions(&mut self) {
        if !self.connected || self.project_actions.request.is_some() {
            return;
        }
        let Some(scope) = self.project_action_scope() else {
            return;
        };
        let id = self.call(
            methods::LIST_PROJECT_ACTIONS,
            json!({"spaceId":scope.space,"targetDeviceId":scope.host}),
        );
        self.project_actions.request = Some(Request {
            id,
            scope,
            operation: Operation::Read,
        });
    }
    fn edit_project_action(
        &mut self,
        action: Option<ProjectActionDraft>,
        id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project_actions.request.is_some() || !self.connected {
            return;
        }
        let Some(scope) = self.project_action_scope() else {
            return;
        };
        let draft = action.unwrap_or(ProjectActionDraft {
            name: String::new(),
            command: String::new(),
            icon: ProjectActionIcon::Play,
            run_on_worktree_create: false,
        });
        let name = cx.new(|cx| input::TextInput::new("Action name", window, cx));
        let command = cx.new(|cx| input::TextInput::multiline("Shell command", window, cx));
        name.update(cx, |input, cx| input.set_value(draft.name, cx));
        command.update(cx, |input, cx| input.set_value(draft.command, cx));
        window.focus(&name.focus_handle(cx), cx);
        self.project_actions.editor = Some(Editor {
            scope,
            id,
            name,
            command,
            icon: draft.icon,
            setup: draft.run_on_worktree_create,
            confirm_delete: false,
        });
        self.project_actions.error = None;
        cx.notify();
    }
    fn project_action_request(
        &mut self,
        operation: Operation,
        action_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.require_connection(cx)
            || !self.project_actions.confirmed
            || self.project_actions.request.is_some()
        {
            return;
        }
        let Some(scope) = self.project_action_scope() else {
            return;
        };
        if self
            .project_actions
            .editor
            .as_ref()
            .is_some_and(|editor| editor.scope != scope)
        {
            self.project_actions.error = Some("This thread moved to another project or computer. Your edit is retained; copy or cancel it before editing actions on the new host.".into());
            cx.notify();
            return;
        }
        let mut params = json!({"spaceId":scope.space,"targetDeviceId":scope.host});
        let method = match operation {
            Operation::Read => return,
            Operation::Save => {
                let Some(editor) = &self.project_actions.editor else {
                    return;
                };
                let name = editor.name.read(cx).value(cx).trim().to_owned();
                let command = editor.command.read(cx).value(cx);
                if name.is_empty() || command.trim().is_empty() {
                    self.project_actions.error =
                        Some("Enter an action name and shell command.".into());
                    cx.notify();
                    return;
                }
                params["actionId"] = json!(editor.id);
                params["action"] = json!(ProjectActionDraft {
                    name,
                    command,
                    icon: editor.icon,
                    run_on_worktree_create: editor.setup
                });
                methods::UPSERT_PROJECT_ACTION
            }
            Operation::Delete | Operation::Run => {
                let Some(id) = action_id else {
                    return;
                };
                if !self
                    .project_actions
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.actions.iter().any(|action| action.id == id))
                {
                    return;
                }
                params["actionId"] = json!(id);
                if operation == Operation::Run {
                    params["chatId"] = json!(scope.chat);
                    params["cols"] = json!(80);
                    params["rows"] = json!(28);
                    methods::RUN_PROJECT_ACTION
                } else {
                    methods::DELETE_PROJECT_ACTION
                }
            }
        };
        if let Some(editor) = &self.project_actions.editor {
            editor
                .name
                .update(cx, |input, cx| input.set_readonly(true, cx));
            editor
                .command
                .update(cx, |input, cx| input.set_readonly(true, cx));
        }
        self.project_actions.error = None;
        let id = self.call(method, params);
        self.project_actions.request = Some(Request {
            id,
            scope,
            operation,
        });
        cx.notify();
    }
    pub(super) fn receive_project_actions(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .project_actions
            .request
            .as_ref()
            .is_some_and(|request| request.id == id)
        {
            return false;
        }
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        let request = self.project_actions.request.take().unwrap();
        self.pending.remove(&id);
        if self.project_action_scope().as_ref() != Some(&request.scope) {
            return true;
        }
        if let Some(editor) = &self.project_actions.editor {
            editor
                .name
                .update(cx, |input, cx| input.set_readonly(false, cx));
            editor
                .command
                .update(cx, |input, cx| input.set_readonly(false, cx));
        }
        let result = if frame.get("ok").is_none() {
            Err(frame["err"].as_str().unwrap_or("The action request could not be confirmed. Check the current project state before retrying.").to_owned())
        } else if request.operation == Operation::Run {
            serde_json::from_value::<ProjectActionRun>(frame["ok"].clone())
                .map_err(|_| "The engine returned invalid action terminal details.".to_owned())
                .map(|run| {
                    self.attach_browser_terminal(
                        run.terminal.id,
                        run.action_name,
                        request.scope.terminal,
                        Some(request.scope.host),
                        cx,
                    );
                    self.right_tab = "terminal".into();
                    self.right_open = true;
                })
        } else {
            serde_json::from_value::<ProjectActionsSnapshot>(frame["ok"].clone())
                .map_err(|_| "The engine returned invalid project actions.".to_owned())
                .and_then(|snapshot| {
                    if snapshot.space_id != request.scope.space {
                        return Err("The engine returned actions for another project.".into());
                    }
                    self.project_actions.snapshot = Some(snapshot);
                    self.project_actions.confirmed = true;
                    if request.operation != Operation::Read {
                        self.project_actions.editor = None;
                    }
                    Ok(())
                })
        };
        match result {
            Ok(()) => self.project_actions.error = None,
            Err(error) => {
                self.project_actions.error = Some(error);
                self.project_actions.confirmed = false;
                if request.operation != Operation::Read {
                    self.refresh_project_actions();
                }
            }
        }
        cx.notify();
        true
    }
    pub(super) fn project_actions_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.project_actions;
        let available = self.connected && state.confirmed && state.request.is_none();
        let mut view = column()
            .id("project-actions")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .gap_3()
            .child(
                row()
                    .gap_2()
                    .child(s::title_text("Project actions").flex_1())
                    .child(
                        s::button("refresh-project-actions", "Refresh").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.refresh_project_actions();
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(s::description_text(
                "Save commands for this project and run them in the selected thread’s checkout.",
            ));
        if self.project_action_scope().is_none() {
            return view
                .child(s::description_text(
                    "Open a thread in a project to manage its actions.",
                ))
                .into_any_element();
        }
        if let Some(error) = &state.error {
            view = view.child(s::description_text(error.clone()).text_color(rgb(0xec9292)));
        }
        if let Some(editor) = &state.editor {
            let busy = state
                .request
                .as_ref()
                .is_some_and(|request| request.operation != Operation::Read);
            let id = editor.id.clone();
            let choices = [
                (ProjectActionIcon::Play, "Run"),
                (ProjectActionIcon::Test, "Test"),
                (ProjectActionIcon::Lint, "Lint"),
                (ProjectActionIcon::Configure, "Setup"),
                (ProjectActionIcon::Build, "Build"),
                (ProjectActionIcon::Debug, "Debug"),
            ];
            return view.child(s::frame().child(s::header(if id.is_some() { "Edit action" } else { "New action" }, "Commands run only when you choose Run, or when enabled for new worktrees."))
                .child(s::card()
                    .child(s::setting_row(true).flex_col().items_stretch().child(s::title_text("Name")).child(s::field().child(editor.name.clone())))
                    .child(s::setting_row(false).flex_col().items_stretch().child(s::title_text("Command")).child(div().rounded(px(7.)).border_1().border_color(rgb(BORDER)).p_2().child(editor.command.clone())))
                    .child(s::setting_row(false).flex_col().items_stretch().child(s::title_text("Icon")).child(row().gap_1().flex_wrap().children(choices.into_iter().enumerate().map(|(index,(icon,label))| {
                        s::button(("project-action-icon",index),label).when(editor.icon==icon,|view|view.bg(s::frame_bg()))
                            .on_click(cx.listener(move|this,_,_,cx|{if !busy {if let Some(editor)=&mut this.project_actions.editor {editor.icon=icon;}cx.notify();}}))
                    }))))
                    .child(s::setting_row(false).child(s::details("Run on worktree creation","Run this setup command when a new worktree is created."))
                        .child(s::toggle("project-action-setup",editor.setup,"Run on worktree creation").opacity(if busy {0.45}else{1.})
                            .on_click(cx.listener(move|this,_,_,cx|{if !busy {if let Some(editor)=&mut this.project_actions.editor {editor.setup=!editor.setup;}cx.notify();}}))))
                    .child(s::setting_row(false).flex_wrap()
                        .child(s::button("cancel-project-action","Cancel").on_click(cx.listener(move|this,_,_,cx|{if !busy {this.project_actions.editor=None;cx.notify();}})))
                        .child(s::button("save-project-action",if busy {"Saving…"}else{"Save action"}).opacity(if available {1.}else{0.45})
                            .on_click(cx.listener(move|this,_,_,cx|{if available {this.project_action_request(Operation::Save,None,cx);}})))
                        .when_some(id.clone(),|view,id|view.child(s::button("delete-project-action",if editor.confirm_delete {"Confirm delete"}else{"Delete…"}).text_color(rgb(0xec9292))
                            .on_click(cx.listener(move|this,_,_,cx|{if !available{return;}if this.project_actions.editor.as_ref().is_some_and(|editor|editor.confirm_delete) {this.project_action_request(Operation::Delete,Some(id.clone()),cx);}else if let Some(editor)=&mut this.project_actions.editor {editor.confirm_delete=true;cx.notify();}})))))))
                .into_any_element();
        }
        view = view.child(
            s::button("new-project-action", "Add action")
                .opacity(if available { 1. } else { 0.45 })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if available {
                        this.edit_project_action(None, None, window, cx);
                    }
                })),
        );
        if let Some(snapshot) = &state.snapshot {
            if let Some(issue) = &snapshot.project_file_issue {
                view = view.child(s::description_text(issue.clone()).text_color(rgb(0xecb66b)));
            }
            if snapshot.actions.is_empty() {
                view = view.child(s::description_text(
                    "No saved actions yet. Add a command or import one below.",
                ));
            }
            for (index, action) in snapshot.actions.iter().enumerate() {
                let edit = action.clone();
                let run = action.id.clone();
                view = view.child(
                    s::frame().child(
                        s::card().child(
                            s::setting_row(true)
                                .flex_col()
                                .items_stretch()
                                .child(s::title_text(action.name.clone()))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .font_family("Geist Mono")
                                        .text_color(rgb(MUTED))
                                        .child(action.command.clone()),
                                )
                                .when(action.run_on_worktree_create, |view| {
                                    view.child(s::description_text("Runs on new worktrees"))
                                })
                                .child(
                                    row()
                                        .gap_2()
                                        .child(
                                            s::button(("run-project-action", index), "Run")
                                                .opacity(if available { 1. } else { 0.45 })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if available {
                                                        this.project_action_request(
                                                            Operation::Run,
                                                            Some(run.clone()),
                                                            cx,
                                                        );
                                                    }
                                                })),
                                        )
                                        .child(
                                            s::button(("edit-project-action", index), "Edit")
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        if available {
                                                            this.edit_project_action(
                                                                Some(ProjectActionDraft {
                                                                    name: edit.name.clone(),
                                                                    command: edit.command.clone(),
                                                                    icon: edit.icon,
                                                                    run_on_worktree_create: edit
                                                                        .run_on_worktree_create,
                                                                }),
                                                                Some(edit.id.clone()),
                                                                window,
                                                                cx,
                                                            );
                                                        }
                                                    },
                                                )),
                                        ),
                                ),
                        ),
                    ),
                );
            }
            if !snapshot.importable_actions.is_empty() {
                view = view.child(s::title_text("Available in project files"));
            }
            for (index, action) in snapshot.importable_actions.iter().enumerate() {
                let draft = action.clone();
                view = view.child(
                    s::card().child(
                        s::setting_row(true)
                            .flex_col()
                            .items_stretch()
                            .child(s::title_text(action.name.clone()))
                            .child(s::description_text(action.command.clone()))
                            .child(
                                s::button(("import-project-action", index), "Review and import")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if available {
                                            this.edit_project_action(
                                                Some(draft.clone()),
                                                None,
                                                window,
                                                cx,
                                            );
                                        }
                                    })),
                            ),
                    ),
                );
            }
        } else if state.request.is_some() {
            view = view.child(s::description_text("Loading project actions…"));
        }
        view.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn workflow_project_action_uses_owning_host_and_receipt_loss_never_reexecutes(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut received) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window.update(cx,|view,_,cx|{
            view.connected=true;view.selected=Some(json!({"id":"chat","deviceId":"remote","spaceId":"project","cwd":"/project"}));
            view.refresh_project_actions();
            let read=view.project_actions.request.as_ref().unwrap().id;
            let snapshot=json!({"spaceId":"project","actions":[{"id":"build","name":"Build","command":"npm run build","icon":"build","runOnWorktreeCreate":false}],"importableActions":[],"projectFileIssue":null});
            view.receive(json!({"id":read,"ok":snapshot}),cx);
            assert!(view.project_actions.confirmed);
            view.project_action_request(Operation::Run,Some("build".into()),cx);
            let run=view.project_actions.request.as_ref().unwrap().id;
            view.project_action_request(Operation::Run,Some("build".into()),cx);
            assert_eq!(view.project_actions.request.as_ref().unwrap().id,run);
            view.receive(json!({"id":run,"ok":{"actionId":"build","actionName":"Build","terminal":{"id":"terminal","cwd":"/project","shell":"bash"}}}),cx);
            assert_eq!(view.terminal_id.as_deref(),Some("terminal"));
            assert_eq!(view.terminal_host.as_deref(),Some("remote"));
            assert_eq!(view.right_tab,"terminal");
            view.project_action_request(Operation::Run,Some("build".into()),cx);
            let lost=view.project_actions.request.as_ref().unwrap().id;
            view.receive(json!({"connection":"disconnected","interrupted":[{"id":lost,"method":methods::RUN_PROJECT_ACTION,"mutation":true}]}),cx);
            assert!(view.project_actions.request.is_none());
            assert!(!view.project_actions.confirmed);
            view.receive(json!({"id":lost,"ok":{"bad":"late terminal"}}),cx);
            assert_eq!(view.terminal_id.as_deref(),Some("terminal"));
        }).unwrap();
        let calls = std::iter::from_fn(|| received.try_recv().ok()).collect::<Vec<_>>();
        let runs = calls
            .iter()
            .filter(|request| request["method"] == methods::RUN_PROJECT_ACTION)
            .collect::<Vec<_>>();
        assert_eq!(runs.len(), 2);
        for request in runs {
            assert_eq!(request["params"]["targetDeviceId"], "remote");
            assert_eq!(request["params"]["chatId"], "chat");
        }
        assert!(
            calls
                .iter()
                .any(|request| request["method"] == methods::SUBSCRIBE_TERMINAL
                    && request["params"]["targetDeviceId"] == "remote")
        );
    }
}
