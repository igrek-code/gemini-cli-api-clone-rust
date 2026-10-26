//! gemini-api — local OpenAI-compatible API backed by your own Google account.
//!
//! First launch: opens your browser for Google login (loopback OAuth, PKCE),
//! stores tokens in a local file, then serves http://localhost:8080/v1.

mod codeassist;
mod config;
mod error;
mod oauth;
mod openai;
mod state;
mod web;

use state::SessionStatus;
use std::io::IsTerminal;

#[tokio::main]
async fn main() {
    let cfg = config::Config::from_env();

    // data dir (0700) + credentials + logs
    if let Err(e) = std::fs::create_dir_all(&cfg.data_dir) {
        eprintln!("fatal: cannot create data dir {}: {e}", cfg.data_dir.display());
        std::process::exit(1);
    }
    init_logging(&cfg);

    let app = state::App::new(cfg);

    // restore previous session
    if let Some(session) = app.load_session_from_disk() {
        tracing::info!("restored session for {}", session.email);
        *app.session.write().await = Some(session);
    }

    let port = app.config.port;
    let needs_login = {
        let s = app.session.read().await;
        match s.as_ref() {
            None => true,
            Some(s) => s.status == SessionStatus::Revoked,
        }
    };
    if needs_login {
        let app2 = app.clone();
        tokio::spawn(async move {
            if let Err(e) = oauth::run_login(&app2, port).await {
                tracing::error!("login failed: {e}");
            }
        });
    } else {
        // warm the onboarding/token in the background so the first request is fast
        let app2 = app.clone();
        tokio::spawn(async move {
            if let Ok(token) = codeassist::access_token(&app2).await {
                let _ = codeassist::ensure_onboarded(&app2, &token).await;
            }
        });
    }

    // bind 127.0.0.1, falling back to the next ports when busy
    let listener = bind_local(port).await.unwrap_or_else(|e| {
        eprintln!("fatal: cannot bind 127.0.0.1:{port} (+fallbacks): {e}");
        std::process::exit(1);
    });
    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    app.port.store(actual_port, std::sync::atomic::Ordering::Relaxed);
    let base_url = format!("http://localhost:{actual_port}");

    let log_path = app.config.data_dir.join("gemini-api.log").display().to_string();
    println!();
    println!("  gemini-api is running ✅");
    println!("    Home page  : {base_url}");
    println!("    OpenAI API : {base_url}/v1");
    println!("    Logs       : {log_path}");
    println!();

    // open the landing page when launched without a terminal (double-clicked .app)
    if !std::io::stdout().is_terminal() || std::env::var("GEMINI_API_OPEN").ok().as_deref() == Some("1") {
        let url = base_url.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let _ = webbrowser::open(&url);
        });
    }

    let router = web::router(app.clone());
    let mut shutdown_rx = app.shutdown.subscribe();

    let serve = axum::serve(listener, router).with_graceful_shutdown(async move {
        tokio::select! {
            _ = shutdown_rx.changed() => {},
            _ = tokio::signal::ctrl_c() => {},
        }
        tracing::info!("shutting down…");
    });

    if let Err(e) = serve.await {
        tracing::error!("server error: {e}");
        std::process::exit(1);
    }
    tracing::info!("bye 👋");
}

async fn bind_local(start_port: u16) -> std::io::Result<tokio::net::TcpListener> {
    let mut last_err = None;
    for port in start_port..start_port + 10 {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => return Ok(l),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                tracing::warn!("port {port} busy, trying {}", port + 1);
                last_err = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::AddrInUse, "no free port")))
}

fn init_logging(cfg: &config::Config) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("gemini_api=info"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_writer(std::io::stderr);

    let file_appender = tracing_appender::rolling::RollingFileAppender::new(
        tracing_appender::rolling::Rotation::NEVER,
        &cfg.data_dir,
        "gemini-api.log",
    );
    let file_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_ansi(false)
        .with_writer(file_appender);

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(file_layer)
        .try_init();
}
