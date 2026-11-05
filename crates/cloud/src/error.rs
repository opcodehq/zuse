use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str) -> Self {
        Self { status, code }
    }
    pub fn unauthorized(code: &'static str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, code)
    }
    pub fn forbidden(code: &'static str) -> Self {
        Self::new(StatusCode::FORBIDDEN, code)
    }
    pub fn unavailable(code: &'static str) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, code)
    }
    pub fn bad_request(code: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code)
    }
    pub fn not_found(code: &'static str) -> Self {
        Self::new(StatusCode::NOT_FOUND, code)
    }
    pub fn conflict(code: &'static str) -> Self {
        Self::new(StatusCode::CONFLICT, code)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.code }))).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        // SQL errors can carry connection details, SQL and row contents. Log
        // only a validated SQLSTATE; never Display/Debug the error itself.
        if let Some(code) = error
            .as_database_error()
            .and_then(|e| e.code())
            .filter(|c| {
                c.len() == 5
                    && c.bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            })
        {
            tracing::warn!(sqlstate = code.as_ref(), "Cloud database operation failed");
        }
        Self::unavailable("cloud_database_unavailable")
    }
}
