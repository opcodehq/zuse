//! Variables for one repository: plain variables (saved with its setup) and
//! encrypted secrets (write-only), with an add sheet that takes a pasted
//! `.env`, an edit dialog and a delete confirmation.
use super::super::form;
use super::*;

/// A pasted `.env`, split into rows and the line numbers that weren't valid.
#[derive(Debug, Default, PartialEq, Eq)]
pub(in crate::cloud_repositories) struct ParsedEnv {
    pub rows: Vec<(String, String)>,
    pub skipped: Vec<usize>,
}

pub(in crate::cloud_repositories) fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 128
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `KEY=value` per line; blank lines and `#` comments are ignored, `export`
/// and matching quotes are stripped. A later duplicate replaces an earlier one.
pub(in crate::cloud_repositories) fn parse_env(text: &str) -> ParsedEnv {
    let mut out = ParsedEnv::default();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((name, value)) = line.split_once('=') else {
            out.skipped.push(index + 1);
            continue;
        };
        let name = name.trim();
        let mut value = value.trim();
        for quote in ['"', '\''] {
            if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
                value = &value[1..value.len() - 1];
            }
        }
        if !valid_name(name) {
            out.skipped.push(index + 1);
            continue;
        }
        out.rows.retain(|(existing, _)| existing != name);
        out.rows.push((name.to_owned(), value.to_owned()));
    }
    out
}

fn kind_badge(secret: bool) -> Div {
    let base = div()
        .flex_none()
        .h(px(18.))
        .px(px(6.))
        .rounded(px(5.))
        .flex()
        .items_center()
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM);
    if secret {
        base.bg(s::frame_bg())
            .text_color(palette::rgb(TEXT))
            .child("Secret")
    } else {
        base.bg(palette::tone(
            hsla(214. / 360., 0.6, 0.22, 1.),
            hsla(214. / 360., 0.9, 0.93, 1.),
        ))
        .text_color(palette::tone(
            hsla(214. / 360., 0.9, 0.68, 1.),
            hsla(214. / 360., 0.8, 0.42, 1.),
        ))
        .child("Variable")
    }
}

fn value_pill(text: String, revealed: bool) -> Div {
    div()
        .flex_none()
        .h(px(28.))
        .w(px(if revealed { 200. } else { 112. }))
        .px(px(10.))
        .rounded(px(7.))
        .bg(s::frame_bg())
        .flex()
        .items_center()
        .justify_center()
        .font_family("Geist Mono")
        .text_size(px(11.5))
        .text_color(palette::rgb(if revealed { TEXT } else { MUTED }))
        .child(div().truncate().child(text))
}

impl RepositoryPanel {
    fn existing_secret_revision(&self, name: &str) -> i64 {
        self.secrets
            .rows
            .iter()
            .find(|row| row["name"] == name && row["revoked"] != true)
            .and_then(|row| row["revision"].as_i64())
            .unwrap_or(0)
    }

    pub(in crate::cloud_repositories) fn open_variables(
        &mut self,
        overlay: Overlay,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.secrets.error = None;
        for input in [&self.secrets.name, &self.secrets.value, &self.secrets.paste] {
            input.update(cx, |input, cx| input.set_value(String::new(), cx));
        }
        let value = match &overlay {
            Overlay::EditVariable { name } => self
                .environment
                .variables()
                .and_then(|vars| vars.get(name).cloned())
                .unwrap_or_default(),
            _ => String::new(),
        };
        self.secrets
            .plain
            .update(cx, |input, cx| input.set_value(value, cx));
        let focus = match &overlay {
            Overlay::Add {
                one_by_one: false, ..
            } => Some(self.secrets.paste.focus_handle(cx)),
            Overlay::Add {
                one_by_one: true, ..
            } => Some(self.secrets.name.focus_handle(cx)),
            Overlay::EditVariable { .. } => Some(self.secrets.plain.focus_handle(cx)),
            _ => None,
        };
        self.secrets.overlay = Some(overlay);
        if let Some(focus) = focus {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn close_variables(&mut self, cx: &mut Context<Self>) {
        self.secrets.overlay = None;
        self.secrets.error = None;
        cx.notify();
    }

    /// Rows the add sheet would save right now.
    fn pending_variables(&self, cx: &App) -> ParsedEnv {
        match self.secrets.overlay {
            Some(Overlay::Add {
                one_by_one: true,
                secret,
            }) => {
                let name = self.secrets.name.read(cx).value(cx).trim().to_owned();
                let value = if secret {
                    self.secrets.value.read(cx).value(cx)
                } else {
                    self.secrets.plain.read(cx).value(cx)
                };
                if name.is_empty() {
                    ParsedEnv::default()
                } else if valid_name(&name) {
                    ParsedEnv {
                        rows: vec![(name, value)],
                        skipped: vec![],
                    }
                } else {
                    ParsedEnv {
                        rows: vec![],
                        skipped: vec![1],
                    }
                }
            }
            Some(Overlay::Add { .. }) => parse_env(&self.secrets.paste.read(cx).value(cx)),
            _ => ParsedEnv::default(),
        }
    }

    fn submit_variables(&mut self, cx: &mut Context<Self>) {
        let Some(overlay) = self.secrets.overlay.clone() else {
            return;
        };
        let Some(project) = self.secrets.project.clone() else {
            return;
        };
        match overlay {
            Overlay::Add { secret, .. } => {
                let parsed = self.pending_variables(cx);
                if parsed.rows.is_empty() {
                    self.secrets.error = Some("Add at least one NAME=value line.".into());
                    cx.notify();
                    return;
                }
                if secret && parsed.rows.iter().any(|(_, v)| v.is_empty()) {
                    self.secrets.error = Some("Every secret needs a value.".into());
                    cx.notify();
                    return;
                }
                let count = parsed.rows.len();
                for (name, value) in parsed.rows {
                    if secret {
                        let revision = self.existing_secret_revision(&name);
                        self.call(
                            "secret-save",
                            methods::CLOUD_PROJECT_SECRET_SAVE,
                            json!({"projectId":project,"name":name,"value":value,"expectedRevision":revision}),
                            cx,
                        );
                    } else if !self.environment.set_variable(&name, Some(value)) {
                        self.secrets.error =
                            Some("This repository's setup file manages its variables.".into());
                        cx.notify();
                        return;
                    }
                }
                self.secrets.notice = Some(if secret {
                    format!(
                        "Saving {count} secret{}…",
                        if count == 1 { "" } else { "s" }
                    )
                } else {
                    format!(
                        "Added {count} variable{}. Save changes to use them on new computers.",
                        if count == 1 { "" } else { "s" }
                    )
                });
            }
            Overlay::EditSecret {
                name,
                revision,
                replace,
                ..
            } => {
                if replace {
                    let value = self.secrets.value.read(cx).value(cx);
                    if value.is_empty() {
                        self.secrets.error = Some("Enter the new value.".into());
                        cx.notify();
                        return;
                    }
                    self.call(
                        "secret-save",
                        methods::CLOUD_PROJECT_SECRET_SAVE,
                        json!({"projectId":project,"name":name,"value":value,"expectedRevision":revision}),
                        cx,
                    );
                }
            }
            Overlay::EditVariable { name } => {
                let value = self.secrets.plain.read(cx).value(cx);
                self.environment.set_variable(&name, Some(value));
            }
            Overlay::DeleteSecret { id, revision, .. } => {
                self.revoke_workload_secret(id, revision, cx)
            }
            Overlay::DeleteVariable { name } => {
                self.environment.set_variable(&name, None);
                self.secrets.revealed.remove(&name);
            }
        }
        for input in [&self.secrets.value, &self.secrets.paste] {
            input.update(cx, |input, cx| input.set_value(String::new(), cx));
        }
        self.close_variables(cx);
    }

    fn variable_row(
        &self,
        first: bool,
        name: &str,
        secret: bool,
        detail: String,
        value: Option<&str>,
        actions: Div,
    ) -> Div {
        let revealed = value.is_some() && self.secrets.revealed.contains(name);
        row()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .py(px(11.))
            .when(!first, |v| {
                v.border_t_1().border_color(palette::rgb(BORDER))
            })
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        row()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .font_family("Geist Mono")
                                    .text_size(px(13.))
                                    .text_color(palette::rgb(TEXT))
                                    .truncate()
                                    .child(name.to_owned()),
                            )
                            .child(kind_badge(secret)),
                    )
                    .child(s::description_text(detail)),
            )
            .child(value_pill(
                if revealed {
                    value.unwrap_or_default().to_owned()
                } else {
                    "••••••••".into()
                },
                revealed,
            ))
            .child(actions)
    }

    pub(in crate::cloud_repositories) fn render_variables(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self
            .pending
            .values()
            .any(|pending| pending.kind.starts_with("secret-"));
        let managed = self.environment.repository_managed();
        let manage = self.can_manage();
        let mut body = column();
        let mut count = 0;
        if let Some(vars) = self.environment.variables() {
            for (name, value) in vars {
                let reveal = name.clone();
                let edit = name.clone();
                let delete = name.clone();
                let shown = self.secrets.revealed.contains(name);
                let actions = row()
                    .gap(px(2.))
                    .child(
                        cs::icon_button(
                            SharedString::from(format!("reveal-variable-{name}")),
                            if shown { "eye-closed" } else { "eye" },
                            if shown {
                                format!("Hide {name}")
                            } else {
                                format!("Show {name}")
                            },
                            false,
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.secrets.revealed.remove(&reveal) {
                                this.secrets.revealed.insert(reveal.clone());
                            }
                            cx.notify();
                        })),
                    )
                    .when(manage && !managed, |v| {
                        v.child(
                            cs::icon_button(
                                SharedString::from(format!("edit-variable-{name}")),
                                "pen",
                                format!("Edit {name}"),
                                false,
                                false,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.open_variables(
                                        Overlay::EditVariable { name: edit.clone() },
                                        window,
                                        cx,
                                    )
                                },
                            )),
                        )
                        .child(
                            cs::icon_button(
                                SharedString::from(format!("delete-variable-{name}")),
                                "trash-bin-minimalistic",
                                format!("Delete {name}"),
                                false,
                                false,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.open_variables(
                                        Overlay::DeleteVariable {
                                            name: delete.clone(),
                                        },
                                        window,
                                        cx,
                                    )
                                },
                            )),
                        )
                    });
                body = body.child(self.variable_row(
                    count == 0,
                    name,
                    false,
                    "Plain text · saved with this repository's setup".into(),
                    Some(value),
                    actions,
                ));
                count += 1;
            }
        }
        for item in &self.secrets.rows {
            let (Some(id), Some(name), Some(revision)) = (
                item["secretId"].as_str(),
                item["name"].as_str(),
                item["revision"].as_i64(),
            ) else {
                continue;
            };
            if item["revoked"] == true {
                continue;
            }
            let used = self.environment.secret_selected(id);
            let edit = Overlay::EditSecret {
                id: id.to_owned(),
                name: name.to_owned(),
                revision,
                replace: false,
            };
            let delete = Overlay::DeleteSecret {
                id: id.to_owned(),
                name: name.to_owned(),
                revision,
            };
            let actions = row()
                .gap(px(2.))
                .child(div().size(px(28.)))
                .when(manage, |v| {
                    v.child(
                        cs::icon_button(
                            SharedString::from(format!("edit-secret-{id}")),
                            "pen",
                            format!("Edit {name}"),
                            busy,
                            false,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_variables(edit.clone(), window, cx)
                            },
                        )),
                    )
                    .child(
                        cs::icon_button(
                            SharedString::from(format!("revoke-secret-{id}")),
                            "trash-bin-minimalistic",
                            format!("Delete {name}"),
                            busy,
                            false,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_variables(delete.clone(), window, cx)
                            },
                        )),
                    )
                });
            body = body.child(self.variable_row(
                count == 0,
                name,
                true,
                format!(
                    "Encrypted · version {revision}{}",
                    if used {
                        " · available to setup commands"
                    } else {
                        ""
                    }
                ),
                None,
                actions,
            ));
            count += 1;
        }
        if count == 0 {
            body = body.child(form::empty(
                "None yet. Paste a .env to add tokens and settings your setup and agent need.",
            ));
        }
        if let Some(notice) = &self.secrets.notice {
            body = body.child(
                div()
                    .px(px(14.))
                    .pb(px(10.))
                    .child(form::message(notice, false)),
            );
        }
        if self.secrets.overlay.is_none() {
            if let Some(error) = &self.secrets.error {
                body = body.child(
                    div()
                        .px(px(14.))
                        .pb(px(10.))
                        .child(form::message(error, true)),
                );
            }
        }
        let header_actions = manage.then(|| {
            row()
                .gap(px(8.))
                .child(
                    cs::icon_label_button(
                        "open-env-paste",
                        "upload",
                        "Paste .env",
                        false,
                        busy,
                        false,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_variables(
                            Overlay::Add {
                                one_by_one: false,
                                secret: true,
                            },
                            window,
                            cx,
                        )
                    })),
                )
                .child(
                    cs::icon_label_button(
                        "open-add-variables",
                        "plus",
                        "Add variables",
                        true,
                        busy,
                        false,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_variables(
                            Overlay::Add {
                                one_by_one: true,
                                secret: true,
                            },
                            window,
                            cx,
                        )
                    })),
                )
                .into_any_element()
        });
        let section = form::section(
            "Variables",
            "Given to this repository's setup commands, services and agent. Secrets are encrypted and can't be read back.",
            header_actions,
            body,
        );
        match self.variables_overlay(window, cx) {
            Some(overlay) => section.child(overlay),
            None => section,
        }
    }

    fn variables_overlay(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let overlay = self.secrets.overlay.clone()?;
        let busy = self
            .pending
            .values()
            .any(|pending| pending.kind.starts_with("secret-"));
        let viewport = window.viewport_size();
        let error = self.secrets.error.clone();
        let footer = |left: Option<AnyElement>,
                      label: String,
                      danger: bool,
                      enabled: bool,
                      cx: &mut Context<Self>| {
            row()
                .items_center()
                .gap(px(10.))
                .px(px(18.))
                .py(px(12.))
                .border_t_1()
                .border_color(palette::rgb(BORDER))
                .bg(s::frame_bg().opacity(0.5))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .when_some(left, |v, left| v.child(left)),
                )
                .child(
                    cs::button("variables-cancel", "Cancel", false, false, false)
                        .on_click(cx.listener(|this, _, _, cx| this.close_variables(cx))),
                )
                .child(
                    cs::button("variables-confirm", label, !danger, !enabled || busy, busy)
                        .when(danger, |v| {
                            v.bg(hsla(0., 0.72, 0.52, 1.)).text_color(white())
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if enabled {
                                this.submit_variables(cx)
                            }
                        })),
                )
        };
        let title = |text: String, detail: &'static str, cx: &mut Context<Self>| {
            row()
                .items_start()
                .gap(px(12.))
                .px(px(18.))
                .pt(px(16.))
                .pb(px(12.))
                .child(
                    column()
                        .flex_1()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(px(16.))
                                .line_height(px(22.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(palette::rgb(TEXT))
                                .child(text),
                        )
                        .when(!detail.is_empty(), |v| v.child(s::description_text(detail))),
                )
                .child(
                    cs::icon_button("variables-close", "close", "Close", false, false)
                        .on_click(cx.listener(|this, _, _, cx| this.close_variables(cx))),
                )
        };
        let (panel, sheet) = match overlay {
            Overlay::Add { one_by_one, secret } => {
                let parsed = self.pending_variables(cx);
                let mut body = column().flex_1().min_h_0().gap(px(10.)).px(px(18.));
                if one_by_one {
                    body = body
                        .child(form::line_field(self.secrets.name.clone()))
                        .child(form::line_field(if secret {
                            self.secrets.value.clone()
                        } else {
                            self.secrets.plain.clone()
                        }));
                } else {
                    body = body.child(
                        div()
                            .w_full()
                            .min_h(px(220.))
                            .px(px(12.))
                            .py(px(10.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(palette::tone(
                                hsla(83. / 360., 0.72, 0.46, 0.7),
                                hsla(83. / 360., 0.74, 0.43, 0.7),
                            ))
                            .bg(palette::tone(hsla(0., 0., 0.06, 1.), hsla(0., 0., 0.98, 1.)))
                            .child(self.secrets.paste.clone()),
                    );
                    if !parsed.rows.is_empty() || !parsed.skipped.is_empty() {
                        let mut list = column()
                            .id("variables-preview")
                            .max_h(px(260.))
                            .overflow_y_scroll()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(palette::rgb(BORDER));
                        for (i, (name, value)) in parsed.rows.iter().enumerate() {
                            let replaces = if secret {
                                self.existing_secret_revision(name) > 0
                            } else {
                                self.environment.variables().is_some_and(|v| v.contains_key(name))
                            };
                            list = list.child(
                                row()
                                    .gap(px(12.))
                                    .items_center()
                                    .px(px(12.))
                                    .py(px(7.))
                                    .when(i > 0, |v| v.border_t_1().border_color(palette::rgb(BORDER)))
                                    .child(
                                        div()
                                            .w(px(200.))
                                            .flex_none()
                                            .font_family("Geist Mono")
                                            .text_size(px(12.))
                                            .text_color(palette::rgb(TEXT))
                                            .truncate()
                                            .child(name.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .child(form::mono(if secret {
                                                "••••••••".to_owned()
                                            } else {
                                                value.clone()
                                            })),
                                    )
                                    .when(replaces, |v| v.child(cs::badge("Replaces", true, false))),
                            );
                        }
                        body = body.child(
                            row()
                                .gap(px(10.))
                                .child(s::title_text(format!(
                                    "{} variable{}",
                                    parsed.rows.len(),
                                    if parsed.rows.len() == 1 { "" } else { "s" }
                                )))
                                .when(!parsed.skipped.is_empty(), |v| {
                                    v.child(form::message(
                                        &format!(
                                            "Skipping line{} {} (not NAME=value)",
                                            if parsed.skipped.len() == 1 { "" } else { "s" },
                                            parsed
                                                .skipped
                                                .iter()
                                                .map(|n| n.to_string())
                                                .collect::<Vec<_>>()
                                                .join(", ")
                                        ),
                                        true,
                                    ))
                                }),
                        )
                        .child(list);
                    }
                }
                body = body.child(
                    row().gap(px(14.)).child(
                        cs::icon_label_button(
                            "variables-mode",
                            if one_by_one { "upload" } else { "plus" },
                            if one_by_one { "Paste a .env instead" } else { "Add one by one" },
                            false,
                            false,
                            false,
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_variables(Overlay::Add { one_by_one: !one_by_one, secret }, window, cx)
                        })),
                    ),
                );
                if let Some(error) = &error {
                    body = body.child(form::message(error, true));
                }
                let count = parsed.rows.len();
                let toggle = row()
                    .gap(px(10.))
                    .items_center()
                    .child(
                        s::toggle("variables-secret", secret, "Secret").on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.secrets.overlay =
                                    Some(Overlay::Add { one_by_one, secret: !secret });
                                cx.notify();
                            },
                        )),
                    )
                    .child(s::title_text("Secret"))
                    .child(s::description_text(if secret {
                        "Encrypted; can't be read back"
                    } else {
                        "Plain text, saved with setup"
                    }))
                    .into_any_element();
                let label = if count > 1 { format!("Save {count} variables") } else { "Save".into() };
                (
                    column()
                        .size_full()
                        .child(title(
                            "Add variables".into(),
                            if one_by_one {
                                "One name and value. Turn Secret off for non-sensitive settings."
                            } else {
                                "Paste a .env file. Every line becomes a row you can review before saving."
                            },
                            cx,
                        ))
                        .child(body)
                        .child(div().flex_1())
                        .child(footer(Some(toggle), label, false, count > 0, cx)),
                    true,
                )
            }
            Overlay::EditSecret { name, replace, id, revision } => {
                let used = self.environment.secret_selected(&id);
                let managed = self.environment.repository_managed();
                let mut body = column().gap(px(12.)).px(px(18.)).pb(px(16.));
                body = body.child(
                    column()
                        .gap(px(6.))
                        .child(s::description_text("Name"))
                        .child(value_pill(name.clone(), true).w_full().justify_start()),
                );
                let edit = Overlay::EditSecret { id: id.clone(), name: name.clone(), revision, replace: true };
                body = body.child(column().gap(px(6.)).child(s::description_text("Value")).child(
                    if replace {
                        form::line_field(self.secrets.value.clone()).into_any_element()
                    } else {
                        row()
                            .h(px(32.))
                            .px(px(12.))
                            .rounded(px(7.))
                            .bg(s::frame_bg())
                            .items_center()
                            .justify_between()
                            .child(form::mono("••••••••"))
                            .child(
                                cs::button("replace-secret-value", "Replace value", false, false, false)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.secrets.overlay = Some(edit.clone());
                                        window.focus(&this.secrets.value.focus_handle(cx), cx);
                                        cx.notify();
                                    })),
                            )
                            .into_any_element()
                    },
                ));
                let bind = id.clone();
                body = body.child(
                    row()
                        .items_center()
                        .gap(px(12.))
                        .p(px(12.))
                        .rounded(px(8.))
                        .bg(s::frame_bg().opacity(0.5))
                        .child(
                            column()
                                .flex_1()
                                .gap(px(2.))
                                .child(s::title_text("Available to setup commands"))
                                .child(s::description_text(
                                    "The agent and services always get it. Setup commands only when this is on.",
                                )),
                        )
                        .child(
                            s::toggle("secret-in-setup", used, "Available to setup commands")
                                .when(managed, |v| v.opacity(0.5))
                                .when(!managed, |v| {
                                    v.on_click(cx.listener(move |this, _, _, cx| {
                                        this.environment.select_secret(bind.clone(), !used);
                                        cx.notify();
                                    }))
                                }),
                        ),
                );
                if let Some(error) = &error {
                    body = body.child(form::message(error, true));
                }
                (
                    column()
                        .child(title("Edit secret".into(), "", cx))
                        .child(body)
                        .child(footer(None, "Save changes".into(), false, true, cx)),
                    false,
                )
            }
            Overlay::EditVariable { name } => (
                column()
                    .child(title("Edit variable".into(), "", cx))
                    .child(
                        column()
                            .gap(px(12.))
                            .px(px(18.))
                            .pb(px(16.))
                            .child(
                                column()
                                    .gap(px(6.))
                                    .child(s::description_text("Name"))
                                    .child(value_pill(name.clone(), true).w_full().justify_start()),
                            )
                            .child(
                                column()
                                    .gap(px(6.))
                                    .child(s::description_text("Value"))
                                    .child(form::line_field(self.secrets.plain.clone())),
                            ),
                    )
                    .child(footer(None, "Save changes".into(), false, true, cx)),
                false,
            ),
            Overlay::DeleteSecret { name, .. } | Overlay::DeleteVariable { name } => (
                column()
                    .child(title(
                        format!("Delete {name}?"),
                        "New computers for this repository won't receive it. Running computers keep their current environment.",
                        cx,
                    ))
                    .child(footer(None, "Delete".into(), true, true, cx)),
                false,
            ),
        };
        let card = div()
            .id("variables-dialog")
            .role(Role::Dialog)
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .bg(palette::tone(hsla(132. / 360., 0.05, 0.105, 1.), white()))
            .border_1()
            .border_color(s::card_border())
            .rounded(px(12.))
            .overflow_hidden()
            .shadow_lg()
            .child(panel);
        let card = if sheet {
            card.w(px(560_f32.min(f32::from(viewport.width) - 32.)))
                .h(viewport.height - px(24.))
        } else {
            card.w(px(460_f32.min(f32::from(viewport.width) - 32.)))
        };
        let backdrop = div()
            .w(viewport.width)
            .h(viewport.height)
            .occlude()
            .bg(rgba(0x00000099))
            .flex()
            .p(px(12.))
            .when(sheet, |v| v.justify_end())
            .when(!sheet, |v| v.items_center().justify_center())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close_variables(cx)),
            )
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close_variables(cx);
                    cx.stop_propagation();
                }
            }))
            .child(card);
        Some(
            deferred(
                anchored()
                    .anchor(Anchor::TopLeft)
                    .position(point(px(0.), px(0.)))
                    .child(backdrop),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_env, valid_name};
    #[test]
    fn env_paste_parses_names_values_comments_quotes_and_bad_lines() {
        let parsed = parse_env(
            "# comment\nDATABASE_URL=postgres://u:p@h/db?x=1\nexport NPM_TOKEN=\"npm_abc\"\n\nbad line\n1BAD=x\nNODE_ENV='production'\nNPM_TOKEN=npm_new\n",
        );
        assert_eq!(
            parsed.rows,
            vec![
                ("DATABASE_URL".into(), "postgres://u:p@h/db?x=1".into()),
                ("NODE_ENV".into(), "production".into()),
                ("NPM_TOKEN".into(), "npm_new".into()),
            ]
        );
        assert_eq!(parsed.skipped, vec![5, 6]);
        assert!(valid_name("_A1") && !valid_name("A-B") && !valid_name(""));
    }
}
