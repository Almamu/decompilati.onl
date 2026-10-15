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
use tokio::sync::{mpsc, oneshot};

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
        // Weak, so Cloudflare keeps it when it re-encodes the body (it drops strong ETags).
        let etag = format!("W/\"{}\"", &sha256_hex(&body)[..20]);
        Arc::new(CachedPage { body, etag, content_type })
    }
}

/// Paths to purge from the edge, and who to tell once that's done.
struct Purge {
    paths: Vec<String>,
    done: Option<oneshot::Sender<()>>,
}

pub struct PageCache {
    pages: RwLock<HashMap<String, Arc<CachedPage>>>,
    purge_tx: mpsc::UnboundedSender<Purge>,
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

    /// Drops the given paths from the local cache and queues them for an edge purge, which
    /// is batched with others arriving shortly after.
    pub fn invalidate<I, S>(&self, paths: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let paths = self.drop_local(paths);
        let _ = self.purge_tx.send(Purge { paths, done: None });
    }

    /// Like `invalidate`, but purges right away and returns once the edge has been purged, so
    /// the person who made the change sees it on their next page load.
    pub async fn invalidate_now<I, S>(&self, paths: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let paths = self.drop_local(paths);
        let (tx, rx) = oneshot::channel();
        if self.purge_tx.send(Purge { paths, done: Some(tx) }).is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(15), rx).await;
        }
    }

    fn drop_local<I, S>(&self, paths: I) -> Vec<String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let paths: Vec<String> = paths.into_iter().map(Into::into).collect();
        let mut pages = self.pages.write();
        for p in &paths {
            pages.remove(p);
        }
        paths
    }

    /// Builds the response for a cached page, honouring `If-None-Match`.
    pub fn respond(&self, page: &CachedPage, req_headers: &HeaderMap) -> Response {
        // Browsers revalidate every time (a cheap 304 from the edge), so a purge is seen at once.
        let cache_control = format!(
            "public, max-age=0, must-revalidate, s-maxage={}, stale-if-error=86400",
            self.edge_ttl
        );
        let not_modified = req_headers
            .get(header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').any(|t| t.trim().trim_start_matches("W/") == page.etag.trim_start_matches("W/")));
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
        format!("/projects/{slug}/feed.xml"),
        format!("/badge/{slug}.svg"),
        format!("/badge/{slug}/matching.svg"),
        format!("/badge/{slug}/data.svg"),
        format!("/badge/{slug}/functions.svg"),
        format!("/badge/{slug}/linked.svg"),
        "/feed.xml".into(),
        "/sitemap.xml".into(),
    ]
}

async fn purge_worker(config: Config, mut rx: mpsc::UnboundedReceiver<Purge>) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("decompsite-cache-warmer")
        .build()
        .expect("http client");
    while let Some(first) = rx.recv().await {
        // Collect everything that arrives during the debounce window into one batch. Someone
        // waiting for the purge ends the window early.
        let mut waiters = Vec::new();
        let mut paths = BTreeSet::new();
        let mut take = |m: Purge, waiters: &mut Vec<oneshot::Sender<()>>| {
            paths.extend(m.paths);
            waiters.extend(m.done);
        };
        take(first, &mut waiters);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(config.purge_debounce_secs);
        while waiters.is_empty() {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(m)) => take(m, &mut waiters),
                _ => break,
            }
        }
        while let Ok(m) = rx.try_recv() {
            take(m, &mut waiters);
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
        // Done (or failed): release whoever is waiting.
        drop(waiters);
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

/// `project_paths` plus the page of the given version.
pub fn version_paths(slug: &str, platform: &str, version: &str) -> Vec<String> {
    let mut paths = project_paths(slug, platform);
    paths.push(crate::util::version_page(slug, version));
    paths
}
