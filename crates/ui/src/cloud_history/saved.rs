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
    cached: Option<Entity<crate::transcript::Transcript>>,
    cache_task: Option<Task<()>>,
    load_generation: u64,
    subscription: Option<Subscription>,
    error: Option<String>,
    loading: bool,
    authorized: bool,
    watch: Option<Task<()>>,
    progress: Option<Task<()>>,
}
fn matches(target: &CloudTarget, value: Value) -> bool {
    value["user"]["id"].as_str() == Some(target.subject.as_str())
        && value["orgId"].as_str() == target.organization.as_deref()
}
impl Saved {
    fn new(target: CloudTarget, window: gpui::AnyWindowHandle, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            target,
            window,
            history: None,
            cached: None,
            cache_task: None,
            load_generation: 0,
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
        self.load_generation = self.load_generation.wrapping_add(1);
        self.cache_task = None;
        self.cached = None;
        self.loading = true;
        self.error = None;
        self.authorized = false;
        self.history = None;
        self.subscription = None;
        self.progress = None;
        self.load_cache(cx);
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
            let read=controller.call_scoped(methods::CLOUD_HISTORY,json!({"workspaceId":target.workspace,"sessionId":target.initial_session,"incremental":true,"expectedAccountId":target.subject,"expectedOrganizationId":target.organization}));
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
                this.cached=None;
                this.cache_task=None;
                this.authorized=false;
                match result {
                    Ok(value) if value["accountId"].as_str()==Some(target.subject.as_str())&&value["organizationId"].as_str()==target.organization.as_deref() => {
                        if value["commandDeliveryMode"]=="server-admitted-v2"&&value["commandAdmissionVersion"]==2&&value["workspace"]["workspaceId"]==target.workspace&&value["head"]["workspaceId"]==target.workspace&&value["head"]["sessionId"]==target.initial_session{
                            let target=target.clone();let seed=value.clone();
                            let _=this.window.update(cx,|_,window,cx|{window.replace_root(cx,|window,cx|super::shared::Shared::new(target,seed,window,cx));});return;
                        }
                        let row=serde_json::from_value::<Workspace>(value["workspace"].clone());
                        let head=serde_json::from_value::<Head>(value["head"].clone());
                        match (row,head) {
                            (Ok(row),Ok(head)) if row.workspace_id==target.workspace && head.workspace_id==target.workspace && head.session_id==target.initial_session => {
                                this.authorized=true;
                                let history=cx.new(|cx|CloudHistory::new(target.controller.clone(),row,head,target.subject.clone(),cx).with_scope(target.organization.clone(),value.clone(),cx));
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
    fn load_cache(&mut self, cx: &mut Context<Self>) {
        let target = self.target.clone();
        let generation = self.load_generation;
        self.cache_task=Some(cx.spawn(async move |this,cx| {
            let result=target.controller.client().call_scoped(methods::CLOUD_HISTORY,json!({"workspaceId":target.workspace,"sessionId":target.initial_session,"cacheOnly":true,"expectedAccountId":target.subject,"expectedOrganizationId":target.organization})).await;
            let Ok(value)=result else{return};
            if value["provisional"]!=true || value["accountId"].as_str()!=Some(target.subject.as_str()) || value["organizationId"].as_str()!=target.organization.as_deref(){return;}
            let Ok(head)=serde_json::from_value::<Head>(value["head"].clone()) else{return};
            if head.workspace_id!=target.workspace || head.session_id!=target.initial_session{return;}
            let _=this.update(cx,|this,cx| {
                if this.load_generation!=generation || !this.loading || this.authorized{return;}
                // No engine handle, composer, command metadata or execution
                // transport is installed from this provisional projection.
                let (_,view)=CloudHistory::frozen(&head,true,cx);
                this.cached=Some(view);
                cx.notify();
            });
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
                        json!({"workspaceId":target.workspace,"expectedAccountId":target.subject,"expectedOrganizationId":target.organization}),
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
    fn connect_with_draft(
        &self,
        session: String,
        draft: Option<(String, bool)>,
        cx: &mut Context<Self>,
    ) {
        if self.authorized {
            let mut target = self.target.clone();
            target.initial_session = session.clone();
            crate::open_cloud_window_with_draft(
                target,
                draft.map(|(text, send)| (session, text, send)),
                cx,
            );
            self.close(cx);
        }
    }
    fn failed(&mut self, error: String, cx: &mut Context<Self>) {
        self.authorized = false;
        self.cached = None;
        self.cache_task = None;
        self.loading = false;
        self.error = Some(error);
        cx.notify();
    }
    fn revoked(&mut self, cx: &mut Context<Self>) {
        self.load_generation = self.load_generation.wrapping_add(1);
        self.cached = None;
        self.cache_task = None;
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
            .flex()
            .flex_col()
            .bg(theme.bg)
            .text_color(theme.text)
            .font_family(theme.font_sans.clone())
            .text_size(crate::typography::text_base())
            .when_some(self.history.clone(), |v, h| v.child(h))
            .when_some(self.cached.clone(), |v, cached| {
                v.child(
                    div()
                        .text_size(crate::typography::text_sm())
                        .px(px(16.))
                        .py(px(6.))
                        .child("Cached conversation · checking access…"),
                )
                .child(div().flex_1().min_h_0().child(cached))
            })
            .when(self.history.is_none() && self.cached.is_none(), |v| {
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

#[cfg(feature = "appshots-fixture")]
pub(crate) fn fixture(client: zuse_rpc::RpcClient, cx: &mut gpui::App) -> gpui::AnyWindowHandle {
    let target = CloudTarget {
        controller: crate::state::EngineHandle::from_test_client(client),
        subject: "fixture-owner".into(),
        organization: None,
        workspace: "fixture-computer".into(),
        initial_session: "fixture-session".into(),
    };
    cx.open_window(
        gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
                None,
                gpui::size(px(1160.), px(850.)),
                cx,
            ))),
            ..Default::default()
        },
        |window, cx| cx.new(|cx| Saved::new(target, window.window_handle(), cx)),
    )
    .unwrap()
    .into()
}
