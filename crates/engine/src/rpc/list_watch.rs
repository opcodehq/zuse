//! Opt-in keyed list updates: keep all rows, send only changed ones after opening.
use futures::{StreamExt, stream::BoxStream};
use serde::Serialize;
use std::collections::HashMap;
use tokio::sync::watch;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListFrame<T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    reset: Option<Vec<T>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    upsert: Vec<T>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    remove: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    order: Option<Vec<String>>,
    count: usize,
}

pub(super) fn stream<T>(
    rx: watch::Receiver<Vec<T>>,
    key: fn(&T) -> &str,
) -> BoxStream<'static, zuse_rpc::RpcStreamItem>
where
    T: Serialize + Clone + PartialEq + Send + Sync + 'static,
{
    futures::stream::unfold(
        (rx, None::<Vec<T>>),
        move |(mut rx, mut previous)| async move {
            loop {
                if previous.is_some() {
                    rx.changed().await.ok()?;
                }
                let current = rx.borrow_and_update().clone();
                let count = current.len();
                let frame = if let Some(previous) = &previous {
                    let by_id: HashMap<_, _> = previous.iter().map(|row| (key(row), row)).collect();
                    let now: HashMap<_, _> = current.iter().map(|row| (key(row), row)).collect();
                    let upsert = current
                        .iter()
                        .filter(|row| by_id.get(key(row)).copied() != Some(*row))
                        .cloned()
                        .collect::<Vec<_>>();
                    let remove = previous
                        .iter()
                        .filter(|row| !now.contains_key(key(row)))
                        .map(|row| key(row).to_owned())
                        .collect::<Vec<_>>();
                    let order = (!previous.iter().map(key).eq(current.iter().map(key)))
                        .then(|| current.iter().map(|row| key(row).to_owned()).collect());
                    if upsert.is_empty() && remove.is_empty() && order.is_none() {
                        continue;
                    }
                    ListFrame {
                        reset: None,
                        upsert,
                        remove,
                        order,
                        count,
                    }
                } else {
                    ListFrame {
                        reset: Some(current.clone()),
                        upsert: Vec::new(),
                        remove: Vec::new(),
                        order: None,
                        count,
                    }
                };
                previous = Some(current);
                return Some((zuse_rpc::RpcStreamItem::new(frame), (rx, previous)));
            }
        },
    )
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    #[tokio::test]
    async fn one_changed_session_does_not_resend_a_thousand_unchanged_rows() {
        let rows=(0..1000).map(|i|json!({"chatId":format!("chat-{i}"),"detail":"retained content ".repeat(40),"status":"idle"})).collect::<Vec<_>>();
        let (tx, rx) = watch::channel(rows.clone());
        let mut stream = stream(rx, |row: &Value| row["chatId"].as_str().unwrap());
        let first = stream.next().await.unwrap().encode(7).unwrap();
        let mut current = Vec::new();
        zuse_protocol::list_delta::apply(
            &mut current,
            &serde_json::from_str::<Value>(&first).unwrap()["item"],
            "chatId",
        )
        .unwrap();
        let mut changed = rows;
        changed[500]["status"] = json!("running");
        tx.send_replace(changed.clone());
        let update = stream.next().await.unwrap().encode(7).unwrap();
        assert!(update.len() * 500 < first.len());
        let frame = serde_json::from_str::<Value>(&update).unwrap();
        assert_eq!(frame["item"]["upsert"].as_array().unwrap().len(), 1);
        zuse_protocol::list_delta::apply(&mut current, &frame["item"], "chatId").unwrap();
        assert_eq!(current, changed);
        changed.remove(200);
        changed.reverse();
        tx.send_replace(changed.clone());
        let update = stream.next().await.unwrap().encode(7).unwrap();
        zuse_protocol::list_delta::apply(
            &mut current,
            &serde_json::from_str::<Value>(&update).unwrap()["item"],
            "chatId",
        )
        .unwrap();
        assert_eq!(current, changed);
        drop(stream);
        assert!(tx.is_closed());
    }
}
