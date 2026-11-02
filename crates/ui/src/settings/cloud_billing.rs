//! Reference Cloud → Usage and billing, backed by the host account authority.
use crate::{settings::widgets, state::AppState, theme::Theme};
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use zuse_model::cloud_billing::{cap_dollars, cap_micros};
use zuse_protocol::cloud_billing::{BillingSummary, BillingUsageItem, BillingUsagePage};
use zuse_rpc::methods;
type Owner = (String, Option<String>);
fn active_owner(state: &AppState) -> Option<Owner> {
    Some((
        state.cloud_user_id()?.to_owned(),
        state.cloud_account.as_ref()?["orgId"]
            .as_str()
            .map(str::to_owned),
    ))
}

fn request_params(owner: &Owner, mut params: Value) -> Value {
    params["expectedAccountId"] = json!(owner.0);
    params["expectedOrganizationId"] = json!(owner.1);
    params
}
fn bound(value: &Value, owner: &Owner) -> bool {
    value["accountId"].as_str() == Some(owner.0.as_str())
        && value["organizationId"].as_str() == owner.1.as_deref()
}
pub struct CloudBillingPage {
    tab: zuse_viewport::cloud_settings::Tab,
    tab_focus: [FocusHandle; 2],
    repository_panel: Entity<zuse_viewport::cloud_repositories::RepositoryPanel>,
    slack_panel: Entity<zuse_viewport::slack_settings::SlackPanel>,
    slack_tasks: std::collections::HashMap<u64, Task<()>>,
    repository_tasks: std::collections::HashMap<u64, Task<()>>,
    state: Entity<AppState>,
    owner: Option<Owner>,
    loaded: bool,
    account_task: Option<Task<()>>,
    account_error: Option<String>,
    task: Option<Task<()>>,
    portal_task: Option<Task<()>>,
    subscription_task: Option<Task<()>>,
    subscribed: Option<bool>,
    credits: Option<zuse_protocol::cloud_billing::PrepaidBalance>,
    credit_task: Option<Task<()>>,
    credit_amount: i64,
    credit_error: Option<String>,
    summary: Option<BillingSummary>,
    items: Vec<BillingUsageItem>,
    cursor: Option<String>,
    error: Option<String>,
    notice: Option<String>,
    cap: Entity<zuse_viewport::input::TextInput>,
    credit_focus: [FocusHandle; 4],
    workspaces: Vec<Value>,
    history_open: bool,
    workspace_task: Option<Task<()>>,
    cap_request: Option<(i64, String)>,
}
impl CloudBillingPage {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        zuse_viewport::input::bind_keys(cx);
        let slack_panel = cx.new(|cx| zuse_viewport::slack_settings::SlackPanel::new(window, cx));
        cx.observe(&slack_panel, |_, _, cx| cx.notify()).detach();
        cx.subscribe(
            &slack_panel,
            |this, panel, event: &zuse_viewport::cloud_repositories::Request, cx| match event {
                zuse_viewport::cloud_repositories::Request::Cancel { id } => {
                    this.slack_tasks.remove(id);
                }
                zuse_viewport::cloud_repositories::Request::Call { id, method, params } => {
                    let Some(engine) = this.state.read(cx).engine().map(|e| e.controller()) else {
                        panel.update(cx, |p, cx| {
                            p.receive(*id, Err("Account connection unavailable.".into()), cx)
                        });
                        return;
                    };
                    let id = *id;
                    let method = *method;
                    let params = params.clone();
                    let panel = panel.clone();
                    let job = gpui_tokio::Tokio::spawn(cx, async move {
                        engine
                            .client()
                            .call_scoped(method, params)
                            .await
                            .map_err(|e| e.to_string())
                    });
                    let task = cx.spawn(async move |this, cx| {
                        let result = job.await.unwrap_or_else(|e| Err(e.to_string()));
                        let _ = this.update(cx, |this, cx| {
                            this.slack_tasks.remove(&id);
                            panel.update(cx, |p, cx| p.receive(id, result, cx));
                        });
                    });
                    this.slack_tasks.insert(id, task);
                }
            },
        )
        .detach();
        let repository_panel =
            cx.new(|cx| zuse_viewport::cloud_repositories::RepositoryPanel::new(window, cx));
        cx.observe(&repository_panel, |_, _, cx| cx.notify())
            .detach();
        cx.subscribe(
            &repository_panel,
            |this, panel, event: &zuse_viewport::cloud_repositories::Request, cx| {
                use zuse_viewport::cloud_repositories::Request;
                match event {
                    Request::Cancel { id } => {
                        this.repository_tasks.remove(id);
                    }
                    Request::Call { id, method, params } => {
                        let Some(engine) = this.state.read(cx).engine().map(|e| e.controller())
                        else {
                            panel.update(cx, |panel, cx| {
                                panel.receive(
                                    *id,
                                    Err("The account connection is unavailable.".into()),
                                    cx,
                                )
                            });
                            return;
                        };
                        let id = *id;
                        let method = *method;
                        let params = params.clone();
                        let panel = panel.clone();
                        let job = gpui_tokio::Tokio::spawn(cx, async move {
                            engine
                                .client()
                                .call_scoped(method, params)
                                .await
                                .map_err(|e| e.to_string())
                        });
                        let task = cx.spawn(async move |this, cx| {
                            let result = job.await.unwrap_or_else(|e| Err(e.to_string()));
                            let _ = this.update(cx, |this, cx| {
                                this.repository_tasks.remove(&id);
                                panel.update(cx, |panel, cx| panel.receive(id, result, cx));
                            });
                        });
                        this.repository_tasks.insert(id, task);
                    }
                }
            },
        )
        .detach();
        let cap = cx.new(|cx| {
            zuse_viewport::input::TextInput::new("Monthly overage cap in dollars", window, cx)
        });
        cx.observe(&state, |_: &mut Self, _, cx| cx.notify())
            .detach();
        Self {
            tab: Default::default(),
            tab_focus: std::array::from_fn(|_| cx.focus_handle()),
            slack_panel,
            slack_tasks: Default::default(),
            repository_panel,
            repository_tasks: Default::default(),
            state,
            owner: None,
            loaded: false,
            account_task: None,
            account_error: None,
            task: None,
            portal_task: None,
            subscription_task: None,
            subscribed: None,
            credits: None,
            credit_task: None,
            credit_amount: 2500,
            credit_error: None,
            summary: None,
            items: vec![],
            cursor: None,
            error: None,
            notice: None,
            cap,
            credit_focus: std::array::from_fn(|_| cx.focus_handle()),
            workspaces: vec![],
            history_open: false,
            workspace_task: None,
            cap_request: None,
        }
    }
    fn sign_in(&mut self, cx: &mut Context<Self>) {
        if self.account_task.is_some() {
            return;
        }
        if let Some(url) = self
            .state
            .read(cx)
            .cloud_account
            .as_ref()
            .and_then(|status| status["login"]["url"].as_str())
            .filter(|url| url.starts_with("https://"))
        {
            cx.open_url(url);
            return;
        }
        let Some(engine) = self
            .state
            .read(cx)
            .engine()
            .map(|engine| engine.controller())
        else {
            return;
        };
        self.account_error = None;
        self.account_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_scoped(methods::CLOUD_ACCOUNT_SIGN_IN, json!({}))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.account_task = None;
                match result {
                    Ok(value) => {
                        if let Some(url) = value["url"]
                            .as_str()
                            .filter(|url| url.starts_with("https://"))
                        {
                            cx.open_url(url);
                        }
                    }
                    Err(error) => this.account_error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn load(&mut self, more: bool, cx: &mut Context<Self>) {
        if self.task.is_some() {
            return;
        }
        let Some(owner) = active_owner(self.state.read(cx)) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().map(|e| e.controller()) else {
            return;
        };
        if !more {
            self.load_workspaces(cx);
            self.load_subscriptions(cx);
            self.load_credits(false, cx);
        }
        self.loaded = true;
        self.error = None;
        let cursor = if more { self.cursor.clone() } else { None };
        let expected = owner.clone();
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            futures::try_join!(
                engine.client().call_scoped(
                    methods::CLOUD_BILLING_SUMMARY,
                    request_params(&expected, json!({}))
                ),
                engine.client().call_scoped(
                    methods::CLOUD_BILLING_USAGE,
                    request_params(&expected, json!({"limit":10,"cursor":cursor}))
                )
            )
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if active_owner(this.state.read(cx)).as_ref() != Some(&owner) {
                    return;
                }
                this.task = None;
                match result {
                    Ok(Ok((summary, page))) if bound(&summary, &owner) && bound(&page, &owner) => {
                        let data =
                            serde_json::from_value::<BillingSummary>(summary["data"].clone())
                                .and_then(|summary| {
                                    serde_json::from_value::<BillingUsagePage>(page["data"].clone())
                                        .map(|page| (summary, page))
                                });
                        match data {
                            Ok((summary, page)) => {
                                if !more {
                                    this.items.clear();
                                    this.cap.update(cx, |input, cx| {
                                        input.set_value(cap_dollars(summary.overage_cap_micros), cx)
                                    });
                                }
                                for item in page.items {
                                    if !this.items.iter().any(|old| old.entry_id == item.entry_id) {
                                        this.items.push(item);
                                    }
                                }
                                this.cursor = page.next_cursor;
                                this.summary = Some(summary);
                            }
                            Err(_) => {
                                this.error = Some(
                                    "Cloud billing returned an invalid response. Refresh to retry."
                                        .into(),
                                )
                            }
                        }
                    }
                    Ok(Err(error)) => {
                        let message = error.to_string();
                        if this.subscribed != Some(false)
                            || message != "An active cloud billing period is required."
                        {
                            this.error = Some(message);
                        }
                    }
                    _ => {
                        this.error = Some("Could not load cloud billing. Refresh to retry.".into())
                    }
                }
                cx.notify();
            });
        }));
    }
    fn load_workspaces(&mut self, cx: &mut Context<Self>) {
        if self.workspace_task.is_some() {
            return;
        }
        let Some(owner) = active_owner(self.state.read(cx)).filter(|(_, org)| org.is_none()) else {
            return;
        };
        let Some(engine) = self
            .state
            .read(cx)
            .engine()
            .map(|engine| engine.controller())
        else {
            return;
        };
        let expected = owner.clone();
        self.workspace_task = Some(cx.spawn(async move |this, cx| {
            let value = engine
                .client()
                .call_scoped(
                    methods::CLOUD_WORKSPACES,
                    request_params(&expected, json!({})),
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                if active_owner(this.state.read(cx)).as_ref() != Some(&owner) {
                    return;
                }
                this.workspace_task = None;
                match value {
                    Ok(value) if bound(&value, &owner) => {
                        this.workspaces =
                            value["workspaces"].as_array().cloned().unwrap_or_default()
                    }
                    Err(error) => this.error = Some(error.to_string()),
                    _ => {
                        this.error =
                            Some("Could not load cloud workspaces. Refresh to retry.".into())
                    }
                }
                cx.notify();
            });
        }));
    }
    fn load_subscriptions(&mut self, cx: &mut Context<Self>) {
        if self.subscription_task.is_some() {
            return;
        }
        let Some(owner) = active_owner(self.state.read(cx)) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().map(|e| e.controller()) else {
            return;
        };
        let expected = owner.clone();
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            engine
                .client()
                .call_scoped(
                    methods::CLOUD_BILLING_ENTITLEMENTS,
                    request_params(&expected, json!({})),
                )
                .await
        });
        self.subscription_task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if active_owner(this.state.read(cx)).as_ref() != Some(&owner) {
                    return;
                }
                this.subscription_task = None;
                match result {
                    Ok(Ok(value)) if bound(&value, &owner) => {
                        this.subscribed = zuse_model::cloud_billing::cloud_subscribed(
                            &value["data"],
                            chrono::Utc::now().timestamp_millis(),
                        );
                        if this.subscribed == Some(false)
                            && this.error.as_deref()
                                == Some("An active cloud billing period is required.")
                        {
                            this.error = None;
                        }
                        if this.subscribed.is_none() {
                            this.error =
                                Some("Could not read the subscription. Refresh to retry.".into());
                        }
                    }
                    Ok(Err(e)) => this.error = Some(e.to_string()),
                    _ => {
                        this.error =
                            Some("Could not load the subscription. Refresh to retry.".into())
                    }
                }
                cx.notify();
            });
        }));
    }
    fn billing_link(&mut self, force_portal: bool, cx: &mut Context<Self>) {
        if self.portal_task.is_some() {
            return;
        }
        let Some(owner) = active_owner(self.state.read(cx)) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().map(|e| e.controller()) else {
            return;
        };
        let subscribed = if force_portal {
            true
        } else if let Some(value) = self.subscribed {
            value
        } else {
            return;
        };
        self.error = None;
        let expected = owner.clone();
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            engine
                .client()
                .call_scoped(
                    if subscribed {
                        methods::CLOUD_BILLING_PORTAL
                    } else {
                        methods::CLOUD_BILLING_CHECKOUT
                    },
                    request_params(
                        &expected,
                        if subscribed {
                            json!({})
                        } else {
                            json!({"offerId":"cloud-workspace-standard-v1"})
                        },
                    ),
                )
                .await
        });
        self.portal_task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if active_owner(this.state.read(cx)).as_ref() != Some(&owner) {
                    return;
                }
                this.portal_task = None;
                match result {
                    Ok(Ok(value)) if bound(&value, &owner) => {
                        if let Some(url) = value["data"][if subscribed {
                            "portalUrl"
                        } else {
                            "checkoutUrl"
                        }]
                        .as_str()
                        .filter(|url| url.starts_with("https://"))
                        {
                            cx.open_url(url);
                        } else {
                            this.error = Some(
                                "The billing provider returned an invalid portal link.".into(),
                            );
                        }
                    }
                    Ok(Err(error)) => this.error = Some(error.to_string()),
                    _ => this.error = Some("Could not open billing. Try again.".into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn load_credits(&mut self, buy: bool, cx: &mut Context<Self>) {
        if self.credit_task.is_some() {
            return;
        }
        if buy && !self.credits.as_ref().is_some_and(|v| v.available) {
            return;
        }
        let Some(owner) = active_owner(self.state.read(cx)) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().map(|e| e.controller()) else {
            return;
        };
        let amount = self.credit_amount;
        self.credit_error = None;
        let expected = owner.clone();
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            engine
                .client()
                .call_scoped(
                    if buy {
                        methods::CLOUD_PREPAID_CHECKOUT
                    } else {
                        methods::CLOUD_PREPAID_BALANCE
                    },
                    request_params(
                        &expected,
                        if buy {
                            json!({"amountCents":amount})
                        } else {
                            json!({})
                        },
                    ),
                )
                .await
        });
        self.credit_task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if active_owner(this.state.read(cx)).as_ref() != Some(&owner) {
                    return;
                }
                this.credit_task = None;
                match result {
                    Ok(Ok(value)) if bound(&value, &owner) => {
                        if buy {
                            if let Some(url) = value["data"]["checkoutUrl"]
                                .as_str()
                                .filter(|s| s.starts_with("https://"))
                            {
                                cx.open_url(url);
                            } else {
                                this.credit_error = Some(
                                    "The provider returned an invalid credit checkout link.".into(),
                                );
                            }
                        } else {
                            match serde_json::from_value(value["data"].clone()) {
                                Ok(balance) => this.credits = Some(balance),
                                Err(_) => {
                                    this.credit_error = Some(
                                        "Could not read the prepaid balance. Refresh to retry."
                                            .into(),
                                    )
                                }
                            }
                        }
                    }
                    Ok(Err(e)) => this.credit_error = Some(e.to_string()),
                    _ => {
                        this.credit_error =
                            Some("Could not load prepaid credits. Refresh to retry.".into())
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        if self.task.is_some() {
            return;
        }
        let Some(amount) = cap_micros(&self.cap.read(cx).value(cx)) else {
            self.error =
                Some("Enter a nonnegative dollar amount with up to six decimal places.".into());
            cx.notify();
            return;
        };
        let Some(owner) = active_owner(self.state.read(cx)) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().map(|e| e.controller()) else {
            return;
        };
        if !self
            .cap_request
            .as_ref()
            .is_some_and(|(value, _)| *value == amount)
        {
            self.cap_request = Some((amount, uuid::Uuid::new_v4().to_string()));
        }
        let key = self.cap_request.as_ref().unwrap().1.clone();
        self.error = None;
        self.notice = None;
        let expected = owner.clone();
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            engine
                .client()
                .call_scoped(
                    methods::CLOUD_BILLING_CAP,
                    request_params(
                        &expected,
                        json!({"overageCapMicros":amount,"idempotencyKey":key}),
                    ),
                )
                .await
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if active_owner(this.state.read(cx)).as_ref() != Some(&owner) {
                    return;
                }
                this.task = None;
                match result {
                    Ok(Ok(value)) if bound(&value, &owner) => {
                        match serde_json::from_value::<BillingSummary>(value["data"].clone()) {
                            Ok(summary) => {
                                this.cap.update(cx, |input, cx| {
                                    input.set_value(cap_dollars(summary.overage_cap_micros), cx)
                                });
                                this.summary = Some(summary);
                                this.cap_request = None;
                                this.notice = Some("Monthly overage cap saved.".into());
                            }
                            Err(_) => this.error = Some(
                                "Could not verify the saved cap. Refresh before changing it again."
                                    .into(),
                            ),
                        }
                    }
                    Ok(Err(error)) => this.error = Some(error.to_string()),
                    _ => {
                        this.error = Some(
                            "The cap request was interrupted. Retry to check the same request."
                                .into(),
                        )
                    }
                }
                cx.notify();
            });
        }));
    }
}
impl Render for CloudBillingPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = active_owner(self.state.read(cx));
        if self.owner != active {
            self.owner = active;
            self.tab = Default::default();
            self.loaded = false;
            self.task = None;
            self.portal_task = None;
            self.subscription_task = None;
            self.subscribed = None;
            self.credits = None;
            self.credit_task = None;
            self.credit_amount = 2500;
            self.credit_error = None;
            self.summary = None;
            self.workspaces.clear();
            self.history_open = false;
            self.workspace_task = None;
            self.items.clear();
            self.cursor = None;
            self.error = None;
            self.notice = None;
            self.cap_request = None;
            self.cap.update(cx, |i, cx| i.set_value("".into(), cx));
        }
        if !self.loaded {
            self.load(false, cx);
        }
        self.repository_panel
            .update(cx, |panel, cx| panel.bind_owner(self.owner.clone(), cx));
        self.slack_panel
            .update(cx, |panel, cx| panel.bind_owner(self.owner.clone(), cx));
        let theme = Theme::of(cx).for_settings_surface();
        zuse_viewport::palette::set_light(theme.appearance.is_light());
        let busy = self.task.is_some();
        let mut body = widgets::settings_page_column(window).child(widgets::section_title_with_action(
            &theme,
            "Cloud workspaces (Beta)",
            None,
            "Connect GitHub and your coding agents, then keep work running when this app is closed.",
            None,
        ));
        if self.owner.is_none() {
            if self.state.read(cx).cloud_account.is_some() {
                let pending = self.account_task.is_some();
                let login = self
                    .state
                    .read(cx)
                    .cloud_account
                    .as_ref()
                    .is_some_and(|value| value["login"].is_object());
                let error = self.account_error.as_deref().or_else(|| {
                    self.state
                        .read(cx)
                        .cloud_account
                        .as_ref()
                        .and_then(|value| value["error"].as_str())
                });
                body = body.child(
                    zuse_viewport::cloud_settings::account_gate(
                        zuse_viewport::cloud_settings::sign_in_button(
                            "cloud-account-sign-in",
                            if pending {
                                "Signing in…"
                            } else if login {
                                "Continue sign-in"
                            } else {
                                "Sign in"
                            },
                        )
                        .when(pending, |view| view.opacity(0.6))
                        .on_click(cx.listener(|this, _, _, cx| this.sign_in(cx))),
                        error,
                    )
                    .mt(px(20.)),
                );
            }
            return body.into_any_element();
        }
        use zuse_viewport::cloud_billing_view as view;
        let data = view::Data {
            subscribed: self.subscribed,
            pending: view::Pending {
                loading: busy || self.subscription_task.is_some(),
                subscription: self.portal_task.is_some(),
                credits: self.credit_task.is_some(),
                cap: busy && self.cap_request.is_some(),
            },
            summary: self.summary.as_ref(),
            usage: &self.items,
            credits: self.credits.as_ref(),
            credit_amount: self.credit_amount,
            error: self.error.as_deref(),
            credit_error: self.credit_error.as_deref(),
            more: self.cursor.is_some(),
            history_open: self.history_open,
            workspaces: &self.workspaces,
            personal: self.owner.as_ref().is_some_and(|(_, org)| org.is_none()),
        };
        let action = |this: &mut Self, action, _window: &mut Window, cx: &mut Context<Self>| {
            match action {
                view::Action::Refresh => this.load(false, cx),
                view::Action::RefreshCredits => this.load_credits(false, cx),
                view::Action::Subscription => this.billing_link(false, cx),
                view::Action::ManageSubscription => this.billing_link(true, cx),
                view::Action::SaveCap => this.save(cx),
                view::Action::SelectCredit(amount) => this.credit_amount = amount,
                view::Action::BuyCredits => this.load_credits(true, cx),
                view::Action::MoreUsage => this.load(true, cx),
                view::Action::ToggleUsageHistory => this.history_open = !this.history_open,
            }
            cx.notify();
        };
        let mut content = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .mt(px(20.))
            .child(view::hosting(
                &data,
                if data.personal {
                    "Personal workspace"
                } else {
                    "Organization workspace"
                },
                cx,
                action,
            ));
        if self.subscribed == Some(true) {
            content = content.child(zuse_viewport::cloud_settings::tabs(
                self.tab,
                &self.tab_focus,
                cx,
                |this: &mut Self, tab, _, cx| {
                    this.tab = tab;
                    if tab == zuse_viewport::cloud_settings::Tab::Usage {
                        this.load(false, cx);
                    }
                    cx.notify();
                },
            ));
        }
        let storage = self.repository_panel.update(cx, |panel, cx| {
            panel.set_usage(
                self.tab == zuse_viewport::cloud_settings::Tab::Usage,
                self.summary.as_ref().map(|s| s.storage_cost_micros),
                cx,
            )
        });
        if self.tab == zuse_viewport::cloud_settings::Tab::Setup {
            if self.subscribed == Some(true) {
                content = content.child(self.repository_panel.clone());
            }
            content = content.child(self.slack_panel.clone());
        } else {
            if storage {
                content = content.child(self.repository_panel.clone());
            }
            content = content.child(view::usage(
                &data,
                self.cap.clone().into_any_element(),
                &self.credit_focus,
                cx,
                action,
            ));
        }
        body = body.child(content);
        div()
            .id("cloud-billing-settings")
            .size_full()
            .overflow_y_scroll()
            .child(body)
            .into_any_element()
    }
}
