//! Shared application state + the local credentials store (single user, no DB).

use crate::config::Config;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, sync::Arc};
use tokio::sync::{watch, Mutex, RwLock, Semaphore};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionStatus {
    #[serde(rename = "ACTIVE")]
    Active,
    #[serde(rename = "NEEDS_VALIDATION")]
    NeedsValidation,
    #[serde(rename = "ERROR")]
    Error,
    #[serde(rename = "REVOKED")]
    Revoked,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionStatus::Active => "ACTIVE",
            SessionStatus::NeedsValidation => "NEEDS_VALIDATION",
            SessionStatus::Error => "ERROR",
            SessionStatus::Revoked => "REVOKED",
        }
    }
}

/// Everything persisted in `<data_dir>/credentials.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub email: String,
    pub refresh_token: String,
    pub access_token: Option<String>,
    /// Unix seconds. Validity is checked with a 60s margin.
    pub token_expires_at: Option<i64>,
    pub project_id: Option<String>,
    pub tier: Option<String>,
    pub onboarded: bool,
    pub status: SessionStatus,
    pub message: Option<String>,
}

impl Session {
    pub fn token_valid(&self) -> bool {
        match (&self.access_token, self.token_expires_at) {
            (Some(_), Some(exp)) => exp > crate::config::now_unix() + 60,
            _ => false,
        }
    }
}

pub struct App {
    pub config: Config,
    pub http: reqwest::Client,
    pub session: RwLock<Option<Session>>,
    /// Actual bound port (may differ from config.port via fallback).
    pub port: std::sync::atomic::AtomicU16,
    /// Single-flight guards.
    pub refresh_lock: Mutex<()>,
    pub login_lock: Mutex<()>,
    pub onboarding_lock: Mutex<()>,
    /// Bulkhead: max concurrent chat completions.
    pub inflight: Arc<Semaphore>,
    pub shutdown: watch::Sender<bool>,
}

impl App {
    pub fn base_url(&self) -> String {
        use std::sync::atomic::Ordering;
        format!("http://localhost:{}", self.port.load(Ordering::Relaxed))
    }
}

pub type SharedApp = Arc<App>;

impl App {
    pub fn new(config: Config) -> SharedApp {
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .build()
            .expect("failed to build http client");
        let (shutdown, _) = watch::channel(false);
        Arc::new(Self {
            inflight: Arc::new(Semaphore::new(config.max_inflight)),
            port: std::sync::atomic::AtomicU16::new(config.port),
            config,
            http,
            session: RwLock::new(None),
            refresh_lock: Mutex::new(()),
            login_lock: Mutex::new(()),
            onboarding_lock: Mutex::new(()),
            shutdown,
        })
    }

    pub fn credentials_path(&self) -> PathBuf {
        self.config.data_dir.join("credentials.json")
    }

    /// Loads credentials.json if present. Directory is created with 0700.
    pub fn load_session_from_disk(&self) -> Option<Session> {
        let path = self.credentials_path();
        let data = fs::read_to_string(path).ok()?;
        match serde_json::from_str::<Session>(&data) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!("credentials.json unreadable ({e}); ignoring");
                None
            }
        }
    }

    /// Atomically writes credentials.json with 0600 permissions.
    pub fn save_session_to_disk(&self, session: &Session) {
        if let Err(e) = save_credentials(self.credentials_path(), session) {
            tracing::error!("failed to save credentials: {e}");
        }
    }

    pub async fn set_session(&self, session: Session) {
        self.save_session_to_disk(&session);
        *self.session.write().await = Some(session);
    }
}

fn save_credentials(path: PathBuf, session: &Session) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    fs::create_dir_all(dir)?;
    set_perms(dir.to_path_buf(), 0o700);

    let tmp = dir.join(".credentials.json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(session).unwrap_or_default())?;
    set_perms(tmp.clone(), 0o600);
    fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(unix)]
fn set_perms(path: PathBuf, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_perms(_path: PathBuf, _mode: u32) {}
