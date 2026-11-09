//! Shared contiguous range validation; existing client regressions stay here.
pub(super) use zuse_model::cloud::merge_loaded_prefix;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::prelude::v1::test;
    use zuse_model::cloud::Head;

    fn head(ids: &[&str], before: i64) -> Head {
        serde_json::from_value(json!({"schemaVersion":1,"payloadFormat":"zuse-cloud-admission-v1","workspaceId":"workspace","sessionId":"session","runtimeGeneration":1,"cursor":{"epoch":"epoch","version":1},"chat":{"id":"session","deviceId":"runtime","archived":false,"createdAt":"2026-01-01T00:00:00Z"},"conversations":[],"transcript":{"reset":ids.iter().map(|id|json!({"id":id,"role":"assistant","parts":[{"id":id,"kind":"text","text":id}],"createdAt":1,"deviceId":"runtime"})).collect::<Vec<_>>()},"queue":[],"queuePaused":false,"olderMessageSequence":before})).unwrap()
    }

    #[test]
    fn latest_page_merge_requires_same_projection_and_contiguous_overlap() {
        let prior = head(&["older", "latest"], 50);
        let next = head(&["latest", "new"], 51);
        let mut valid = next.clone();
        assert!(merge_loaded_prefix(&prior, &mut valid));
        assert_eq!(valid.older_message_sequence, Some(50));
        for field in [
            "workspace",
            "session",
            "generation",
            "epoch",
            "version",
            "gap",
            "reordered",
            "duplicate",
        ] {
            let mut invalid = next.clone();
            match field {
                "workspace" => invalid.workspace_id = "other".into(),
                "session" => invalid.session_id = "other".into(),
                "generation" => invalid.runtime_generation += 1,
                "epoch" => invalid.cursor.epoch = "replacement".into(),
                "version" => invalid.cursor.version = 0,
                "gap" => invalid.older_message_sequence = Some(52),
                "reordered" => invalid = head(&["new", "latest"], 51),
                "duplicate" => invalid = head(&["latest", "latest"], 51),
                _ => unreachable!(),
            }
            assert!(!merge_loaded_prefix(&prior, &mut invalid), "{field}");
        }
    }
}
