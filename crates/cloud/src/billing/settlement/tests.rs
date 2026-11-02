use super::*;
use crate::providers::boxd_wire as wire;
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, Response},
    routing::post,
};
use prost::Message;
use std::sync::{Arc, Mutex};
#[derive(Clone)]
struct Fixture {
    start: i64,
    end: i64,
    calls: Arc<Mutex<Vec<(String, i64, i64)>>>,
}
fn frame(bytes: &[u8]) -> Vec<u8> {
    let mut r = vec![0];
    r.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    r.extend_from_slice(bytes);
    r
}
fn reply(message: impl Message) -> Response<Body> {
    let mut r = frame(&message.encode_to_vec());
    let mut trailer = frame(b"grpc-status: 0\r\n");
    trailer[0] = 128;
    r.extend(trailer);
    Response::builder()
        .header("content-type", "application/grpc-web+proto")
        .body(Body::from(r))
        .unwrap()
}
async fn token() -> axum::Json<Value> {
    axum::Json(json!({"token":"usage-token","expires_at":chrono::Utc::now().timestamp()+3600}))
}
fn usage(id: &str, cost: i64, complete: bool) -> wire::VmUsage {
    wire::VmUsage {
        vm_id: id.into(),
        vcpu: 4,
        memory_bytes: 16 * 1024 * 1024 * 1024,
        disk_bytes: 0,
        seconds: None,
        cost_micro: cost,
        complete,
    }
}
async fn rpc(
    State(f): State<Fixture>,
    Path(method): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    assert_eq!(headers["authorization"], "Bearer usage-token");
    assert_eq!(body[0], 0);
    assert_eq!(
        u32::from_be_bytes(body[1..5].try_into().unwrap()) as usize,
        body.len() - 5
    );
    if method == "GetOrgUsage" {
        let r = wire::OrgUsageRequest::decode(&body[5..]).unwrap();
        assert_eq!(r.org, "fixture-org");
        assert_eq!((r.since, r.until), (f.start / 1000, f.end / 1000));
        reply(wire::OrgUsageResponse {
            org_id: "org".into(),
            org: "fixture-org".into(),
            period_start: r.since,
            period_end: r.until,
            currency: "usd".into(),
            total_cost_micro: 200_000_000,
            complete: true,
            vms: vec![
                usage("machine-alice", 15_000_000, true),
                usage("retired-machine", 2_000_000, true),
                usage("build-machine", 3_000_000, true),
                usage("machine-bob", 999_000_000, true),
                usage("incomplete", 1, false),
                usage("unknown", 1, true),
                usage("manual-machine", 50_000_000, true),
                usage("cap-machine", 100_000_000, true),
                usage("legacy-machine", 1, true),
            ],
        })
    } else {
        assert_eq!(method, "GetVmUsage");
        let r = wire::VmUsageRequest::decode(&body[5..]).unwrap();
        assert_eq!(r.org, "fixture-org");
        f.calls
            .lock()
            .unwrap()
            .push((r.vm_id.clone(), r.since, r.until));
        let first = r.until == (f.start + (f.end - f.start) / 2) / 1000;
        let cost = match r.vm_id.as_str() {
            "machine-alice" => {
                if first {
                    10_000_000
                } else {
                    5_000_000
                }
            }
            "retired-machine" => 1_000_000,
            "build-machine" => {
                if first {
                    2_000_000
                } else {
                    1_000_000
                }
            }
            "manual-machine" => 50_000_000,
            "cap-machine" => 100_000_000,
            _ => panic!("Unowned/incomplete/finalized resource read"),
        };
        reply(wire::VmUsageResponse {
            period_start: r.since,
            period_end: r.until,
            currency: "usd".into(),
            usage: Some(usage(&r.vm_id, cost, true)),
        })
    }
}
#[test]
fn rotating_closed_windows_and_original_financial_policy() {
    let start = 1700006400000;
    let w = windows(start, start + 30 * DAY + 3600000).unwrap();
    assert!(w.len() <= 8);
    assert_eq!(w.last().unwrap().1, start + 30 * DAY);
    assert!(windows(start, start + 20 * 60_000).unwrap().is_empty());
    let p = json!({"included_provider_cost_micros":35_000_000,"markup_basis_points":500,"overage_cap_micros":25_000_000,"status":"active"});
    let d = deltas(&p, 30_000_000, 0, 10_000_000).unwrap();
    assert_eq!(
        d,
        vec![
            ("included-allowance", 5_000_000),
            ("overage-provider-cost", 5_000_000),
            ("overage-charge", 5_250_000),
            ("markup", 250_000),
            ("overshoot-absorbed", 0)
        ]
    );
    let mut cap = p.clone();
    cap["overage_cap_micros"] = json!(10_000_000);
    assert_eq!(deltas(&cap, 0, 0, 100_000_000).unwrap()[4].1, 58_250_000);
    cap["status"] = json!("manual");
    assert_eq!(
        deltas(&cap, 0, 0, 50_000_000).unwrap()[4],
        ("manual-entitlement-credit", 15_750_000)
    );
    assert!(
        segments(
            &[
                json!({"period_start":1,"period_end":5,"currency":"USD"}),
                json!({"period_start":4,"period_end":10,"currency":"USD"})
            ],
            1,
            10
        )
        .is_err()
    );
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn boxd_reports_settle_periods_old_machines_manual_caps_and_atomic_retries() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated database required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("settlement_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let copy = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .after_connect(move |conn, _| {
            let schema = copy.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={schema}"))
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    for sql in [
        include_str!("../../../tests/fixtures/legacy-catalog.sql"),
        include_str!("../../../tests/fixtures/settlement.sql"),
        crate::jobs::MIGRATION,
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    let now = chrono::Utc::now().timestamp_millis();
    let end = (now - 30 * 60_000) / DAY * DAY;
    let start = end - DAY;
    let middle = start + DAY / 2;
    for (id, owner, begin, finish, status, cap) in [
        ("p-first", "user-alice", start, middle, "active", 25_000_000),
        ("p-second", "user-alice", middle, end, "active", 25_000_000),
        ("p-manual", "manual", start, end, "manual", 25_000_000),
        ("p-cap", "cap", start, end, "active", 10_000_000),
    ] {
        sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,billing_provider,provider_subscription_id,status,period_start,period_end,base_price_micros,included_provider_cost_micros,markup_basis_points,overage_cap_micros,created_at,updated_at) VALUES($1,$2,'stripe','sub-preserved',$3,$4,$5,40000000,35000000,500,$6,$4,$4)").bind(id).bind(owner).bind(status).bind(begin).bind(finish).bind(cap as i64).execute(&pool).await.unwrap();
    }
    sqlx::query("UPDATE api_cloud_workspaces SET created_at=$1")
        .bind(start + 1000)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("UPDATE api_cloud_workspaces SET request_config='{\"providerConnectionId\":\"customer-key\"}' WHERE workspace_id='ws-bob'; INSERT INTO zuse_rust_control_owners VALUES('billing','user-alice','rust',1,0),('billing','manual','rust',1,0),('billing','cap','rust',1,0); INSERT INTO api_cloud_billing_ledger VALUES('original-cost','p-first','user-alice','provider-cost',30000000,'original','{}',1,1)").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO api_cloud_project_builds VALUES('build-alice','user-alice','boxd','build-machine','{}',$1),('manual-build','manual','boxd','manual-machine','{}',$1),('cap-build','cap','boxd','cap-machine','{}',$1)").bind(start+1000).execute(&pool).await.unwrap();
    for machine in ["retired-machine", "legacy-machine"] {
        sqlx::query("INSERT INTO api_cloud_runtime_observations VALUES('boxd',$1,$2)").bind(machine).bind(json!({"accountId":"user-alice","resourceKind":"workspace","resourceId":"ws-alice"})).execute(&pool).await.unwrap();
    }
    let legacy = format!("usage:legacy-machine:{start}:{end}");
    sqlx::query(
        "INSERT INTO api_provider_event_finalizations VALUES('boxd','historical-event',$1,$2,$3)",
    )
    .bind(legacy)
    .bind(now)
    .bind(now + DAY)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO api_cloud_billing_reservations VALUES('p-first','user-alice','workspace','ws-alice','boxd',1000000,$1,4,16384,$2,$1)").bind(start).bind(end).execute(&pool).await.unwrap();
    // Fail the second half of a build: the first half must roll back with it.
    sqlx::raw_sql("CREATE FUNCTION fail_build() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.resource_id='build-alice' AND NEW.period_id='p-second' THEN RAISE EXCEPTION 'fixture interrupted settlement'; END IF; RETURN NEW; END $$; CREATE TRIGGER fail_build BEFORE INSERT ON api_cloud_billing_usage FOR EACH ROW EXECUTE FUNCTION fail_build()").execute(&pool).await.unwrap();
    let fixture = Fixture {
        start,
        end,
        calls: Arc::new(Mutex::new(Vec::new())),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/token", post(token))
        .route("/boxd.api.v1.BoxdApi/{method}", post(rpc))
        .with_state(fixture.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut provider = Boxd::new("key".into(), Some("fixture-org".into())).unwrap();
    provider.set_test_endpoint(format!("http://{address}"));
    let control = BillingControl::from_pool(
        pool.clone(),
        vec![
            "user-alice".into(),
            "user-bob".into(),
            "manual".into(),
            "cap".into(),
        ],
    );
    let config = BoxdSettlementConfig {
        cutover_at_ms: start,
    };
    let first = control
        .settle_boxd_once(&provider, config, now)
        .await
        .unwrap();
    assert_eq!((first.settled, first.deferred), (4, 1));
    let partial: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM api_cloud_billing_usage WHERE resource_id='build-alice'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(partial, 0);
    sqlx::query("DROP TRIGGER fail_build ON api_cloud_billing_usage")
        .execute(&pool)
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        control.settle_boxd_once(&provider, config, now),
        control.settle_boxd_once(&provider, config, now)
    );
    assert_eq!(a.unwrap().settled + b.unwrap().settled, 1);
    let costs:(i64,i64)=sqlx::query_as("SELECT (SELECT SUM(amount_micros)::bigint FROM api_cloud_billing_ledger WHERE period_id='p-first' AND kind='provider-cost'),(SELECT SUM(amount_cents)::bigint FROM api_cloud_billing_outbox WHERE period_id='p-first')").fetch_one(&pool).await.unwrap();
    assert_eq!(costs, (43_000_000, 840));
    let manual:i64=sqlx::query_scalar("SELECT SUM(amount_micros)::bigint FROM api_cloud_billing_ledger WHERE kind='manual-entitlement-credit'").fetch_one(&pool).await.unwrap();
    assert_eq!(manual, 15_750_000);
    let cap:i64=sqlx::query_scalar("SELECT SUM(amount_micros)::bigint FROM api_cloud_billing_ledger WHERE kind='overshoot-absorbed'").fetch_one(&pool).await.unwrap();
    assert_eq!(cap, 58_250_000);
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_cloud_billing_usage")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 8);
    let reservations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM api_cloud_billing_reservations")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(reservations, 0);
    let telemetry:i64=sqlx::query_scalar("SELECT COUNT(*) FROM api_cloud_usage_outbox WHERE payload->'metadata'->>'billable'='false'").fetch_one(&pool).await.unwrap();
    assert_eq!(telemetry, 8);
    let calls = fixture.calls.lock().unwrap().len();
    let again = control
        .settle_boxd_once(&provider, config, now)
        .await
        .unwrap();
    assert_eq!((again.settled, again.deferred), (0, 0));
    assert_eq!(fixture.calls.lock().unwrap().len(), calls);
    assert!(
        fixture
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(id, a, _)| id == "machine-alice" && *a == start / 1000)
    );
    let original: String = sqlx::query_scalar(
        "SELECT provider_subscription_id FROM api_cloud_billing_periods WHERE period_id='p-first'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(original, "sub-preserved");
    server.abort();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
