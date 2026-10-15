use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use parking_lot::Mutex;

use crate::{cache::PageCache, config::Config, db::Db, views};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: Db,
    pub cache: Arc<PageCache>,
    pub http: reqwest::Client,
    pub limiter: Arc<RateLimiter>,
    /// Per-user cache of pushable GitHub repos: user id -> (fetched at, repos).
    pub repo_cache: Arc<Mutex<HashMap<i64, (i64, Vec<String>)>>>,
    /// History imports by project id (see `github_app::start_history_import`).
    pub imports: Arc<Mutex<HashMap<i64, crate::github_app::ImportStatus>>>,
}

impl AppState {
    pub fn new(config: Config, db: Db) -> anyhow::Result<Self> {
        let cache = PageCache::new(&config);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent("decompsite (+https://github.com)")
            .build()?;
        Ok(AppState {
            config: Arc::new(config),
            db,
            cache,
            http,
            limiter: Arc::new(RateLimiter::default()),
            repo_cache: Arc::default(),
            imports: Arc::default(),
        })
    }
}

/// Fixed-window rate limiter keyed by arbitrary strings.
#[derive(Default)]
pub struct RateLimiter {
    windows: Mutex<HashMap<String, (i64, u32)>>,
}

impl RateLimiter {
    /// Returns `true` if the call is allowed: at most `max` calls per `window` seconds.
    pub fn check(&self, key: &str, max: u32, window: i64) -> bool {
        let now = crate::util::now();
        let mut map = self.windows.lock();
        if map.len() > 50_000 {
            map.retain(|_, (start, _)| now - *start < 3600);
        }
        let entry = map.entry(key.to_string()).or_insert((now, 0));
        if now - entry.0 >= window {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= max
    }
}

/// Errors rendered as HTML pages.
#[derive(Debug)]
pub enum AppError {
    NotFound,
    BadRequest(String),
    Forbidden(String),
    Internal(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        AppError::Internal(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "Page not found.".to_string()),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            AppError::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            AppError::Internal(e) => {
                tracing::error!(error = ?e, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on our side.".to_string())
            }
        };
        let body = views::layout::error_page(status, &msg);
        (status, [(header::CACHE_CONTROL, "no-store")], body).into_response()
    }
}

/// Errors rendered as JSON (`{"error": "..."}`) for API endpoints.
#[derive(Debug)]
pub struct ApiError(pub StatusCode, pub String);

impl ApiError {
    pub fn bad(msg: impl Into<String>) -> Self {
        ApiError(StatusCode::BAD_REQUEST, msg.into())
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!(error = ?e, "internal api error");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, [(header::CACHE_CONTROL, "no-store")], Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
