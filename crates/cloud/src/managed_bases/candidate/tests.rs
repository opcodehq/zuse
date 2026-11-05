use super::*;
use zuse_protocol::cloud_connections::{AgentConnection, ConnectionMethod, ConnectionState};
pub(super) fn candidate() -> ManagedBaseCandidate {
    let r = super::super::tests::release();
    let harnesses = [
        (AgentId::Claude, zuse_protocol::HarnessId::ClaudeCode),
        (AgentId::Codex, zuse_protocol::HarnessId::Codex),
        (AgentId::Grok, zuse_protocol::HarnessId::Grok),
    ]
    .into_iter()
    .map(|(agent, id)| {
        let mut h = r.harnesses[0].clone();
        h.agent = agent;
        h.catalog.id = id;
        h
    })
    .collect();
    ManagedBaseCandidate {
        provider: r.provider,
        release_id: r.release_id,
        artifact_id: r.artifact_id,
        provider_artifact_binding: None,
        initial_runtime_bootstrap: None,
        architecture: r.architecture,
        capabilities: r.capabilities,
        manifest: r.manifest,
        harnesses,
        harness_release: "d".repeat(64),
        runtime_sha256: "e".repeat(64),
        provenance_report_sha256: "f".repeat(64),
    }
}
#[test]
fn command_receipt_matches_actual_candidate_runtime() {
    let mut candidate = candidate();
    let mut release = super::super::tests::release();
    super::super::tests::enable_command_admission(&mut release);
    candidate.manifest = release.manifest;
    candidate.validate().unwrap();
    candidate.runtime_sha256 = "0".repeat(64);
    assert!(candidate.validate().is_err());
}
#[test]
fn candidate_pins_require_staging_single_owner_and_exact_content() {
    let c = candidate();
    let p = c.pin("owner", 2000, true).unwrap();
    p.deployment("staging", &["owner".into()], &["owner".into()], 1000)
        .unwrap();
    assert!(
        p.deployment("production", &["owner".into()], &["owner".into()], 1000)
            .is_err()
    );
    assert!(
        p.deployment(
            "staging",
            &["owner".into(), "other".into()],
            &["owner".into()],
            1000
        )
        .is_err()
    );
    let mut changed = c.clone();
    changed.harnesses[0].sha256 = "1".repeat(64);
    assert!(p.matches(&changed, 1000).is_err());
    changed = c.clone();
    changed.manifest.plan_digest = "2".repeat(64);
    assert!(p.matches(&changed, 1000).is_err());
    assert!(p.matches(&c, 2000).is_err());
    assert!(
        serde_json::to_value(&c)
            .unwrap()
            .get("certificationReportSha256")
            .is_none()
    );
}
#[test]
fn candidate_catalog_revision_reuses_clean_artifact_without_inventing_models() {
    let c = candidate();
    let mut claude = c.clone();
    claude.harnesses.retain(|h| h.agent == AgentId::Claude);
    claude.release_id = format!("{}-catalog-claude", c.manifest.build_id);
    claude.validate().unwrap();
    let p = claude.pin("owner", 2000, true).unwrap();
    assert_eq!(claude.artifact_id, c.artifact_id);
    assert_eq!(claude.manifest.build_id, c.manifest.build_id);
    assert_eq!(claude.harness_release, c.harness_release);
    assert_ne!(claude.release_id, c.release_id);
    assert_eq!(claude.harnesses.len(), 1);
    assert!(p.matches(&c, 1000).is_err());
    claude.harnesses.clear();
    assert!(claude.validate().is_err());
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn staging_candidate_is_owner_only_uncertified_and_fenced() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let schema = format!("candidate_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .after_connect(move |c, _| {
            let scope = scope.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={scope}"))
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE cloud_managed_bases(provider text,release_id text,artifact_id text,architecture text,capabilities jsonb,state text,created_at bigint,runtime_manifest jsonb,PRIMARY KEY(provider,release_id));CREATE TABLE api_entitlements(account_id text,kind text,status text,paid_through bigint);").execute(&pool).await.unwrap();
    sqlx::raw_sql(MIGRATION).execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/0023_managed_base_admission_lock.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let c = candidate();
    let pin = Arc::new(c.pin("owner", now + 60000, true).unwrap());
    c.stage(&pool, &pin, now).await.unwrap();
    c.stage(&pool, &pin, now).await.unwrap();
    let state: (String, bool) =
        sqlx::query_as("SELECT state,runtime_manifest IS NULL FROM cloud_managed_bases")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, ("preparing".into(), true));
    let connection = AgentConnection {
        connection_id: "selected".into(),
        agent: AgentId::Codex,
        method: ConnectionMethod::DeviceCode,
        state: ConnectionState::Connected,
        account_label: None,
        expires_at: None,
        version: 1,
        error_code: None,
        credential_update: None,
        credential_update_key: None,
    };
    let public = crate::store::Store::from_pool(pool.clone());
    assert!(
        public
            .managed_launch_catalog_for("owner", &[connection.clone()])
            .await
            .unwrap()
            .providers
            .is_empty()
    );
    let store = public.with_staging_base_pin(Some(pin.clone()));
    assert!(
        store
            .managed_launch_catalog_for("other", &[connection.clone()])
            .await
            .unwrap()
            .providers
            .is_empty()
    );
    assert_eq!(
        store
            .managed_launch_catalog_for("owner", &[connection.clone()])
            .await
            .unwrap()
            .providers
            .len(),
        1
    );
    let caps = store.computer_capabilities("owner", now).await.unwrap();
    assert!(caps.can_allocate);
    assert_eq!(caps.staging_candidate.unwrap().status, "uncertified");
    assert!(
        !store
            .computer_capabilities("other", now)
            .await
            .unwrap()
            .can_allocate
    );
    // Match the deployed API pool, not the writable fixture pool: catalog and
    // capability reads must not take FOR SHARE locks in read-only transactions.
    let readonly_url = format!(
        "{url}{}options=-csearch_path%3D{schema}",
        if url.contains('?') { "&" } else { "?" }
    );
    let readonly = crate::store::Store::connect_read_only(&readonly_url)
        .await
        .unwrap()
        .with_staging_base_pin(Some(pin.clone()));
    let enforced: bool = sqlx::query_scalar("SHOW default_transaction_read_only")
        .fetch_one(&readonly.pool)
        .await
        .map(|v: String| v == "on")
        .unwrap();
    assert!(enforced);
    assert_eq!(
        readonly
            .managed_launch_catalog_for("owner", &[connection.clone()])
            .await
            .unwrap()
            .providers
            .len(),
        1
    );
    assert_eq!(
        readonly
            .computer_capabilities("owner", now)
            .await
            .unwrap()
            .staging_candidate
            .unwrap()
            .status,
        "uncertified"
    );
    // A caller accidentally requesting a writer lock on that pool still fails;
    // admissions and lifecycle fencing keep their original locked query.
    let mut readonly_tx = readonly.pool.begin().await.unwrap();
    assert!(pin.load(&mut readonly_tx, now, true).await.is_err());
    readonly_tx.rollback().await.unwrap();
    // Discovery survives an expired/missing exact pin even when a different
    // verified base could otherwise be offered. Admission still rejects it.
    let mut fallback = super::super::tests::release();
    fallback.release_id.push_str("-fallback");
    fallback.manifest.build_id = fallback.release_id.clone();
    fallback.artifact_id.push_str("-different");
    fallback.validate().unwrap();
    sqlx::query("INSERT INTO cloud_managed_bases(provider,release_id,artifact_id,architecture,capabilities,state,created_at,runtime_manifest) VALUES($1,$2,$3,$4,$5,'verified',$6,$7)")
        .bind(fallback.provider.as_str()).bind(&fallback.release_id)
        .bind(&fallback.artifact_id).bind(&fallback.architecture)
        .bind(json!(fallback.capabilities)).bind(now).bind(json!(fallback))
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO api_entitlements VALUES('owner','cloud-workspace','active',NULL)")
        .execute(&pool)
        .await
        .unwrap();
    let unpinned = crate::store::Store::from_pool(pool.clone());
    assert!(
        !unpinned
            .provider_launch_catalogs_for("owner", &[connection.clone()])
            .await
            .unwrap()["boxd"]
            .providers
            .is_empty()
    );
    let mut expired = (*pin).clone();
    expired.expires_at = now - 1;
    let expired = Arc::new(expired);
    let expired_store = crate::store::Store::connect_read_only(&readonly_url)
        .await
        .unwrap()
        .with_staging_base_pin(Some(expired.clone()));
    let mut fenced = (*pin).clone();
    fenced.candidate_sha256 = "0".repeat(64);
    let fenced_store = crate::store::Store::from_pool(readonly.pool.clone())
        .with_staging_base_pin(Some(Arc::new(fenced)));
    for (selected, reason) in [
        (&expired_store, "staging_managed_base_candidate_expired"),
        (&fenced_store, "staging_managed_base_candidate_fenced"),
        (&readonly, "staging_managed_base_candidate_unavailable"),
    ] {
        if reason.ends_with("unavailable") {
            sqlx::query("UPDATE cloud_managed_bases SET state='failed' WHERE release_id=$1")
                .bind(&pin.release_id)
                .execute(&pool)
                .await
                .unwrap();
        }
        assert!(
            selected
                .provider_launch_catalogs_for("owner", &[connection.clone()])
                .await
                .unwrap()["boxd"]
                .providers
                .is_empty()
        );
        assert!(
            selected
                .managed_launch_catalog_for("owner", &[connection.clone()])
                .await
                .unwrap()
                .providers
                .is_empty()
        );
        let caps = selected.computer_capabilities("owner", now).await.unwrap();
        assert!(!caps.can_allocate);
        assert!(caps.allocation_blockers.iter().any(|b| b == reason));
        let provider = caps
            .providers
            .iter()
            .find(|p| p.id == ProviderId::Boxd)
            .unwrap();
        assert!(!provider.can_create());
        assert!(provider.sizes.is_empty());
        assert_eq!(provider.unavailable_reason.as_deref(), Some(reason));
        assert_eq!(caps.staging_candidate.unwrap().status, "unavailable");
    }
    let mut expired_tx = pool.begin().await.unwrap();
    // Restore the candidate so this specifically proves expiry, not absence.
    sqlx::query("UPDATE cloud_managed_bases SET state='preparing' WHERE release_id=$1")
        .bind(&pin.release_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        expired
            .load(&mut expired_tx, now, false)
            .await
            .err()
            .unwrap()
            .code,
        "staging_managed_base_candidate_expired"
    );
    expired_tx.rollback().await.unwrap();
    expired_store.pool.close().await;
    sqlx::query("DELETE FROM cloud_managed_bases WHERE release_id=$1")
        .bind(&fallback.release_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM api_entitlements WHERE account_id='owner'")
        .execute(&pool)
        .await
        .unwrap();
    readonly.pool.close().await;
    let mut tx = pool.begin().await.unwrap();
    assert!(
        source_for(
            &mut tx,
            Some(&pin),
            "other",
            "boxd",
            "codex",
            "fixture",
            "default",
            false,
            now
        )
        .await
        .is_err()
    );
    let source = source_for(
        &mut tx,
        Some(&pin),
        "owner",
        "boxd",
        "codex",
        "fixture",
        "default",
        false,
        now,
    )
    .await
    .unwrap();
    assert_eq!(source["snapshot_id"], c.artifact_id);
    assert_eq!(
        source["settings"]["managedBaseReadiness"],
        "uncertified-staging"
    );
    tx.commit().await.unwrap();
    let row = json!({"account_id":"owner","provider":"boxd","request_config":source["settings"]});
    pin.row_allowed(&pool, &row, now).await.unwrap();
    let mut wrong = row.clone();
    wrong["account_id"] = json!("other");
    assert!(pin.row_allowed(&pool, &wrong, now).await.is_err());
    wrong = row.clone();
    wrong["request_config"]["managedCandidateSha256"] = json!("0".repeat(64));
    assert!(pin.row_allowed(&pool, &wrong, now).await.is_err());
    let mut revision = c.clone();
    revision.release_id = format!("{}-catalog-claude", c.manifest.build_id);
    revision.harnesses.retain(|h| h.agent == AgentId::Claude);
    let revision_pin = Arc::new(revision.pin("owner", now + 60000, true).unwrap());
    revision.stage(&pool, &revision_pin, now).await.unwrap();
    let mut claude_connection = connection.clone();
    claude_connection.agent = AgentId::Claude;
    let revised = crate::store::Store::from_pool(pool.clone())
        .with_staging_base_pin(Some(revision_pin.clone()));
    assert!(
        revised
            .managed_launch_catalog_for("owner", &[connection.clone()])
            .await
            .unwrap()
            .providers
            .is_empty()
    );
    assert_eq!(
        revised
            .managed_launch_catalog_for("owner", &[claude_connection])
            .await
            .unwrap()
            .providers
            .len(),
        1
    );
    let mut tx = pool.begin().await.unwrap();
    let revised_source = source_for(
        &mut tx,
        Some(&revision_pin),
        "owner",
        "boxd",
        "claude",
        "fixture",
        "default",
        false,
        now,
    )
    .await
    .unwrap();
    assert_eq!(revised_source["snapshot_id"], c.artifact_id);
    assert_eq!(
        revised_source["settings"]["managedReleaseId"],
        revision.release_id
    );
    tx.commit().await.unwrap();
    let counts: (i64, i64) =
        sqlx::query_as("SELECT count(*),count(DISTINCT artifact_id) FROM cloud_managed_bases")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(counts, (2, 1));
    let mut changed = c.clone();
    changed.runtime_sha256 = "3".repeat(64);
    assert!(
        changed
            .stage(
                &pool,
                &changed.pin("owner", now + 60000, true).unwrap(),
                now
            )
            .await
            .is_err()
    );
    assert!(
        super::super::tests::release()
            .register(&pool, now)
            .await
            .is_err(),
        "ordinary registration cannot label a candidate certified"
    );
    sqlx::query("UPDATE cloud_managed_bases SET artifact_id='tampered'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(pin.row_allowed(&pool, &row, now).await.is_err());
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
