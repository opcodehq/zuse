//! One locked metadata snapshot replaces repeated scans before each lease poll.
use super::*;

struct Pending {
    command_id: String,
    state: String,
    ever_leased: bool,
    destruction_fence: Option<i64>,
    runtime_generation: Option<i64>,
    storage_incarnation_id: Option<String>,
    lease_deadline: Option<i64>,
}

pub(super) async fn run(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    destruction: i64,
    generation: i64,
    storage: &str,
    now: i64,
) -> Result<bool, ApiError> {
    // The caller holds the exclusive workspace row, as every command mutator
    // does. Classify in the original order, updating this snapshot after each
    // write so a terminal command cannot be reconsidered by a later stage.
    let records = sqlx::query(&format!("SELECT command_id,state,ever_leased,(envelope->>'destructionFence')::bigint AS destruction_fence,runtime_generation,storage_incarnation_id,lease_deadline FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND state NOT IN {TERMINAL} AND ((envelope->>'destructionFence')::bigint<$2 OR (ever_leased AND storage_incarnation_id<>$3) OR state IN ('leased','lease-expired-awaiting-fence'))"))
        .bind(id).bind(destruction).bind(storage).fetch_all(&mut **tx).await?;
    let mut rows: Vec<Pending> = records
        .into_iter()
        .map(|row| Pending {
            command_id: row.get("command_id"),
            state: row.get("state"),
            ever_leased: row.get("ever_leased"),
            destruction_fence: row.get("destruction_fence"),
            runtime_generation: row.get("runtime_generation"),
            storage_incarnation_id: row.get("storage_incarnation_id"),
            lease_deadline: row.get("lease_deadline"),
        })
        .collect();
    for row in &mut rows {
        if row
            .destruction_fence
            .is_some_and(|fence| fence < destruction)
        {
            let (state, assignment) = if row.ever_leased {
                (
                    "outcome-unknown",
                    "state='outcome-unknown',category='workspace-destruction-fence-advanced-after-lease',lease_token=NULL,lease_deadline=NULL",
                )
            } else {
                (
                    "cancelled",
                    "state='cancelled',category='workspace-destruction-fence-advanced',lease_token=NULL,lease_deadline=NULL",
                )
            };
            change(tx, id, &row.command_id, assignment, now).await?;
            row.state = state.into();
        }
    }
    for row in &mut rows {
        if row.ever_leased
            && !terminal(&row.state)
            && row
                .storage_incarnation_id
                .as_deref()
                .is_some_and(|disk| disk != storage)
        {
            change(tx,id,&row.command_id,"state='outcome-unknown',category='runtime-storage-replaced',lease_token=NULL,lease_deadline=NULL",now).await?;
            row.state = "outcome-unknown".into();
        }
    }
    for row in &mut rows {
        if row.state == "leased" && row.lease_deadline.is_some_and(|until| until <= now) {
            change(
                tx,
                id,
                &row.command_id,
                "state='lease-expired-awaiting-fence',category='runtime-lease-expired'",
                now,
            )
            .await?;
            row.state = "lease-expired-awaiting-fence".into();
        }
    }
    for row in &mut rows {
        if row.state == "lease-expired-awaiting-fence"
            && row
                .runtime_generation
                .is_some_and(|prior| prior < generation)
            && row.storage_incarnation_id.as_deref() == Some(storage)
        {
            change(tx,id,&row.command_id,"state='waiting-for-runtime',category='runtime-generation-fenced',lease_token=NULL,lease_deadline=NULL",now).await?;
            row.state = "waiting-for-runtime".into();
        }
    }
    Ok(rows
        .iter()
        .any(|row| row.state == "lease-expired-awaiting-fence"))
}

fn terminal(state: &str) -> bool {
    matches!(
        state,
        "applied" | "rejected" | "expired" | "cancelled" | "outcome-unknown"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires isolated localhost PostgreSQL"]
    async fn one_snapshot_matches_ordered_lease_maintenance_including_nulls() {
        let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
        assert!(matches!(
            reqwest::Url::parse(&url).unwrap().host_str(),
            Some("localhost" | "127.0.0.1" | "::1")
        ));
        let admin = PgPool::connect(&url).await.unwrap();
        let mut schemas = Vec::new();
        let mut pools = Vec::new();
        for _ in 0..2 {
            let schema = format!("lease_scan_{}", uuid::Uuid::new_v4().simple());
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
            sqlx::raw_sql(MIGRATION).execute(&pool).await.unwrap();
            sqlx::raw_sql("INSERT INTO zuse_rust_mailboxes(workspace_id) VALUES('ws'),('empty'); INSERT INTO zuse_rust_mailbox_commands(workspace_id,command_id,workspace_sequence,revision,lane,fingerprint,envelope,envelope_bytes,state,ever_leased,lease_token,lease_deadline,runtime_generation,storage_incarnation_id,accepted_at,updated_at) SELECT 'ws',row_number()OVER(ORDER BY state,ever,destruction NULLS FIRST,disk NULLS FIRST,generation NULLS FIRST,deadline NULLS FIRST)::text,row_number()OVER(ORDER BY state,ever,destruction NULLS FIRST,disk NULLS FIRST,generation NULLS FIRST,deadline NULLS FIRST),0,'task','fingerprint',jsonb_build_object('destructionFence',destruction),0,state,ever,'lease',deadline,generation,disk,1,1 FROM(VALUES('accepted'),('waiting-for-runtime'),('leased'),('lease-expired-awaiting-fence'),('blocked'),('applied'),('cancelled'),('outcome-unknown'))s(state) CROSS JOIN(VALUES(false),(true))e(ever) CROSS JOIN(VALUES(NULL::bigint),(4),(5))d(destruction) CROSS JOIN(VALUES(NULL::text),('disk'),('other'))k(disk) CROSS JOIN(VALUES(NULL::bigint),(1),(2))g(generation) CROSS JOIN(VALUES(NULL::bigint),(50),(150))l(deadline);").execute(&pool).await.unwrap();
            schemas.push(schema);
            pools.push(pool);
        }
        let old = &pools[0];
        let mut old_tx = old.begin().await.unwrap();
        let mut transitions = 0;
        for sql in [
            format!("UPDATE zuse_rust_mailbox_commands SET state=CASE WHEN ever_leased THEN 'outcome-unknown' ELSE 'cancelled' END,category=CASE WHEN ever_leased THEN 'workspace-destruction-fence-advanced-after-lease' ELSE 'workspace-destruction-fence-advanced' END,lease_token=NULL,lease_deadline=NULL WHERE workspace_id='ws' AND(envelope->>'destructionFence')::bigint<5 AND state NOT IN {TERMINAL}"),
            format!("UPDATE zuse_rust_mailbox_commands SET state='outcome-unknown',category='runtime-storage-replaced',lease_token=NULL,lease_deadline=NULL WHERE workspace_id='ws' AND ever_leased AND storage_incarnation_id<>'disk' AND state NOT IN {TERMINAL}"),
            "UPDATE zuse_rust_mailbox_commands SET state='lease-expired-awaiting-fence',category='runtime-lease-expired' WHERE workspace_id='ws' AND state='leased' AND lease_deadline<=100".into(),
            "UPDATE zuse_rust_mailbox_commands SET state='waiting-for-runtime',category='runtime-generation-fenced',lease_token=NULL,lease_deadline=NULL WHERE workspace_id='ws' AND state='lease-expired-awaiting-fence' AND runtime_generation<2 AND storage_incarnation_id='disk'".into(),
        ] {
            transitions += sqlx::query(&sql).execute(&mut *old_tx).await.unwrap().rows_affected();
        }
        let old_fence: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_mailbox_commands WHERE workspace_id='ws' AND state='lease-expired-awaiting-fence')").fetch_one(&mut *old_tx).await.unwrap();
        old_tx.commit().await.unwrap();
        let mut tx = pools[1].begin().await.unwrap();
        assert_eq!(
            run(&mut tx, "ws", 5, 2, "disk", 100).await.unwrap(),
            old_fence
        );
        assert!(!run(&mut tx, "empty", 5, 2, "disk", 100).await.unwrap());
        let revision: i64 =
            sqlx::query_scalar("SELECT revision FROM zuse_rust_mailboxes WHERE workspace_id='ws'")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(
            revision as u64, transitions,
            "retain each ordered revision, including expired then generation-fenced"
        );
        tx.commit().await.unwrap();
        let sql = "SELECT jsonb_agg(jsonb_build_array(command_id,state,category,lease_token,lease_deadline)ORDER BY command_id) FROM zuse_rust_mailbox_commands WHERE workspace_id='ws'";
        let expected: Value = sqlx::query_scalar(sql).fetch_one(old).await.unwrap();
        let actual: Value = sqlx::query_scalar(sql).fetch_one(&pools[1]).await.unwrap();
        assert_eq!(actual, expected);
        for (pool, schema) in pools.into_iter().zip(schemas) {
            pool.close().await;
            sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
                .execute(&admin)
                .await
                .unwrap();
        }
        admin.close().await;
    }
}
