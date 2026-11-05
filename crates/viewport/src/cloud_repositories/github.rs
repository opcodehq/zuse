//! One pending GitHub transaction, shared by native and browser settings.
use super::*;
impl RepositoryPanel {
    pub(super) fn github_started(&mut self, data: Value, cx: &mut Context<Self>) {
        if let Some(id) = data["operationId"].as_str() {
            self.github_operation =
                Some(json!({"operationId":id,"state":"authorizing","expiresAt":data["expiresAt"]}));
            self.schedule_github_poll(cx);
        }
        if let Some(url) = data["url"]
            .as_str()
            .filter(|url| url.starts_with("https://"))
        {
            cx.open_url(url);
            self.notice = Some(
                if self.github_operation.is_some() {
                    "Finish signing in with GitHub in your browser. Zuse will continue here."
                } else {
                    "Finish connecting GitHub in your browser, then click Refresh here."
                }
                .into(),
            );
        } else {
            self.error =
                Some("GitHub returned an invalid connection link. Refresh and try again.".into());
        }
    }
    pub(super) fn github_status(&mut self, data: Value, cx: &mut Context<Self>) {
        if self
            .github_operation
            .as_ref()
            .and_then(|op| op["operationId"].as_str())
            != data["operationId"].as_str()
        {
            return;
        }
        if data["state"] == "complete" {
            self.github_operation = None;
            self.github_poll = None;
            self.refresh(cx);
            return;
        }
        self.github_operation = Some(data);
        self.schedule_github_poll(cx);
    }
    pub(super) fn github_action(
        &mut self,
        kind: &'static str,
        action: &str,
        installation: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self
            .github_operation
            .as_ref()
            .and_then(|op| op["operationId"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        if self.pending.values().any(|p| p.kind == kind) {
            return;
        }
        self.error = None;
        self.call(
            kind,
            methods::CLOUD_GITHUB_INSTALL,
            json!({"action":action,"operationId":id,"installationId":installation}),
            cx,
        );
        cx.notify();
    }
    fn schedule_github_poll(&mut self, cx: &mut Context<Self>) {
        if self.account_feed_live {
            return;
        }
        if self.github_poll.is_some() || self.pending.values().any(|p| p.kind == "github-status") {
            return;
        }
        let Some(operation) = &self.github_operation else {
            return;
        };
        if !matches!(
            operation["state"].as_str(),
            Some("created" | "authorizing" | "exchanging")
        ) {
            return;
        }
        if operation["expiresAt"]
            .as_i64()
            .is_none_or(|expiry| expiry <= chrono::Utc::now().timestamp_millis())
        {
            if let Some(operation) = &mut self.github_operation {
                operation["state"] = json!("expired");
            }
            return;
        }
        self.github_poll = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(2))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.github_poll = None;
                this.github_action("github-status", "status", None, cx);
            });
        }));
    }
    pub(super) fn github_disconnect_row(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.github_native {
            return None;
        }
        let busy = self.repository_busy();
        if self.github_connection_state == "revoking" {
            return Some(cs::setting_row(
                false,
                "Finish disconnecting GitHub",
                "GitHub has not confirmed revocation yet. Retry to finish safely.",
                Some(
                    cs::button(
                        "retry-github-disconnect",
                        "Retry disconnect",
                        false,
                        busy,
                        busy,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.disconnect_github(cx)))
                    .into_any_element(),
                ),
            ));
        }
        if self.github_connection_state != "connected" {
            return None;
        }
        let controls = if self.github_disconnect_confirm {
            row()
                .gap(px(8.))
                .child(
                    cs::button(
                        "confirm-github-disconnect",
                        "Disconnect GitHub",
                        false,
                        busy,
                        busy,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.disconnect_github(cx))),
                )
                .child(
                    cs::button(
                        "keep-github-connected",
                        "Keep connected",
                        false,
                        busy,
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.github_disconnect_confirm = false;
                        cx.notify();
                    })),
                )
        } else {
            row().child(
                cs::button("ask-github-disconnect", "Disconnect…", false, busy, false).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.github_disconnect_confirm = true;
                        cx.notify();
                    }),
                ),
            )
        };
        Some(cs::setting_row_content(
            false,
            "GitHub connection",
            if self.github_disconnect_confirm {
                "This revokes your GitHub authorization and scoped Git credentials. Reconnect to use private repositories."
            } else {
                "Manage your own authorization without changing another member’s account."
            },
            None,
            controls,
        ))
    }
    fn disconnect_github(&mut self, cx: &mut Context<Self>) {
        if self.repository_busy() {
            return;
        }
        self.call(
            "github-disconnect",
            methods::CLOUD_GITHUB_INSTALL,
            json!({"action":"disconnect"}),
            cx,
        );
        cx.notify();
    }
    pub(super) fn github_operation_rows(&self, cx: &mut Context<Self>) -> Option<Div> {
        let operation = self.github_operation.as_ref()?;
        let state = operation["state"].as_str().unwrap_or("failed");
        let busy = self
            .pending
            .values()
            .any(|p| matches!(p.kind, "github-select" | "github-cancel"));
        let title = match state {
            "choosing" => "Choose a GitHub installation",
            "expired" => "GitHub sign-in expired",
            "cancelled" => "GitHub sign-in cancelled",
            "failed" => "GitHub connection needs attention",
            _ => "Finish signing in with GitHub",
        };
        let description = if state == "choosing" {
            operation["user"]["login"]
                .as_str()
                .map(|login| {
                    format!("Signed in as {login}. Choose where Zuse may access repositories.")
                })
                .unwrap_or_else(|| "Choose where Zuse may access repositories.".into())
        } else {
            "Complete authorization in your browser. This connection belongs to your account."
                .into()
        };
        let mut body = column().child(cs::setting_row(true, title, description, None));
        if state == "choosing" {
            if let Some(installations) = operation["installations"].as_array() {
                for installation in installations {
                    let Some(id) = installation["installationId"].as_i64() else {
                        continue;
                    };
                    let login = installation["accountLogin"]
                        .as_str()
                        .unwrap_or("GitHub account")
                        .to_owned();
                    let suspended = installation["suspended"].as_bool().unwrap_or(false);
                    body = body.child(cs::setting_row(
                        false,
                        login,
                        if suspended {
                            "Installation suspended"
                        } else {
                            "Use this installation with your own GitHub identity"
                        },
                        Some(
                            cs::button(
                                SharedString::from(format!("github-installation-{id}")),
                                "Connect",
                                false,
                                busy || suspended,
                                busy,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.github_action("github-select", "select", Some(id), cx)
                            }))
                            .into_any_element(),
                        ),
                    ));
                }
                if installations.is_empty() {
                    body=body.child(cs::setting_row(false,"No installations yet","Install the app on your account or an organization below. If an organization needs approval, GitHub sends the request to its owners; refresh once it's approved.",None));
                }
            }
            if let Some(url) = operation["installUrl"]
                .as_str()
                .filter(|url| url.starts_with("https://github.com/apps/"))
            {
                let url = url.to_owned();
                body = body.child(cs::setting_row(
                    false,
                    "Add an installation",
                    "Install the App or request organization approval, then refresh this list.",
                    Some(
                        cs::button("github-install-app", "Open GitHub", false, busy, false)
                            .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url)))
                            .into_any_element(),
                    ),
                ));
            }
        }
        if let Some(error) = &self.error {
            body = body.child(cs::setting_row(
                false,
                "Connection needs attention",
                error.clone(),
                None,
            ));
        }
        let finished = matches!(state, "expired" | "cancelled" | "failed");
        let controls = row()
            .gap(px(8.))
            .when(finished, |controls| {
                controls.child(
                    cs::button("github-operation-retry", "Try again", true, busy, false).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.github_poll = None;
                            this.github_operation = None;
                            this.error = None;
                            this.call(
                                "github-install",
                                methods::CLOUD_GITHUB_INSTALL,
                                serde_json::json!({}),
                                cx,
                            );
                        }),
                    ),
                )
            })
            .child(
                cs::button("github-operation-refresh", "Refresh", false, busy, false).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.github_action("github-status", "status", None, cx)
                    }),
                ),
            )
            .child(
                cs::button(
                    "github-operation-cancel",
                    if finished {
                        "Close"
                    } else {
                        "Cancel"
                    },
                    false,
                    busy,
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.github_poll = None;
                    this.github_action("github-cancel", "cancel", None, cx);
                })),
            );
        Some(body.child(cs::setting_row_content(false, "", "", None, controls)))
    }
}
