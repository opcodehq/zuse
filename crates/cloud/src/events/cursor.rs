//! Opaque replay positions are bound to the authenticated actor, selected scope
//! and API issuer. Organization-private event counts are not exposed as numbers.
use crate::{crypto::MasterKey, error::ApiError};
use serde::{Deserialize, Serialize};

const MAX_SEQUENCE: i64 = 9_007_199_254_740_991;
const LIFETIME: i64 = 7 * 24 * 60 * 60_000;
pub struct Cursors {
    key: MasterKey,
    issuer: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Position {
    version: u32,
    sequence: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    personal_sequence: Option<i64>,
    issued_at: i64,
    expires_at: i64,
}
impl Cursors {
    pub fn new(key: MasterKey, issuer: String) -> Self {
        Self { key, issuer }
    }
    fn context(&self, actor: &str, owner: &str) -> Result<Vec<u8>, ApiError> {
        if [actor, owner]
            .iter()
            .any(|s| s.is_empty() || s.len() > 512 || s.chars().any(char::is_control))
        {
            return Err(ApiError::bad_request("invalid_account_event_scope"));
        }
        Ok(format!(
            "zuse-account-event-cursor-v1\n{}\n{actor}\n{owner}",
            self.issuer
        )
        .into_bytes())
    }
    pub fn encode(
        &self,
        actor: &str,
        owner: &str,
        sequence: i64,
        now: i64,
    ) -> Result<String, ApiError> {
        self.encode_pair(actor, owner, sequence, None, now)
    }
    pub fn encode_pair(
        &self,
        actor: &str,
        owner: &str,
        sequence: i64,
        personal_sequence: Option<i64>,
        now: i64,
    ) -> Result<String, ApiError> {
        if !(0..=MAX_SEQUENCE).contains(&sequence)
            || personal_sequence.is_some_and(|n| !(0..=MAX_SEQUENCE).contains(&n))
            || (actor == owner && personal_sequence.is_some())
            || now < 0
            || now.checked_add(LIFETIME).is_none()
        {
            return Err(ApiError::bad_request("invalid_account_event_cursor"));
        }
        let payload = serde_json::to_vec(&Position {
            version: 2,
            sequence,
            personal_sequence,
            issued_at: now,
            expires_at: now + LIFETIME,
        })
        .map_err(|_| ApiError::unavailable("account_event_cursor_unavailable"))?;
        self.key
            .seal(&self.context(actor, owner)?, &payload)
            .map_err(|_| ApiError::unavailable("account_event_cursor_unavailable"))
    }
    pub fn decode(
        &self,
        actor: &str,
        owner: &str,
        cursor: &str,
        now: i64,
    ) -> Result<i64, ApiError> {
        self.decode_pair(actor, owner, cursor, now)
            .map(|position| position.0)
    }
    pub fn decode_pair(
        &self,
        actor: &str,
        owner: &str,
        cursor: &str,
        now: i64,
    ) -> Result<(i64, Option<i64>), ApiError> {
        let invalid = || ApiError::conflict("account_event_cursor_resync_required");
        if cursor.is_empty() || cursor.len() > 2048 {
            return Err(invalid());
        }
        let payload = self
            .key
            .open(&self.context(actor, owner)?, cursor)
            .map_err(|_| invalid())?;
        let position: Position = serde_json::from_slice(&payload).map_err(|_| invalid())?;
        if ![1, 2].contains(&position.version)
            || position
                .personal_sequence
                .is_some_and(|n| !(0..=MAX_SEQUENCE).contains(&n))
            || (actor == owner && position.personal_sequence.is_some())
            || !(0..=MAX_SEQUENCE).contains(&position.sequence)
            || position.issued_at > now + 60_000
            || position.expires_at <= now
            || position.expires_at.checked_sub(position.issued_at) != Some(LIFETIME)
        {
            return Err(invalid());
        }
        Ok((position.sequence, position.personal_sequence))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    fn cursors(issuer: &str) -> Cursors {
        Cursors::new(
            MasterKey::parse(&URL_SAFE_NO_PAD.encode([71; 32])).unwrap(),
            issuer.into(),
        )
    }
    #[test]
    fn replay_cursor_is_private_and_bound_to_actor_scope_issuer_and_expiry() {
        let authority = cursors("https://staging.example.test");
        let pair = authority
            .encode_pair("actor-a", "organization:team", 42, Some(8), 1000)
            .unwrap();
        assert_eq!(
            authority
                .decode_pair("actor-a", "organization:team", &pair, 2000)
                .unwrap(),
            (42, Some(8))
        );
        assert!(
            authority
                .decode_pair("actor-b", "organization:team", &pair, 2000)
                .is_err()
        );
        assert!(
            authority
                .encode_pair("actor-a", "actor-a", 42, Some(8), 1000)
                .is_err()
        );
        let cursor = authority
            .encode("actor-a", "organization:team", 42, 1000)
            .unwrap();
        assert_eq!(
            authority
                .decode("actor-a", "organization:team", &cursor, 2000)
                .unwrap(),
            42
        );
        assert!(
            authority
                .decode("actor-b", "organization:team", &cursor, 2000)
                .is_err()
        );
        assert!(
            authority
                .decode("actor-a", "organization:other", &cursor, 2000)
                .is_err()
        );
        assert!(
            cursors("https://production.example.test")
                .decode("actor-a", "organization:team", &cursor, 2000)
                .is_err()
        );
        assert!(
            authority
                .decode("actor-a", "organization:team", &cursor, 1000 + LIFETIME)
                .is_err()
        );
        let mut modified = cursor.into_bytes();
        let at = modified.len() / 2;
        modified[at] = if modified[at] == b'A' { b'B' } else { b'A' };
        assert!(
            authority
                .decode(
                    "actor-a",
                    "organization:team",
                    std::str::from_utf8(&modified).unwrap(),
                    2000
                )
                .is_err()
        );
        assert!(
            authority
                .encode("actor-a", "organization:team", MAX_SEQUENCE + 1, 1000)
                .is_err()
        );
    }
}
