use std::{env, net::SocketAddr, path::PathBuf};

/// Runtime configuration, read from environment variables (see `.env.example`).
#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    /// Public base URL, without trailing slash (e.g. `https://decompilati.onl`).
    pub base_url: String,
    pub database_path: PathBuf,
    pub data_dir: PathBuf,
    pub github_client_id: String,
    pub github_client_secret: String,
    /// Optional token for GitHub API calls made by the server (commit lists); raises the
    /// unauthenticated rate limit of 60 requests/hour.
    pub github_api_token: Option<String>,
    pub cloudflare: Option<CloudflareConfig>,
    /// GitHub App for decomp.dev-style artifact imports (optional).
    pub github_app: Option<crate::github_app::GithubAppConfig>,
    /// GitHub logins with admin rights (spotlight, hide, ban).
    pub admins: Vec<String>,
    /// Projects whose vote score is at or below this value are hidden from listings.
    pub hide_threshold: i64,
    /// Seconds to wait collecting changes before purging Cloudflare.
    pub purge_debounce_secs: u64,
    pub secure_cookies: bool,
}

#[derive(Clone, Debug)]
pub struct CloudflareConfig {
    pub api_token: String,
    pub zone_id: String,
}

fn var(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.trim().is_empty())
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let base_url = var("BASE_URL")
            .unwrap_or_else(|| "http://localhost:8080".into())
            .trim_end_matches('/')
            .to_string();
        let data_dir = PathBuf::from(var("DATA_DIR").unwrap_or_else(|| "./data".into()));
        let cloudflare = match (var("CF_API_TOKEN"), var("CF_ZONE_ID")) {
            (Some(api_token), Some(zone_id)) => Some(CloudflareConfig { api_token, zone_id }),
            _ => None,
        };
        let github_app = match (var("GITHUB_APP_ID"), var("GITHUB_APP_SLUG"), var("GITHUB_APP_WEBHOOK_SECRET"), var("GITHUB_APP_PRIVATE_KEY_FILE")) {
            (Some(app_id), Some(slug), Some(webhook_secret), Some(key_file)) => Some(crate::github_app::GithubAppConfig {
                app_id,
                slug,
                webhook_secret,
                private_key_pem: std::fs::read_to_string(&key_file)
                    .map_err(|e| anyhow::anyhow!("reading GITHUB_APP_PRIVATE_KEY_FILE {key_file}: {e}"))?,
            }),
            _ => None,
        };
        Ok(Config {
            bind: var("BIND").unwrap_or_else(|| "127.0.0.1:8080".into()).parse()?,
            secure_cookies: base_url.starts_with("https://"),
            base_url,
            database_path: var("DATABASE_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| data_dir.join("decompsite.db")),
            data_dir,
            github_client_id: var("GITHUB_CLIENT_ID").unwrap_or_default(),
            github_client_secret: var("GITHUB_CLIENT_SECRET").unwrap_or_default(),
            github_api_token: var("GITHUB_API_TOKEN"),
            cloudflare,
            github_app,
            admins: var("ADMIN_LOGINS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
            hide_threshold: var("HIDE_THRESHOLD").and_then(|v| v.parse().ok()).unwrap_or(-5),
            purge_debounce_secs: var("PURGE_DEBOUNCE_SECS").and_then(|v| v.parse().ok()).unwrap_or(10),
        })
    }

    pub fn uploads_dir(&self) -> PathBuf {
        self.data_dir.join("uploads")
    }

    pub fn is_admin(&self, login: &str) -> bool {
        self.admins.iter().any(|a| a.eq_ignore_ascii_case(login))
    }
}
