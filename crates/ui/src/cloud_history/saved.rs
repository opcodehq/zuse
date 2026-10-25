//! Native saved history uses account HTTP reads and never opens an execution connection.
use super::*;
use crate::state::CloudTarget;
use gpui::{App, Subscription, Window, WindowBounds, WindowOptions, size};
use serde_json::{Value, json};
use zuse_protocol::methods;

pub(crate) fn open(target: CloudTarget, cx: &mut App) {
    let (bounds, display_id) = crate::restored_main_window_bounds(cx);
    if let Err(error) = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            display_id,
            window_min_size: Some(size(px(900.), px(600.))),
            ..Default::default()
        },
        |window, cx| {
            window.set_rem_size(px(crate::typography::font_size(cx).pixels()));
            crate::appearance::observe_window(window, cx).detach();
            cx.new(|cx| Saved::new(target, window.window_handle(), cx))
        },
    ) {
        tracing::warn!(%error,"Could not open saved cloud conversation");
    }
    cx.activate(true);
}
struct Saved {
    target: CloudTarget,
    window: gpui::AnyWindowHandle,
    history: Option<Entity<CloudHistory>>,
    subscription: Option<Subscription>,
    error: Option<String>,
    loading: bool,
    authorized: bool,
    watch: Option<Task<()>>,
    progress: Option<Task<()>>,
}
fn matches(target: &CloudTarget, value: Value) -> bool {
    value["user"]["id"].as_str()==Some(target.subject.as_str()) && value["orgId"].is_null()
}
impl Saved {
    fn new(target: CloudTarget, window: gpui::AnyWindowHandle, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            target,
            window,
            history: None,
            subscription: None,
            error: None,
            loading: true,
            authorized: false,
            watch: None,
            progress: None,
        };
        this.load(cx);
        this
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        self.authorized = false;
        self.history = None;
        self.subscription = None;
        self.progress = None;
        let target = self.target.clone();
        self.watch=Some(cx.spawn(async move |this,cx| {
            let result=target.controller.client().subscribe_scoped(methods::CLOUD_ACCOUNT_STATUS,json!({})).await;
            let mut auth=match result {
                Ok(auth)=>auth,
                Err(error)=>{let _=this.update(cx,|this,cx|this.failed(error.to_string(),cx));return;}
            };
            let first=auth.recv().await;
            if !first.is_some_and(|value|matches(&target,value)) {
                let _=this.update(cx,|this,cx|this.revoked(cx));return;
            }
            let controller=target.controller.client();
            let read=controller.call_scoped(methods::CLOUD_HISTORY,json!({"workspaceId":target.workspace,"sessionId":target.initial_session,"expectedAccountId":target.subject,"expectedOrganizationId":null}));
            tokio::pin!(read);
            let result=loop {
                tokio::select! { biased;
                    value=auth.recv()=>{
                        if !value.is_some_and(|value|matches(&target,value)) {let _=this.update(cx,|this,cx|this.revoked(cx));return;}
                    },
                    result=&mut read=>break result,
                }
            };
            let _=this.update(cx,|this,cx| {
                this.loading=false;
                this.authorized=true;
                match result {
                    Ok(value) if value["accountId"].as_str()==Some(target.subject.as_str()) => {
                        let row=serde_json::from_value::<Workspace>(value["workspace"].clone());
                        let head=serde_json::from_value::<Head>(value["head"].clone());
                        match (row,head) {
                            (Ok(row),Ok(head)) if row.workspace_id==target.workspace && head.workspace_id==target.workspace && head.session_id==target.initial_session => {
                                let history=cx.new(|cx|CloudHistory::new(target.controller.clone(),row,head,target.subject.clone(),cx));
                                this.subscription=Some(cx.subscribe(&history,|this,_,event:&Event,cx|match event {
                                    Event::Back=>this.close(cx),
                                    Event::Connect(session)=>this.connect(session.clone(),cx),
                                    Event::Resume { session, text, send } => this.connect_with_draft(session.clone(), Some((text.clone(), *send)), cx),
                                    Event::Restored(_)=> {
                                        if let Some(history)=&this.history {this.target.initial_session=history.read(cx).selected_session(cx);}
                                        this.load(cx);
                                    }
                                }));
                                this.history=Some(history);
                                this.watch_pause(cx);
                            }
                            _=>this.failed("Saved conversation has the wrong workspace identity.".into(),cx),
                        }
                    }
                    Ok(_)=>this.revoked(cx),
                    Err(error)=>this.failed(error.to_string(),cx),
                }
                cx.notify();
            });
            while let Some(value)=auth.recv().await {
                if !matches(&target,value) { let _=this.update(cx,|this,cx|this.revoked(cx));return; }
            }
            let _=this.update(cx,|this,cx|this.revoked(cx));
        }));
    }
    fn watch_pause(&mut self, cx: &mut Context<Self>) {
        let Some(history) = &self.history else {
            return;
        };
        if history.read(cx).workspace.state != "pausing" {
            return;
        }
        let target = self.target.clone();
        self.progress = Some(cx.spawn(async move |this, cx| {
            let mut failures = 0;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let result = target
                    .controller
                    .client()
                    .call_scoped(
                        methods::CLOUD_WORKSPACE,
                        json!({"workspaceId":target.workspace,"expectedAccountId":target.subject}),
                    )
                    .await;
                let keep = this
                    .update(cx, |this, cx| {
                        if !this.authorized {
                            return false;
                        }
                        match result {
                            Ok(value)
                                if value["accountId"].as_str() == Some(target.subject.as_str()) =>
                            {
                                if let Ok(row) =
                                    serde_json::from_value::<Workspace>(value["workspace"].clone())
                                {
                                    if row.workspace_id != target.workspace {
                                        this.revoked(cx);
                                        return false;
                                    }
                                    failures = 0;
                                    let pending = row.state == "pausing";
                                    if let Some(history) = &this.history {
                                        history.update(cx, |h, cx| h.update_workspace(row, cx));
                                    }
                                    return pending;
                                }
                            }
                            Err(error) => {
                                failures += 1;
                                if let Some(history) = &this.history {
                                    history.update(cx, |h, cx| {
                                        h.error = Some(error.to_string());
                                        cx.notify();
                                    });
                                }
                            }
                            _ => {
                                this.revoked(cx);
                                return false;
                            }
                        }
                        failures < 8
                    })
                    .unwrap_or(false);
                if !keep {
                    return;
                }
            }
        }));
    }
    fn close(&self, cx: &mut Context<Self>) {
        let _ = self
            .window
            .update(cx, |_, window, _| window.remove_window());
    }
    fn connect(&self, session: String, cx: &mut Context<Self>) {
        self.connect_with_draft(session, None, cx);
    }
    fn connect_with_draft(&self, session: String, draft: Option<(String, bool)>, cx: &mut Context<Self>) {
        if self.authorized {
            let mut target = self.target.clone();
            target.initial_session = session.clone();
            crate::open_cloud_window_with_draft(target, draft.map(|(text, send)| (session, text, send)), cx);
            self.close(cx);
        }
    }
    fn failed(&mut self, error: String, cx: &mut Context<Self>) {
        self.loading = false;
        self.error = Some(error);
        cx.notify();
    }
    fn revoked(&mut self, cx: &mut Context<Self>) {
        self.progress = None;
        self.history = None;
        self.subscription = None;
        self.authorized = false;
        self.failed(
            "The account changed. Select this workspace from its personal account.".into(),
            cx,
        );
    }
}
impl Render for Saved {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        div()
            .size_full()
            .bg(theme.bg)
            .text_color(theme.text)
            .font_family(theme.font_sans.clone())
            .text_size(crate::typography::text_base())
            .when_some(self.history.clone(), |v, h| v.child(h))
            .when(self.history.is_none(), |v| {
                v.flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(12.))
                    .child(if self.loading {
                        "Opening saved conversation…".into()
                    } else {
                        self.error
                            .clone()
                            .unwrap_or_else(|| "Saved history is unavailable.".into())
                    })
                    .child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .child(
                                div()
                                    .id("saved-cloud-retry")
                                    .cursor_pointer()
                                    .child("Refresh")
                                    .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                            )
                            .when(self.authorized, |v| {
                                v.child(
                                    div()
                                        .id("saved-cloud-connect")
                                        .cursor_pointer()
                                        .child("Resume")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.connect(this.target.initial_session.clone(), cx)
                                        })),
                                )
                            }),
                    )
            })
    }
}
