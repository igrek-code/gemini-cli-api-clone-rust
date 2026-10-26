//! Chat Completions orchestration: auth → token → onboarding → Google → OpenAI.

pub mod converter;
pub mod dtos;
pub mod stream;

use crate::codeassist::{self, dtos::*, extract_sse_data};
use crate::config::now_unix;
use crate::error::AppError;
use crate::state::{SessionStatus, SharedApp};
use axum::{
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    Json,
};
use dtos::*;
use futures::StreamExt;
use std::{
    convert::Infallible,
    pin::Pin,
    time::{Duration, Instant},
};
use tokio::sync::OwnedSemaphorePermit;

pub async fn chat_completions(State(app): State<SharedApp>, body: Bytes) -> Response {
    match chat_completions_inner(&app, body).await {
        Ok(resp) => resp,
        Err(e) => e.into_response(),
    }
}

async fn chat_completions_inner(app: &SharedApp, body: Bytes) -> Result<Response, AppError> {
    let req: ChatCompletionRequest = serde_json::from_slice(&body)
        .map_err(|e| AppError::bad_request_param(format!("Invalid JSON body: {e}."), "body"))?;

    // ---- validation (mirrors ChatCompletionService.validate) ----
    if req.messages.is_empty() {
        return Err(AppError::bad_request_param("'messages' must be a non-empty list.", "messages"));
    }
    if let Some(n) = req.n {
        if n > 1 {
            return Err(AppError::bad_request_param("Only n = 1 is supported.", "n"));
        }
    }
    let model = converter::resolve_model(&app.config, &req.model)
        .ok_or_else(|| AppError::model_not_found(&req.model))?;

    // ---- bulkhead ----
    let permit = app
        .inflight
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            AppError::rate_limited(
                format!(
                    "You already have {} requests in flight. Wait for one to complete.",
                    app.config.max_inflight
                ),
                Some(2),
            )
        })?;

    // ---- auth + onboarding ----
    let token = codeassist::access_token(app).await?;
    let project = codeassist::ensure_onboarded(app, &token).await?;

    let ca_req = converter::to_ca_request(&app.config, &req, &model, Some(&project))?;
    let include_usage = req.include_usage();
    let requested_model = req.model.clone();
    let stream = req.stream.unwrap_or(false);

    if stream {
        let resp = codeassist::stream_generate_content(app, &token, &model, &ca_req).await?;
        Ok(build_stream_response(app, resp, permit, requested_model, include_usage))
    } else {
        let _permit = permit;
        let ca_resp = codeassist::generate_content(app, &token, &model, &ca_req).await?;
        let completion = converter::to_completion(&ca_resp, &converter::new_completion_id(), now_unix(), &requested_model);
        Ok(Json(completion).into_response())
    }
}

fn build_stream_response(
    app: &SharedApp,
    resp: reqwest::Response,
    permit: OwnedSemaphorePermit,
    model: String,
    include_usage: bool,
) -> Response {
    let start = Instant::now();
    let ttft_deadline = tokio::time::Instant::from_std(start) + app.config.ttft_timeout;
    let idle = app.config.idle_chunk_timeout;
    let deadline = start + app.config.stream_deadline;

    let upstream: Pin<Box<dyn futures::Stream<Item = reqwest::Result<Bytes>> + Send>> = Box::pin(resp.bytes_stream());
    let mut relay = stream::StreamRelay::new(&model, include_usage);

    let body = async_stream::stream! {
        // Hold the bulkhead permit until the stream completes.
        let _permit = permit;
        let mut upstream = upstream;
        let mut buf = String::new();
        let mut started = false;
        let mut parsed: Vec<CaGenerateContentResponse> = Vec::new();

        loop {
            parsed.clear();
            if Instant::now() > deadline {
                for payload in relay.finish() {
                    yield Ok::<_, Infallible>(Event::default().data(payload));
                }
                break;
            }

            let next = if started {
                tokio::time::timeout(idle, upstream.next()).await
            } else {
                tokio::time::timeout_at(ttft_deadline, upstream.next()).await
            };

            match next {
                Err(_) => {
                    // timeout: TTFT or idle depending on `started`
                    let err = if started {
                        AppError::upstream(StatusCode::GATEWAY_TIMEOUT, "Google stream stalled: no chunk in time.", "upstream_timeout")
                    } else {
                        AppError::upstream(StatusCode::GATEWAY_TIMEOUT, "Timed out waiting for the first token.", "upstream_timeout")
                    };
                    for payload in relay.error_payload(&err) {
                        yield Ok(Event::default().data(payload));
                    }
                    break;
                }
                Ok(None) => {
                    // upstream completed normally
                    for payload in relay.finish() {
                        yield Ok(Event::default().data(payload));
                    }
                    break;
                }
                Ok(Some(Err(e))) => {
                    tracing::warn!("upstream stream error: {e}");
                    let err = AppError::upstream(StatusCode::BAD_GATEWAY, "Upstream connection error.", "google_upstream_error");
                    for payload in relay.error_payload(&err) {
                        yield Ok(Event::default().data(payload));
                    }
                    break;
                }
                Ok(Some(Ok(bytes))) => {
                    buf.push_str(&String::from_utf8_lossy(&bytes));
                    for data in extract_sse_data(&mut buf) {
                        let trimmed = data.trim();
                        if trimmed.is_empty() || trimmed == "[DONE]" {
                            continue;
                        }
                        match serde_json::from_str::<CaGenerateContentResponse>(&data) {
                            Ok(ca) => parsed.push(ca),
                            Err(e) => {
                                tracing::warn!("skipping malformed SSE chunk from Google: {e}");
                            }
                        }
                    }
                }
            }

            if !parsed.is_empty() {
                started = true;
            }
            for ca in &parsed {
                for payload in relay.handle_chunk(ca) {
                    yield Ok(Event::default().data(payload));
                }
            }
        }

        let result = &relay.result;
        tracing::info!(
            model,
            finish = result.finish_reason.as_deref().unwrap_or("?"),
            chars = result.content_chars,
            tool_calls = result.tool_calls,
            error = result.error.as_deref().unwrap_or(""),
            elapsed_ms = start.elapsed().as_millis() as u64,
            "stream complete"
        );
    };

    Sse::new(body)
        .keep_alive(
            KeepAlive::new()
                .interval(app.config.keep_alive_interval)
                .text("keep-alive"),
        )
        .into_response()
}

// ---------------------------------------------------------------- other routes

pub async fn models(State(app): State<SharedApp>) -> Json<ModelListResponse> {
    let mut ids: Vec<String> = app.config.models_available.clone();
    ids.extend(app.config.model_aliases.keys().cloned());
    ids.sort();
    ids.dedup();
    Json(ModelListResponse {
        object: "list",
        data: ids
            .into_iter()
            .map(|id| ModelInfo { id, object: "model", created: 0, owned_by: "google" })
            .collect(),
    })
}

pub async fn account_status(State(app): State<SharedApp>) -> Result<Json<AccountStatusResponse>, AppError> {
    let session = app.session.read().await;
    let Some(s) = session.as_ref() else {
        return Err(AppError::unauthorized(
            format!("Not logged in. Open {}/ in your browser to login with Google.", app.base_url()),
            "not_logged_in",
        ));
    };

    let mut resp = AccountStatusResponse {
        email: s.email.clone(),
        status: s.status.as_str().to_string(),
        message: s.message.clone(),
        project_id: s.project_id.clone(),
        tier: s.tier.clone(),
        onboarded: s.onboarded,
        google_quota: None,
    };
    drop(session);

    if resp.status == SessionStatus::Active.as_str() && resp.project_id.is_some() && resp.onboarded {
        if let Ok(token) = codeassist::access_token(&app).await {
            if let Ok(quota) = codeassist::retrieve_user_quota(&app, &token, resp.project_id.as_deref().unwrap()).await {
                resp.google_quota = quota.buckets.map(|buckets| {
                    buckets
                        .into_iter()
                        .map(|b| QuotaBucket {
                            token_type: b.token_type,
                            model_id: b.model_id,
                            remaining_amount: b.remaining_amount,
                            remaining_fraction: b.remaining_fraction,
                            reset_time: b.reset_time,
                        })
                        .collect()
                });
            }
        }
    }
    Ok(Json(resp))
}

// silence "unused Duration" if configs change
#[allow(unused)]
fn _d(_: Duration) {}
