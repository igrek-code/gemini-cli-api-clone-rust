//! Google OAuth: PKCE loopback flow (open browser → Google redirects back),
//! token exchange/refresh and userinfo.

use crate::{
    config::Config,
    error::AppError,
    state::{Session, SessionStatus, SharedApp},
};
use base64::Engine;
use rand::Rng;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const PKCE_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";

// --------------------------------------------------------------------- helpers

fn random_verifier() -> String {
    let mut rng = rand::rng();
    (0..64).map(|_| PKCE_CHARS[rng.random_range(0..PKCE_CHARS.len())] as char).collect()
}

fn sha256_b64url(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

pub fn random_hex(bytes: usize) -> String {
    let mut rng = rand::rng();
    (0..bytes)
        .map(|_| format!("{:02x}", rng.random::<u8>()))
        .collect()
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencode(v)))
        .collect::<Vec<_>>()
        .join("&")
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

// ------------------------------------------------------------------ token API

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GoogleTokens {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)] // wire fields kept for fidelity with the OpenAI API
pub struct GoogleUserInfo {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default, rename = "verified_email")]
    pub verified_email: Option<bool>,
}

/// `true` when Google says `invalid_grant` (refresh token revoked/expired).
#[derive(Debug)]
pub enum TokenError {
    Revoked,
    Other(AppError),
}

async fn post_token(
    http: &reqwest::Client,
    cfg: &Config,
    form: &[(&str, &str)],
    what: &str,
) -> Result<GoogleTokens, TokenError> {
    let send_fut = http
        .post(&cfg.token_url)
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(form_encode(form))
        .send();
    let resp = match tokio::time::timeout(Duration::from_secs(15), send_fut).await {
        Err(_) => {
            return Err(TokenError::Other(AppError::upstream(
                axum::http::StatusCode::BAD_GATEWAY,
                format!("Google OAuth timed out during {what}."),
                "google_oauth_error",
            )))
        }
        Ok(r) => r.map_err(|e| {
            tracing::warn!("Google OAuth error during {what}: {e}");
            TokenError::Other(AppError::upstream(
                axum::http::StatusCode::BAD_GATEWAY,
                format!("Google OAuth error during {what}: {e}."),
                "google_oauth_error",
            ))
        })?,
    };

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let code = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str().map(String::from)));
        if code.as_deref() == Some("invalid_grant") {
            tracing::warn!("Google rejected the token: invalid_grant");
            return Err(TokenError::Revoked);
        }
        tracing::warn!("Google OAuth error during {what} -> {status} {}", &body[..body.len().min(300)]);
        return Err(TokenError::Other(AppError::upstream(
            axum::http::StatusCode::BAD_GATEWAY,
            format!("Google OAuth error during {what} (status {}).", status.as_u16()),
            "google_oauth_error",
        )));
    }

    serde_json::from_str(&body).map_err(|e| {
        TokenError::Other(AppError::upstream(
            axum::http::StatusCode::BAD_GATEWAY,
            format!("Unexpected response from Google token endpoint: {e}."),
            "google_oauth_error",
        ))
    })
}

pub async fn exchange_code(
    http: &reqwest::Client,
    cfg: &Config,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<GoogleTokens, TokenError> {
    post_token(
        http,
        cfg,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", &cfg.client_id),
            ("client_secret", &cfg.client_secret),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
        ],
        "authorization code exchange",
    )
    .await
}

pub async fn refresh(http: &reqwest::Client, cfg: &Config, refresh_token: &str) -> Result<GoogleTokens, TokenError> {
    post_token(
        http,
        cfg,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &cfg.client_id),
            ("client_secret", &cfg.client_secret),
        ],
        "token refresh",
    )
    .await
}

pub async fn userinfo(http: &reqwest::Client, cfg: &Config, access_token: &str) -> Result<GoogleUserInfo, AppError> {
    let resp = http
        .get(&cfg.userinfo_url)
        .bearer_auth(access_token)
        .timeout(Duration::from_secs(15))
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::upstream(
            axum::http::StatusCode::BAD_GATEWAY,
            format!("Google userinfo failed (status {}).", status.as_u16()),
            "google_oauth_error",
        )
        .tap_log(&body));
    }
    resp.json().await.map_err(|e| {
        AppError::upstream(
            axum::http::StatusCode::BAD_GATEWAY,
            format!("Unexpected Google userinfo response: {e}."),
            "google_oauth_error",
        )
    })
}

trait TapLog {
    fn tap_log(self, body: &str) -> Self;
}
impl TapLog for AppError {
    fn tap_log(self, body: &str) -> Self {
        tracing::warn!("userinfo error body: {}", &body[..body.len().min(200)]);
        self
    }
}

// ------------------------------------------------------------- loopback login

/// Runs the full browser login flow. No-op when already logged in and healthy.
///
/// 1. PKCE S256 challenge
/// 2. tiny HTTP listener on 127.0.0.1:<random>
/// 3. open the browser at Google
/// 4. catch the redirect, exchange the code, fetch the email, onboard
pub async fn run_login(app: &SharedApp, main_port: u16) -> Result<(), AppError> {
    let _guard = app.login_lock.lock().await;

    if let Some(s) = app.session.read().await.as_ref() {
        if s.status != SessionStatus::Revoked {
            return Ok(()); // already logged in
        }
    }

    let cfg = &app.config;
    let verifier = random_verifier();
    let challenge = sha256_b64url(verifier.as_bytes());

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| AppError::upstream(axum::http::StatusCode::BAD_GATEWAY, format!("Cannot bind loopback listener: {e}."), "login_failed"))?;
    let redirect_port = listener.local_addr().map_err(|e| AppError::upstream(axum::http::StatusCode::BAD_GATEWAY, format!("Cannot get loopback port: {e}."), "login_failed"))?.port();
    let redirect_uri = format!("http://127.0.0.1:{redirect_port}/callback");

    let auth_url = format!(
        "{}?{}",
        cfg.auth_url,
        form_encode(&[
            ("client_id", cfg.client_id.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", cfg.scopes_joined().as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("access_type", "offline"),
            ("prompt", "consent"),
        ]),
    );

    tracing::info!("waiting for Google login…");
    println!("┌─────────────────────────────────────────────────────────────");
    println!("│  Google login required");
    println!("│  Your browser should have opened — if not, open:");
    println!("│  {auth_url}");
    println!("└─────────────────────────────────────────────────────────────");
    let _ = webbrowser::open(&auth_url);

    let code = wait_for_code(&listener, &format!("http://127.0.0.1:{main_port}/"), Duration::from_secs(600)).await?;

    let tokens = exchange_code(&app.http, cfg, &code, &verifier, &redirect_uri)
        .await
        .map_err(|e| match e {
            TokenError::Revoked => AppError::upstream(
                axum::http::StatusCode::BAD_GATEWAY,
                "Google rejected the authorization code. Try logging in again.",
                "google_oauth_error",
            ),
            TokenError::Other(e) => e,
        })?;

    let access_token = tokens.access_token.clone().ok_or_else(|| {
        AppError::upstream(axum::http::StatusCode::BAD_GATEWAY, "Google returned no access token.", "google_oauth_error")
    })?;

    let ui = userinfo(&app.http, cfg, &access_token).await?;
    let email = ui.email.unwrap_or_else(|| "unknown".to_string());
    tracing::info!("logged in as {email}");

    let mut session = Session {
        email,
        refresh_token: tokens.refresh_token.clone().unwrap_or_default(),
        access_token: tokens.access_token,
        token_expires_at: tokens
            .expires_in
            .map(|s| crate::config::now_unix() + s),
        project_id: None,
        tier: None,
        onboarded: false,
        status: SessionStatus::Active,
        message: None,
    };
    // A login always rotates the refresh token; if Google did not return one
    // we cannot continue without the previous one.
    if session.refresh_token.is_empty() {
        if let Some(old) = app.session.read().await.as_ref().map(|s| s.refresh_token.clone()) {
            session.refresh_token = old;
        }
    }

    // Code Assist onboarding right away so the API is immediately usable.
    match crate::codeassist::onboarding::setup(app, &access_token).await {
        Ok(crate::codeassist::onboarding::OnboardingResult::Success { project, tier }) => {
            session.project_id = Some(project);
            session.tier = tier;
            session.onboarded = true;
            session.status = SessionStatus::Active;
            session.message = None;
        }
        Ok(crate::codeassist::onboarding::OnboardingResult::NeedsValidation { url, message }) => {
            session.onboarded = false;
            session.status = SessionStatus::NeedsValidation;
            session.message = Some(format!("{message} ({url})"));
        }
        Ok(crate::codeassist::onboarding::OnboardingResult::Failure(message)) => {
            session.onboarded = false;
            session.status = SessionStatus::Error;
            session.message = Some(message);
        }
        Err(e) => {
            session.onboarded = false;
            session.status = SessionStatus::Error;
            session.message = Some(e.message);
        }
    }

    app.set_session(session).await;
    tracing::info!("login complete — API ready");
    Ok(())
}

/// Accepts connections on the loopback listener until a `code` (or `error`) arrives.
async fn wait_for_code(listener: &TcpListener, back_url: &str, timeout: Duration) -> Result<String, AppError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let (mut socket, _) = tokio::select! {
            accepted = listener.accept() => accepted.map_err(|e| AppError::upstream(
                axum::http::StatusCode::BAD_GATEWAY,
                format!("Loopback listener failed: {e}."),
                "login_failed",
            ))?,
            _ = tokio::time::sleep_until(deadline) => {
                return Err(AppError::upstream(
                    axum::http::StatusCode::BAD_GATEWAY,
                    "Login timed out (10 min). Open the app again to retry.",
                    "login_timeout",
                ));
            }
        };

        let head = {
            let read_all = async {
                let mut buf = Vec::with_capacity(2048);
                let mut tmp = [0u8; 2048];
                // read the request head
                loop {
                    match socket.read(&mut tmp).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 65536 {
                                break;
                            }
                        }
                    }
                }
                buf
            };
            let buf = match tokio::time::timeout(Duration::from_secs(10), read_all).await {
                Ok(buf) => buf,
                Err(_) => {
                    // hung connection without a request — ignore it
                    continue;
                }
            };
            String::from_utf8_lossy(&buf).to_string()
        };
        let request_line = head.lines().next().unwrap_or_default().to_string();
        let path = request_line.split_whitespace().nth(1).unwrap_or("").to_string();

        async fn respond(
            socket: &mut tokio::net::TcpStream,
            status: &str,
            title: &str,
            body: &str,
            back_url: &str,
        ) {
            let body = format!(
                "<!doctype html><html><head><meta charset=utf-8><meta http-equiv=refresh content=\"2;url={back_url}\"><title>{title}</title>\
                 <style>body{{font-family:-apple-system,system-ui,sans-serif;background:#0e1116;color:#e6edf3;display:flex;align-items:center;justify-content:center;height:100vh;margin:0}}\
                 .card{{background:#161b22;border:1px solid #30363d;border-radius:12px;padding:32px 48px;text-align:center}}</style></head>\
                 <body><div class=card><h2>{title}</h2><p>{body}</p><p style=color:#8b949e>Returning to the app…</p></div></body></html>"
            );
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(resp.as_bytes()).await;
            let _ = socket.shutdown().await;
        }

        if !path.starts_with("/callback") {
            respond(&mut socket, "404 Not Found", "Not found", "This URL is not part of the login flow.", back_url).await;
            continue;
        }

        let query = path.split_once('?').map(|(_, q)| q.to_string()).unwrap_or_default();
        let mut code: Option<String> = None;
        let mut oauth_error: Option<String> = None;
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or(("", ""));
            match k {
                "code" => code = Some(v.to_string()),
                "error" => oauth_error = Some(v.to_string()),
                _ => {}
            }
        }

        if let Some(err) = oauth_error {
            respond(&mut socket, "200 OK", "Login cancelled", &format!("Google returned: {err}"), back_url).await;
            return Err(AppError::upstream(
                axum::http::StatusCode::BAD_GATEWAY,
                format!("Google login was refused: {err}."),
                "login_refused",
            ));
        }

        match code {
            Some(c) if !c.is_empty() => {
                respond(&mut socket, "200 OK", "✅ Connected!", "You can close this tab.", back_url).await;
                return Ok(c);
            }
            _ => {
                respond(&mut socket, "400 Bad Request", "Missing code", "The callback had no authorization code.", back_url).await;
                continue;
            }
        }
    }
}
