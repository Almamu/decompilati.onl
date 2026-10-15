mod app;
mod assets;
mod auth;
mod cache;
mod config;
mod github;
mod github_app;
mod db;
mod images;
mod objdiff;
mod platforms;
mod report;
mod routes;
mod seed;
mod util;
mod views;

use std::time::Duration;

use axum::{
    extract::DefaultBodyLimit,
    http::{header, HeaderValue, StatusCode},
    routing::{get, post},
    Router,
};
use tower_http::{
    compression::CompressionLayer, set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer, trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;

use crate::{app::AppState, routes::{api, dashboard, public}};

fn router(state: AppState) -> Router {
    let public = Router::new()
        .route("/", get(public::home))
        .route("/projects", get(public::projects))
        .route("/platforms", get(public::platforms_index))
        .route("/platform/{slug}", get(public::platform))
        .route("/projects/{slug}", get(public::project))
        .route("/projects/{slug}/progress.json", get(public::project_json))
        .route("/projects/{slug}/versions/{version}", get(public::project_version))
        // Old project URLs, kept working for links and badges shared before the move.
        .route("/p/{*rest}", get(public::old_project_url))
        .route("/badge/{file}", get(public::badge))
        .route("/badge/{slug}/{kind}", get(public::badge_kind))
        .route("/docs", get(public::docs))
        .route("/docs/{slug}", get(public::docs_page))
        .route("/feed.xml", get(public::feed))
        .route("/robots.txt", get(public::robots))
        .route("/sitemap.xml", get(public::sitemap))
        .route("/uploads/{file}", get(public::upload))
        .route("/static/{version}/{name}", get(assets::serve));

    let private = Router::new()
        .route("/auth/github", get(auth::login))
        .route("/auth/github/callback", get(auth::callback))
        .route("/auth/logout", post(auth::logout))
        .route("/dashboard", get(dashboard::index))
        .route(
            "/dashboard/new",
            get(dashboard::new_form).post(dashboard::create).layer(DefaultBodyLimit::max(images::MAX_UPLOAD + 64 * 1024)),
        )
        .route("/dashboard/p/{id}", get(dashboard::edit).post(dashboard::update))
        .route(
            "/dashboard/p/{id}/image",
            post(dashboard::upload_image).layer(DefaultBodyLimit::max(images::MAX_UPLOAD + 64 * 1024)),
        )
        .route("/dashboard/p/{id}/key", post(dashboard::regenerate_key))
        .route("/dashboard/p/{id}/import-history", post(dashboard::import_history))
        .route("/dashboard/p/{id}/delete", post(dashboard::delete))
        .route("/admin", get(dashboard::admin))
        .route("/admin/p/{id}/spotlight", post(dashboard::admin_spotlight))
        .route("/admin/p/{id}/hide", post(dashboard::admin_hide))
        .route("/admin/u/{id}/ban", post(dashboard::admin_ban))
        .route("/api/me", get(api::me))
        .route("/api/github/webhook", post(github_app::webhook).layer(DefaultBodyLimit::max(8 * 1024 * 1024)))
        .route("/api/v1/vote", post(api::vote))
        // objdiff reports can be large (they may also arrive gzip-compressed).
        .route(
            "/api/v1/progress",
            post(api::progress).layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
        .layer(SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store")));

    Router::new()
        .merge(public)
        .merge(private)
        .fallback(|| async { app::AppError::NotFound })
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(CompressionLayer::new())
        .layer(SetResponseHeaderLayer::if_not_present(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::if_not_present(header::REFERRER_POLICY, HeaderValue::from_static("strict-origin-when-cross-origin")))
        .layer(SetResponseHeaderLayer::if_not_present(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY")))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; img-src 'self' https://avatars.githubusercontent.com data:; style-src 'self' 'unsafe-inline'; script-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self' https://github.com",
            ),
        ))
        .layer(TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, Duration::from_secs(30)))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=warn".into()))
        .init();

    let config = config::Config::from_env()?;
    if config.github_client_id.is_empty() {
        tracing::warn!("GITHUB_CLIENT_ID is not set: login is disabled");
    }
    if config.cloudflare.is_none() {
        tracing::warn!("CF_API_TOKEN / CF_ZONE_ID not set: edge cache purging is disabled (short edge TTLs are used)");
    }
    std::fs::create_dir_all(config.uploads_dir())?;
    let db = db::Db::open(&config.database_path)?;
    if std::env::args().nth(1).as_deref() == Some("seed-demo") {
        return seed::run(&db, &config.uploads_dir());
    }
    views::layout::set_base_url(&config.base_url);
    let bind = config.bind;
    let state = AppState::new(config, db)?;

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("listening on http://{bind}");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
