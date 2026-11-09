//! Keep verified, contiguous older ranges when a bounded latest page is refreshed.
use crate::{TranscriptBaseline, TranscriptFrame, cloud::Head};

pub fn merge_loaded_prefix(prior: &Head, next: &mut Head) -> bool {
    if prior.workspace_id != next.workspace_id
        || prior.session_id != next.session_id
        || prior.runtime_generation != next.runtime_generation
        || prior.schema_version != next.schema_version
        || prior.payload_format != next.payload_format
        || prior.cursor.epoch.is_empty()
        || prior.cursor.epoch != next.cursor.epoch
        || next.cursor.version < prior.cursor.version
    {
        return false;
    }
    let (TranscriptFrame::Reset { reset: old }, TranscriptFrame::Reset { reset: incoming }) =
        (&prior.transcript.frame, &next.transcript.frame)
    else {
        return false;
    };
    let Some(first) = incoming.first() else {
        return false;
    };
    let Some(prefix) = old.iter().position(|entry| entry.id == first.id) else {
        return false;
    };
    let before = prior.older_message_sequence.unwrap_or(0);
    if before < 0
        || before.checked_add(prefix as i64) != Some(next.older_message_sequence.unwrap_or(0))
        || incoming.len() < old.len() - prefix
        || !old[prefix..]
            .iter()
            .zip(incoming.iter())
            .all(|(old, new)| old.id == new.id)
    {
        return false;
    }
    let mut ids = std::collections::HashSet::new();
    if !old[..prefix]
        .iter()
        .chain(incoming.iter())
        .all(|entry| ids.insert(&entry.id))
    {
        return false;
    }
    if prefix > 0 {
        let mut entries = old[..prefix].to_vec();
        entries.extend(incoming.iter().cloned());
        next.transcript.replay_baseline = Some(TranscriptBaseline::capture(&entries));
        next.transcript.frame = TranscriptFrame::Reset {
            reset: entries.into(),
        };
        next.older_message_sequence = prior.older_message_sequence;
    }
    true
}
