//! English display rules shared by the overview, axes, tooltip and tables.
pub(super) fn tokens(value: u64) -> String {
    let units = [
        (1_000_u64, "K"),
        (1_000_000, "M"),
        (1_000_000_000, "B"),
        (1_000_000_000_000, "T"),
    ];
    let Some(mut unit) = units.iter().rposition(|(base, _)| value >= *base) else {
        return value.to_string();
    };
    let rounded =
        |unit: usize| (value as u128 * 10 + units[unit].0 as u128 / 2) / units[unit].0 as u128;
    if rounded(unit) >= 10_000 && unit + 1 < units.len() {
        unit += 1;
    }
    let tenths = rounded(unit);
    let whole = tenths / 10;
    let whole = if whole >= 10_000 {
        grouped(&whole.to_string())
    } else {
        whole.to_string()
    };
    format!(
        "{whole}{}{}",
        if tenths % 10 == 0 {
            String::new()
        } else {
            format!(".{}", tenths % 10)
        },
        units[unit].1
    )
}
fn grouped(digits: &str) -> String {
    let mut result = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            result.push(',');
        }
        result.push(digit);
    }
    result
}
pub(super) fn share(value: f64) -> String {
    // toFixed rounds the exactly representable .25 tie upward; Rust's formatter uses even ties.
    let value = if value.fract() == 0.25 {
        value + 0.05
    } else {
        value
    };
    format!("{value:.1}%")
}
pub(super) fn money(value: Option<f64>) -> String {
    let Some(value) = value.filter(|value| value.is_finite()) else {
        return "N/A".into();
    };
    // Round the shortest decimal representation, as Intl does. Binary formatting
    // rounds exact halves such as 0.125 differently from the reference's currency formatter.
    let decimal = value.abs().to_string();
    let (whole, fraction) = decimal.split_once('.').unwrap_or((&decimal, ""));
    let digits = fraction.as_bytes();
    let mut cents = (digits.first().copied().unwrap_or(b'0') - b'0') as u16 * 10
        + (digits.get(1).copied().unwrap_or(b'0') - b'0') as u16;
    if digits.get(2).is_some_and(|digit| *digit >= b'5') {
        cents += 1;
    }
    let mut whole = whole.as_bytes().to_vec();
    if cents == 100 {
        cents = 0;
        let mut carry = true;
        for digit in whole.iter_mut().rev() {
            if *digit == b'9' {
                *digit = b'0';
            } else {
                *digit += 1;
                carry = false;
                break;
            }
        }
        if carry {
            whole.insert(0, b'1');
        }
    }
    format!(
        "{}${}.{:02}",
        if value.is_sign_negative() { "-" } else { "" },
        grouped(std::str::from_utf8(&whole).unwrap()),
        cents
    )
}
