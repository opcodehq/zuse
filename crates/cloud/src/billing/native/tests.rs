use super::*;
mod integration;
mod live;

#[test]
fn explicit_schema_matches_pinned_engine() {
    let sql = MIGRATION.split("-- Enrollment").next().unwrap();
    for statement in brokeboy::core::schema::migration_sql(Dialect::Postgres) {
        assert!(
            sql.contains(&format!("{statement};")),
            "Missing engine DDL: {statement}"
        );
    }
}

#[test]
fn unsafe_checkout_links_and_incomplete_operations_never_succeed() {
    for url in [
        "http://stripe.com",
        "https://user:password@stripe.com",
        "javascript:alert(1)",
    ] {
        assert!(secure_url(url).is_err());
    }
    assert!(secure_url("https://checkout.stripe.com/c/pay/example").is_ok());
    for status in ["pending", "processing", "failed", "reconciliation"] {
        assert!(
            commerce::completed(
                &json!({"status":status,"result":{"url":"https://checkout.stripe.com/example"}})
            )
            .is_err()
        );
    }
}
