//! The inline provider sign-in states shared by desktop and browser settings.
use crate::{acp_catalog::spinner, assets::icon, palette::rgb, settings_style as s, shell::MUTED};
use gpui::{prelude::*, *};
use zuse_protocol::HarnessId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoginOption {
    pub provider: Option<&'static str>,
    pub label: &'static str,
}
pub fn login_options(harness: HarnessId) -> Vec<LoginOption> {
    let option = |provider, label| LoginOption {
        provider: Some(provider),
        label,
    };
    match harness {
        HarnessId::Opencode => vec![
            option("openai", "ChatGPT"),
            option("github-copilot", "GitHub Copilot"),
        ],
        HarnessId::Pi => vec![option("openai-codex", "ChatGPT")],
        HarnessId::Hermes => vec![
            option("openai-codex", "ChatGPT"),
            option("nous", "Nous Portal"),
        ],
        _ => {
            let label = match harness {
                HarnessId::ClaudeCode => "Claude Code",
                HarnessId::Codex => "Codex",
                HarnessId::Cursor => "Cursor",
                HarnessId::Antigravity => "Antigravity",
                HarnessId::Grok => "Grok",
                HarnessId::Devin => "Devin",
                _ => return Vec::new(),
            };
            vec![LoginOption {
                provider: None,
                label,
            }]
        }
    }
}
pub fn login_command(harness: HarnessId) -> Option<&'static str> {
    Some(match harness {
        HarnessId::ClaudeCode => "claude /login",
        HarnessId::Codex => "codex login",
        HarnessId::Cursor => "cursor-agent login",
        HarnessId::Gemini => "gemini /auth",
        HarnessId::Grok => "grok login",
        HarnessId::Devin => "devin auth login",
        HarnessId::Opencode => "opencode auth login",
        HarnessId::Pi => "pi",
        HarnessId::Hermes => "hermes auth add",
        HarnessId::Kiro => "kiro-cli login",
        HarnessId::Muse => "muse login",
        _ => return None,
    })
}
/// Setup guidance for agents whose CLI owns sign-in entirely.
pub fn login_hint(harness: HarnessId) -> Option<&'static str> {
    match harness {
        HarnessId::Muse => Some(
            "Muse Code manages sign-in. Run `muse login` in a terminal, or save a Meta API key \
             with `muse auth set`. Zuse doesn't pass META_API_KEY through.",
        ),
        _ => None,
    }
}
#[derive(Clone)]
pub enum Phase {
    Idle,
    Waiting { message: Option<String> },
    Failed(String),
    Success,
}
pub struct Data {
    pub harness: HarnessId,
    pub phase: Phase,
    pub url: Option<String>,
    pub submitting: bool,
    pub error: Option<String>,
    pub blocked: bool,
}
#[derive(Clone)]
pub enum Kind {
    Start(Option<&'static str>),
    Retry,
    Cancel,
    Complete,
    Open,
}
#[derive(Clone)]
pub struct Action {
    pub harness: HarnessId,
    pub kind: Kind,
}
fn button<C: 'static>(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    data: &Data,
    kind: Kind,
    handler: fn(&mut C, Action, &mut Window, &mut Context<C>),
    cx: &mut Context<C>,
) -> Stateful<Div> {
    let primary = matches!(kind, Kind::Start(_));
    let action = Action {
        harness: data.harness,
        kind,
    };
    let label = label.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .tab_index(0)
        .cursor_pointer()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(28.))
        .px(px(if primary { 12. } else { 8. }))
        .rounded(px(6.))
        .border_1()
        .border_color(transparent_black())
        .font_weight(FontWeight::MEDIUM)
        .text_size(px(11.))
        .bg(if primary {
            rgb(0x89ca21).into()
        } else {
            transparent_black()
        })
        .text_color(if primary {
            gpui::rgb(0x152000)
        } else {
            rgb(crate::shell::TEXT)
        })
        .hover(move |v| {
            v.bg(if primary {
                rgb(0x81bf1d).into()
            } else {
                s::frame_bg()
            })
        })
        .on_key_down(|event, _, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation();
            }
        })
        .child(label)
        .on_click(cx.listener(move |this, _, window, cx| handler(this, action.clone(), window, cx)))
}
pub fn render<C: 'static>(
    data: Data,
    code: Option<AnyElement>,
    handler: fn(&mut C, Action, &mut Window, &mut Context<C>),
    cx: &mut Context<C>,
) -> AnyElement {
    let command = login_command(data.harness);
    let command_row = |label: &str| {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(11.))
            .text_color(rgb(MUTED))
            .child(label.to_owned())
            .when_some(command, |v, command| {
                v.child(div().font_family("Geist Mono").child(command))
            })
    };
    let mut body = div()
        .id("provider-inline-signin")
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .line_height(px(14.85))
        .gap(px(6.));
    match &data.phase {
        Phase::Idle => {
            body = body.child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(MUTED))
                    .child("Sign in"),
            );
            let mut choices = div().flex().flex_wrap().items_center().gap(px(8.));
            for (i, option) in login_options(data.harness).into_iter().enumerate() {
                let label = if option.provider.is_some() {
                    format!("Sign in with {}", option.label)
                } else {
                    format!("Sign in to {}", option.label)
                };
                choices = choices.child(
                    button(
                        ("provider-start-signin", i),
                        label,
                        &data,
                        Kind::Start(option.provider),
                        handler,
                        cx,
                    )
                    .when(data.blocked, |v| v.opacity(0.64).cursor_default()),
                );
            }
            if command.is_some() {
                let label = if login_options(data.harness).is_empty() {
                    "Run"
                } else {
                    "or run"
                };
                choices = choices.child(command_row(label).text_size(px(10.)));
            }
            body = body.child(choices);
            if let Some(hint) = login_hint(data.harness) {
                body = body.child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(hint));
            }
        }
        Phase::Waiting { message } => {
            let text = message.clone().unwrap_or_else(|| {
                if data.url.is_some() {
                    "Waiting for browser sign-in…".into()
                } else {
                    "Starting sign-in…".into()
                }
            });
            let strip = div()
                .h(px(28.))
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .pl(px(10.))
                .pr(px(2.))
                .rounded(px(6.))
                .bg(s::frame_bg().opacity(0.4))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child(spinner("provider-signin-spinner", 14.))
                        .child(div().truncate().child(text)),
                )
                .when(data.url.is_some(), |v| {
                    v.child(
                        button(
                            "provider-reopen-login",
                            "Open browser again",
                            &data,
                            Kind::Open,
                            handler,
                            cx,
                        )
                        .relative()
                        .pl(px(24.))
                        .child(icon("link").absolute().left(px(8.)).size(px(12.))),
                    )
                })
                .child(button(
                    "provider-cancel-login",
                    "Cancel",
                    &data,
                    Kind::Cancel,
                    handler,
                    cx,
                ));
            body = body.child(strip);
            if let Some(code) = code {
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(s::field().h(px(28.)).child(code))
                        .child(
                            button(
                                "provider-complete-login",
                                if data.submitting {
                                    "Working…"
                                } else {
                                    "Complete sign-in"
                                },
                                &data,
                                Kind::Complete,
                                handler,
                                cx,
                            )
                            .when(data.submitting, |v| v.opacity(0.64).cursor_default()),
                        ),
                );
            }
        }
        Phase::Failed(message) => {
            body = body
                .gap(px(8.))
                .child(
                    div()
                        .rounded(px(6.))
                        .px(px(10.))
                        .py(px(6.))
                        .text_size(px(11.))
                        .text_color(s::destructive_text())
                        .bg(crate::palette::tone(
                            hsla(0., 0.7, 0.15, 0.6),
                            hsla(0., 0.8, 0.95, 1.),
                        ))
                        .child(message.clone()),
                )
                .child(
                    div().flex().child(
                        button(
                            "provider-retry-login",
                            "Try again",
                            &data,
                            Kind::Retry,
                            handler,
                            cx,
                        )
                        .border_1()
                        .border_color(s::card_border())
                        .bg(s::card_bg()),
                    ),
                );
            if command.is_some() {
                body = body.child(command_row("Or run manually"));
            }
        }
        Phase::Success => {
            body = body.child(
                div()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .px(px(10.))
                    .rounded(px(6.))
                    .text_size(px(11.))
                    .text_color(rgb(0x34d399))
                    .bg(crate::palette::tone(
                        hsla(160. / 360., 0.5, 0.12, 1.),
                        hsla(160. / 360., 0.5, 0.95, 1.),
                    ))
                    .child("Signed in. Refreshing…"),
            );
        }
    }
    if let Some(error) = data.error {
        body = body.child(s::description_text(error).text_color(s::destructive_text()));
    }
    body.into_any_element()
}

pub fn is_openable_login_url(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    match scheme.to_ascii_lowercase().as_str() {
        "https" => true,
        "http" => {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
            let authority = authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host);
            let host = match authority.strip_prefix('[') {
                Some(v6) => v6.split(']').next().unwrap_or(""),
                None => authority.split(':').next().unwrap_or(""),
            };
            matches!(host, "localhost" | "127.0.0.1" | "::1")
        }
        _ => false,
    }
}
