//! Exact money conversion shared by the billing controls on every client.
pub const MAX_SAFE_MICROS: i64 = 9_007_199_254_740_991;
pub fn cap_micros(input: &str) -> Option<i64> {
    let mut parts = input.trim().split('.');
    let whole = parts.next()?;
    let fraction = parts.next().unwrap_or("");
    if parts.next().is_some()
        || whole.is_empty()
        || fraction.len() > 6
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let whole = whole.parse::<i64>().ok()?.checked_mul(1_000_000)?;
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<i64>()
            .ok()?
            .checked_mul(10_i64.pow(6 - fraction.len() as u32))?
    };
    whole
        .checked_add(fraction)
        .filter(|n| *n <= MAX_SAFE_MICROS)
}
pub fn cap_dollars(micros: i64) -> String {
    let whole = micros / 1_000_000;
    let fraction = micros % 1_000_000;
    if fraction == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{:06}", fraction)
            .trim_end_matches('0')
            .into()
    }
}
pub fn usd(micros: i64) -> String {
    let value = i128::from(micros);
    let cents = (value.abs() + 5000) / 10000;
    format!(
        "{}${}.{:02}",
        if value < 0 { "−" } else { "" },
        cents / 100,
        cents % 100
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monthly_cap_uses_exact_decimal_micros() {
        assert_eq!(cap_micros("25.01"), Some(25_010_000));
        assert_eq!(cap_micros("9007199254.740991"), Some(MAX_SAFE_MICROS));
        for invalid in [
            "-1",
            "1e3",
            "NaN",
            "1.0000001",
            "9007199254.740992",
            "999999999999999999999999",
        ] {
            assert_eq!(cap_micros(invalid), None, "{invalid}");
        }
        for n in [0, 1, 25_010_000, MAX_SAFE_MICROS] {
            assert_eq!(cap_micros(&cap_dollars(n)), Some(n));
        }
        assert_eq!(usd(1_005_000), "$1.01");
    }
}

/// Catalog presentation only; checkout independently reconciles canonical provider state.
pub fn cloud_subscribed(value: &serde_json::Value, now: i64) -> Option<bool> {
    Some(value["entitlements"].as_array()?.iter().any(|ent| {
        ent["offerId"] == "cloud-workspace-standard-v1"
            && matches!(ent["status"].as_str(), Some("pending" | "active" | "grace"))
            && ent["paidThrough"].as_i64().is_none_or(|end| end > now)
    }))
}

pub fn cents(value: i64) -> String {
    let amount = value.unsigned_abs();
    format!(
        "{}${}.{:02}",
        if value < 0 { "-" } else { "" },
        amount / 100,
        amount % 100
    )
}
