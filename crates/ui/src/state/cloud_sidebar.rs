//! Scoped read-only sidebar metadata. Saved rows are provisional, never compute authority.
use super::*;
use futures::StreamExt;
use serde_json::{Value, json};
#[derive(Clone, Debug, PartialEq, Eq)]
struct Scope {
    account: String,
    organization: Option<String>,
}
impl Scope {
    fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            account: value["user"]["id"].as_str()?.to_owned(),
            organization: value["orgId"].as_str().map(str::to_owned),
        })
    }
    fn params(&self) -> Value {
        json!({"expectedAccountId":self.account,"expectedOrganizationId":self.organization})
    }
    fn matches(&self, state: &AppState) -> bool {
        state.cloud_user_id() == Some(self.account.as_str())
            && state.cloud_organization_id() == self.organization.as_deref()
    }
    fn accepts(&self, value: &Value) -> bool {
        value["accountId"].as_str() == Some(self.account.as_str())
            && value["organizationId"].as_str() == self.organization.as_deref()
    }
    fn owner(&self) -> String {
        self.organization
            .as_ref()
            .map(|v| format!("organization:{v}"))
            .unwrap_or_else(|| self.account.clone())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Projects,
    Workspaces,
}
impl Part {
    fn method(self) -> &'static str {
        match self {
            Self::Projects => methods::CLOUD_PROJECTS,
            Self::Workspaces => methods::CLOUD_WORKSPACES,
        }
    }
}
#[derive(Default)]
pub(super) struct Catalog {
    scope: Option<Scope>,
    origin: Option<String>,
    projects: Vec<Value>,
    workspaces: Vec<Value>,
    projects_fresh: bool,
    workspaces_fresh: bool,
    denied: bool,
    loading: bool,
    error: Option<String>,
    refresh: Arc<tokio::sync::Notify>,
}
fn access_denied(error: &RpcError) -> bool {
    matches!(error,RpcError::Capability(code) if code=="workspace_access_denied"||code=="workspace_not_found")
        || matches!(error,RpcError::Failed(message) if matches!(message.as_str(),"You do not have access to this workspace."|"Cloud could not verify your session. Reconnect in Settings → General."))
}
impl AppState {
    pub(crate) fn cloud_sidebar_projects(&self) -> Vec<Value> {
        if self
            .cloud_sidebar
            .scope
            .as_ref()
            .is_some_and(|s| s.matches(self))
        {
            self.cloud_sidebar.projects.clone()
        } else {
            vec![]
        }
    }
    pub(crate) fn cloud_sidebar_workspaces(&self) -> Vec<Value> {
        if self
            .cloud_sidebar
            .scope
            .as_ref()
            .is_some_and(|s| s.matches(self))
        {
            self.cloud_sidebar.workspaces.clone()
        } else {
            vec![]
        }
    }
    /// The sidebar refreshes silently; tests still observe its phases.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn cloud_sidebar_loading(&self) -> bool {
        self.cloud_sidebar.loading
    }
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn cloud_sidebar_stale(&self) -> bool {
        !self.cloud_sidebar.projects_fresh || !self.cloud_sidebar.workspaces_fresh
    }
    pub(crate) fn cloud_sidebar_error(&self) -> Option<&str> {
        self.cloud_sidebar.error.as_deref()
    }
    pub(crate) fn retry_cloud_sidebar(&mut self, cx: &mut Context<Self>) {
        self.cloud_sidebar.refresh.notify_one();
        self.cloud_sidebar.loading = true;
        cx.notify();
    }
    pub(super) fn clear_cloud_sidebar(&mut self) {
        self.cloud_sidebar = Catalog::default();
    }
    fn sidebar_error(&mut self, scope: &Scope, part: Part, error: &RpcError) {
        if !scope.matches(self) {
            return;
        }
        match part {
            Part::Projects => self.cloud_sidebar.projects_fresh = false,
            Part::Workspaces => self.cloud_sidebar.workspaces_fresh = false,
        }
        self.cloud_sidebar.error = Some(
            if access_denied(error) {
                "Cloud access changed. Reconnect in Settings → General."
            } else {
                "Showing saved cloud items. Reconnecting…"
            }
            .into(),
        );
        if access_denied(error) {
            self.cloud_sidebar.projects.clear();
            self.cloud_sidebar.workspaces.clear();
            self.cloud_sidebar.denied = true;
            self.cloud_sidebar.projects_fresh = false;
            self.cloud_sidebar.workspaces_fresh = false;
        }
    }
    fn apply_sidebar_part(&mut self, part: Part, value: &Value) -> bool {
        let Some(scope) = self.cloud_account.as_ref().and_then(Scope::from_value) else {
            return false;
        };
        if !scope.accepts(value) || self.cloud_sidebar.denied {
            return false;
        }
        let Some(origin) = value["apiOrigin"].as_str().filter(|v| !v.is_empty()) else {
            return false;
        };
        let provisional = value["provisional"] == true;
        if provisional
            && match part {
                Part::Projects => self.cloud_sidebar.projects_fresh,
                Part::Workspaces => self.cloud_sidebar.workspaces_fresh,
            }
        {
            return false;
        }
        if self
            .cloud_sidebar
            .origin
            .as_deref()
            .is_some_and(|v| v != origin)
        {
            if provisional
                && (self.cloud_sidebar.projects_fresh || self.cloud_sidebar.workspaces_fresh)
            {
                return false;
            }
            let refresh = self.cloud_sidebar.refresh.clone();
            self.clear_cloud_sidebar();
            self.cloud_sidebar.refresh = refresh;
        }
        self.cloud_sidebar.scope = Some(scope.clone());
        self.cloud_sidebar.origin = Some(origin.into());
        if provisional && value["cacheMiss"] == true {
            return true;
        }
        let rows = match part {
            Part::Projects => value["data"]["projects"]
                .as_array()
                .or_else(|| value["projects"].as_array()),
            Part::Workspaces => value["workspaces"].as_array(),
        };
        let Some(rows) = rows else {
            return false;
        };
        match part {
            Part::Projects => {
                let owner = scope.owner();
                self.cloud_sidebar.projects = rows
                    .iter()
                    .cloned()
                    .map(|mut row| {
                        row["accountId"] = json!(owner);
                        if row["providerId"].is_null() || row["providerId"] == "box" {
                            row["providerId"] = json!("boxd");
                        }
                        if provisional {
                            row["canCreate"] = json!(false);
                            row["provisional"] = json!(true);
                        }
                        row
                    })
                    .collect();
                self.cloud_sidebar.projects_fresh = !provisional;
            }
            Part::Workspaces => {
                self.cloud_sidebar.workspaces = rows
                    .iter()
                    .filter(|row| {
                        !matches!(
                            row["state"].as_str(),
                            Some("deleted" | "archived" | "archiving")
                        ) && !matches!(row["desiredState"].as_str(), Some("deleted" | "archived"))
                    })
                    .cloned()
                    .collect();
                self.cloud_sidebar.workspaces_fresh = !provisional;
            }
        }
        if !provisional && self.cloud_sidebar.projects_fresh && self.cloud_sidebar.workspaces_fresh
        {
            self.cloud_sidebar.error = None;
        }
        true
    }
    // Compatibility reducer for fixture/seed callers; the live pump uses independent parts.
    pub(crate) fn apply_cloud_sidebar_catalog(&mut self, context: &Value, list: &Value) -> bool {
        let Some(scope) = self.cloud_account.as_ref().and_then(Scope::from_value) else {
            return false;
        };
        if !scope.accepts(context) || !scope.accepts(list) || context["ownerId"] != scope.owner() {
            return false;
        }
        let mut context = context.clone();
        let mut list = list.clone();
        let origin = context["apiOrigin"]
            .as_str()
            .or_else(|| list["apiOrigin"].as_str())
            .unwrap_or("fixture://cloud-sidebar")
            .to_owned();
        context["apiOrigin"] = json!(origin);
        list["apiOrigin"] = json!(origin);
        self.apply_sidebar_part(Part::Projects, &context)
            | self.apply_sidebar_part(Part::Workspaces, &list)
    }
}
async fn request(
    handle: EngineHandle,
    scope: Scope,
    part: Part,
    provisional: bool,
    executor: gpui::BackgroundExecutor,
) -> (Part, bool, Result<Value, RpcError>) {
    let mut params = scope.params();
    if provisional {
        params["cacheOnly"] = json!(true);
    }
    let read = handle.client().call_scoped(part.method(), params);
    futures::pin_mut!(read);
    let result = tokio::select! { result=&mut read=>result, _=executor.timer(std::time::Duration::from_secs(30))=>Err(RpcError::Failed("Cloud sidebar request timed out.".into())) };
    (part, provisional, result)
}
pub(super) fn spawn(cx: &mut Context<AppState>, handle: EngineHandle) -> Task<()> {
    // Cloud computers are a beta; without it there is nothing to list.
    if !crate::cloud_beta() {
        return Task::ready(());
    }
    // A replacement controller must not inherit another API's visible catalog.
    cx.spawn(async move |this,cx| {
        if this.update(cx,|state,_|state.clear_cloud_sidebar()).is_err(){return;}
        'reconnect: loop {
            let mut auth=match handle.client().subscribe_scoped(methods::CLOUD_ACCOUNT_STATUS,json!({})).await{Ok(v)=>v,Err(_)=>{cx.background_executor().timer(std::time::Duration::from_secs(2)).await;continue;}};
            let mut pending=None;
            'account: loop {
                let value=match pending.take(){Some(v)=>v,None=>match auth.recv().await{Some(v)=>v,None=>continue 'reconnect}};
                let scope=Scope::from_value(&value);
                if this.update(cx,|state,cx|{state.apply_cloud_account(value);if state.cloud_sidebar.scope!=scope{state.cloud_sidebar=Catalog{scope:scope.clone(),..Default::default()};}cx.notify();}).is_err(){return;}
                let Some(scope)=scope else{continue;};
                let mut events:Option<zuse_rpc::RpcSubscription>=None;
                let mut initial=true;let mut load_projects=true;let mut retry_seconds=2;
                loop {
                    if this.update(cx,|state,cx|{if scope.matches(state){state.cloud_sidebar.loading=true;state.cloud_sidebar.denied=false;cx.notify();}}).is_err(){return;}
                    let mut reads=futures::stream::FuturesUnordered::new();
                    for part in [Part::Projects,Part::Workspaces]{
                        if part==Part::Projects&&!load_projects{continue;}
                        reads.push(request(handle.clone(),scope.clone(),part,false,cx.background_executor().clone()));
                        if initial{reads.push(request(handle.clone(),scope.clone(),part,true,cx.background_executor().clone()));}
                    }
                    initial=false;let mut retry=false;let mut retry_projects=false;
                    while !reads.is_empty(){
                        tokio::select!{biased;
                            value=auth.recv()=>{let Some(value)=value else{continue 'reconnect;};if Scope::from_value(&value).as_ref()!=Some(&scope){pending=Some(value);continue 'account;}},
                            value=reads.next()=>{if let Some((part,provisional,result))=value{
                                if let Err(ref error)=result{if !provisional&&!access_denied(error){retry=true;retry_projects|=part==Part::Projects;}}
                                if this.update(cx,|state,cx|{if !scope.matches(state){return;}match result{Ok(value)=>{if state.apply_sidebar_part(part,&value){cx.notify();}},Err(error)if !provisional=>{state.sidebar_error(&scope,part,&error);cx.notify();},_=>{}}}).is_err(){return;}
                            }}
                        }
                    }
                    if this.update(cx,|state,cx|{if scope.matches(state){state.cloud_sidebar.loading=false;cx.notify();}}).is_err(){return;}
                    let delay=retry_seconds;retry_seconds=if retry{(retry_seconds*2).min(30)}else{2};
                    loop {
                        let manual=match this.update(cx,|state,_|state.cloud_sidebar.refresh.clone()){Ok(value)=>value,Err(_)=>return};
                        let executor=cx.background_executor().clone();
                        let needs_connect=events.is_none();
                        let connect_executor=executor.clone();
                        let connect=async{
                            if !needs_connect{futures::future::pending::<()>().await;}
                            connect_executor.timer(std::time::Duration::from_secs(2)).await;
                            let open=handle.client().subscribe_scoped(methods::CLOUD_EVENTS,scope.params());futures::pin_mut!(open);
                            tokio::select!{value=&mut open=>value.ok(),_=connect_executor.timer(std::time::Duration::from_secs(30))=>None}
                        };
                        let event=async{match &mut events{Some(v)=>v.recv().await,None=>futures::future::pending::<Option<Value>>().await}};
                        let retry_executor=cx.background_executor().clone();
                        let retry_wait=async{if retry{retry_executor.timer(std::time::Duration::from_secs(delay)).await;}else{futures::future::pending::<()>().await;}};
                        tokio::select!{biased;
                            value=auth.recv()=>{let Some(value)=value else{continue 'reconnect;};if Scope::from_value(&value).as_ref()!=Some(&scope){pending=Some(value);continue 'account;}},
                            _=manual.notified()=>{load_projects=true;retry_seconds=2;break;},
                            _=retry_wait=>{load_projects=retry_projects;break;},
                            connection=connect=>{if let Some(connection)=connection{events=Some(connection);}continue;},
                            value=event=>{
                                let Some(value)=value else{events=None;continue;};
                                if !scope.accepts(&value){continue;}
                                let data=&value["data"];
                                let authority_denied=data["code"]=="cloud_event_access_denied"||(data["cacheRevoked"]==true && data["type"]!="reset");
                                let disconnected=matches!(data["type"].as_str(),Some("error"|"closed"));
                                if this.update(cx,|state,cx|{if !scope.matches(state){return;}if let Some(revoked)=data["revokedWorkspaceIds"].as_array(){state.cloud_sidebar.workspaces.retain(|row|!revoked.iter().any(|id|row["workspaceId"]==*id));}if authority_denied{state.sidebar_error(&scope,Part::Projects,&RpcError::Capability("workspace_access_denied".into()));}else if disconnected{state.sidebar_error(&scope,Part::Projects,&RpcError::Transport("Cloud events disconnected".into()));}cx.notify();}).is_err(){return;}
                                if disconnected&&!authority_denied{events=None;retry=true;retry_projects=true;continue;}
                                let refresh=matches!(data["type"].as_str(),Some("connected"|"reset"|"revalidate"))||data["events"].as_array().into_iter().flatten().any(|e|matches!(e["kind"].as_str(),Some("workspace"|"project"|"image-catalog"|"project-catalog"|"github"|"environment"|"connection"|"authentication")));
                                if refresh{load_projects=true;break;}
                            }
                        }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cloud_only_catalog_needs_no_local_chat_and_scope_changes_clear_it() {
        let mut state = AppState::new();
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
        let context = json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[{"projectId":"cloud-project","repositoryIdentity":"github.com/o/app"}]});
        let list = json!({"accountId":"owner","organizationId":null,"workspaces":[{"workspaceId":"task","projectId":"cloud-project","state":"paused"},{"workspaceId":"deleted","state":"deleted"}]});
        assert!(state.apply_cloud_sidebar_catalog(&context, &list));
        assert!(state.chats.is_empty() && state.spaces.is_empty());
        assert_eq!(state.cloud_sidebar_projects().len(), 1);
        assert_eq!(state.cloud_sidebar_workspaces().len(), 1);
        assert_eq!(state.cloud_sidebar_projects()[0]["accountId"], "owner");
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":"another-org"}));
        assert!(state.cloud_sidebar_projects().is_empty());
        assert!(state.cloud_sidebar_workspaces().is_empty());
        assert!(!state.apply_cloud_sidebar_catalog(&context, &list));
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use gpui::AppContext;
    use std::sync::{Arc, Mutex};
    fn projects(provisional: bool, name: &str) -> Value {
        json!({"accountId":"owner","organizationId":null,"apiOrigin":"https://cloud.test","provisional":provisional,"data":{"projects":[{"projectId":name,"repositoryIdentity":"github.com/o/p","canCreate":true}]}})
    }
    fn workspaces(provisional: bool) -> Value {
        json!({"accountId":"owner","organizationId":null,"apiOrigin":"https://cloud.test","provisional":provisional,"workspaces":[{"workspaceId":"saved","state":"paused","initialSessionId":"session"}]})
    }
    #[test]
    fn independent_projects_survive_workspace_failure_and_late_cache_cannot_replace_fresh() {
        let mut state = AppState::new();
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
        assert!(state.apply_sidebar_part(Part::Projects, &projects(true, "cached")));
        assert_eq!(state.cloud_sidebar_projects()[0]["canCreate"], false);
        let scope = Scope::from_value(state.cloud_account.as_ref().unwrap()).unwrap();
        state.sidebar_error(
            &scope,
            Part::Workspaces,
            &RpcError::Transport("offline".into()),
        );
        assert_eq!(state.cloud_sidebar_projects()[0]["projectId"], "cached");
        assert!(state.apply_sidebar_part(Part::Projects, &projects(false, "fresh")));
        assert!(!state.apply_sidebar_part(Part::Projects, &projects(true, "old")));
        assert_eq!(state.cloud_sidebar_projects()[0]["projectId"], "fresh");
        assert!(state.cloud_sidebar_stale());
    }
    #[test]
    fn denial_and_account_org_or_api_changes_do_not_resurrect_saved_catalog() {
        let mut state = AppState::new();
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
        state.apply_sidebar_part(Part::Projects, &projects(true, "cached"));
        state.apply_sidebar_part(Part::Workspaces, &workspaces(true));
        let scope = Scope::from_value(state.cloud_account.as_ref().unwrap()).unwrap();
        state.sidebar_error(
            &scope,
            Part::Projects,
            &RpcError::Failed("You do not have access to this workspace.".into()),
        );
        assert!(
            state.cloud_sidebar_projects().is_empty()
                && state.cloud_sidebar_workspaces().is_empty()
        );
        assert!(!state.apply_sidebar_part(Part::Projects, &projects(true, "late")));
        state.apply_cloud_account(json!({"user":{"id":"another"},"orgId":null}));
        assert!(!state.apply_sidebar_part(Part::Projects, &projects(true, "wrong-account")));
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
        state.apply_sidebar_part(Part::Projects, &projects(false, "current"));
        let mut wrong = projects(true, "other-api");
        wrong["apiOrigin"] = json!("https://other.test");
        assert!(!state.apply_sidebar_part(Part::Projects, &wrong));
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":"organization"}));
        assert!(state.cloud_sidebar_projects().is_empty());
        assert!(!state.apply_sidebar_part(Part::Workspaces, &workspaces(true)));
    }
    struct Controller {
        calls: Mutex<Vec<(String, bool)>>,
        fresh: tokio::sync::Semaphore,
    }
    #[async_trait::async_trait]
    impl RpcService for Controller {
        async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
            self.calls
                .lock()
                .unwrap()
                .push((method.into(), params["cacheOnly"] == true));
            match method {
                methods::CLOUD_ACCOUNT_STATUS => Ok(RpcReply::Stream(Box::pin(
                    futures::stream::once(async { json!({"user":{"id":"owner"},"orgId":null}) })
                        .chain(futures::stream::pending()),
                ))),
                methods::CLOUD_EVENTS => futures::future::pending().await,
                methods::CLOUD_PROJECTS | methods::CLOUD_WORKSPACES => {
                    let cached = params["cacheOnly"] == true;
                    if !cached {
                        self.fresh.acquire().await.unwrap().forget();
                    }
                    Ok(RpcReply::Value(if method == methods::CLOUD_PROJECTS {
                        projects(
                            cached,
                            if cached {
                                "cached-project"
                            } else {
                                "fresh-project"
                            },
                        )
                    } else {
                        workspaces(cached)
                    }))
                }
                // A context failure must never block sidebar metadata.
                methods::CLOUD_CONTEXT => Err(RpcError::Failed("managed base unavailable".into())),
                _ => Err(RpcError::UnknownMethod(method.into())),
            }
        }
    }
    #[gpui::test]
    fn sidebar_paints_scoped_saved_catalog_while_fresh_reads_are_held(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let service = Arc::new(Controller {
            calls: Mutex::new(vec![]),
            fresh: tokio::sync::Semaphore::new(0),
        });
        let handle = EngineHandle::from_test_client(memory_client(service.clone()));
        let state = cx.new(|_| AppState::new());
        let task = state.update(cx, |_, cx| spawn(cx, handle));
        for _ in 0..24 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        state.read_with(cx, |s, _| {
            assert_eq!(s.cloud_sidebar_projects()[0]["projectId"], "cached-project");
            assert_eq!(s.cloud_sidebar_workspaces()[0]["state"], "paused");
            assert!(s.cloud_sidebar_loading() && s.cloud_sidebar_stale());
            assert!(s.chats.is_empty() && s.spaces.is_empty());
        });
        assert!(
            !service
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|(m, _)| m == methods::CLOUD_CONTEXT
                    || m == methods::CLOUD_ENVIRONMENT
                    || m == methods::CLOUD_PAUSE
                    || m == "CloudResume"
                    || m == methods::CLOUD_CREATE)
        );
        service.fresh.add_permits(2);
        for _ in 0..24 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        state.read_with(cx, |s, _| {
            assert_eq!(s.cloud_sidebar_projects()[0]["projectId"], "fresh-project");
            assert!(!s.cloud_sidebar_loading());
            assert!(!s.cloud_sidebar_stale());
        });
        drop(task);
        drop(state);
        for _ in 0..4 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
    }
}
