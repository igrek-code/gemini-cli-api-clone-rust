//! Router assembly + landing page + login/quit endpoints.

use crate::{error::AppError, oauth, openai, state::SharedApp};
use axum::{
    extract::{DefaultBodyLimit, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};

pub fn router(app: SharedApp) -> Router {
    Router::new()
        .route("/", get(landing))
        .route("/login", get(login))
        .route("/quit", post(quit))
        .route("/v1/models", get(openai::models))
        .route("/v1/chat/completions", post(openai::chat_completions))
        .route("/v1/account/status", get(openai::account_status))
        .layer(axum::middleware::from_fn_with_state(app.clone(), api_key_guard))
        .layer(DefaultBodyLimit::max(20 * 1024 * 1024))
        .with_state(app)
}

/// Optional API key guard for /v1/* when GEMINI_API_KEY is configured.
async fn api_key_guard(State(app): State<SharedApp>, req: Request, next: Next) -> Response {
    let Some(expected) = app.config.api_key.as_ref() else {
        return next.run(req).await;
    };
    if !req.uri().path().starts_with("/v1/") {
        return next.run(req).await;
    }
    let ok = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|key| key == expected)
        .unwrap_or(false);
    if !ok {
        return AppError::unauthorized("Invalid API key. Set 'Authorization: Bearer <GEMINI_API_KEY>'.", "invalid_api_key")
            .into_response();
    }
    next.run(req).await
}

async fn landing(State(app): State<SharedApp>) -> Html<String> {
    let session = app.session.read().await;
    let (logged, email, tier, project, status, message) = match session.as_ref() {
        Some(s) => (
            s.status != crate::state::SessionStatus::Revoked,
            s.email.clone(),
            s.tier.clone().unwrap_or_default(),
            s.project_id.clone().unwrap_or_default(),
            s.status.as_str().to_string(),
            s.message.clone().unwrap_or_default(),
        ),
        None => (false, String::new(), String::new(), String::new(), "NOT_LOGGED_IN".into(), String::new()),
    };
    drop(session);

    let esc = |s: &str| -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    };
    let models = app
        .config
        .models_available
        .iter()
        .map(|m| format!("<code>{}</code>", esc(m)))
        .collect::<Vec<_>>()
        .join(", ");

    let status_card = if !logged {
        format!(
            r#"<div class="card warn">
                <h2>🔑 Connexion requise</h2>
                <p>{msg}</p>
                <p><a class="btn" href="/login">Se connecter avec Google</a></p>
                <p class="muted">Le navigateur va s'ouvrir — autorise, et c'est tout.</p>
            </div>"#,
            msg = if message.is_empty() { "Ton compte Google n'est pas encore connecté.".into() } else { esc(&message) }
        )
    } else if status == "ACTIVE" {
        format!(
            r#"<div class="card ok">
                <h2>✅ Connecté</h2>
                <p><strong>{email}</strong> &nbsp;<span class="tag">{tier}</span></p>
                <p class="muted">projet : <code>{project}</code></p>
            </div>"#,
            email = esc(&email),
            tier = esc(if tier.is_empty() { "free-tier" } else { &tier }),
            project = esc(&project),
        )
    } else {
        format!(
            r#"<div class="card warn">
                <h2>⚠️ {status}</h2>
                <p>{msg}</p>
                <p><a class="btn" href="/login">Re-se connecter</a></p>
            </div>"#,
            status = esc(&status),
            msg = esc(&message),
        )
    };

    Html(format!(
        r#"<!doctype html>
<html lang="fr">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>gemini-api</title>
<style>
  :root {{ color-scheme: dark; }}
  * {{ box-sizing: border-box; }}
  body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
         background: #0d1117; color: #e6edf3; margin: 0; padding: 40px 16px; }}
  .wrap {{ max-width: 720px; margin: 0 auto; display: flex; flex-direction: column; gap: 20px; }}
  h1 {{ font-size: 22px; margin: 0; }}
  h1 span {{ color: #8b949e; font-weight: 400; font-size: 14px; }}
  .card {{ background: #161b22; border: 1px solid #30363d; border-radius: 12px; padding: 22px 26px; }}
  .card.ok {{ border-color: #238636; }}
  .card.warn {{ border-color: #d29922; }}
  .muted {{ color: #8b949e; font-size: 13px; }}
  code {{ background: #21262d; padding: 2px 7px; border-radius: 6px; font-size: 13px; }}
  pre {{ background: #0a0d12; border: 1px solid #30363d; border-radius: 8px; padding: 14px 16px;
        overflow-x: auto; font-size: 12.5px; line-height: 1.5; }}
  .btn {{ display: inline-block; background: #238636; color: #fff; padding: 10px 18px; border-radius: 8px;
         text-decoration: none; font-weight: 600; margin-top: 6px; }}
  .btn.danger {{ background: #da3633; }}
  .tag {{ background: #1f6feb33; color: #79c0ff; padding: 2px 10px; border-radius: 999px; font-size: 12px; }}
  .row {{ display: flex; gap: 10px; align-items: center; flex-wrap: wrap; }}
  a {{ color: #58a6ff; }}
</style>
</head>
<body>
<div class="wrap">
  <h1>gemini-api <span>— ton quota Gemini CLI en API OpenAI-compatible</span></h1>

  {status_card}

  <div class="card">
    <h2 style="font-size:16px">Base URL</h2>
    <pre id="base">{base_url}/v1</pre>
    <p class="muted">Utilisable depuis n'importe quel client OpenAI (curl, libs, agents…).
    {keynote}</p>
  </div>

  <div class="card">
    <h2 style="font-size:16px">Test rapide</h2>
    <pre>curl {base_url}/v1/chat/completions \
  -H "Content-Type: application/json" \
  {auth_line}-d '{{
    "model": "gemini-2.5-flash",
    "messages": [{{"role": "user", "content": "Salut !"}}]
  }}'</pre>
  </div>

  <div class="card">
    <h2 style="font-size:16px">Modèles</h2>
    <p>{models}</p>
    <p class="muted">Alias : gpt-4o, gpt-4o-mini, gpt-4.1, gpt-5, o3, o4-mini… (config Cargo / <code>src/config.rs</code>)</p>
  </div>

  <div class="card">
    <div class="row" style="justify-content:space-between">
      <div>
        <h2 style="font-size:16px;margin:0">Quota Google</h2>
        <p class="muted" style="margin:4px 0 0" id="quota">—</p>
      </div>
      <div class="row">
        <button class="btn" onclick="refreshQuota()" style="cursor:pointer;border:none">Rafraîchir</button>
        <button class="btn danger" onclick="quit()" style="cursor:pointer;border:none">Quitter</button>
      </div>
    </div>
  </div>

  <p class="muted" style="text-align:center">mimique Gemini CLI → cloudcode-pa.googleapis.com · free tier : 60 req/min, 1000 req/jour</p>
</div>
<script>
async function refreshQuota() {{
  let el = document.getElementById('quota');
  el.textContent = 'chargement…';
  try {{
    const r = await fetch('/v1/account/status');
    const j = await r.json();
    if (!j.googleQuota || !j.googleQuota.length) {{ el.textContent = j.status || 'ok'; return; }}
    el.innerHTML = j.googleQuota.map(b =>
      `${{b.tokenType ?? '??'}} : ${{b.remainingAmount ?? Math.round((b.remainingFraction ?? 0) * 100) + '%'}} restant` +
      (b.resetTime ? ` (reset ${{new Date(b.resetTime).toLocaleTimeString()}})` : '')
    ).join('<br>');
  }} catch (e) {{ el.textContent = e.message; }}
}}
async function quit() {{
  if (!confirm('Arrêter gemini-api ?')) return;
  await fetch('/quit', {{method: 'POST'}});
  document.body.innerHTML = '<div style="text-align:center;padding:80px 0"><h2>👋 À bientôt</h2><p class=muted>gemini-api est arrêté.</p></div>';
}}
refreshQuota();
</script>
</body>
</html>"#,
        status_card = status_card,
        base_url = app.base_url(),
        keynote = if app.config.api_key.is_some() {
            "Une clé API est requise (<code>Authorization: Bearer …</code>)."
        } else {
            "Aucune clé requise en local (bind 127.0.0.1 uniquement)."
        },
        auth_line = if app.config.api_key.is_some() { "-H \"Authorization: Bearer TA_CLE\" \\\n  " } else { "" },
        models = models,
    ))
}

async fn login(State(app): State<SharedApp>) -> Response {
    let port = app.config.port;
    let app = app.clone();
    tokio::spawn(async move {
        if let Err(e) = oauth::run_login(&app, port).await {
            tracing::error!("login failed: {e}");
        }
    });
    Redirect::to("/").into_response()
}

async fn quit(State(app): State<SharedApp>) -> StatusCode {
    tracing::info!("quit requested from the web UI");
    let _ = app.shutdown.send(true);
    StatusCode::OK
}
