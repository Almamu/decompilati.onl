//! Two cache layers:
//!
//! 1. An in-process cache of rendered public responses, so Cloudflare cache misses are
//!    served from memory without touching the database.
//! 2. Cloudflare's edge cache. Public pages are sent with a long `s-maxage`; whenever data
//!    changes, the affected URLs are queued, purged through the Cloudflare API in debounced
//!    batches, and then re-requested so the edge fetches the fresh version right away.

use std::{collections::{BTreeSet, HashMap}, sync::Arc, time::Duration};

use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use parking_lot::RwLock;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::util::sha256_hex;

pub struct CachedPage {
    pub body: Bytes,
    pub etag: String,
    pub content_type: &'static str,
}

impl CachedPage {
    pub fn new(body: impl Into<Bytes>, content_type: &'static str) -> Arc<Self> {
        let body = body.into();
        let etag = format!("\"{}\"", &sha256_hex(&body)[..20]);
        Arc::new(CachedPage { body, etag, content_type })
    }
}

pub struct PageCache {
    pages: RwLock<HashMap<String, Arc<CachedPage>>>,
    purge_tx: mpsc::UnboundedSender<String>,
    /// Edge TTL. Long when we can purge Cloudflare, short otherwise.
    edge_ttl: u64,
}

impl PageCache {
    pub fn new(config: &Config) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let cache = Arc::new(PageCache {
            pages: RwLock::new(HashMap::new()),
            purge_tx: tx,
            edge_ttl: if config.cloudflare.is_some() { 30 * 86400 } else { 60 },
        });
        tokio::spawn(purge_worker(config.clone(), rx));
        cache
    }

    pub fn get(&self, key: &str) -> Option<Arc<CachedPage>> {
        self.pages.read().get(key).cloned()
    }

    pub fn put(&self, key: String, page: Arc<CachedPage>) {
        let mut pages = self.pages.write();
        // Bound memory: the key space is small (pages per project), but stay safe.
        if pages.len() > 20_000 {
            pages.clear();
        }
        pages.insert(key, page);
    }

    /// Drops the given paths from the local cache and queues them for an edge purge.
    pub fn invalidate<I, S>(&self, paths: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let paths: Vec<String> = paths.into_iter().map(Into::into).collect();
        {
            let mut pages = self.pages.write();
            for p in &paths {
                pages.remove(p);
            }
        }
        for p in paths {
            let _ = self.purge_tx.send(p);
        }
    }

    /// Builds the response for a cached page, honouring `If-None-Match`.
    pub fn respond(&self, page: &CachedPage, req_headers: &HeaderMap) -> Response {
        let cache_control = format!(
            "public, max-age=60, s-maxage={}, stale-while-revalidate=60, stale-if-error=86400",
            self.edge_ttl
        );
        let not_modified = req_headers
            .get(header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').any(|t| t.trim().trim_start_matches("W/") == page.etag));
        let mut res = if not_modified {
            StatusCode::NOT_MODIFIED.into_response()
        } else {
            (
                [(header::CONTENT_TYPE, HeaderValue::from_static(page.content_type))],
                page.body.clone(),
            )
                .into_response()
        };
        let h = res.headers_mut();
        h.insert(header::CACHE_CONTROL, HeaderValue::from_str(&cache_control).unwrap());
        h.insert(header::ETAG, HeaderValue::from_str(&page.etag).unwrap());
        h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
        res
    }
}

/// Paths whose content depends on a project. Listing pages change whenever any project does.
pub fn project_paths(slug: &str, platform: &str) -> Vec<String> {
    vec![
        "/".into(),
        "/projects".into(),
        "/highlights".into(),
        "/platforms".into(),
        format!("/platform/{platform}"),
        format!("/projects/{slug}"),
        format!("/projects/{slug}/progress.json"),
        format!("/badge/{slug}.svg"),
        format!("/badge/{slug}/matching.svg"),
        format!("/badge/{slug}/data.svg"),
        format!("/badge/{slug}/functions.svg"),
        "/feed.xml".into(),
        "/sitemap.xml".into(),
    ]
}

async fn purge_worker(config: Config, mut rx: mpsc::UnboundedReceiver<String>) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("decompsite-cache-warmer")
        .build()
        .expect("http client");
    while let Some(first) = rx.recv().await {
        // Collect everything that arrives during the debounce window into one batch.
        tokio::time::sleep(Duration::from_secs(config.purge_debounce_secs)).await;
        let mut paths = BTreeSet::from([first]);
        while let Ok(p) = rx.try_recv() {
            paths.insert(p);
        }
        let Some(cf) = &config.cloudflare else {
            tracing::debug!("cloudflare not configured; skipping purge of {} paths", paths.len());
            continue;
        };
        let urls: Vec<String> = paths.iter().map(|p| format!("{}{}", config.base_url, p)).collect();
        let mut purged_ok = true;
        // Free plans accept up to 30 URLs per purge request.
        for chunk in urls.chunks(30) {
            let res = client
                .post(format!("https://api.cloudflare.com/client/v4/zones/{}/purge_cache", cf.zone_id))
                .bearer_auth(&cf.api_token)
                .json(&serde_json::json!({ "files": chunk }))
                .send()
                .await;
            match res {
                Ok(r) if r.status().is_success() => {}
                Ok(r) => {
                    purged_ok = false;
                    let status = r.status();
                    let body = r.text().await.unwrap_or_default();
                    tracing::warn!(%status, %body, "cloudflare purge failed");
                }
                Err(e) => {
                    purged_ok = false;
                    tracing::warn!(error = %e, "cloudflare purge request failed");
                }
            }
        }
        tracing::info!(count = urls.len(), ok = purged_ok, "purged cloudflare cache");
        if !purged_ok {
            continue;
        }
        // Re-request the URLs through Cloudflare so the edge caches the fresh copy.
        for url in urls {
            let client = client.clone();
            tokio::spawn(async move {
                if let Err(e) = client.get(&url).header("Accept-Encoding", "br, gzip").send().await {
                    tracing::debug!(error = %e, %url, "cache warm failed");
                }
            });
        }
    }
}

/// `project_paths` plus every platform page: used when a project appears, disappears or
/// changes platform, since the platform chips on every listing only show platforms in use.
pub fn membership_paths(slug: &str, platform: &str) -> Vec<String> {
    let mut paths = project_paths(slug, platform);
    paths.extend(crate::platforms::PLATFORMS.iter().map(|p| format!("/platform/{}", p.slug)));
    paths
}

/// `project_paths` plus the page of the given version (named versions have their own page).
pub fn version_paths(slug: &str, platform: &str, version: &str) -> Vec<String> {
    let mut paths = project_paths(slug, platform);
    if !version.is_empty() {
        paths.push(format!("/projects/{slug}/versions/{version}"));
    }
    paths
}
