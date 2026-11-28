//! Pull requests linked to chats, for the session card: the account-wide list
//! this computer's engine keeps, followed while connected.
use super::*;
use zuse_protocol::pull_request_links::OwnedPullRequest;

#[derive(Default)]
pub(super) struct PullRequestLinks {
    pub(super) owned: Vec<OwnedPullRequest>,
    task: Option<Task<()>>,
}

impl Shell {
    pub(super) fn refresh_pull_request_links(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        if !matches!(state.connection, ConnectionStatus::Ready) {
            self.pull_request_links = PullRequestLinks::default();
            return;
        }
        if self.pull_request_links.task.is_some() {
            return;
        }
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        self.pull_request_links.task = Some(cx.spawn(async move |this, cx| {
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
                                .update(cx, |shell, cx| {
                                    shell.pull_request_links.owned = owned;
                                    cx.notify();
                                })
                                .is_err()
                            {
                                return;
                            }
                            delay = 1;
                        }
                    }
                    Err(zuse_rpc::RpcError::UnknownMethod(_)) => return,
                    Err(_) => {}
                }
                cx.background_executor()
                    .timer(Duration::from_secs(delay))
                    .await;
                delay = (delay * 2).min(60);
            }
        }));
    }

    /// Rows for the pull requests `chat` is linked to, besides `except` (the
    /// branch's own, which has its own row).
    pub(super) fn render_linked_pull_requests(
        &mut self,
        chat: &zuse_protocol::Chat,
        device: Option<String>,
        except: Option<&str>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let key = format!("chat:{}", chat.id);
        self.pull_request_links
            .owned
            .iter()
            .filter(|pr| pr.chat_key == key && Some(pr.url.as_str()) != except)
            .enumerate()
            .map(|(ix, pr)| {
                let (glyph, color) = crate::pull_request_detail::state_glyph(
                    &pr.state.to_ascii_uppercase(),
                    pr.draft,
                    theme,
                );
                let url = pr.url.clone();
                let device = device.clone();
                let name = pr.repository.split('/').nth(1).unwrap_or(&pr.repository);
                super::session_info::action_row(
                    theme,
                    format!("session-info-linked-pull-request-{ix}"),
                )
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(format!(
                    "Linked pull request {name}#{}: {}",
                    pr.number, pr.title
                )))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close_session_info_overlay(cx);
                    crate::pull_request_detail::open_on_device(&url, device.clone(), window, cx);
                }))
                .child(popover::picker_icon_slot(
                    icon(glyph)
                        .size(px(16.0))
                        .text_color(color)
                        .into_any_element(),
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from(pr.title.clone())),
                )
                .child(
                    super::session_info::row_meta(theme)
                        .when(pr.checks.as_deref() == Some("failed"), |meta| {
                            meta.child(div().text_color(theme.danger).child("Checks failing"))
                        })
                        .child(SharedString::from(format!("{name}#{}", pr.number))),
                )
                .into_any_element()
            })
            .collect()
    }
}
