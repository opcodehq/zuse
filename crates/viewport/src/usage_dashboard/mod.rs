//! One recorded-usage controller and presentation for native and browser hosts.
mod chart;
mod format;
mod provider_menu;
mod scroll;
mod table;
mod view;
use crate::input::TextInput;
use chrono::{DateTime, Duration, Local, TimeZone, Utc};
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use zuse_protocol::{
    AgentAccountsSnapshot, methods,
    usage::{Overview, Request, SessionSort, SessionsPage},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub label: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Properties {
    pub available: bool,
    pub owner: String,
    pub target: Option<String>,
    pub project: Option<Project>,
}
pub enum Event {
    Call {
        id: u64,
        method: &'static str,
        params: Value,
    },
    Cancel(u64),
    ScopeChanged(bool),
    Close,
}
#[derive(Clone, Copy)]
enum Kind {
    Report,
    Sessions,
    Limits,
}
#[derive(Clone)]
enum Action {
    Refresh,
    Scope(bool),
    Period(i64),
    Measure(bool),
    Contributors(usize),
    Expand,
    ToggleSessions,
    ProviderMenu,
    Provider(Option<&'static str>),
    Sort(SessionSort),
    Page(usize),
    Day(usize),
    ClearDay,
}
pub struct UsageDashboard {
    properties: Properties,
    visible: bool,
    project_scope: bool,
    period: i64,
    range: Option<(DateTime<Utc>, DateTime<Utc>, String)>,
    report: Option<Arc<Overview>>,
    report_cache: BTreeMap<String, Arc<Overview>>,
    report_error: Option<String>,
    page: Option<SessionsPage>,
    page_error: Option<String>,
    accounts: AgentAccountsSnapshot,
    limits_error: Option<String>,
    next: u64,
    requests: BTreeMap<u64, Kind>,
    query: Entity<TextInput>,
    query_value: String,
    query_task: Option<Task<()>>,
    sessions_open: bool,
    provider: Option<&'static str>,
    provider_open: bool,
    provider_cursor: usize,
    provider_focus: Option<usize>,
    provider_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    sort: SessionSort,
    offset: usize,
    cost: bool,
    contributor: usize,
    expanded: bool,
    hovered: Option<usize>,
    hover_cursor: Option<f32>,
    scroll: ScrollHandle,
    scroll_drag: Option<f32>,
    plot_cache: std::rc::Rc<std::cell::RefCell<Option<chart::Raster>>>,
    plot_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    focus: BTreeMap<String, FocusHandle>,
    root_focus: FocusHandle,
    _query: Subscription,
}
impl EventEmitter<Event> for UsageDashboard {}
impl UsageDashboard {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextInput::new("Search sessions…", window, cx));
        let observe = cx.observe(&query, |this: &mut Self, query, cx| {
            let value = query.read(cx).value(cx);
            if this.query_value == value {
                return;
            }
            this.query_value = value;
            this.offset = 0;
            this.cancel(Kind::Sessions, cx);
            this.query_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                let _ = this.update(cx, |this, cx| this.load_sessions(cx));
            }));
            cx.notify();
        });
        Self {
            properties: Properties {
                available: false,
                owner: String::new(),
                target: None,
                project: None,
            },
            visible: false,
            project_scope: false,
            period: 7,
            range: None,
            report: None,
            report_cache: Default::default(),
            report_error: None,
            page: None,
            page_error: None,
            accounts: Default::default(),
            limits_error: None,
            next: 0,
            requests: Default::default(),
            query,
            query_value: String::new(),
            query_task: None,
            sessions_open: false,
            provider: None,
            provider_open: false,
            provider_cursor: 0,
            provider_focus: None,
            provider_bounds: Default::default(),
            sort: SessionSort::Tokens,
            offset: 0,
            cost: true,
            contributor: 0,
            expanded: false,
            hovered: None,
            hover_cursor: None,
            scroll: ScrollHandle::new(),
            scroll_drag: None,
            plot_cache: Default::default(),
            plot_bounds: Default::default(),
            focus: Default::default(),
            root_focus: cx.focus_handle(),
            _query: observe,
        }
    }
    pub fn set_properties(&mut self, properties: Properties, cx: &mut Context<Self>) {
        if self.properties == properties {
            return;
        }
        let owner = self.properties.owner != properties.owner
            || self.properties.target != properties.target;
        let changed = owner
            || self.project_scope
                && self.properties.project.as_ref().map(|p| &p.id)
                    != properties.project.as_ref().map(|p| &p.id);
        let availability_changed = self.properties.available != properties.available;
        self.properties = properties;
        if self.project_scope && self.properties.project.is_none() {
            self.project_scope = false;
            cx.emit(Event::ScopeChanged(false));
        }
        if !self.properties.available {
            self.cancel_all(cx);
        }
        if changed {
            self.cancel_all(cx);
            if owner {
                self.report_cache.clear();
                self.report = None;
            }
            self.page = None;
            self.report_error = None;
            self.page_error = None;
            self.hovered = None;
            self.hover_cursor = None;
            self.provider_open = false;
            self.provider_focus = None;
            if owner {
                self.accounts = Default::default();
                self.limits_error = None;
            }
        }
        if self.visible && (changed || availability_changed) {
            self.refresh(false, cx);
        }
        cx.notify();
    }
    pub fn open(&mut self, properties: Properties, project: bool, cx: &mut Context<Self>) {
        self.hide(cx);
        if self.properties.owner != properties.owner || self.properties.target != properties.target
        {
            self.accounts = Default::default();
            self.limits_error = None;
            self.report_cache.clear();
            self.report = None;
        }
        self.cost = true;
        self.contributor = 0;
        self.expanded = false;
        self.sessions_open = false;
        self.provider = None;
        self.sort = SessionSort::Tokens;
        self.query
            .update(cx, |input, cx| input.set_value(String::new(), cx));
        self.properties = properties;
        self.project_scope = project && self.properties.project.is_some();
        cx.emit(Event::ScopeChanged(self.project_scope));
        self.page = None;
        self.report_error = None;
        self.page_error = None;
        self.hovered = None;
        self.hover_cursor = None;
        self.provider_open = false;
        self.scroll.set_offset(Point::default());
        self.scroll_drag = None;
        self.visible = true;
        self.offset = 0;
        self.refresh(false, cx);
        cx.notify();
    }
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        self.cancel_all(cx);
        self.query_task = None;
        self.provider_open = false;
        self.provider_focus = None;
        self.hovered = None;
        self.hover_cursor = None;
        cx.notify();
    }
    pub fn visible(&self) -> bool {
        self.visible
    }
    pub fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.provider_open {
            self.provider_open = false;
            if let Some(focus) = self.focus.get("usage-provider") {
                window.focus(focus, cx);
            }
        } else {
            cx.emit(Event::Close);
        }
        cx.notify();
    }
    fn pending(&self, kind: Kind) -> bool {
        self.requests
            .values()
            .any(|k| std::mem::discriminant(k) == std::mem::discriminant(&kind))
    }
    fn cancel(&mut self, kind: Kind, cx: &mut Context<Self>) {
        let ids: Vec<_> = self
            .requests
            .iter()
            .filter(|(_, k)| std::mem::discriminant(*k) == std::mem::discriminant(&kind))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.requests.remove(&id);
            cx.emit(Event::Cancel(id));
        }
    }
    fn cancel_all(&mut self, cx: &mut Context<Self>) {
        for id in std::mem::take(&mut self.requests).into_keys() {
            cx.emit(Event::Cancel(id));
        }
    }
    fn request(
        &mut self,
        kind: Kind,
        method: &'static str,
        mut params: Value,
        cx: &mut Context<Self>,
    ) {
        if !self.visible || self.pending(kind) {
            return;
        }
        if !self.properties.available {
            match kind {
                Kind::Report => {
                    self.report_error =
                        Some("Computer unavailable. Reconnect and refresh usage.".into())
                }
                Kind::Sessions => {
                    self.page_error = Some("Computer unavailable. Reconnect and retry.".into())
                }
                Kind::Limits => self.limits_error = Some("Computer unavailable.".into()),
            };
            cx.notify();
            return;
        }
        if let Some(target) = &self.properties.target {
            params["targetDeviceId"] = json!(target);
        }
        self.next += 1;
        let id = self.next;
        self.requests.insert(id, kind);
        cx.emit(Event::Call { id, method, params });
    }
    fn query_request(&self, cx: &App) -> Request {
        let since = Local::now()
            .date_naive()
            .checked_sub_signed(Duration::days(self.period - 1))
            .and_then(|day| {
                Local
                    .from_local_datetime(&day.and_hms_opt(0, 0, 0)?)
                    .earliest()
            })
            .map(|date| date.with_timezone(&Utc));
        Request {
            since: self.range.as_ref().map(|r| r.0).or(since),
            until: self.range.as_ref().map(|r| r.1),
            project_id: self
                .project_scope
                .then(|| self.properties.project.as_ref().map(|p| p.id.clone()))
                .flatten(),
            query: self.query.read(cx).value(cx).trim().to_owned(),
            provider_id: self.provider.map(str::to_owned),
            sort: self.sort,
            offset: self.offset,
            limit: Some(10),
            ..Default::default()
        }
    }
    fn refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        self.report_error = None;
        if let Some(report) = self.report_cache.get(&self.report_key()).cloned() {
            self.report = Some(report);
        }
        self.cancel(Kind::Report, cx);
        let mut request = self.query_request(cx);
        request.force_refresh = force;
        self.request(
            Kind::Report,
            zuse_protocol::usage::OVERVIEW,
            serde_json::to_value(request).unwrap(),
            cx,
        );
        self.limits_error = None;
        self.request(
            Kind::Limits,
            methods::LIST_AGENT_ACCOUNTS,
            json!({"forceUsage":force}),
            cx,
        );
        if self.sessions_open {
            self.cancel(Kind::Sessions, cx);
            self.load_sessions(cx);
        }
    }
    fn load_sessions(&mut self, cx: &mut Context<Self>) {
        if !self.visible || !self.sessions_open {
            return;
        }
        self.page_error = None;
        let params = serde_json::to_value(self.query_request(cx)).unwrap();
        self.request(Kind::Sessions, zuse_protocol::usage::SESSIONS, params, cx);
        cx.notify();
    }
    pub fn receive(&mut self, id: u64, result: Result<Value, String>, cx: &mut Context<Self>) {
        let Some(kind) = self.requests.remove(&id) else {
            return;
        };
        match kind {
            Kind::Report => match result.and_then(|v| {
                serde_json::from_value::<Overview>(v)
                    .map_err(|_| "Usage report could not be read. Refresh to try again.".into())
            }) {
                Ok(report) => {
                    let report = Arc::new(report);
                    self.report_cache.insert(self.report_key(), report.clone());
                    self.report = Some(report);
                    self.report_error = None;
                }
                Err(e) => self.report_error = Some(e),
            },
            Kind::Sessions => match result.and_then(|v| {
                serde_json::from_value::<SessionsPage>(v)
                    .map_err(|_| "Saved sessions could not be read. Retry to load them.".into())
            }) {
                Ok(page) => {
                    self.page = Some(page);
                    self.page_error = None;
                }
                Err(e) => self.page_error = Some(e),
            },
            Kind::Limits => match result.and_then(|v| {
                serde_json::from_value::<AgentAccountsSnapshot>(v)
                    .map_err(|_| "Usage limits are unavailable.".into())
            }) {
                Ok(accounts) => {
                    self.accounts = accounts;
                    self.limits_error = None;
                }
                Err(e) => self.limits_error = Some(e),
            },
        }
        cx.notify();
    }
    fn action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::Refresh => {
                if !self.pending(Kind::Report) {
                    self.refresh(true, cx);
                }
            }
            Action::Scope(project) => {
                if self.project_scope == project || project && self.properties.project.is_none() {
                    return;
                }
                self.project_scope = project;
                cx.emit(Event::ScopeChanged(project));
                self.page = None;
                self.offset = 0;
                self.hovered = None;
                self.hover_cursor = None;
                self.refresh(false, cx);
            }
            Action::Period(period) => {
                self.period = period;
                self.range = None;
                self.page = None;
                self.offset = 0;
                self.hovered = None;
                self.hover_cursor = None;
                self.refresh(false, cx);
            }
            Action::Measure(cost) => {
                self.cost = cost;
                self.hovered = None;
                self.hover_cursor = None;
            }
            Action::Contributors(tab) => {
                self.contributor = tab;
                self.expanded = false;
            }
            Action::Expand => self.expanded = !self.expanded,
            Action::ToggleSessions => {
                self.sessions_open = !self.sessions_open;
                self.provider_open = false;
                if self.sessions_open {
                    self.load_sessions(cx);
                } else {
                    self.cancel(Kind::Sessions, cx);
                }
            }
            Action::ProviderMenu => {
                self.provider_open = !self.provider_open;
                self.provider_cursor = self
                    .provider
                    .and_then(|id| view::PROVIDERS.iter().position(|p| p.1 == id))
                    .map(|i| i + 1)
                    .unwrap_or(0);
                self.provider_focus = self.provider_open.then_some(self.provider_cursor);
            }
            Action::Provider(provider) => {
                if let Some(focus) = self.focus.get("usage-provider") {
                    window.focus(focus, cx);
                }
                self.provider = provider;
                self.provider_open = false;
                self.provider_focus = None;
                self.query_task = None;
                self.offset = 0;
                self.cancel(Kind::Sessions, cx);
                self.load_sessions(cx);
            }
            Action::Sort(sort) => {
                self.sort = sort;
                self.offset = 0;
                self.cancel(Kind::Sessions, cx);
                self.load_sessions(cx);
            }
            Action::Page(offset) => {
                if offset == self.offset || self.pending(Kind::Sessions) {
                    return;
                }
                self.offset = offset;
                self.cancel(Kind::Sessions, cx);
                self.load_sessions(cx);
            }
            Action::Day(index) => {
                window.focus(&self.root_focus, cx);
                if let Some(group) = self
                    .report
                    .as_ref()
                    .and_then(|r| r.groups.get(r.groups.len().saturating_sub(90) + index))
                {
                    if let Some(since) = group.started_at {
                        self.range = Some((
                            since,
                            group.ended_at.unwrap_or(since + Duration::days(1)),
                            group.label.clone(),
                        ));
                        self.page = None;
                        self.offset = 0;
                        self.hovered = None;
                        self.hover_cursor = None;
                        self.refresh(false, cx);
                    }
                }
            }
            Action::ClearDay => {
                window.focus(&self.root_focus, cx);
                self.range = None;
                self.page = None;
                self.offset = 0;
                self.hovered = None;
                self.hover_cursor = None;
                self.refresh(false, cx);
            }
        }
        if self.provider_open {
            if let Some(focus) = self.focus.get("usage-provider") {
                window.focus(focus, cx);
            }
        }
        cx.notify();
    }
    fn report_key(&self) -> String {
        json!([
            self.period,
            if self.project_scope {
                self.properties.project.as_ref().map(|p| &p.id)
            } else {
                None
            },
            self.range.as_ref().map(|r| (r.0, r.1)),
        ])
        .to_string()
    }
}
impl Render for UsageDashboard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.view(window, cx)
    }
}
