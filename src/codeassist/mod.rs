//! HTTP client for the private Google Code Assist API (`v1internal`).

pub mod dtos;
pub mod onboarding;

use crate::{
    error::AppError,
    state::{App, Session, SessionStatus, SharedApp},
};
use axum::http::StatusCode;
use dtos::*;
use rand::Rng;
use std::time::Duration;

// ------------------------------------------------------------------ generation

/// Non-streaming completion (120s timeout, retry on 502/503/network).
pub async fn generate_content(
    app: &SharedApp,
    token: &str,
    model: &str,
    body: &CaGenerateContentRequest,
) -> Result<CaGenerateContentResponse, AppError> {
    let url = app.config.ca_method_url("generateContent");
    let ua = app.config.user_agent(Some(model));
    let payload = serde_json::to_value(body).unwrap_or(serde_json::Value::Null);
    let timeout = app.config.response_timeout;
    let http = app.http.clone();
    let token = token.to_string();
    let login_url = app.base_url();

    let make = move || {
        let http = http.clone();
        let url = url.clone();
        let ua = ua.clone();
        let payload = payload.clone();
        let token = token.clone();
        let login_url = login_url.clone();
        async move {
            let fut = async {
                http.post(&url)
                    .bearer_auth(&token)
                    .header("User-Agent", &ua)
                    .header("Content-Type", "application/json")
                    .json(&payload)
                    .send()
                    .await
            };
            let resp = match tokio::time::timeout(timeout, fut).await {
                Err(_) => {
                    return Err(AppError::upstream(
                        StatusCode::GATEWAY_TIMEOUT,
                        "Google did not respond in time.",
                        "upstream_timeout",
                    ))
                }
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) => return Err(e.into()),
            };
            let status = resp.status();
            if !status.is_success() {
                let body_text = resp.text().await.unwrap_or_default();
                return Err(map_google_error("generateContent", status, body_text, None, &login_url));
            }
            resp.json::<CaGenerateContentResponse>().await.map_err(|e| {
                AppError::upstream(
                    StatusCode::BAD_GATEWAY,
                    format!("Unexpected Google response: {e}."),
                    "google_upstream_error",
                )
            })
        }
    };
    retry(3, Duration::from_secs(1), make).await
}

/// Sends the streaming request and awaits response headers (with retry on
/// 502/503/network). Body is consumed by the caller as a byte stream.
pub async fn stream_generate_content(
    app: &SharedApp,
    token: &str,
    model: &str,
    body: &CaGenerateContentRequest,
) -> Result<reqwest::Response, AppError> {
    let url = format!("{}?alt=sse", app.config.ca_method_url("streamGenerateContent"));
    let ua = app.config.user_agent(Some(model));
    let payload = serde_json::to_value(body).unwrap_or(serde_json::Value::Null);
    let http = app.http.clone();
    let token = token.to_string();
    let login_url = app.base_url();

    let make = move || {
        let http = http.clone();
        let url = url.clone();
        let ua = ua.clone();
        let payload = payload.clone();
        let token = token.clone();
        let login_url = login_url.clone();
        async move {
            let resp = http
                .post(&url)
                .bearer_auth(&token)
                .header("User-Agent", &ua)
                .header("Accept", "text/event-stream")
                .header("Content-Type", "application/json")
                .json(&payload)
                .send()
                .await?;
            let status = resp.status();
            if !status.is_success() {
                let retry_after = resp
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok());
                let body_text = resp.text().await.unwrap_or_default();
                return Err(map_google_error("streamGenerateContent", status, body_text, retry_after, &login_url));
            }
            Ok(resp)
        }
    };
    retry(3, Duration::from_millis(500), make).await
}

/// Extracts complete SSE events from `buf`, returning their `data` payloads.
pub fn extract_sse_data(buf: &mut String) -> Vec<String> {
    let mut events = Vec::new();
    while let Some(idx) = buf.find("\n\n") {
        let raw_event: String = buf[..idx].to_string();
        buf.drain(..idx + 2);
        let mut data_lines: Vec<&str> = Vec::new();
        for line in raw_event.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if let Some(d) = line.strip_prefix("data:") {
                data_lines.push(d.strip_prefix(' ').unwrap_or(d));
            }
        }
        if !data_lines.is_empty() {
            events.push(data_lines.join("\n"));
        }
    }
    events
}

// ------------------------------------------------------------------ onboarding

pub async fn load_code_assist(app: &SharedApp, token: &str, cloudaicompanion_project: Option<&str>) -> Result<CaLoadCodeAssistResponse, AppError> {
    let req = CaLoadCodeAssistRequest {
        cloudaicompanion_project: cloudaicompanion_project.map(String::from),
        metadata: CaClientMetadata {
            duet_project: cloudaicompanion_project.map(String::from),
            ..Default::default()
        },
    };
    post_json(app, token, "loadCodeAssist", &serde_json::to_value(&req).unwrap(), 30).await
}

pub async fn onboard_user(app: &SharedApp, token: &str, req: &CaOnboardUserRequest) -> Result<CaLongRunningOperation, AppError> {
    post_json(app, token, "onboardUser", &serde_json::to_value(req).unwrap(), 30).await
}

pub async fn get_operation(app: &SharedApp, token: &str, name: &str) -> Result<CaLongRunningOperation, AppError> {
    let encoded = name
        .split('/')
        .map(|seg| urlencode(seg))
        .collect::<Vec<_>>()
        .join("/");
    let url = format!("{}/{}", app.config.ca_base_url(), encoded);
    let resp = app
        .http
        .get(url)
        .bearer_auth(token)
        .header("User-Agent", app.config.user_agent(None))
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(map_google_error("getOperation", status, body, None, &app.base_url()));
    }
    resp.json().await.map_err(|e| {
        AppError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("Unexpected Google response during getOperation: {e}."),
            "google_upstream_error",
        )
    })
}

pub async fn retrieve_user_quota(app: &SharedApp, token: &str, project: &str) -> Result<CaRetrieveUserQuotaResponse, AppError> {
    let req = CaRetrieveUserQuotaRequest {
        project: project.to_string(),
        user_agent: Some(app.config.user_agent(None)),
    };
    post_json(app, token, "retrieveUserQuota", &serde_json::to_value(&req).unwrap(), 15).await
}

async fn post_json<T: serde::de::DeserializeOwned>(
    app: &SharedApp,
    token: &str,
    method: &str,
    payload: &serde_json::Value,
    timeout_secs: u64,
) -> Result<T, AppError> {
    let url = app.config.ca_method_url(method);
    let fut = app
        .http
        .post(url)
        .bearer_auth(token)
        .header("User-Agent", app.config.user_agent(None))
        .header("Content-Type", "application/json")
        .json(payload)
        .send();
    let resp = match tokio::time::timeout(Duration::from_secs(timeout_secs), fut).await {
        Err(_) => {
            return Err(AppError::upstream(
                StatusCode::GATEWAY_TIMEOUT,
                format!("Google {method} timed out."),
                "upstream_timeout",
            ))
        }
        Ok(Ok(resp)) => resp,
        Ok(Err(e)) => return Err(e.into()),
    };
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(map_google_error(method, status, body, None, &app.base_url()));
    }
    resp.json().await.map_err(|e| {
        AppError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("Unexpected Google response during {method}: {e}."),
            "google_upstream_error",
        )
    })
}

// -------------------------------------------------------------- error mapping

/// Maps Google HTTP errors to OpenAI-style errors, mirroring the Kotlin client.
pub fn map_google_error(what: &str, status: reqwest::StatusCode, body: String, retry_after: Option<u64>, login_url: &str) -> AppError {
    let google_msg = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str().map(String::from)));
    let detail = google_msg.unwrap_or_else(|| {
        let t = body.trim();
        if t.is_empty() { "no details".to_string() } else { t.chars().take(300).collect() }
    });
    let message = format!("Google Code Assist error during {what} (status {}): {detail}", status.as_u16());
    match status.as_u16() {
        401 => AppError::unauthorized(
            format!("Google rejected the access token. Re-login from the app home page (open {login_url}/)."),
            "google_account_revoked",
        ),
        403 => AppError::forbidden(message, "google_forbidden"),
        429 => AppError::rate_limited(
            format!(
                "Google rate limit hit for your account (per Google account: 60 req/min, 1000 req/day). Details: {message}"
            ),
            retry_after,
        ),
        500..=599 => AppError::upstream(StatusCode::BAD_GATEWAY, message, "google_upstream_error"),
        _ => AppError::bad_request(message),
    }
}

// -------------------------------------------------------------------- retries

/// Retry helper with exponential backoff + jitter. Retries 502/503 (Google 5xx
/// and network errors are both mapped to status 502 upstream errors).
pub async fn retry<T, F, Fut>(max_attempts: u32, base: Duration, mut f: F) -> Result<T, AppError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, AppError>>,
{
    let mut attempt = 0;
    loop {
        attempt += 1;
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                let retryable =
                    e.status == StatusCode::BAD_GATEWAY || e.status == StatusCode::SERVICE_UNAVAILABLE;
                if !retryable || attempt >= max_attempts {
                    return Err(e);
                }
                let mult = 2f64.powi((attempt - 1) as i32);
                let jitter: f64 = rand::rng().random_range(0.5..1.5);
                let delay = base.mul_f64(mult * jitter);
                tracing::warn!("attempt {attempt} failed ({}), retrying in {delay:?}", e.code);
                tokio::time::sleep(delay).await;
            }
        }
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// -------------------------------------------------------------- token manager

/// Returns a valid access token, refreshing it (single-flight) when needed.
pub async fn access_token(app: &SharedApp) -> Result<String, AppError> {
    {
        let session = app.session.read().await;
        if let Some(s) = session.as_ref() {
            if s.status == SessionStatus::Revoked {
                return Err(revoked_error(app));
            }
            if s.token_valid() {
                return Ok(s.access_token.clone().unwrap_or_default());
            }
        }
    }

    // Single-flight: only one refresh per process at a time.
    let _guard = app.refresh_lock.lock().await;

    // Double-check after acquiring the lock (another task may have refreshed).
    {
        let session = app.session.read().await;
        if let Some(s) = session.as_ref() {
            if s.status == SessionStatus::Revoked {
                return Err(revoked_error(app));
            }
            if s.token_valid() {
                return Ok(s.access_token.clone().unwrap_or_default());
            }
        }
    }

    let mut session = {
        let session = app.session.read().await;
        session.as_ref().cloned().ok_or_else(|| revoked_error(app))?
    };

    match crate::oauth::refresh(&app.http, &app.config, &session.refresh_token).await {
        Ok(tokens) => {
            let access = tokens.access_token.unwrap_or_default();
            if let Some(new_refresh) = tokens.refresh_token {
                session.refresh_token = new_refresh;
            }
            session.access_token = Some(access.clone());
            session.token_expires_at = Some(crate::config::now_unix() + tokens.expires_in.unwrap_or(3600) - 30);
            if session.status == SessionStatus::Revoked {
                session.status = SessionStatus::Active;
            }
            app.set_session(session).await;
            Ok(access)
        }
        Err(crate::oauth::TokenError::Revoked) => {
            tracing::warn!("refresh token revoked — re-login required");
            session.status = SessionStatus::Revoked;
            session.access_token = None;
            session.message = Some("Google session expired. Re-login required.".to_string());
            app.set_session(session).await;
            Err(revoked_error(app))
        }
        Err(crate::oauth::TokenError::Other(e)) => {
            Err(AppError::upstream(
                StatusCode::BAD_GATEWAY,
                format!("Failed to refresh Google access token: {}", e.message),
                "google_oauth_error",
            ))
        }
    }
}

pub fn revoked_error(app: &App) -> AppError {
    AppError::unauthorized(
        format!("Your Google session expired or was revoked. Open {}/ in your browser to re-login.", app.base_url()),
        "google_account_revoked",
    )
}

/// Ensures Code Assist onboarding is done; updates and persists the session.
pub async fn ensure_onboarded(app: &SharedApp, token: &str) -> Result<String, AppError> {
    {
        let session = app.session.read().await;
        if let Some(s) = session.as_ref() {
            if s.onboarded {
                if let Some(p) = &s.project_id {
                    return Ok(p.clone());
                }
            }
            match s.status {
                SessionStatus::NeedsValidation => {
                    return Err(AppError::forbidden(
                        format!(
                            "Google requires you to validate your account: {} Open the validation URL in your browser, complete it, then retry.",
                            s.message.as_deref().unwrap_or("")
                        ),
                        "google_validation_required",
                    ));
                }
                SessionStatus::Error => {
                    return Err(AppError::forbidden(
                        s.message.clone().unwrap_or_else(|| "Google account issue.".into()),
                        "google_account_ineligible",
                    ));
                }
                _ => {}
            }
        }
    }

    let _guard = app.onboarding_lock.lock().await;
    // double-check
    {
        let session = app.session.read().await;
        if let Some(s) = session.as_ref() {
            if s.onboarded {
                if let Some(p) = &s.project_id {
                    return Ok(p.clone());
                }
            }
        }
    }

    match onboarding::setup(app, token).await {
        Ok(onboarding::OnboardingResult::Success { project, tier }) => {
            let mut session: Session = {
                let session = app.session.read().await;
                session.as_ref().cloned().unwrap()
            };
            session.project_id = Some(project.clone());
            session.tier = tier;
            session.onboarded = true;
            session.status = SessionStatus::Active;
            session.message = None;
            app.set_session(session).await;
            Ok(project)
        }
        Ok(onboarding::OnboardingResult::NeedsValidation { url, message }) => {
            let mut session: Session = {
                let session = app.session.read().await;
                session.as_ref().cloned().unwrap()
            };
            session.status = SessionStatus::NeedsValidation;
            session.message = Some(format!("{message} ({url})"));
            session.onboarded = false;
            app.set_session(session).await;
            Err(AppError::forbidden(
                format!("Google requires you to validate your account: {message} Open {url} in your browser, complete the validation, then retry."),
                "google_validation_required",
            ))
        }
        Ok(onboarding::OnboardingResult::Failure(message)) => {
            let mut session: Session = {
                let session = app.session.read().await;
                session.as_ref().cloned().unwrap()
            };
            session.status = SessionStatus::Error;
            session.message = Some(message.clone());
            session.onboarded = false;
            app.set_session(session).await;
            Err(AppError::forbidden(message, "google_account_ineligible"))
        }
        Err(e) => Err(e),
    }
}

/// Parses an SSE byte stream from Google into parsed CA responses, skipping
/// malformed chunks (used by tests).
#[cfg(test)]
pub fn parse_stream_for_test(data: &str) -> Vec<CaGenerateContentResponse> {
    let mut buf = data.to_string();
    extract_sse_data(&mut buf)
        .into_iter()
        .filter(|d| !d.trim().is_empty())
        .filter_map(|d| match serde_json::from_str::<CaGenerateContentResponse>(&d) {
            Ok(v) => Some(v),
            Err(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_extraction_handles_crlf_and_multiline() {
        let mut buf = String::new();
        buf.push_str("data: {\"a\":1}\r\n\r\n");
        buf.push_str(": comment\n\n");
        buf.push_str("data: {\"b\":");
        let events = extract_sse_data(&mut buf);
        assert_eq!(events, vec!["{\"a\":1}".to_string()]);
        assert_eq!(buf, "data: {\"b\":"); // incomplete event stays buffered

        buf.push_str("2}\n\n");
        let events = extract_sse_data(&mut buf);
        assert_eq!(events, vec!["{\"b\":2}".to_string()]);
        assert!(buf.is_empty());
    }

    #[test]
    fn sse_multiline_data_joined() {
        let mut buf = "data: line1\ndata: line2\n\n".to_string();
        let events = extract_sse_data(&mut buf);
        assert_eq!(events, vec!["line1\nline2".to_string()]);
    }

    #[test]
    fn parse_stream_skips_malformed() {
        let data = "data: {\"response\":{\"candidates\":[]}}\n\ndata: not-json\n\ndata: {}\n\n";
        let parsed = parse_stream_for_test(data);
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn google_error_mapping() {
        let e = map_google_error(
            "generateContent",
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":{"message":"Resource exhausted"}}"#.into(),
            Some(30),
            "http://localhost:8080",
        );
        assert_eq!(e.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(e.retry_after, Some(30));
        assert!(e.message.contains("Resource exhausted"));

        let e = map_google_error("x", reqwest::StatusCode::UNAUTHORIZED, String::new(), None, "http://localhost:8080");
        assert_eq!(e.status, StatusCode::UNAUTHORIZED);
        assert_eq!(e.code, "google_account_revoked");

        let e = map_google_error("x", reqwest::StatusCode::BAD_GATEWAY, String::new(), None, "http://localhost:8080");
        assert_eq!(e.status, StatusCode::BAD_GATEWAY);
        assert_eq!(e.retry_after, None);
    }
}
