//! Merchant-authorized Stripe test-mode exercise, excluded from ordinary CI.
use super::*;
use anyhow::{Context, ensure};
use std::{path::PathBuf, time::Duration};

struct StripeApi {
    client: reqwest::Client,
    key: String,
}
impl StripeApi {
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        params: &[(&str, String)],
    ) -> anyhow::Result<Value> {
        let request = self
            .client
            .request(method.clone(), format!("https://api.stripe.com/v1/{path}"))
            .bearer_auth(&self.key)
            .header("Stripe-Version", "2026-09-30.endive");
        let response = if method == reqwest::Method::GET {
            request.query(params)
        } else {
            request.form(params)
        }
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let status = response.status();
        let value: Value = response
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        ensure!(
            status.is_success(),
            "Stripe status {} code {}",
            status,
            value["error"]["code"].as_str().unwrap_or("unspecified")
        );
        Ok(value)
    }
}
async fn wait_marker(path: PathBuf) -> anyhow::Result<()> {
    for _ in 0..120 {
        if path.exists() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    anyhow::bail!("Browser completion marker timed out")
}

#[tokio::test]
#[ignore = "requires merchant-authorized test Stripe credentials and explicit browser completion"]
async fn stripe_merchant_checkout_wallet_refund_and_compute() -> anyhow::Result<()> {
    // Two independent opt-ins prevent this exercise from using normal CI/deploy credentials.
    if std::env::var("ZUSE_STRIPE_LIVE_QUALIFICATION").as_deref() != Ok("test-only") {
        return Ok(());
    }
    let key = std::fs::read_to_string(std::env::var("ZUSE_STRIPE_TEST_KEY_FILE")?)?
        .trim()
        .to_owned();
    ensure!(
        key.starts_with("sk_test_"),
        "Only Stripe test keys are accepted"
    );
    let directory = PathBuf::from(std::env::var("ZUSE_STRIPE_EVIDENCE_DIR")?);
    std::fs::create_dir_all(&directory)?;
    let api = StripeApi {
        client: reqwest::Client::new(),
        key: key.clone(),
    };
    ensure!(
        api.call(reqwest::Method::GET, "balance", &[])
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?["livemode"]
            == false,
        "Test account required"
    );
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL")?;
    ensure!(
        matches!(
            reqwest::Url::parse(&url)?.host_str(),
            Some("localhost" | "127.0.0.1")
        ),
        "Local disposable database required"
    );
    let admin = PgPool::connect(&url)
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let schema = format!("merchant_billing_{suffix}");
    let owner = format!("merchant-{suffix}");
    sqlx::raw_sql(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let mut scoped = reqwest::Url::parse(&url)?;
    scoped
        .query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema}"));
    let pool = PgPool::connect(scoped.as_str())
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    for sql in [
        include_str!("../../../../migrations/0000_application.sql"),
        include_str!("../../../../migrations/0001_control_plane.sql"),
        MIGRATION,
    ] {
        sqlx::raw_sql(sql)
            .execute(&pool)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    }
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('billing',$1,'rust',1,0)")
        .bind(&owner)
        .execute(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let product = api
        .call(
            reqwest::Method::POST,
            "products",
            &[("name", format!("Zuse qualification {suffix}"))],
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let product_id = field(&product, "id")?.to_owned();
    let price = api
        .call(
            reqwest::Method::POST,
            "prices",
            &[
                ("product", product_id.clone()),
                ("currency", "usd".into()),
                ("unit_amount", "4000".into()),
                ("recurring[interval]", "month".into()),
            ],
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let price_id = field(&price, "id")?.to_owned();
    let provider: Arc<dyn PaymentProvider> = Arc::new(Providers::new(
        "stripe".into(),
        vec![Arc::new(brokeboy::stripe::Stripe::new(
            brokeboy::stripe::StripeOptions::new(key, "whsec_fixture".into()),
        )?)],
    )?);
    let engine = Billing::open(
        DatabaseConnection {
            driver: Dialect::Postgres,
            url: scoped.to_string(),
            max_connections: Some(4),
            acquire_timeout_ms: Some(5000),
        },
        BillingOptions {
            provider: Some(provider.clone()),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let native = NativeBilling {
        engine: engine.clone(),
        provider,
        accounts: HashSet::from([owner.clone()]),
        default_provider: "stripe".into(),
        return_origin: "https://example.com".into(),
        automatic_tax: false,
        signer: Some(super::integration::signer()),
        stripe_cloud_price: Some(price_id.clone()),
        stripe_overage_price: None,
        compute_metered: false,
    };
    let control =
        BillingControl::from_pool(pool.clone(), vec![owner.clone()]).with_native_billing(native);
    let mut customer_id = None;
    let outcome: anyhow::Result<()> = async {
        control.publish_native_catalog(json!({"features":[{"id":MODEL_WALLET,"kind":"consumable"}],"plans":[{"id":CLOUD_PLAN,"name":"Zuse qualification","kind":"subscription","currency":"usd","amount":"40","interval":"month","stripePriceId":price_id,"items":[]}]}),0).await.map_err(|e|anyhow::anyhow!("{e:?}"))?;
        control.enroll_native(&owner,false,chrono::Utc::now().timestamp_millis()).await.map_err(|e|anyhow::anyhow!("{e:?}"))?;
        let checkout=control.native_checkout(&owner,CLOUD_PLAN,chrono::Utc::now().timestamp_millis(),Some("merchant-subscription")).await.map_err(|e|anyhow::anyhow!("{e:?}"))?.context("checkout URL")?;
        let prepaid=control.native_prepaid_checkout(&owner,1000,Some("merchant-wallet")).await.map_err(|e|anyhow::anyhow!("{e:?}"))?.context("wallet URL")?;
        let customer=engine.call("customers.get",json!({"customerId":owner})).await.map_err(|e|anyhow::anyhow!("{e:?}"))?;
        let id=field(&customer,"stripeCustomerId")?.to_owned();customer_id=Some(id.clone());
        std::fs::write(directory.join("checkouts.json"),serde_json::to_vec_pretty(&json!({"subscription":checkout,"prepaid":prepaid,"customer":id,"price":price_id,"product":product_id,"schema":schema}))?)?;
        eprintln!("Merchant checkout URLs ready in evidence directory");
        wait_marker(directory.join("browser-completed")).await.map_err(|e|anyhow::anyhow!("{e:?}"))?;
        let events=api.call(reqwest::Method::GET,"events",&[("limit","100".into())]).await.map_err(|e|anyhow::anyhow!("{e:?}"))?;
        let store=crate::store::Store::from_pool(pool.clone()).with_billing_control(control.clone());
        let mut delivered=vec![];
        for event in events["data"].as_array().context("events")?.iter().rev() {
            if event["data"]["object"]["customer"]!=id || !matches!(event["type"].as_str(),Some("checkout.session.completed"|"checkout.session.async_payment_succeeded"|"customer.subscription.created"|"customer.subscription.updated"|"invoice.paid")) {continue;}
            let (headers,body)=super::integration::signed(event);
            store.billing_webhook(Some("stripe"),&headers,&body,chrono::Utc::now().timestamp_millis()).await.map_err(|e|anyhow::anyhow!("{e:?}"))?;
            delivered.push(event["id"].clone());
        }
        control.native_maintenance().await.map_err(|e|anyhow::anyhow!("{e:?}"))?;
        ensure!(store.prepaid_balance(&owner).await.map_err(|e|anyhow::anyhow!("{e:?}"))?.credit_cents==1000,"Wallet should have exactly $10 after paid checkout");
        let summary=store.billing_summary(&owner,chrono::Utc::now().timestamp_millis()).await.map_err(|e|anyhow::anyhow!("{e:?}"))?;
        ensure!(summary.base_price_micros==40_000_000,"Compute subscription projection");
        let sessions=api.call(reqwest::Method::GET,"checkout/sessions",&[("customer",id.clone()),("limit","10".into())]).await.map_err(|e|anyhow::anyhow!("{e:?}"))?;
        let paid=sessions["data"].as_array().context("sessions")?.iter().find(|s|s["mode"]=="payment").context("prepaid session")?;
        ensure!(paid["payment_status"]=="paid","Actual Stripe payment required");
        let intent=field(paid,"payment_intent")?;
        let refund=api.call(reqwest::Method::POST,"refunds",&[("payment_intent",intent.into()),("amount","500".into())]).await.map_err(|e|anyhow::anyhow!("{e:?}"))?;
        ensure!(refund["status"]=="succeeded","Actual refund must succeed");
        tokio::time::sleep(Duration::from_secs(2)).await;
        let events=api.call(reqwest::Method::GET,"events",&[("type","charge.refunded".into()),("limit","20".into())]).await.map_err(|e|anyhow::anyhow!("{e:?}"))?;
        let event=events["data"].as_array().context("refund events")?.iter().find(|e|e["data"]["object"]["customer"]==id).context("real refund event")?;
        let (headers,body)=super::integration::signed(event);
        for _ in 0..2 {store.billing_webhook(Some("stripe"),&headers,&body,chrono::Utc::now().timestamp_millis()).await.map_err(|e|anyhow::anyhow!("{e:?}"))?;control.native_maintenance().await.map_err(|e|anyhow::anyhow!("{e:?}"))?;}
        ensure!(store.prepaid_balance(&owner).await.map_err(|e|anyhow::anyhow!("{e:?}"))?.credit_cents==500,"Partial refund must claw back exactly $5 once");
        let request=ModelReservation{request_id:"merchant-model".into(),chat_id:"qualification-chat".into(),model:"qualification-model".into(),estimate_micros:1_000_000};
        store.reserve_hosted_model(&owner,&request,chrono::Utc::now().timestamp_millis()).await.map_err(|e|anyhow::anyhow!("{e:?}"))?;
        for _ in 0..2 {store.settle_hosted_model(&owner,&request.request_id,250_000).await.map_err(|e|anyhow::anyhow!("{e:?}"))?;}
        ensure!(store.prepaid_balance(&owner).await.map_err(|e|anyhow::anyhow!("{e:?}"))?.credit_cents==475,"Settlement replay must charge $0.25 once");
        std::fs::write(directory.join("result.json"),serde_json::to_vec_pretty(&json!({"passed":true,"customer":id,"events":delivered,"refund":refund["id"],"walletAfterPurchaseCents":1000,"walletAfterRefundCents":500,"walletAfterUsageCents":475,"computeBaseMicros":summary.base_price_micros,"delivery":"Retrieved real Stripe events signed locally; public delivery not qualified"}))?)?;
        Ok(())
    }.await;
    // Delete only resources created by this exercise; never inspect/change other customers.
    if let Some(id) = customer_id {
        let result = api
            .call(reqwest::Method::DELETE, &format!("customers/{id}"), &[])
            .await;
        ensure!(result.is_ok(), "Fixture customer cleanup failed");
    }
    api.call(
        reqwest::Method::POST,
        &format!("prices/{price_id}"),
        &[("active", "false".into())],
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    api.call(
        reqwest::Method::POST,
        &format!("products/{product_id}"),
        &[("active", "false".into())],
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    control
        .close_native_billing()
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    admin.close().await;
    outcome
}

fn field<'a>(value: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    value[key].as_str().context("Missing Stripe field")
}
