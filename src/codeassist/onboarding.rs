//! Mirrors gemini-cli's `setupUser()`: loadCodeAssist then onboardUser (+ LRO polling).

use super::{dtos::*, get_operation, load_code_assist, onboard_user};
use crate::{error::AppError, state::SharedApp};
use axum::http::StatusCode;

#[derive(Debug)]
pub enum OnboardingResult {
    Success { project: String, tier: Option<String> },
    NeedsValidation { url: String, message: String },
    Failure(String),
}

pub async fn setup(app: &SharedApp, token: &str) -> Result<OnboardingResult, AppError> {
    let configured_project = app.config.google_cloud_project.clone();
    match load_code_assist(app, token, configured_project.as_deref()).await {
        Ok(load) => resolve_from_load(app, token, &load, configured_project.as_deref()).await,
        Err(e) => {
            if e.status == StatusCode::UNAUTHORIZED {
                Err(e)
            } else {
                tracing::warn!("onboarding failed: {}", e.message);
                Ok(OnboardingResult::Failure(e.message))
            }
        }
    }
}

async fn resolve_from_load(
    app: &SharedApp,
    token: &str,
    load: &CaLoadCodeAssistResponse,
    configured_project: Option<&str>,
) -> Result<OnboardingResult, AppError> {
    if load.current_tier.is_some() {
        let project = load
            .cloudaicompanion_project
            .clone()
            .or_else(|| configured_project.map(String::from));
        return Ok(match project {
            Some(p) => OnboardingResult::Success {
                project: p,
                tier: load
                    .paid_tier
                    .as_ref()
                    .and_then(|t| t.id.clone())
                    .or_else(|| load.current_tier.as_ref().and_then(|t| t.id.clone())),
            },
            None => ineligible_result(load),
        });
    }

    // Not onboarded yet: pick the default tier and run onboardUser.
    let tier = load
        .allowed_tiers
        .as_ref()
        .and_then(|tiers| tiers.iter().find(|t| t.is_default == Some(true)))
        .and_then(|t| t.id.clone());
    let tier_id = tier.unwrap_or_else(|| "legacy-tier".to_string());
    let free_tier = tier_id == "free-tier";
    let req = CaOnboardUserRequest {
        tier_id: tier_id.clone(),
        // Free tier uses a Google-managed project: passing one causes Precondition Failed.
        cloudaicompanion_project: if free_tier { None } else { configured_project.map(String::from) },
        metadata: CaClientMetadata {
            duet_project: if free_tier { None } else { configured_project.map(String::from) },
            ..Default::default()
        },
    };

    let lro = onboard_user(app, token, &req).await?;
    let done = await_operation(app, token, lro).await?;
    let project = done
        .response
        .and_then(|r| r.cloudaicompanion_project)
        .and_then(|p| p.id)
        .or_else(|| configured_project.map(String::from));
    match project {
        Some(p) => Ok(OnboardingResult::Success { project: p, tier: Some(tier_id) }),
        None => Err(AppError::upstream(
            StatusCode::BAD_GATEWAY,
            "Google onboarding completed but returned no project.",
            "google_onboarding_error",
        )),
    }
}

async fn await_operation(
    app: &SharedApp,
    token: &str,
    initial: CaLongRunningOperation,
) -> Result<CaLongRunningOperation, AppError> {
    if initial.done == Some(true) {
        return Ok(initial);
    }
    let name = initial.name.clone().ok_or_else(|| {
        AppError::upstream(
            StatusCode::BAD_GATEWAY,
            "Google onboarding returned an operation with no name.",
            "google_onboarding_error",
        )
    })?;

    let interval = app.config.onboarding_poll_interval;
    let max_attempts =
        (app.config.onboarding_poll_timeout.as_secs() / interval.as_secs()).max(1);
    let deadline = tokio::time::Instant::now() + app.config.onboarding_poll_timeout;

    for _ in 0..max_attempts {
        let op = get_operation(app, token, &name).await;
        match op {
            Ok(op) => {
                if op.done == Some(true) {
                    return Ok(op);
                }
            }
            Err(e) => return Err(e),
        }
        if tokio::time::Instant::now() + interval > deadline {
            break;
        }
        tokio::time::sleep(interval).await;
    }
    Err(AppError::upstream(
        StatusCode::BAD_GATEWAY,
        "Google onboarding operation did not complete in time.",
        "google_onboarding_timeout",
    ))
}

fn ineligible_result(load: &CaLoadCodeAssistResponse) -> OnboardingResult {
    if let Some(validation) = load
        .ineligible_tiers
        .as_ref()
        .and_then(|tiers| {
            tiers
                .iter()
                .find(|t| t.reason_code.as_deref() == Some("VALIDATION_REQUIRED") && t.validation_url.is_some())
        })
    {
        return OnboardingResult::NeedsValidation {
            url: validation.validation_url.clone().unwrap_or_default(),
            message: validation
                .reason_message
                .clone()
                .unwrap_or_else(|| "Google requires account validation before using this API.".into()),
        };
    }
    let reasons = load
        .ineligible_tiers
        .as_ref()
        .map(|tiers| {
            tiers
                .iter()
                .filter_map(|t| t.reason_message.clone())
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    OnboardingResult::Failure(format!(
        "Your Google account is not eligible for Gemini Code Assist{}{} If you want to use your own Google Cloud project, set GOOGLE_CLOUD_PROJECT and retry.",
        if reasons.is_empty() { "." } else { ": " },
        reasons
    ))
}
