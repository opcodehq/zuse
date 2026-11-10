//! Internal staging-pilot diagnostics. Never a customer or model-selected capability.
use super::*;
fn requested(environment: Option<&str>, pilot_owner: Option<&str>, row: &Value) -> bool {
    environment == Some("staging")
        && pilot_owner.is_some_and(|owner| {
            !owner.is_empty()
                && owner.len() <= 256
                && owner
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b':' | b'-'))
                && row["account_id"].as_str() == Some(owner)
        })
        && row["request_config"]["agent"] == "codex"
        && row["request_config"]["managedBaseReadiness"] == "uncertified-staging"
        && row["request_config"]["managedCandidateSha256"]
            .as_str()
            .is_some_and(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
}
pub(super) fn same_binding(before: &Value, after: &Value) -> bool {
    before["account_id"] == after["account_id"]
        && before["provider"] == after["provider"]
        && [
            "agent",
            "rustActorId",
            "managedReleaseId",
            "managedCandidateSha256",
            "managedCandidateArtifactId",
            "runtimeGeneration",
        ]
        .iter()
        .all(|key| before["request_config"][key] == after["request_config"][key])
}
impl RuntimeControl {
    pub(super) async fn internal_codex_tool_observations(
        &self,
        row: &Value,
        environment: Option<&str>,
        pilot_owner: Option<&str>,
        now: i64,
    ) -> bool {
        self.internal_codex_tool_observations_on(row, environment, pilot_owner, now, None)
            .await
    }
    pub(super) async fn internal_codex_tool_observations_on(
        &self,
        row: &Value,
        environment: Option<&str>,
        pilot_owner: Option<&str>,
        now: i64,
        connection: Option<&mut sqlx::PgConnection>,
    ) -> bool {
        if !requested(environment, pilot_owner, row) || self.staging_candidate.is_none() {
            return false;
        }
        // Reuse full persisted candidate/digest/owner/actor/expiry validation before
        // the workspace transaction; it never runs under its SQL lock.
        match connection {
            Some(connection) => self
                .staging_candidate
                .as_ref()
                .unwrap()
                .row_allowed_on(connection, row, now)
                .await
                .is_ok(),
            None => self.candidate_row_allowed(row, now).await.is_ok(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn row() -> Value {
        json!({"account_id":"user_pilot","provider":"boxd","request_config":{"agent":"codex","rustActorId":"user_pilot","managedBaseReadiness":"uncertified-staging","managedCandidateSha256":"a".repeat(64),"managedCandidateArtifactId":"artifact","managedReleaseId":"release","runtimeGeneration":1}})
    }
    #[tokio::test]
    async fn unsigned_candidate_without_operator_pin_cannot_enable_internal_observations() {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        let key = ed25519_dalek::SigningKey::from_bytes(&[39; 32]);
        let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([39;32]),"x":B64.encode(key.verifying_key().as_bytes())});
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        let c = RuntimeControl::from_pool(
            pool,
            MasterKey::parse(&B64.encode([38; 32])).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://test.invalid").unwrap(),
        );
        assert!(
            !c.internal_codex_tool_observations(&row(), Some("staging"), Some("user_pilot"), 1)
                .await,
            "candidate-looking client metadata without operator pin must not access DB or enable raw events"
        );
    }
    #[test]
    fn production_unsigned_default_and_model_options_cannot_request_internal_surface() {
        let row = row();
        assert!(!requested(None, None, &row));
        assert!(!requested(Some("production"), Some("user_pilot"), &row));
        assert!(!requested(Some("staging"), None, &row));
        assert!(!requested(Some("staging"), Some("*"), &row));
        assert!(!requested(Some("staging"), Some("different_owner"), &row));
        let mut unsigned = row.clone();
        unsigned["request_config"]["managedCandidateSha256"] = Value::Null;
        unsigned["request_config"]["modelOptions"] =
            json!({"internalCodexToolObservations":true,"experimentalRawEvents":true});
        assert!(!requested(Some("staging"), Some("user_pilot"), &unsigned));
        assert!(requested(Some("staging"), Some("user_pilot"), &row));
        let mut changed = row.clone();
        changed["request_config"]["runtimeGeneration"] = json!(2);
        assert!(!same_binding(&row, &changed));
    }
}
