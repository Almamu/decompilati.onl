//! Static assets compiled into the binary and served with immutable caching.

use std::sync::LazyLock;

use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

pub struct Asset {
    pub name: &'static str,
    pub body: &'static str,
    pub content_type: &'static str,
    pub version: String,
}

static ASSETS: LazyLock<Vec<Asset>> = LazyLock::new(|| {
    [
        ("app.css", include_str!("../static/app.css"), "text/css; charset=utf-8"),
        ("app.js", include_str!("../static/app.js"), "text/javascript; charset=utf-8"),
        ("favicon.svg", include_str!("../static/favicon.svg"), "image/svg+xml"),
    ]
    .into_iter()
    .map(|(name, body, content_type)| Asset {
        name,
        body,
        content_type,
        version: crate::util::sha256_hex(body)[..12].to_string(),
    })
    .collect()
});

/// Versioned URL for an asset, e.g. `/static/3fa2c1d0e9b8/app.css`.
pub fn url(name: &str) -> String {
    let a = ASSETS.iter().find(|a| a.name == name).expect("unknown asset");
    format!("/static/{}/{}", a.version, a.name)
}

pub async fn serve(Path((version, name)): Path<(String, String)>) -> Response {
    match ASSETS.iter().find(|a| a.name == name) {
        Some(a) => {
            // An outdated version still gets the current file, but only briefly cached.
            let cc = if a.version == version { "public, max-age=31536000, immutable" } else { "public, max-age=300" };
            ([(header::CONTENT_TYPE, a.content_type), (header::CACHE_CONTROL, cc)], a.body).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
