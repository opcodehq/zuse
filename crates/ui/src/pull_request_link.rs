//! Which chat a pull request is linked to. The linked chat hears about the
//! pull request's comments, reviews, check results and conflicts.
//!
//! Backed by this computer's engine, whose list covers every chat of the
//! account. A link change goes to the engine hosting the chat; a cloud
//! chat's goes through this engine, naming the chat's cloud computer.
use crate::{state::AppState, theme::Theme};
use gpui::{Context, Entity, SharedString, Subscription, Task, Window, div, prelude::*, px};
use std::time::Duration;
use zuse_protocol::pull_request_links::{
    EventAction, EventPolicy, OwnedPullRequest, PullRequestEventKind,
};
use zuse_rpc::methods;

pub(crate) struct PullRequestLinkControl {
    state: Entity<AppState>,
    chat_id: String,
    url: String,
    /// The device hosting the chat; `None` when it is this computer.
    target: Option<String>,
    owned: Vec<OwnedPullRequest>,
    connected: bool,
    busy: bool,
    error: Option<String>,
    _watch: Option<Task<()>>,
    action: Option<Task<()>>,
    engine: Option<crate::state::EngineHandle>,
    _state: Subscription,
}

/// The settings rows, in the order the menu shows them.
const GROUPS: [(&str, &[PullRequestEventKind]); 5] = [
    (
        "Comments and reviews",
        &[
            PullRequestEventKind::Comment,
            PullRequestEventKind::Review,
            PullRequestEventKind::ReviewComment,
        ],
    ),
    ("Failed checks", &[PullRequestEventKind::ChecksFailed]),
    (
        "Checks passing again",
        &[PullRequestEventKind::ChecksPassed],
    ),
    ("Merge conflicts", &[PullRequestEventKind::Conflict]),
    (
        "Merged, closed or reopened",
        &[
            PullRequestEventKind::Merged,
            PullRequestEventKind::Closed,
            PullRequestEventKind::Reopened,
        ],
    ),
];

fn action_label(action: EventAction) -> &'static str {
    match action {
        EventAction::Wake => "Start a turn",
        EventAction::Context => "Note for next turn",
        EventAction::Ignore => "Ignore",
    }
}

fn next_action(action: EventAction) -> EventAction {
    match action {
        EventAction::Wake => EventAction::Context,
        EventAction::Context => EventAction::Ignore,
        EventAction::Ignore => EventAction::Wake,
    }
}

/// `policy` with every kind in `kinds` moved to the next choice.
pub(crate) fn cycled(policy: &EventPolicy, kinds: &[PullRequestEventKind]) -> EventPolicy {
    let mut next = policy.clone();
    match kinds.first() {
        Some(first) => {
            let action = next_action(policy.action(*first, false));
            for kind in kinds {
                next.actions.insert(*kind, action);
            }
        }
        None => {
            next.bots = Some(next_action(
                policy.action(PullRequestEventKind::Comment, true),
            ))
        }
    }
    next
}

/// Where a link change for a chat on `target` is sent.
fn route(target: Option<&str>) -> serde_json::Value {
    match target {
        Some(device) => match device.strip_prefix("cloud:") {
            Some(workspace) => serde_json::json!({"workspaceId": workspace}),
            None => serde_json::json!({"targetDeviceId": device}),
        },
        None => serde_json::json!({}),
    }
}

impl PullRequestLinkControl {
    pub(crate) fn new(
        state: Entity<AppState>,
        chat_id: String,
        url: String,
        target: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&state, |control, _, cx| {
            let engine = control.state.read(cx).engine().cloned();
            let same = match (&control.engine, &engine) {
                (Some(a), Some(b)) => a.same_connection(b),
                (None, None) => true,
                _ => false,
            };
            if !same {
                control._watch = None;
                control.action = None;
                control.connected = false;
                control.busy = false;
                control.engine = engine;
                control.connect(cx);
                cx.notify();
            }
        });
        let engine = state.read(cx).engine().cloned();
        let mut control = Self {
            state,
            chat_id,
            url,
            target,
            owned: Vec::new(),
            connected: false,
            busy: false,
            error: None,
            _watch: None,
            action: None,
            engine,
            _state: observe,
        };
        control.connect(cx);
        control
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self._watch = Some(cx.spawn(async move |this, cx| {
            let mut delay = 1;
            loop {
                match engine
                    .client()
                    .subscribe_checked(methods::WATCH_PULL_REQUEST_LINKS, serde_json::json!({}))
                    .await
                {
                    Ok(mut stream) => {
                        while let Some(value) = stream.recv().await {
                            let Ok(owned) = serde_json::from_value::<Vec<OwnedPullRequest>>(value)
                            else {
                                break;
                            };
                            if this
                                .update(cx, |control, cx| {
                                    control.owned = owned;
                                    control.connected = true;
                                    cx.notify();
                                })
                                .is_err()
                            {
                                return;
                            }
                            delay = 1;
                        }
                    }
                    // An engine from before pull request links: nothing to show.
                    Err(zuse_rpc::RpcError::UnknownMethod(_)) => return,
                    Err(_) => {}
                }
                if this
                    .update(cx, |control, cx| {
                        control.connected = false;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(delay))
                    .await;
                delay = (delay * 2).min(60);
            }
        }));
    }

    fn owner(&self) -> Option<&OwnedPullRequest> {
        self.owned.iter().find(|pr| {
            zuse_protocol::pull_request_links::parse_url(&self.url).is_some_and(|(repo, n)| {
                repo.eq_ignore_ascii_case(&pr.repository) && n == pr.number
            })
        })
    }

    fn call(&mut self, method: &'static str, extra: serde_json::Value, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let mut params = serde_json::json!({"chatId": self.chat_id, "url": self.url});
        for (key, value) in route(self.target.as_deref())
            .as_object()
            .into_iter()
            .flatten()
            .chain(extra.as_object().into_iter().flatten())
        {
            params[key] = value.clone();
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        self.action = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(method, params).await;
            let _ = this.update(cx, |control, cx| {
                control.busy = false;
                if let Err(error) = result {
                    control.error = Some(error.to_string());
                }
                cx.notify();
            });
        }));
    }

    fn chat_title(&self, chat_key: &str, cx: &Context<Self>) -> SharedString {
        let id = chat_key.strip_prefix("chat:").unwrap_or(chat_key);
        self.state
            .read(cx)
            .chats
            .iter()
            .find(|c| c.id == id)
            .and_then(|c| c.title.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "another chat".into())
            .into()
    }
}

impl Render for PullRequestLinkControl {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        if !self.connected {
            return div().into_any_element();
        }
        let owner = self.owner().cloned();
        let mine = owner
            .as_ref()
            .is_some_and(|o| o.chat_id() == Some(self.chat_id.as_str()));
        let label: SharedString = match &owner {
            Some(_) if mine => "Linked to this chat".into(),
            Some(owner) => format!("Linked to {}", self.chat_title(&owner.chat_key, cx)).into(),
            None => "Link to this chat".into(),
        };
        let tooltip: SharedString = self
            .error
            .clone()
            .unwrap_or_else(|| {
                if mine {
                    "Comments, reviews, check results and conflicts on this pull request come to this chat. Click to unlink.".into()
                } else if owner.is_some() {
                    "Click to link it to this chat instead.".into()
                } else {
                    "Send this pull request's comments, reviews, check results and conflicts to this chat.".into()
                }
            })
            .into();
        let ready = !self.busy;
        let chip = crate::pull_request_card::retro_chip(&theme, cx)
            .id("pr-link-chat")
            .debug_selector(|| "pr-link-chat".into())
            .role(gpui::Role::Button)
            .aria_label(label.clone())
            .text_color(if mine { theme.text } else { theme.text_muted })
            .when(!ready, |el| el.opacity(0.5))
            .tooltip(crate::settings::widgets::text_tooltip(tooltip))
            .child(crate::pull_request_card::pixel_light(if mine {
                theme.loader_glyph().mid
            } else {
                theme.text_muted.opacity(0.35)
            }))
            .child(label)
            .when(ready, |el| {
                el.cursor_pointer()
                    .hover(|style| style.bg(theme.element_active))
                    .on_click(cx.listener(move |control, _, _, cx| {
                        cx.stop_propagation();
                        if mine {
                            control.call(methods::RELEASE_PULL_REQUEST, serde_json::json!({}), cx);
                        } else {
                            control.call(
                                methods::CLAIM_PULL_REQUEST,
                                serde_json::json!({"source": "manual"}),
                                cx,
                            );
                        }
                    }))
            });
        let settings = owner.filter(|_| mine).map(|owner| {
            let policy = owner.policy.clone();
            let rows = GROUPS
                .iter()
                .map(|(name, kinds)| (*name, kinds.to_vec()))
                .chain(std::iter::once(("Bot comments", Vec::new())))
                .enumerate()
                .map(|(ix, (name, kinds))| {
                    let current = match kinds.first() {
                        Some(kind) => policy.action(*kind, false),
                        None => policy.action(PullRequestEventKind::Comment, true),
                    };
                    let next = cycled(&policy, &kinds);
                    div()
                        .id(("pr-link-policy", ix))
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.element_active))
                        .child(format!("{name}: {}", action_label(current)))
                        .on_click(cx.listener(move |control, _, _, cx| {
                            cx.stop_propagation();
                            control.call(
                                methods::SET_PULL_REQUEST_POLICY,
                                serde_json::json!({"policy": next.clone()}),
                                cx,
                            );
                        }))
                })
                .collect::<Vec<_>>();
            div()
                .id("pr-link-policy-rows")
                .flex()
                .flex_wrap()
                .gap(px(2.0))
                .children(rows)
        });
        div()
            .flex()
            .flex_col()
            .items_end()
            .gap(px(2.0))
            .child(chip)
            .children(settings)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_cycle_each_group_together() {
        let policy = EventPolicy::default();
        let feedback = cycled(&policy, GROUPS[0].1);
        for kind in GROUPS[0].1 {
            assert_eq!(feedback.action(*kind, false), EventAction::Context);
        }
        assert_eq!(
            feedback.action(PullRequestEventKind::ChecksFailed, false),
            EventAction::Wake
        );
        let merged = cycled(&policy, GROUPS[4].1);
        assert_eq!(
            merged.action(PullRequestEventKind::Merged, false),
            EventAction::Ignore
        );
        let bots = cycled(&policy, &[]);
        assert_eq!(
            bots.action(PullRequestEventKind::Comment, true),
            EventAction::Wake
        );
    }

    #[test]
    fn link_changes_go_to_the_chat_host() {
        assert_eq!(route(None), serde_json::json!({}));
        assert_eq!(
            route(Some("mac")),
            serde_json::json!({"targetDeviceId": "mac"})
        );
        assert_eq!(
            route(Some("cloud:w1")),
            serde_json::json!({"workspaceId": "w1"})
        );
    }
}
