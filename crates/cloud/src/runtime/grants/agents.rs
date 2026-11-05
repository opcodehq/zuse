//! Pin a permitted connection on its first agent use; revocation never selects another identity.
use super::*;

fn initial_agent(config: &Value) -> Option<&str> {
    config["agent"].as_str().map(|agent| {
        if agent == "claude-code" {
            "claude"
        } else {
            agent
        }
    })
}
fn binding(config: &Value, agent: &str) -> Result<Option<(String, String)>, ApiError> {
    if config
        .get("agentConnectionBindings")
        .is_some_and(|map| !map.is_object())
    {
        return Err(ApiError::conflict("agent_connection_binding_invalid"));
    }
    if let Some(value) = config
        .get("agentConnectionBindings")
        .and_then(|map| map.get(agent))
    {
        let owner = value["ownerId"].as_str().filter(|s| !s.is_empty());
        let id = value["connectionId"].as_str().filter(|s| !s.is_empty());
        return owner
            .zip(id)
            .map(|(owner, id)| Some((owner.into(), id.into())))
            .ok_or_else(|| ApiError::conflict("agent_connection_binding_invalid"));
    }
    Ok(None)
}
pub(super) fn selected(
    config: &Value,
    agent: &str,
    account: &str,
) -> Result<Option<(String, String)>, ApiError> {
    if let Some(value) = binding(config, agent)? {
        return Ok(Some(value));
    }
    Ok(config["agentConnectionId"].as_str().map(|id| {
        (
            config["agentConnectionOwnerId"]
                .as_str()
                .unwrap_or(account)
                .into(),
            id.into(),
        )
    }))
}

// Match the native new-draft resolver only when no identity has been bound yet.
// Existing task bindings and the initial connection never pass through this chooser.
fn unbound_connection(
    candidates: Vec<(String, String)>,
    account: &str,
    actor: &str,
) -> Option<(String, String)> {
    candidates
        .into_iter()
        .filter(|(owner, id)| {
            (owner == account || owner == actor) && !id.is_empty() && id.trim() == id
        })
        .min_by(|left, right| {
            (left.0.as_str() != account, &left.1).cmp(&(right.0.as_str() != account, &right.1))
        })
}

impl RuntimeControl {
    pub(in crate::runtime) async fn bind_task_agent_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        agent: &str,
    ) -> Result<Option<(String, String)>, ApiError> {
        let config = &row["request_config"];
        let Some(initial_id) = config["agentConnectionId"].as_str() else {
            return Ok(None);
        };
        let account = required(row, "account_id")?;
        let actor = config["rustActorId"].as_str().unwrap_or(account);
        let initial_owner = config["agentConnectionOwnerId"].as_str().unwrap_or(account);
        if initial_owner != account && initial_owner != actor {
            return Err(ApiError::forbidden("agent_connection_owner_mismatch"));
        }
        let known_agent = match initial_agent(config) {
            Some(agent) => Some(agent.to_owned()),
            None => sqlx::query_scalar::<_, String>(
                "SELECT agent FROM cloud_agent_connections WHERE owner_id=$1 AND connection_id=$2",
            )
            .bind(initial_owner)
            .bind(initial_id)
            .fetch_optional(&mut **tx)
            .await?,
        };
        if known_agent.as_deref() == Some(agent) {
            return Ok(Some((initial_owner.into(), initial_id.into())));
        }
        if !matches!(agent, "claude" | "codex" | "grok") {
            return Err(ApiError::conflict("cloud_task_agent_not_installed"));
        }
        let (policy, _) = self.task_policy_tx(tx, row).await?;
        let harness = match agent {
            "claude" => zuse_protocol::HarnessId::ClaudeCode,
            "codex" => zuse_protocol::HarnessId::Codex,
            _ => zuse_protocol::HarnessId::Grok,
        };
        policy.catalog_for(&harness)?;
        if let Some((owner, id)) = binding(config, agent)? {
            if owner != account && owner != actor {
                return Err(ApiError::forbidden("agent_connection_owner_mismatch"));
            }
            let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM cloud_agent_connections WHERE owner_id=$1 AND connection_id=$2 AND agent=$3 AND state='connected' AND credential_ciphertext IS NOT NULL)")
                .bind(&owner).bind(&id).bind(agent).fetch_one(&mut **tx).await?;
            if !valid {
                return Err(ApiError::unavailable("agent_connection_reconnect_required"));
            }
            return Ok(Some((owner, id)));
        }
        let candidates: Vec<(String,String)> = sqlx::query_as("SELECT owner_id,connection_id FROM cloud_agent_connections WHERE owner_id IN($1,$2) AND agent=$3 AND state='connected' AND credential_ciphertext IS NOT NULL ORDER BY CASE WHEN owner_id=$1 THEN 0 ELSE 1 END,connection_id LIMIT 2")
            .bind(account).bind(actor).bind(agent).fetch_all(&mut **tx).await?;
        let (owner, id) = unbound_connection(candidates, account, actor)
            .ok_or_else(|| ApiError::conflict("agent_connection_required"))?;
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{agentConnectionBindings}',COALESCE(request_config->'agentConnectionBindings','{}'::jsonb)||jsonb_build_object($2::text,$3::jsonb)) WHERE workspace_id=$1")
            .bind(required(row,"workspace_id")?).bind(agent).bind(json!({"ownerId":owner,"connectionId":id})).execute(&mut **tx).await?;
        Ok(Some((owner, id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{crypto::MasterKey, tickets::TicketSigner};
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};

    #[test]
    fn malformed_bound_identity_never_falls_back_to_the_launch_connection() {
        let config = json!({"agent":"codex","agentConnectionId":"codex-a","agentConnectionBindings":{"claude":{"connectionId":"claude-a"}}});
        assert!(selected(&config, "claude", "owner").is_err());
        let config = json!({"agentConnectionId":"codex-a","agentConnectionBindings":null});
        assert!(selected(&config, "claude", "owner").is_err());
    }

    #[tokio::test]
    #[ignore = "requires explicit isolated localhost PostgreSQL"]
    async fn provider_binding_is_scoped_stable_and_revocation_does_not_switch_accounts() {
        let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
        assert!(matches!(
            reqwest::Url::parse(&url).unwrap().host_str(),
            Some("localhost" | "127.0.0.1" | "::1")
        ));
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("agent_binding_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .after_connect(move |c, _| {
                let search = search.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={search}"))
                        .execute(c)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,account_id text,request_config jsonb);CREATE TABLE cloud_agent_connections(connection_id text PRIMARY KEY,owner_id text,agent text,state text,credential_ciphertext text,created_at bigint);CREATE TABLE zuse_rust_task_command_policies(workspace_id text,owner_id text,policy_revision bigint,ciphertext text)").execute(&pool).await.unwrap();
        let encoded = B64.encode([63; 32]);
        let key = ed25519_dalek::SigningKey::from_bytes(&[64; 32]);
        let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([64;32]),"x":B64.encode(key.verifying_key().as_bytes())});
        let control = RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(&encoded).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.fixture.test").unwrap(),
        );
        let config = json!({"agent":"codex","rustActorId":"member","agentConnectionId":"codex-a","agentConnectionOwnerId":"member"});
        sqlx::query("INSERT INTO api_cloud_workspaces VALUES('task','organization:team',$1)")
            .bind(config)
            .execute(&pool)
            .await
            .unwrap();
        for (id, owner, agent) in [
            ("codex-a", "member", "codex"),
            ("claude-z", "member", "claude"),
            ("claude-a", "member", "claude"),
            ("grok-a", "member", "grok"),
            ("grok-org", "organization:team", "grok"),
            ("foreign", "administrator", "claude"),
        ] {
            sqlx::query("INSERT INTO cloud_agent_connections VALUES($1,$2,$3,'connected','synthetic-never-opened',0)").bind(id).bind(owner).bind(agent).execute(&pool).await.unwrap();
        }
        let catalog = |id: zuse_protocol::HarnessId| json!({"id":id,"name":"Fixture","defaultModel":"fixture","models":[{"id":"fixture","label":"Fixture"}]});
        let policy = json!({"agent":"codex","connectionId":"codex-a","connectionOwnerId":"member","runtimeMode":"auto","initialModel":"fixture","reasoning":null,"modelOptions":{},"catalog":catalog(zuse_protocol::HarnessId::Codex),"catalogs":{"codex":catalog(zuse_protocol::HarnessId::Codex),"claude":catalog(zuse_protocol::HarnessId::ClaudeCode),"grok":catalog(zuse_protocol::HarnessId::Grok)}});
        let cipher = control
            .master
            .seal_api(
                "task-command-v2:organization:team:task::policy:1",
                &policy.to_string(),
            )
            .unwrap();
        sqlx::query(
            "INSERT INTO zuse_rust_task_command_policies VALUES('task','organization:team',1,$1)",
        )
        .bind(cipher)
        .execute(&pool)
        .await
        .unwrap();
        // A second first-use request must observe the first committed binding,
        // even if a newly added connection would now win a fresh selection.
        let mut first = pool.begin().await.unwrap();
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='task' FOR UPDATE",
        )
        .fetch_one(&mut *first)
        .await
        .unwrap();
        assert_eq!(
            control
                .bind_task_agent_tx(&mut first, &row, "claude")
                .await
                .unwrap(),
            Some(("member".into(), "claude-a".into()))
        );
        let second_started = tokio::sync::Notify::new();
        let second = async {
            let mut tx = pool.begin().await.unwrap();
            second_started.notify_one();
            let row: Value = sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='task' FOR UPDATE")
                .fetch_one(&mut *tx).await.unwrap();
            let selected = control
                .bind_task_agent_tx(&mut tx, &row, "claude")
                .await
                .unwrap();
            tx.commit().await.unwrap();
            selected
        };
        let release = async {
            second_started.notified().await;
            sqlx::query("INSERT INTO cloud_agent_connections VALUES('claude-0','member','claude','connected','synthetic-never-opened',1)")
                .execute(&pool).await.unwrap();
            first.commit().await.unwrap();
        };
        let (second, ()) = tokio::join!(second, release);
        assert_eq!(second, Some(("member".into(), "claude-a".into())));
        for (agent, expected_owner, expected) in [
            ("codex", "member", "codex-a"),
            ("claude", "member", "claude-a"),
            ("grok", "organization:team", "grok-org"),
            ("codex", "member", "codex-a"),
        ] {
            let mut tx = pool.begin().await.unwrap();
            let row:Value=sqlx::query_scalar("SELECT to_jsonb(w)FROM api_cloud_workspaces w WHERE workspace_id='task' FOR UPDATE").fetch_one(&mut *tx).await.unwrap();
            assert_eq!(
                control
                    .bind_task_agent_tx(&mut tx, &row, agent)
                    .await
                    .unwrap(),
                Some((expected_owner.into(), expected.into()))
            );
            tx.commit().await.unwrap();
        }
        sqlx::query(
            "UPDATE cloud_agent_connections SET state='revoked' WHERE connection_id='claude-a'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO cloud_agent_connections VALUES('claude-b','member','claude','connected','another-synthetic',1)").execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w)FROM api_cloud_workspaces w WHERE workspace_id='task' FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        let failure = control
            .bind_task_agent_tx(&mut tx, &row, "claude")
            .await
            .err()
            .unwrap();
        assert_eq!(failure.code, "agent_connection_reconnect_required");
        assert_eq!(
            control
                .bind_task_agent_tx(&mut tx, &row, "codex")
                .await
                .unwrap(),
            Some(("member".into(), "codex-a".into()))
        );
        assert_eq!(
            row["request_config"]["agentConnectionBindings"]["claude"]["connectionId"],
            "claude-a"
        );
        tx.rollback().await.unwrap();
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
}

#[cfg(test)]
mod unbound_selection_tests {
    use super::unbound_connection;

    #[test]
    fn first_binding_is_stable_across_response_order_and_prefers_workspace_scope() {
        let personal = vec![
            ("member".into(), "claude-z".into()),
            ("member".into(), "claude-a".into()),
        ];
        let expected = Some(("member".into(), "claude-a".into()));
        assert_eq!(
            unbound_connection(personal.clone(), "organization:team", "member"),
            expected
        );
        assert_eq!(
            unbound_connection(
                personal.into_iter().rev().collect(),
                "organization:team",
                "member"
            ),
            expected
        );
        let both = vec![
            ("member".into(), "a-personal".into()),
            ("organization:team".into(), "z-organization".into()),
        ];
        let expected = Some(("organization:team".into(), "z-organization".into()));
        assert_eq!(
            unbound_connection(both.clone(), "organization:team", "member"),
            expected
        );
        assert_eq!(
            unbound_connection(
                both.into_iter().rev().collect(),
                "organization:team",
                "member"
            ),
            expected
        );
    }

    #[test]
    fn first_binding_never_selects_another_actor_or_malformed_identity() {
        assert_eq!(
            unbound_connection(
                vec![
                    ("administrator".into(), "a".into()),
                    ("member".into(), " ".into())
                ],
                "organization:team",
                "member"
            ),
            None
        );
        assert_eq!(
            unbound_connection(
                vec![("member".into(), "z".into()), ("member".into(), "a".into())],
                "member",
                "member"
            ),
            Some(("member".into(), "a".into()))
        );
    }
}
