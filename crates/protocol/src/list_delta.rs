//! Keyed presentation lists. Legacy arrays remain accepted during mixed upgrades.
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub fn apply(current: &mut Vec<Value>, frame: &Value, key: &str) -> Result<(), &'static str> {
    if let Some(rows) = frame
        .as_array()
        .or_else(|| frame.get("reset").and_then(Value::as_array))
    {
        *current = rows.clone();
        return Ok(());
    }
    let upsert = frame
        .get("upsert")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let remove = frame
        .get("remove")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let id = |row: &Value| {
        row.get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or("Missing list row id")
    };
    let mut ids = current.iter().map(id).collect::<Result<Vec<_>, _>>()?;
    let removed = remove
        .iter()
        .map(|v| v.as_str().ok_or("Invalid removed id"))
        .collect::<Result<HashSet<_>, _>>()?;
    ids.retain(|id| !removed.contains(id.as_str()));
    let mut seen: HashSet<String> = ids.iter().cloned().collect();
    if seen.len() != ids.len() {
        return Err("Duplicate existing list id");
    }
    let mut updates = HashSet::new();
    for row in upsert {
        let key = id(row)?;
        if !updates.insert(key.clone()) {
            return Err("Duplicate updated list id");
        }
        if seen.insert(key.clone()) {
            ids.push(key);
        }
    }
    if let Some(order) = frame.get("order").and_then(Value::as_array) {
        let order = order
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or("Invalid ordered id"))
            .collect::<Result<Vec<_>, _>>()?;
        if order.len() != ids.len()
            || order.iter().collect::<HashSet<_>>().len() != order.len()
            || order.iter().any(|id| !seen.contains(id))
        {
            return Err("List order mismatch");
        }
        ids = order;
    }
    if frame.get("count").and_then(Value::as_u64) != Some(ids.len() as u64) {
        return Err("List count mismatch");
    }
    // Validation finishes before mutation. Move existing rows instead of deep
    // copying every sidebar row on every status update.
    let mut rows: HashMap<_, _> = std::mem::take(current)
        .into_iter()
        .map(|row| (row[key].as_str().unwrap().to_owned(), row))
        .collect();
    for row in upsert {
        rows.insert(id(row)?, row.clone());
    }
    *current = ids
        .into_iter()
        .map(|id| rows.remove(&id).unwrap())
        .collect();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn updates_order_and_removal_preserve_unrelated_rows_and_reject_desync_atomically() {
        let mut rows = vec![json!({"id":"a","text":"keep"}), json!({"id":"b"})];
        apply(&mut rows,&json!({"upsert":[{"id":"c"},{"id":"b","status":"running"}],"order":["c","a","b"],"count":3}),"id").unwrap();
        assert_eq!(rows[1], json!({"id":"a","text":"keep"}));
        apply(&mut rows, &json!({"remove":["b"],"count":2}), "id").unwrap();
        let previous = rows.clone();
        assert!(apply(&mut rows, &json!({"order":["c","c"],"count":2}), "id").is_err());
        assert_eq!(rows, previous);
        assert!(apply(&mut rows, &json!({"count":500}), "id").is_err());
        assert_eq!(rows, previous);
        apply(&mut rows, &json!([{"id":"legacy"}]), "id").unwrap();
        assert_eq!(rows.len(), 1);
    }
}
