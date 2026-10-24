//! Quoted command fields are tokenized as data; saving never expands a shell expression.
use super::CustomAcpAgent;
use std::collections::BTreeMap;

pub fn words(line: &str) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    let mut current: Option<String> = None;
    let mut quote = None;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if quote == Some('\'') {
            if ch == '\'' {
                quote = None;
            } else {
                current.get_or_insert_default().push(ch);
            }
        } else if ch == '\\' && chars.peek().is_some() {
            current.get_or_insert_default().push(chars.next().unwrap());
        } else if quote == Some('"') {
            if ch == '"' {
                quote = None;
            } else {
                current.get_or_insert_default().push(ch);
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            current.get_or_insert_default();
        } else if matches!(ch, '\t'..='\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
        {
            if let Some(value) = current.take() {
                result.push(value);
            }
        } else {
            current.get_or_insert_default().push(ch);
        }
    }
    if quote.is_some() {
        return Err("Close the quoted value before saving.".into());
    }
    if let Some(value) = current {
        result.push(value);
    }
    Ok(result)
}

pub fn arguments(values: &[String]) -> String {
    values
        .iter()
        .map(|value| {
            if !value.is_empty()
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&c))
            {
                value.clone()
            } else {
                format!("'{}'", value.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A blank environment field preserves saved values unless Clear was selected.
pub fn definition(
    fields: &[String; 6],
    preserved: Option<&BTreeMap<String, String>>,
) -> Result<CustomAcpAgent, String> {
    let pairs = words(&fields[4])?;
    let mut env = BTreeMap::new();
    for pair in &pairs {
        let (key, value) = pair
            .split_once('=')
            .ok_or("Environment entries need NAME=value.")?;
        if env.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(format!("Duplicate environment variable: {key}"));
        }
    }
    if pairs.is_empty() {
        env = preserved.cloned().unwrap_or_default();
    }
    let agent = CustomAcpAgent {
        id: crate::HarnessId::try_from(fields[0].trim().to_owned())?,
        name: fields[1].trim().to_owned(),
        command: fields[2].trim().to_owned(),
        args: words(&fields[3])?,
        env,
        model_ids: fields[5]
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
    };
    agent.validate()?;
    Ok(agent)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoted_arguments_round_trip_without_expansion() {
        let args = [
            "--acp",
            "",
            "two words",
            "a'b",
            "line\nbreak",
            "literal ; $(no-shell)",
            "back\\slash",
            "α β",
        ]
        .map(str::to_owned);
        assert_eq!(words(&arguments(&args)).unwrap(), args);
        assert_eq!(
            words("one\\ two \"three four\" '' trailing\\").unwrap(),
            ["one two", "three four", "", "trailing\\"]
        );
        assert!(words("'unclosed").is_err());
    }
    #[test]
    fn environment_preservation_replacement_and_clear_are_distinct() {
        let mut fields =
            ["acp-test", "Test", "python3", "--acp", "", "fallback/model"].map(str::to_owned);
        let saved = BTreeMap::from([("TOKEN".into(), "old secret".into())]);
        assert_eq!(definition(&fields, Some(&saved)).unwrap().env, saved);
        assert!(definition(&fields, None).unwrap().env.is_empty());
        fields[4] = "TOKEN='new value' EMPTY=''".into();
        assert_eq!(
            definition(&fields, Some(&saved)).unwrap().env,
            BTreeMap::from([
                ("TOKEN".into(), "new value".into()),
                ("EMPTY".into(), "".into())
            ])
        );
        fields[4] = "TOKEN=one TOKEN=two".into();
        assert!(definition(&fields, None).is_err());
        fields[4] = "1INVALID=value".into();
        assert!(definition(&fields, None).is_err());
    }
}
