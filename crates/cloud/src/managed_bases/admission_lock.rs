//! A narrowly privileged row lock, not registry mutation authority.
use super::*;
pub(crate) async fn record(
    tx: &mut Transaction<'_, Postgres>,
    provider: &str,
    release: &str,
) -> Result<Option<Value>, ApiError> {
    sqlx::query_scalar("SELECT to_jsonb(b) FROM zuse_rust_managed_base_admission_lock($1,$2) b")
        .bind(provider)
        .bind(release)
        .fetch_optional(&mut **tx)
        .await
        .map_err(error)
}
pub(super) fn error(error: sqlx::Error) -> ApiError {
    match error.as_database_error().and_then(|e| e.code()).as_deref() {
        Some("42883") => ApiError::conflict("managed_base_lock_migration_required"),
        Some("42501") => ApiError::unavailable("managed_base_lock_permission_required"),
        _ => ApiError::from(error),
    }
}
