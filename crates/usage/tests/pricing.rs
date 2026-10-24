use chrono::Utc;
use serde_json::json;
use std::{fs, sync::Arc};
use zuse_usage::{
    CostStatus, Record, TokenCounts,
    aggregate::Selection,
    import::{Format, Importer, Source},
    pricing::Catalogue,
};

fn catalogue(json: &str) -> Catalogue {
    Catalogue::parse(json.as_bytes()).unwrap()
}
fn input(count: u64) -> TokenCounts {
    TokenCounts {
        input_tokens: count,
        ..Default::default()
    }
}
fn close(actual: Option<f64>, expected: f64) {
    assert!(
        (actual.unwrap() - expected).abs() < 1e-9,
        "{actual:?} != {expected}"
    );
}

#[test]
fn resolves_exact_prefix_alias_and_closest_fuzzy_matches_in_source_order() {
    let table = catalogue(
        r#"{
        "anthropic/model":{"input_cost_per_token":2},
        "openai/model":{"input_cost_per_token":3},
        "model":{"input_cost_per_token":1},
        "anthropic/prefix-only":{"input_cost_per_token":4},
        "openai/prefix-only":{"input_cost_per_token":5},
        "gpt-5.4":{"input_cost_per_token":6},
        "gpt-5.4-pro":{"input_cost_per_token":7},
        "xai/grok-code-fast-1":{"input_cost_per_token":8}
    }"#,
    );
    close(table.estimate(" model ", &input(1), false), 1.);
    close(table.estimate("prefix-only", &input(1), false), 4.);
    close(table.estimate("gpt-5.4-mini", &input(1), false), 6.);
    close(table.estimate("GPT-5.4", &input(1), false), 6.);
    close(table.estimate("grok-build", &input(1), false), 8.);
    for missing in ["", "unknown", "unpriced-xyz"] {
        assert_eq!(table.estimate(missing, &input(1), false), None);
    }
    // A JSON object sorted alphabetically would pick a-model instead.
    let tie = catalogue(
        r#"{"z-model":{"input_cost_per_token":9},"a-model":{"input_cost_per_token":10}}"#,
    );
    close(tie.estimate("model", &input(1), false), 9.);
}

#[test]
fn prices_exclusive_token_categories_marginal_tiers_and_fast_mode() {
    let table = catalogue(
        r#"{"model":{
        "input_cost_per_token":0.000001,"output_cost_per_token":0.000002,
        "cache_creation_input_token_cost":0.000003,"cache_read_input_token_cost":0.000004,
        "input_cost_per_token_above_200k_tokens":0.000002,
        "output_cost_per_token_above_200k_tokens":0.000004,
        "cache_creation_input_token_cost_above_200k_tokens":0.000006,
        "cache_read_input_token_cost_above_200k_tokens":0.000008,
        "provider_specific_entry":{"fast":6}
    }}"#,
    );
    let counts = |n| TokenCounts {
        input_tokens: n,
        output_tokens: n,
        cache_creation_tokens: n,
        cache_read_tokens: n,
        reasoning_tokens: 99_000_000,
    };
    close(table.estimate("model", &counts(200_000), false), 2.);
    close(table.estimate("model", &counts(300_000), false), 4.);
    close(table.estimate("model", &counts(300_000), true), 24.);
    let base_only = catalogue(r#"{"model":{"input_cost_per_token":0.000001}}"#);
    close(base_only.estimate("model", &input(300_000), true), 0.3);
    let tier_only = catalogue(r#"{"model":{"input_cost_per_token_above_200k_tokens":0.000002}}"#);
    close(tier_only.estimate("model", &input(300_000), false), 0.2);
}

#[test]
fn ignores_invalid_rates_and_metadata_but_preserves_explicit_free_models() {
    let table = catalogue(
        r#"{
        "metadata":{"max_input_tokens":1000000},
        "broken-rates":{"input_cost_per_token":-1,"output_cost_per_token":"free"},
        "free":{"input_cost_per_token":0,"output_cost_per_token":0},
        "valid":{"input_cost_per_token":0.5,"provider_specific_entry":{"fast":-2}}
    }"#,
    );
    for name in ["metadata", "broken-rates"] {
        assert_eq!(table.estimate(name, &input(1), false), None);
    }
    close(table.estimate("free", &input(1), false), 0.);
    close(table.estimate("valid", &input(1), true), 0.5);
    for invalid in ["{}", "null", "[]", "{", r#"{"m":{"max_input_tokens":1}}"#] {
        assert!(Catalogue::parse(invalid.as_bytes()).is_err());
    }
}

#[test]
fn estimates_reports_without_overriding_logged_costs_or_pricing_unknown_records() {
    let temp = tempfile::tempdir().unwrap();
    let row = |id, model, cost, speed| json!({"timestamp":"2026-10-23T12:00:00Z","sessionId":id,"costUSD":cost,"message":{"id":id,"model":model,"usage":{"input_tokens":100,"output_tokens":10,"speed":speed}}});
    let rows = [
        row("estimated", "priced-model", None, "fast"),
        row("logged", "priced-model", Some(0.25), "fast"),
        row("free", "priced-model", Some(0.), "fast"),
        row("unknown", "missing-xyz", None, "standard"),
    ];
    fs::write(
        temp.path().join("a.jsonl"),
        rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    let mut data = Importer::default().scan(&[Source::new(
        "claude",
        "Claude Code",
        Format::Claude,
        vec![temp.path().into()],
    )]);
    let prices = catalogue(
        r#"{"priced-model":{"input_cost_per_token":0.001,"output_cost_per_token":0.002,"provider_specific_entry":{"fast":6}}}"#,
    );
    prices.apply(&mut data);
    let cost = |id| {
        data.records
            .iter()
            .find(|r| r.session_id.as_deref() == Some(id))
            .unwrap()
            .cost_usd
    };
    close(cost("estimated"), 0.72);
    close(cost("logged"), 0.25);
    close(cost("free"), 0.);
    assert_eq!(cost("unknown"), None);
    let report = data.overview(&Selection::default(), Utc::now()).unwrap();
    assert_eq!(report.summary.cost_status, CostStatus::Partial);
    close(report.summary.cost_usd, 0.97);
    // Older ledger rows remain readable after adding the source fast-mode flag.
    let mut old = serde_json::to_value(&data.records[0]).unwrap();
    old.as_object_mut().unwrap().remove("fast");
    assert!(!serde_json::from_value::<Record>(old).unwrap().fast);
}

#[test]
fn bundled_catalogue_supplies_offline_rates_for_common_models() {
    let table = Catalogue::bundled();
    assert!(Arc::ptr_eq(&table, &Catalogue::bundled()));
    close(table.estimate("gpt-5.4", &input(1_000_000), false), 2.5);
    close(
        table.estimate("claude-opus-4-6", &input(1_000_000), true),
        30.,
    );
    assert_eq!(table.estimate("unpublished-xyz", &input(100), false), None);
}
