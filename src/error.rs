//! Application error carrying an OpenAI-style payload and HTTP status.

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};

#[derive(Debug, Clone)]
pub struct AppError {
    pub status: StatusCode,
    pub message: String,
    pub etype: &'static str,
    pub code: String,
    pub param: Option<String>,
    pub retry_after: Option<u64>,
}

impl AppError {
    pub fn new(status: StatusCode, message: impl Into<String>, etype: &'static str, code: impl Into<String>) -> Self {
        Self { status, message: message.into(), etype, code: code.into(), param: None, retry_after: None }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message, "invalid_request_error", "invalid_request")
    }

    pub fn bad_request_param(message: impl Into<String>, param: &str) -> Self {
        let mut e = Self::bad_request(message);
        e.param = Some(param.to_string());
        e
    }

    pub fn unauthorized(message: impl Into<String>, code: &str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message, "authentication_error", code)
    }

    pub fn forbidden(message: impl Into<String>, code: &str) -> Self {
        Self::new(StatusCode::FORBIDDEN, message, "authentication_error", code)
    }

    pub fn model_not_found(model: &str) -> Self {
        let mut e = Self::new(
            StatusCode::NOT_FOUND,
            format!("The model '{model}' does not exist or is not available on this API."),
            "invalid_request_error",
            "model_not_found",
        );
        e.param = Some("model".to_string());
        e
    }

    pub fn rate_limited(message: impl Into<String>, retry_after: Option<u64>) -> Self {
        let mut e = Self::new(StatusCode::TOO_MANY_REQUESTS, message, "rate_limit_error", "rate_limit_exceeded");
        e.retry_after = retry_after;
        e
    }

    pub fn upstream(status: StatusCode, message: impl Into<String>, code: &str) -> Self {
        Self::new(status, message, "api_error", code)
    }

    #[allow(dead_code)] // kept for API completeness
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, message, "service_unavailable", "service_unavailable")
    }

    /// JSON payload `{"error": {...}}` used for both HTTP errors and mid-stream SSE events.
    pub fn error_json(&self) -> serde_json::Value {
        let mut inner = serde_json::json!({
            "message": self.message,
            "type": self.etype,
            "code": self.code,
        });
        if let Some(p) = &self.param {
            inner["param"] = serde_json::json!(p);
        }
        serde_json::json!({ "error": inner })
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let mut resp = (self.status, Json(self.error_json())).into_response();
        if let Some(secs) = self.retry_after {
            if let Ok(v) = secs.to_string().parse() {
                resp.headers_mut().insert(header::RETRY_AFTER, v);
            }
        }
        resp
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.status.as_u16(), self.code, self.message)
    }
}

impl std::error::Error for AppError {}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        tracing::warn!("http error: {e}");
        AppError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("Network error talking to Google: {e}"),
            "google_upstream_error",
        )
    }
}
