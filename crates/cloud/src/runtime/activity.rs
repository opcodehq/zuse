//! Durable inactivity deadlines advance only for meaningful user work. Passive
//! subscriptions, gateway heartbeat traffic and replayed receipts are not activity.
use super::*;

/// Gateway writes and running progress may coalesce within this interval. The
/// idle scheduler adds the same grace so the last coalesced input gets a full
/// idle timeout, including across API restarts.
pub(crate) const COALESCE_MS: i64 = 15_000;

#[derive(Deserialize)]
struct RequestMetadata {
    id: u64,
    method: Option<String>,
    #[serde(default)]
    cancel: bool,
}

pub(crate) fn meaningful_rpc(payload: &zuse_protocol::cloud_gateway::Payload) -> bool {
    use zuse_protocol::methods as m;
    let text = match payload {
        zuse_protocol::cloud_gateway::Payload::Text(text) => text.as_str(),
        zuse_protocol::cloud_gateway::Payload::Binary(bytes) => {
            let Ok(text) = std::str::from_utf8(bytes) else {
                return false;
            };
            text
        }
    };
    if text.len() > zuse_protocol::cloud_gateway::MAX_PAYLOAD {
        return false;
    }
    text.lines()
        .filter_map(|line| serde_json::from_str::<RequestMetadata>(line).ok())
        .any(|request| {
            let _ = request.id;
            !request.cancel
                && request.method.as_deref().is_some_and(|method| {
                    matches!(
                        method,
                        m::QUEUE_COMMAND
                            | m::RELAY_COMMAND
                            | m::QUEUE_MESSAGE
                            | m::UPDATE_QUEUED_MESSAGE
                            | m::BEGIN_QUEUED_MESSAGE_EDIT
                            | m::FINISH_QUEUED_MESSAGE_EDIT
                            | m::MOVE_QUEUED_MESSAGE
                            | m::REMOVE_QUEUED_MESSAGE
                            | m::SEND_QUEUED_MESSAGE_NOW
                            | m::STEER_QUEUED_MESSAGE_NOW
                            | m::CREATE_THREAD_SESSION
                            | m::FORK_SIDE_CHAT
                            | m::FOCUS_CHAT
                            | m::OPEN_TERMINAL
                            | m::WRITE_TERMINAL
                            | m::CLOSE_TERMINAL
                            | m::WRITE_WORKSPACE_FILE
                            | m::MOVE_WORKSPACE_ENTRY
                            | m::DELETE_WORKSPACE_ENTRY
                            | m::CREATE_WORKTREE
                            | m::DELETE_WORKTREE
                            | m::SWITCH_REF
                            | m::FETCH_ALL
                            | m::RUN_PROJECT_ACTION
                            | m::POST_CHANGE_REQUEST_COMMENT
                            | m::UPDATE_CHANGE_REQUEST
                    )
                })
        })
}

impl RuntimeControl {
    /// Called while the caller owns the workspace row lock. This does not bump
    /// revision or updated_at: activity must not emit history revalidation events.
    pub(in crate::runtime) async fn record_activity_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        now: i64,
        force: bool,
    ) -> Result<(), ApiError> {
        let generation = row["request_config"]["runtimeGeneration"]
            .as_i64()
            .unwrap_or(1);
        let epoch = row["request_config"]["gatewayEpoch"]
            .as_i64()
            .unwrap_or(generation);
        sqlx::query("UPDATE api_cloud_workspaces w SET last_activity_at=GREATEST(COALESCE(last_activity_at,0),$4),next_action_at=CASE WHEN request_config->>'runtimeReleaseChangeRequested'='true' OR request_config->'runtimeActivation'->>'phase' IN ('preparing','launching','confirming','rolling-back') THEN next_action_at ELSE GREATEST(COALESCE(next_action_at,0),$5) END WHERE workspace_id=$1 AND state='ready' AND desired_state='ready' AND COALESCE((request_config->>'runtimeGeneration')::bigint,1)=$2 AND COALESCE((request_config->>'gatewayEpoch')::bigint,COALESCE((request_config->>'runtimeGeneration')::bigint,1))=$3 AND EXISTS(SELECT 1 FROM zuse_rust_control_owners o WHERE o.resource_kind='workspace' AND o.resource_id=w.workspace_id AND o.backend='rust' AND o.generation=$2) AND ($6 OR COALESCE(last_activity_at,0)<=$7)")
            .bind(required(row,"workspace_id")?).bind(generation).bind(epoch).bind(now)
            .bind(now.saturating_add(self.idle_ms).saturating_add(COALESCE_MS))
            .bind(force).bind(now.saturating_sub(COALESCE_MS)).execute(&mut **tx).await?;
        Ok(())
    }

    /// The first meaningful frame after a quiet interval is persisted before
    /// forwarding. Pause admission and this write share ownership/row lock order.
    pub(crate) async fn record_gateway_activity(
        &self,
        grant: &crate::gateway::Grant,
        now: i64,
    ) -> Result<bool, ApiError> {
        if grant.runtime
            || grant.protocol != zuse_protocol::cloud_gateway::RUST_PROTOCOL
            || grant.permission != "edit"
            || grant.expires_at <= now
        {
            return Ok(false);
        }
        let mut tx = self.pool.begin().await?;
        let (row, generation, epoch) =
            locked_workspace_with(&mut tx, &grant.workspace, WorkspaceLock::Command).await?;
        if generation != grant.generation
            || epoch != grant.epoch
            || row["state"] != "ready"
            || row["desired_state"] != "ready"
            || row["request_config"]["runtimeRpcProtocol"] != zuse_protocol::cloud_gateway::RUST_RPC
        {
            return Ok(false);
        }
        self.record_activity_tx(&mut tx, &row, now, true).await?;
        tx.commit().await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zuse_protocol::cloud_gateway::Payload;
    #[test]
    fn actual_user_requests_extend_idle_deadline_but_watches_and_transport_do_not() {
        for method in [
            "WriteTerminal",
            "OpenTerminal",
            "CloseTerminal",
            "QueueCommand",
            "WriteWorkspaceFile",
            "SwitchRef",
            "RunProjectAction",
            "FocusChat",
        ] {
            let wire =
                json!({"id":1,"method":method,"params":{"text":"opaque-user-content"}}).to_string();
            assert!(meaningful_rpc(&Payload::Text(wire.clone())), "{method}");
            assert!(
                meaningful_rpc(&Payload::Binary(wire.into_bytes())),
                "{method}"
            );
        }
        for method in [
            "SubscribeTerminal",
            "ResizeTerminal",
            "WatchDocMessages",
            "WatchSessions",
            "WatchChatConfig",
            "WatchWorkspaceFiles",
            "WatchPreviews",
            "CloudHistory",
            "ListTerminals",
            "ReadWorkspaceFile",
            "RenewQueuedMessageEdit",
            "EngineReady",
            "UnknownMethod",
        ] {
            assert!(
                !meaningful_rpc(&Payload::Text(
                    json!({"id":1,"method":method,"params":{}}).to_string()
                )),
                "{method}"
            );
        }
        for payload in [
            "ping",
            "{}",
            "{\"id\":1,\"cancel\":true}",
            "{\"id\":1,\"method\":\"WriteTerminal\",\"cancel\":true}",
            "{\"method\":\"WriteTerminal\"}",
        ] {
            assert!(!meaningful_rpc(&Payload::Text(payload.into())));
        }
        assert!(!meaningful_rpc(&Payload::Binary(vec![0xff, 0x00])));
        assert!(meaningful_rpc(&Payload::Text(
            "{\"id\":1,\"method\":\"SubscribeTerminal\"}\n{\"id\":2,\"method\":\"WriteTerminal\"}"
                .into()
        )));
    }
    #[tokio::test]
    #[ignore = "requires explicit isolated localhost PostgreSQL"]
    async fn durable_activity_updates_share_generation_and_pause_fences_without_revalidation_churn()
    {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        let url =
            std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
        let parsed = reqwest::Url::parse(&url).unwrap();
        assert!(matches!(
            parsed.host_str(),
            Some("127.0.0.1" | "localhost" | "::1")
        ));
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("activity_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let scoped = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .after_connect(move |connection, _| {
                let scoped = scoped.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={scoped}"))
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE zuse_rust_control_owners(resource_kind text,resource_id text,backend text,generation bigint); CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,state text,desired_state text,last_activity_at bigint,next_action_at bigint,request_config jsonb,updated_at bigint,revision bigint); INSERT INTO zuse_rust_control_owners VALUES('workspace','task','rust',2)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO api_cloud_workspaces VALUES('task','ready','ready',0,0,$1,17,3)")
            .bind(
                json!({"runtimeGeneration":2,"gatewayEpoch":4,"runtimeRpcProtocol":"zuse-rpc-v1"}),
            )
            .execute(&pool)
            .await
            .unwrap();
        let signing = ed25519_dalek::SigningKey::from_bytes(&[70; 32]);
        let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([70;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
        let control = RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(&URL_SAFE_NO_PAD.encode([71; 32])).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
        );
        let grant = crate::gateway::Grant {
            workspace: "task".into(),
            generation: 2,
            epoch: 4,
            runtime: false,
            protocol: zuse_protocol::cloud_gateway::RUST_PROTOCOL.into(),
            actor: "owner".into(),
            permission: "edit".into(),
            expires_at: 9_000_000,
        };
        let now = 1_000_000;
        assert!(control.record_gateway_activity(&grant, now).await.unwrap());
        let observed: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT last_activity_at,next_action_at,updated_at,revision FROM api_cloud_workspaces",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(observed, (now, now + 600_000 + COALESCE_MS, 17, 3));
        let mut tx = pool.begin().await.unwrap();
        let (row, _, _) = locked_workspace_with(&mut tx, "task", WorkspaceLock::Command)
            .await
            .unwrap();
        control
            .record_activity_tx(&mut tx, &row, now + 100, false)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let latest: i64 = sqlx::query_scalar("SELECT last_activity_at FROM api_cloud_workspaces")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(latest, now, "streaming progress coalesces writes");
        let mut tx = pool.begin().await.unwrap();
        let (row, _, _) = locked_workspace_with(&mut tx, "task", WorkspaceLock::Command)
            .await
            .unwrap();
        control
            .record_activity_tx(&mut tx, &row, now + 100, true)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let latest: i64 = sqlx::query_scalar("SELECT last_activity_at FROM api_cloud_workspaces")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            latest,
            now + 100,
            "terminal completion always starts a fresh window"
        );
        for invalid in [
            crate::gateway::Grant {
                epoch: 3,
                ..grant.clone()
            },
            crate::gateway::Grant {
                generation: 1,
                ..grant.clone()
            },
            crate::gateway::Grant {
                permission: "view".into(),
                ..grant.clone()
            },
            crate::gateway::Grant {
                expires_at: now,
                ..grant.clone()
            },
            crate::gateway::Grant {
                runtime: true,
                ..grant.clone()
            },
        ] {
            assert!(
                !control
                    .record_gateway_activity(&invalid, now + 200)
                    .await
                    .unwrap()
            );
        }
        sqlx::query(
            "UPDATE api_cloud_workspaces SET next_action_at=11,request_config=request_config||$1",
        )
        .bind(json!({"runtimeReleaseChangeRequested":true}))
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            control
                .record_gateway_activity(&grant, now + 500)
                .await
                .unwrap()
        );
        let due: i64 = sqlx::query_scalar("SELECT next_action_at FROM api_cloud_workspaces")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(due, 11, "activity cannot delay release activation");
        sqlx::query("UPDATE api_cloud_workspaces SET state='pausing',desired_state='paused'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !control
                .record_gateway_activity(&grant, now + 1000)
                .await
                .unwrap(),
            "already-admitted pause must fence forwarding"
        );
        let latest: i64 = sqlx::query_scalar("SELECT last_activity_at FROM api_cloud_workspaces")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(latest, now + 500);
        sqlx::query("UPDATE api_cloud_workspaces SET state='ready',desired_state='ready',request_config=request_config||$1")
            .bind(json!({"runtimeGeneration":3,"gatewayEpoch":5})).execute(&pool).await.unwrap();
        sqlx::query("UPDATE zuse_rust_control_owners SET generation=3")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !control
                .record_gateway_activity(&grant, now + 2000)
                .await
                .unwrap(),
            "old generation cannot postpone new computer sleep"
        );
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
}
