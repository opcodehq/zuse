use super::*;
pub(super) fn binding(pin: &StagingBasePin, workspace: &str) -> RetainedWorkspaceBinding {
    RetainedWorkspaceBinding {
        workspace_id: workspace.into(),
        provider: None,
        release_id: pin.release_id.clone(),
        artifact_id: pin.artifact_id.clone(),
        harness_release: pin.harness_release.clone(),
        manifest_sha256: pin.manifest_sha256.clone(),
        candidate_sha256: pin.candidate_sha256.clone(),
    }
}
#[test]
fn retained_pin_is_exact_workspace_scoped_and_inherits_current_authority() {
    let old = tests::candidate().pin("owner", 2000, true).unwrap();
    let mut newer = tests::candidate();
    newer.release_id.push_str("-next");
    let mut pin = newer.pin("owner", 3000, true).unwrap();
    pin.retained_workspaces
        .push(binding(&old, "workspace-retained"));
    let row = json!({"workspace_id":"workspace-retained","account_id":"owner","provider":"boxd", "request_config":{"managedReleaseId":old.release_id,"managedCandidateSha256":old.candidate_sha256,"managedCandidateArtifactId":old.artifact_id}});
    let effective = pin.pin_for_row(&row).unwrap();
    assert_eq!(effective.expires_at, pin.expires_at);
    assert_eq!(effective.candidate_sha256, old.candidate_sha256);
    assert_eq!(pin.candidate_sha256, digest(&newer).unwrap());
    assert!(effective.matches(&tests::candidate(), 3000).is_err());
    for (field, value) in [
        ("workspace_id", "unlisted"),
        ("account_id", "other"),
        ("provider", "boat"),
    ] {
        let mut wrong = row.clone();
        wrong[field] = json!(value);
        assert!(pin.pin_for_row(&wrong).is_err(), "{field}");
    }
    let mut wrong = row.clone();
    wrong["request_config"]["managedCandidateArtifactId"] = json!("forged");
    assert!(pin.pin_for_row(&wrong).is_err());
    let mut disabled = pin.clone();
    disabled.allow_staging_compute = false;
    assert!(disabled.pin_for_row(&row).is_err());
    let mut duplicate = pin.clone();
    duplicate
        .retained_workspaces
        .push(binding(&old, "workspace-retained"));
    assert!(duplicate.validate().is_err());
    let mut incomplete = serde_json::to_value(&pin).unwrap();
    incomplete["retainedWorkspaces"][0]
        .as_object_mut()
        .unwrap()
        .remove("manifestSha256");
    assert!(serde_json::from_value::<StagingBasePin>(incomplete).is_err());
    let legacy = serde_json::to_value(&old).unwrap();
    assert!(legacy.get("retainedWorkspaces").is_none());
    assert!(
        serde_json::from_value::<StagingBasePin>(legacy)
            .unwrap()
            .retained_workspaces
            .is_empty()
    );
}

#[test]
fn retained_rollover_preserves_the_existing_staging_capacity_policy() {
    let old = tests::candidate().pin("owner", 2000, true).unwrap();
    let mut newer = tests::candidate();
    newer.release_id.push_str("-rollover");
    let mut pin = newer.pin("owner", 3000, true).unwrap();
    pin.retained_workspaces = (0..3)
        .map(|i| binding(&old, &format!("workspace-retained-{i}")))
        .collect();
    assert!(
        pin.validate().is_err(),
        "default pilot still permits only two retained bindings"
    );
    pin.unlimited_computers = true;
    assert!(
        pin.validate().is_ok(),
        "all three existing computers must survive an unlimited staging rollout"
    );
    pin.unlimited_computers = false;
    pin.max_computers = Some(3);
    assert!(pin.validate().is_ok());
    pin.retained_workspaces
        .push(binding(&old, "workspace-fourth"));
    assert!(pin.validate().is_err());
    pin.max_computers = None;
    pin.unlimited_computers = true;
    pin.retained_workspaces = (0..129)
        .map(|i| binding(&old, &format!("workspace-retained-{i}")))
        .collect();
    assert!(
        pin.validate().is_err(),
        "configuration size remains bounded"
    );
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn retained_workspace_resume_is_bound_to_registered_old_candidate_not_new_allocation() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let schema = format!("retained_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
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
    sqlx::raw_sql("CREATE TABLE cloud_managed_bases(provider text,release_id text,artifact_id text,architecture text,capabilities jsonb,state text,created_at bigint,runtime_manifest jsonb,PRIMARY KEY(provider,release_id)); CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,account_id text,provider text,state text,request_config jsonb);").execute(&pool).await.unwrap();
    sqlx::raw_sql(MIGRATION).execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/0023_managed_base_admission_lock.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let old = tests::candidate();
    let old_pin = old.pin("owner", now + 60000, true).unwrap();
    old.stage(&pool, &old_pin, now).await.unwrap();
    let mut newer = old.clone();
    newer.release_id.push_str("-next");
    newer.artifact_id.push_str("-next");
    let mut pin = newer.pin("owner", now + 60000, true).unwrap();
    newer.stage(&pool, &pin, now).await.unwrap();
    pin.retained_workspaces
        .push(binding(&old_pin, "workspace-retained"));
    let row = json!({"workspace_id":"workspace-retained","account_id":"owner","provider":"boxd","state":"paused","request_config":old.settings(&old_pin)});
    sqlx::query("INSERT INTO api_cloud_workspaces VALUES($1,$2,$3,$4,$5)")
        .bind("workspace-retained")
        .bind("owner")
        .bind("boxd")
        .bind("paused")
        .bind(&row["request_config"])
        .execute(&pool)
        .await
        .unwrap();
    pin.row_allowed(&pool, &row, now).await.unwrap();
    let mut connection = pool.acquire().await.unwrap();
    pin.row_allowed_on(&mut connection, &row, now)
        .await
        .unwrap();
    drop(connection);
    let mut tx = pool.begin().await.unwrap();
    assert!(
        compute_allowance(&mut tx, Some(&pin), &row, now)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    // A retained exception cannot select the old image for a new task.
    let mut tx = pool.begin().await.unwrap();
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
    assert_eq!(source["snapshot_id"], newer.artifact_id);
    assert_eq!(
        source["settings"]["managedCandidateSha256"],
        pin.candidate_sha256
    );
    tx.commit().await.unwrap();
    let mut wrong = row.clone();
    wrong["workspace_id"] = json!("workspace-not-retained");
    assert!(pin.row_allowed(&pool, &wrong, now).await.is_err());
    let mut tx = pool.begin().await.unwrap();
    assert!(
        compute_allowance(&mut tx, Some(&pin), &wrong, now)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let mut forged = pin.clone();
    forged.retained_workspaces[0].manifest_sha256 = "0".repeat(64);
    assert!(forged.row_allowed(&pool, &row, now).await.is_err());
    let mut expired = pin.clone();
    expired.expires_at = now;
    assert!(expired.row_allowed(&pool, &row, now).await.is_err());
    sqlx::query("INSERT INTO api_cloud_workspaces SELECT 'extra-'||n,'owner','boxd','ready',$1 FROM generate_series(1,2) n").bind(&row["request_config"]).execute(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        compute_allowance(&mut tx, Some(&pin), &row, now)
            .await
            .unwrap_err()
            .code,
        "staging_candidate_computer_limit"
    );
    tx.rollback().await.unwrap();
    sqlx::query("UPDATE cloud_managed_bases SET artifact_id='tampered' WHERE release_id=$1")
        .bind(&old.release_id)
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

fn candidate_for(provider: ProviderId) -> ManagedBaseCandidate {
    let mut value = tests::candidate();
    value.provider = provider;
    value.capabilities.id = provider;
    value.release_id = format!("{}-{}", value.release_id, provider.as_str());
    value.artifact_id = format!("{}-{}", value.artifact_id, provider.as_str());
    if provider == ProviderId::E2b {
        value.provider_artifact_binding = Some(crate::providers::artifact::ArtifactBinding {
            version: 1,
            mode: "private-single-build".into(),
            artifact_id: value.artifact_id.clone(),
            artifact_version: "retained-fixture-version".into(),
            artifact_name: "zuse-e2b-stage-retained-fixture".into(),
            runtime_sha256: value.runtime_sha256.clone(),
            manifest_sha256: digest(&value.manifest).unwrap(),
        });
    }
    value
}

#[test]
fn retained_provider_is_explicit_legacy_compatible_and_never_inferred_from_row() {
    let old = tests::candidate().pin("owner", 2000, true).unwrap();
    let row = json!({"workspace_id":"workspace-retained","account_id":"owner","provider":"boxd","request_config":tests::candidate().settings(&old)});
    let legacy = binding(&old, "workspace-retained");
    let encoded = serde_json::to_value(&legacy).unwrap();
    assert!(encoded.get("provider").is_none());
    let decoded: RetainedWorkspaceBinding = serde_json::from_value(encoded.clone()).unwrap();
    assert!(decoded.provider.is_none());
    assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
    for provider in [ProviderId::E2b, ProviderId::Boat] {
        let candidate = candidate_for(provider);
        let mut pin = candidate.pin("owner", 3000, true).unwrap();
        pin.retained_workspaces.push(legacy.clone());
        assert!(
            pin.pin_for_row(&row).is_err(),
            "legacy must inherit current provider"
        );
        pin.retained_workspaces[0].provider = Some(ProviderId::Boxd);
        let effective = pin.pin_for_row(&row).unwrap();
        assert_eq!(effective.provider, ProviderId::Boxd);
        assert_eq!(effective.expires_at, pin.expires_at);
        effective.matches(&tests::candidate(), 1000).unwrap();
        assert!(effective.matches(&tests::candidate(), 3000).is_err());
        let current = json!({"account_id":"owner","provider":provider.as_str(),"request_config":candidate.settings(&pin)});
        assert_eq!(pin.pin_for_row(&current).unwrap().provider, provider);
        for (field, value) in [
            ("provider", "daytona"),
            ("workspace_id", "unlisted"),
            ("account_id", "other"),
        ] {
            let mut wrong = row.clone();
            wrong[field] = json!(value);
            assert!(pin.pin_for_row(&wrong).is_err(), "{field}");
        }
        pin.retained_workspaces[0].provider = Some(provider);
        assert!(pin.pin_for_row(&row).is_err());
    }
}

#[test]
fn cross_provider_retained_rule_preserves_organization_actor_fence() {
    let owner = "organization:fixture";
    let mut old = tests::candidate();
    old.manifest.rust_organization_execution_version = Some(1);
    let old_pin = old.pin_for_actor(owner, "actor", 2000, true).unwrap();
    let mut current = candidate_for(ProviderId::E2b);
    current.manifest.rust_organization_execution_version = Some(1);
    let mut pin = current.pin_for_actor(owner, "actor", 3000, true).unwrap();
    let mut rule = binding(&old_pin, "workspace-retained");
    rule.provider = Some(ProviderId::Boxd);
    pin.retained_workspaces.push(rule);
    let mut config = old.settings(&old_pin);
    config["rustActorId"] = json!("actor");
    config["rustOrganizationExecutionVersion"] = json!(1);
    config["agentConnectionId"] = json!("personal");
    config["agentConnectionOwnerId"] = json!("actor");
    config["providerAuthMode"] = json!("broker-v1");
    config["sharingPolicy"] =
        json!({"creatorSubject":"actor","creatorMembershipId":"membership-actor"});
    let row = json!({"workspace_id":"workspace-retained","account_id":owner,"provider":"boxd","request_config":config});
    assert_eq!(pin.pin_for_row(&row).unwrap().provider, ProviderId::Boxd);
    let mut wrong = row;
    wrong["request_config"]["rustActorId"] = json!("another-actor");
    assert!(pin.pin_for_row(&wrong).is_err());
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn retained_boxd_resume_with_e2b_pin_is_provider_fenced_and_allocation_stays_current() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let schema = format!("retained_provider_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .after_connect(move |connection, _| {
            let scope = scope.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={scope}"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE cloud_managed_bases(provider text,release_id text,artifact_id text,architecture text,capabilities jsonb,state text,created_at bigint,runtime_manifest jsonb,PRIMARY KEY(provider,release_id));CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,account_id text,provider text,state text,request_config jsonb);").execute(&pool).await.unwrap();
    sqlx::raw_sql(MIGRATION).execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/0023_managed_base_admission_lock.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let old = tests::candidate();
    let old_pin = old.pin("owner", now + 60000, true).unwrap();
    old.stage(&pool, &old_pin, now).await.unwrap();
    let current = candidate_for(ProviderId::E2b);
    let mut pin = current.pin("owner", now + 60000, true).unwrap();
    current.stage(&pool, &pin, now).await.unwrap();
    let mut rule = binding(&old_pin, "workspace-retained");
    rule.provider = Some(ProviderId::Boxd);
    pin.retained_workspaces.push(rule);
    let row = json!({"workspace_id":"workspace-retained","account_id":"owner","provider":"boxd","state":"paused","request_config":old.settings(&old_pin)});
    sqlx::query("INSERT INTO api_cloud_workspaces VALUES($1,'owner','boxd','paused',$2)")
        .bind("workspace-retained")
        .bind(&row["request_config"])
        .execute(&pool)
        .await
        .unwrap();
    pin.row_allowed(&pool, &row, now).await.unwrap();
    let mut connection = pool.acquire().await.unwrap();
    pin.row_allowed_on(&mut connection, &row, now)
        .await
        .unwrap();
    drop(connection);
    let mut tx = pool.begin().await.unwrap();
    assert!(
        compute_allowance(&mut tx, Some(&pin), &row, now)
            .await
            .unwrap()
    );
    let source = source_for(
        &mut tx,
        Some(&pin),
        "owner",
        "e2b",
        "codex",
        "fixture",
        "default",
        false,
        now,
    )
    .await
    .unwrap();
    assert_eq!(source["snapshot_id"], current.artifact_id);
    assert_eq!(
        source["settings"]["managedCandidateSha256"],
        pin.candidate_sha256
    );
    tx.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    assert!(
        source_for(
            &mut tx,
            Some(&pin),
            "owner",
            "boxd",
            "codex",
            "fixture",
            "default",
            false,
            now
        )
        .await
        .is_err(),
        "retained candidate is not a new allocation source"
    );
    tx.rollback().await.unwrap();
    let mut wrong = row.clone();
    wrong["provider"] = json!("e2b");
    assert!(pin.row_allowed(&pool, &wrong, now).await.is_err());
    let mut missing = pin.clone();
    missing.retained_workspaces[0].provider = None;
    assert!(missing.row_allowed(&pool, &row, now).await.is_err());
    let mut tampered = pin.clone();
    tampered.retained_workspaces[0].manifest_sha256 = "0".repeat(64);
    assert!(tampered.row_allowed(&pool, &row, now).await.is_err());
    sqlx::query("INSERT INTO api_cloud_workspaces SELECT 'extra-'||n,'owner','e2b','ready',$1 FROM generate_series(1,2) n").bind(current.settings(&pin)).execute(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        compute_allowance(&mut tx, Some(&pin), &row, now)
            .await
            .unwrap_err()
            .code,
        "staging_candidate_computer_limit"
    );
    tx.rollback().await.unwrap();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
