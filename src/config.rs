//! Static configuration with environment overrides.
//!
//! Defaults mirror gemini-cli behavior (client ID, endpoints, user agent)
//! so the API is indistinguishable from the real Gemini CLI for Google.

use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone)]
pub struct Config {
    /// Port for the local API server.
    pub port: u16,
    /// Optional API key. When set, /v1/* endpoints require `Authorization: Bearer <key>`.
    pub api_key: Option<String>,
    /// Directory holding credentials.json and logs.
    pub data_dir: PathBuf,
    /// Optional: force the paid tier by using your own Google Cloud project.
    pub google_cloud_project: Option<String>,

    pub client_id: String,
    pub client_secret: String,
    pub scopes: Vec<String>,
    pub auth_url: String,
    pub token_url: String,
    pub userinfo_url: String,

    pub ca_endpoint: String,
    pub ca_api_version: String,
    pub cli_version: String,

    pub onboarding_poll_interval: Duration,
    pub onboarding_poll_timeout: Duration,

    pub response_timeout: Duration,
    pub ttft_timeout: Duration,
    pub idle_chunk_timeout: Duration,
    pub stream_deadline: Duration,
    pub keep_alive_interval: Duration,

    pub max_inflight: usize,

    pub default_model: String,
    pub models_available: Vec<String>,
    pub model_aliases: HashMap<String, String>,
}

impl Config {
    pub fn from_env() -> Self {
        let model_aliases = [
            ("gpt-4o", "gemini-2.5-pro"),
            ("gpt-4o-mini", "gemini-2.5-flash"),
            ("gpt-4.1", "gemini-2.5-pro"),
            ("gpt-4.1-mini", "gemini-2.5-flash"),
            ("gpt-5", "gemini-3-pro-preview"),
            ("gpt-5-mini", "gemini-2.5-flash"),
            ("o3", "gemini-2.5-pro"),
            ("o4-mini", "gemini-2.5-flash"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

        let data_dir = std::env::var("GEMINI_API_DATA_DIR")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::config_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("gemini-api")
            });

        Self {
            port: env_parse("PORT").unwrap_or(8080),
            api_key: std::env::var("GEMINI_API_KEY").ok().filter(|s| !s.is_empty()),
            google_cloud_project: std::env::var("GOOGLE_CLOUD_PROJECT")
                .ok()
                .filter(|s| !s.is_empty()),
            data_dir,

            // Public "installed app" client used by the real Gemini CLI
            // (secret is public by design for this app class).
            client_id: "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com"
                .to_string(),
            client_secret: "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".to_string(),
            scopes: [
                "https://www.googleapis.com/auth/cloud-platform",
                "https://www.googleapis.com/auth/userinfo.email",
                "https://www.googleapis.com/auth/userinfo.profile",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            auth_url: env_or("GEMINI_AUTH_URL", "https://accounts.google.com/o/oauth2/v2/auth"),
            token_url: env_or("GEMINI_TOKEN_URL", "https://oauth2.googleapis.com/token"),
            userinfo_url: env_or("GEMINI_USERINFO_URL", "https://www.googleapis.com/oauth2/v2/userinfo"),

            ca_endpoint: env_or("CODE_ASSIST_ENDPOINT", "https://cloudcode-pa.googleapis.com"),
            ca_api_version: env_or("CODE_ASSIST_API_VERSION", "v1internal"),
            cli_version: env_or("GEMINI_CLI_VERSION", "0.16.0"),

            onboarding_poll_interval: Duration::from_secs(5),
            onboarding_poll_timeout: Duration::from_secs(120),

            response_timeout: Duration::from_secs(120),
            ttft_timeout: Duration::from_secs(60),
            idle_chunk_timeout: Duration::from_secs(120),
            stream_deadline: Duration::from_secs(600),
            keep_alive_interval: Duration::from_secs(15),

            max_inflight: env_parse("GEMINI_API_MAX_INFLIGHT").unwrap_or(10),

            default_model: "gemini-2.5-flash".to_string(),
            models_available: [
                "gemini-2.5-pro",
                "gemini-2.5-flash",
                "gemini-2.5-flash-lite",
                "gemini-3-pro-preview",
                "gemini-2.0-flash",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            model_aliases,
        }
    }

    /// `GeminiCLI/<cliVersion>/<model> (linux; x86_64; cli)` — identical to gemini-cli.
    pub fn user_agent(&self, model: Option<&str>) -> String {
        format!(
            "GeminiCLI/{}/{} (linux; x86_64; cli)",
            self.cli_version,
            model.unwrap_or(&self.default_model),
        )
    }

    pub fn ca_base_url(&self) -> String {
        format!("{}/{}", self.ca_endpoint, self.ca_api_version)
    }

    pub fn ca_method_url(&self, method: &str) -> String {
        format!("{}:{}", self.ca_base_url(), method)
    }

    pub fn scopes_joined(&self) -> String {
        self.scopes.join(" ")
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| default.to_string())
}

fn env_parse<T: std::str::FromStr>(key: &str) -> Option<T> {
    std::env::var(key).ok().and_then(|v| v.parse().ok())
}

pub fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
