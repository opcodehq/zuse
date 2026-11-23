//! Screenshot fixture for image links in a cloud chat: the real shell opens
//! a scripted cloud chat whose computer is asleep, a reply links an image,
//! and the fixture clicks it, fails the wake, and finally wakes the computer
//! with the image. No live provider is involved.
use super::*;
use crate::state::CloudTarget;
use serde_json::{Value, json};
use std::sync::Arc;
use zuse_protocol::methods;
use zuse_rpc::{RpcError, RpcReply, RpcService};

const IMAGE: &str = "/home/zuse/workspace/artifacts/reasoning-slider.png";

fn chat() -> Value {
    json!({"id":"session","deviceId":"cloud:task","archived":false,"createdAt":"2026-11-23T11:00:00Z",
        "title":"Reasoning slider","cwd":"/home/zuse/workspace","spaceId":"space","checkoutId":"checkout"})
}

fn history() -> Value {
    let reply = format!(
        "The reasoning slider now snaps to each level and shows its name while you drag.\n\n![Updated reasoning slider]({IMAGE})"
    );
    json!({"accountId":"owner","organizationId":null,
        "workspace":{"workspaceId":"task","providerId":"boxd","state":"paused","desiredState":"paused"},
        "head":{"schemaVersion":1,"payloadFormat":"fixture","workspaceId":"task","sessionId":"session",
            "runtimeGeneration":1,"cursor":{"epoch":"fixture","version":1},"chat":chat(),"conversations":[chat()],
            "transcript":{"reset":[
                {"id":"user","role":"user","parts":[{"id":"user-text","kind":"text","text":"Make the reasoning slider snap to each level, then show me."}],"createdAt":1,"deviceId":"cloud:task"},
                {"id":"reply","role":"assistant","status":"complete","parts":[{"id":"reply-text","kind":"text","text":reply}],"createdAt":2,"deviceId":"cloud:task"}
            ]},
            "queue":[],"queuePaused":false,"olderMessageSequence":null}})
}

/// The account's controller: the owner is signed in, the chat's history is
/// saved, and the computer stays asleep while it is being woken.
struct SleepingComputer;
#[async_trait::async_trait]
impl RpcService for SleepingComputer {
    async fn handle(&self, method: &str, _params: Value) -> Result<RpcReply, RpcError> {
        use futures::StreamExt as _;
        let pending = |initial: Value| {
            RpcReply::Stream(Box::pin(
                futures::stream::once(async move { initial }).chain(futures::stream::pending()),
            ))
        };
        match method {
            methods::CLOUD_ACCOUNT_STATUS => {
                Ok(pending(json!({"user":{"id":"owner"},"orgId":null})))
            }
            methods::CLOUD_HISTORY => RpcReply::value(&history()),
            methods::CLOUD_ENVIRONMENT => Ok(pending(json!({"state":"paused","epoch":1}))),
            _ => Ok(RpcReply::Stream(Box::pin(futures::stream::pending()))),
        }
    }
}

/// The computer once awake: it serves the linked image.
struct AwakeComputer {
    png: Vec<u8>,
}
#[async_trait::async_trait]
impl RpcService for AwakeComputer {
    async fn handle(&self, method: &str, _params: Value) -> Result<RpcReply, RpcError> {
        use base64::Engine as _;
        if method == methods::READ_WORKSPACE_IMAGE {
            return RpcReply::value(&zuse_protocol::WorkspaceImageChunk {
                checkout_id: "checkout".into(),
                content_hash: "hash".into(),
                mime_type: "image/png".into(),
                data: base64::engine::general_purpose::STANDARD.encode(&self.png),
                next_offset: self.png.len(),
                size: self.png.len(),
                done: true,
            });
        }
        Ok(RpcReply::Stream(Box::pin(futures::stream::pending())))
    }
}

impl Shell {
    /// Open the scripted cloud chat whose computer is asleep.
    pub fn fixture_open_sleeping_cloud_chat(&mut self, cx: &mut Context<Self>) {
        self.reduced_motion = true;
        self.state.update(cx, |state, _| {
            state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
            let project = json!({"projectId":"project","accountId":"owner",
                "repositoryIdentity":"github.com/owner/repo","displayName":"repo"});
            state.apply_cloud_sidebar_catalog(
                &json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[project]}),
                &json!({"accountId":"owner","organizationId":null,"workspaces":[]}),
            );
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            Arc::new(SleepingComputer),
        ));
        self.open_cloud_conversation(
            CloudTarget {
                controller,
                subject: "owner".into(),
                organization: None,
                workspace: "task".into(),
                initial_session: "session".into(),
            },
            json!({"workspaceId":"task","initialSessionId":"session","title":"Reasoning slider",
                "state":"paused","desiredState":"paused"}),
            Value::Null,
            cx,
        );
    }

    /// Click the reply's image link, as the transcript does.
    pub fn fixture_click_image_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.cloud_conversation.clone() else {
            return;
        };
        let transcript = view.read(cx).transcript_for_test().clone();
        let Some(link) = transcript.update(cx, |transcript, cx| transcript.link_ui(cx)) else {
            return;
        };
        let activation = crate::markdown::render::LinkActivation {
            target: crate::markdown::render::LinkTarget::new("Updated reasoning slider", IMAGE),
            action: crate::markdown::render::LinkAction::Primary,
            source_session: Some("session".into()),
        };
        window.defer(cx, move |window, cx| {
            let _ = (link.handler)(&activation, window, cx);
        });
    }

    /// The wake did not reach the computer.
    pub fn fixture_fail_wake(&mut self, cx: &mut Context<Self>) {
        self.conversation_state(cx).update(cx, |state, cx| {
            state.cloud_wake_failure = Some("Couldn't reach the computer.".into());
            cx.notify();
        });
    }

    /// The computer wakes and its tools connect, serving `png`.
    pub fn fixture_wake_with_image(&mut self, png: Vec<u8>, cx: &mut Context<Self>) {
        let engine = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            Arc::new(AwakeComputer { png }),
        ));
        self.conversation_state(cx).update(cx, |state, cx| {
            state.set_test_engine(engine);
            cx.notify();
        });
    }

    /// Show the conversation again (the chat tab), keeping the file tab.
    pub fn fixture_show_conversation(&mut self, cx: &mut Context<Self>) {
        self.activate_main_tab(None, cx);
    }
}
