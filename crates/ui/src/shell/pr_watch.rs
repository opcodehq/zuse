//! One failure transition produces one attention notification, regardless of
//! which chat is selected. Initial snapshots seed silently; repeated reconnect snapshots stay silent.
use super::*;
use zuse_protocol::pr_watch::PrWatchStatus;

#[derive(Default)]
pub(super) struct DeviceWatch {
    task: Option<Task<()>>,
    notifications: zuse_protocol::pr_watch::PrWatchNotificationState,
}

impl Shell {
    pub(super) fn refresh_pr_watches(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        if !matches!(state.connection, ConnectionStatus::Ready) {
            self.pr_watch_devices.clear();
            return;
        }
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        let devices: std::collections::HashSet<_> = state
            .devices
            .iter()
            .map(|d| d.id.clone())
            .chain(std::iter::once(engine.engine_info().device_id.clone()))
            .filter(|id| {
                state.device_supports(id, zuse_protocol::capabilities::PR_WATCH_V1)
                    && state.device_online(id, Utc::now())
            })
            .collect();
        self.pr_watch_devices.retain(|id, _| devices.contains(id));
        for device in devices {
            let entry = self.pr_watch_devices.entry(device.clone()).or_default();
            if entry.task.is_some() {
                continue;
            }
            let engine = engine.clone();
            entry.task = Some(cx.spawn(async move |this, cx| {
                let mut delay = 1;
                loop {
                    if let Ok(mut stream) = engine
                        .client()
                        .subscribe_checked(
                            methods::WATCH_PR_WATCHES,
                            serde_json::json!({"targetDeviceId": device}),
                        )
                        .await
                    {
                        while let Some(value) = stream.recv().await {
                            let Ok(statuses) = serde_json::from_value::<Vec<PrWatchStatus>>(value)
                            else {
                                break;
                            };
                            if this
                                .update(cx, |shell, cx| {
                                    shell.notify_pr_failures(&device, statuses, cx)
                                })
                                .is_err()
                            {
                                return;
                            }
                            delay = 1;
                        }
                    }
                    cx.background_executor()
                        .timer(Duration::from_secs(delay))
                        .await;
                    delay = (delay * 2).min(60);
                }
            }));
        }
    }

    fn notify_pr_failures(
        &mut self,
        device: &str,
        statuses: Vec<PrWatchStatus>,
        cx: &mut Context<Self>,
    ) {
        let Some(watch) = self.pr_watch_devices.get_mut(device) else {
            return;
        };
        let failures = watch.notifications.consume(statuses);
        for status in failures {
            let sound = crate::sound::Sound::Attention;
            if self.settings.session_sound_enabled(sound)
                && self
                    .attention_sound_gate
                    .should_play(std::time::Instant::now())
            {
                crate::sound::play(sound);
            }
            if self.settings.notifications_enabled
                && !(self.settings.notifications_background_only && cx.active_window().is_some())
            {
                let title = self
                    .state
                    .read(cx)
                    .chats
                    .iter()
                    .find(|c| c.id == status.chat_id)
                    .and_then(|c| c.title.clone())
                    .unwrap_or_else(|| "Pull request CI failed".into());
                let body = if status.auto_fix {
                    "CI failed. Auto-fix will investigate when the chat is idle."
                } else {
                    "CI failed. Open the pull request to inspect the checks."
                };
                crate::notify::post(&title, body, Some(&status.chat_id));
            }
        }
    }
}
