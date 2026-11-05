//! One-use, short-lived event upgrade tickets. The user's bearer stays in the
//! authority/host; only an opaque ticket enters the WebSocket subprotocol header.
use crate::error::ApiError;
use axum::http::StatusCode;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

pub struct Pending {
    pub(crate) bearer: Zeroizing<String>,
    pub(crate) actor: String,
    pub(crate) owner: String,
    pub(crate) organization: Option<String>,
    pub(crate) device_id: uuid::Uuid,
    pub(crate) cursor: Option<String>,
    until: Instant,
}
pub struct Tickets {
    pending: Mutex<HashMap<[u8; 32], Pending>>,
    limit: usize,
}
impl Tickets {
    pub fn new(limit: usize) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            limit,
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn issue(
        &self,
        bearer: String,
        actor: String,
        owner: String,
        organization: Option<String>,
        device_id: uuid::Uuid,
        cursor: Option<String>,
    ) -> Result<String, ApiError> {
        if bearer.is_empty()
            || bearer.len() > 16 * 1024
            || cursor.as_ref().is_some_and(|c| c.len() > 2048)
            || [&actor, &owner]
                .iter()
                .any(|s| s.is_empty() || s.len() > 512 || s.chars().any(char::is_control))
        {
            return Err(ApiError::bad_request("invalid_account_event_ticket"));
        }
        let mut pending = self.pending.lock().await;
        let now = Instant::now();
        // A scope switch cannot leave a previous unused ticket for this device.
        pending.retain(|_, p| p.until > now && !(p.actor == actor && p.device_id == device_id));
        if pending.len() >= self.limit {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "account_event_capacity",
            ));
        }
        let ticket = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let hash: [u8; 32] = Sha256::digest(ticket.as_bytes()).into();
        pending.insert(
            hash,
            Pending {
                bearer: Zeroizing::new(bearer),
                actor,
                owner,
                organization,
                device_id,
                cursor,
                until: now + Duration::from_secs(20),
            },
        );
        Ok(ticket)
    }
    pub async fn take(&self, ticket: &str) -> Result<Pending, ApiError> {
        let invalid = || ApiError::unauthorized("account_event_ticket_rejected");
        if ticket.len() != 64 || !ticket.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        let hash: [u8; 32] = Sha256::digest(ticket.as_bytes()).into();
        let value = self
            .pending
            .lock()
            .await
            .remove(&hash)
            .ok_or_else(invalid)?;
        if value.until <= Instant::now() {
            return Err(invalid());
        }
        Ok(value)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn ticket_is_one_use_capacity_bounded_and_invalidates_prior_device_scope() {
        let store = Tickets::new(2);
        let device = uuid::Uuid::new_v4();
        let first = store
            .issue(
                "private-bearer".into(),
                "actor".into(),
                "actor".into(),
                None,
                device,
                None,
            )
            .await
            .unwrap();
        let second = store
            .issue(
                "private-bearer".into(),
                "actor".into(),
                "organization:team".into(),
                Some("team".into()),
                device,
                None,
            )
            .await
            .unwrap();
        assert!(store.take(&first).await.is_err());
        let actual = store.take(&second).await.unwrap();
        assert_eq!(actual.actor, "actor");
        assert_eq!(actual.owner, "organization:team");
        assert_eq!(actual.organization.as_deref(), Some("team"));
        assert_eq!(actual.bearer.as_str(), "private-bearer");
        assert_eq!(actual.device_id, device);
        assert!(actual.cursor.is_none());
        assert!(store.take(&second).await.is_err());
        for _ in 0..2 {
            store
                .issue(
                    "secret".into(),
                    "actor".into(),
                    "actor".into(),
                    None,
                    uuid::Uuid::new_v4(),
                    None,
                )
                .await
                .unwrap();
        }
        let error = store
            .issue(
                "secret".into(),
                "actor".into(),
                "actor".into(),
                None,
                uuid::Uuid::new_v4(),
                None,
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
    }
}
