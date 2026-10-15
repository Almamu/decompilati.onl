//! Public, edge-cacheable pages. These must never depend on the visitor (no cookies read,
//! none set): user-specific bits are loaded client-side from `/api/me`.

use std::{collections::HashMap, future::Future, sync::Arc};

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

use crate::{
    app::{AppError, AppResult, AppState},
    cache::CachedPage,
    db::{self, Project},
    platforms::{self, PLATFORMS},
    util::{now, sha256_hex},
    views::{self, layout::Page},
};

const HTML: &str = "text/html; charset=utf-8";

/// Serves `key` from the in-memory cache, rendering it on a miss.
async fn cached<F, Fut>(state: &AppState, headers: &HeaderMap, key: String, render: F) -> AppResult<Response>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = AppResult<Arc<CachedPage>>>,
{
    if let Some(page) = state.cache.get(&key) {
        return Ok(state.cache.respond(&page, headers));
    }
    let page = render().await?;
    state.cache.put(key, page.clone());
    Ok(state.cache.respond(&page, headers))
}

fn is_visible(p: &Project, threshold: i64) -> bool {
    p.score() > threshold
}

/// Default ordering: negatively rated projects sink, then most recently updated first.
fn hot_sort(list: &mut [&Project]) {
    list.sort_by_key(|p| (p.score() < 0, -(p.last_report_at.unwrap_or(p.created_at))));
}

fn og_image(state: &AppState, p: &Project) -> Option<String> {
    p.image.as_ref().map(|i| format!("{}/uploads/{i}", state.config.base_url))
}

pub async fn home(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    cached(&state, &headers, "/".into(), || async {
        let since = now() - 30 * 86400;
        let (all, baselines) = state
            .db
            .run(move |c| Ok((db::public_projects(c)?, db::baselines_since(c, since)?)))
            .await?;
        let threshold = state.config.hide_threshold;
        let mut visible: Vec<&Project> = all.iter().filter(|p| is_visible(p, threshold)).collect();
        hot_sort(&mut visible);

        let mut spotlight: Vec<&Project> = visible.iter().copied().filter(|p| p.spotlight).take(3).collect();
        if spotlight.is_empty() {
            // No curated picks: feature the best-rated project that's been active recently.
            spotlight = visible
                .iter()
                .copied()
                .filter(|p| p.score() > 0 && p.last_report_at.is_some_and(|t| t >= since))
                .max_by_key(|p| p.score())
                .into_iter()
                .collect();
        }

        let base: HashMap<i64, _> = baselines.into_iter().collect();
        let mut movers: Vec<(&Project, f64)> = visible
            .iter()
            .filter(|p| p.score() >= 0)
            .filter_map(|p| base.get(&p.id).map(|b| (*p, p.code.decompiled_pct() - b.decompiled_pct())))
            .filter(|(_, gain)| *gain >= 0.01)
            .collect();
        movers.sort_by(|a, b| b.1.total_cmp(&a.1));
        movers.truncate(6);

        let recent: Vec<&Project> = visible.iter().copied().take(24).collect();
        let html = views::layout::render(Page {
            title: "",
            description: "Progress tracker for video game decompilation projects.",
            image: None,
            body: views::home::home(&spotlight, &movers, &recent, visible.len()),
        });
        Ok(CachedPage::new(html, HTML))
    })
    .await
}

async fn listing_page(state: &AppState, headers: &HeaderMap, key: String, platform: Option<&'static platforms::Platform>) -> AppResult<Response> {
    cached(state, headers, key, || async {
        let all = state.db.run(db::public_projects_owned).await?;
        let threshold = state.config.hide_threshold;
        let mut shown: Vec<&Project> = all.iter().filter(|p| platform.is_none_or(|pl| p.platform == pl.slug)).collect();
        hot_sort(&mut shown);
        let (visible, hidden): (Vec<&Project>, Vec<&Project>) = shown.into_iter().partition(|p| is_visible(p, threshold));
        let (title, intro, desc) = match platform {
            Some(pl) => (pl.name.to_string(), None, format!("Decompilation projects for the {}.", pl.name)),
            None => ("All projects".to_string(), None, "Every decompilation project tracked on the site.".to_string()),
        };
        let html = views::layout::render(Page {
            title: &title,
            description: &desc,
            image: None,
            body: views::home::listing(&title, intro, &visible, &hidden, platform.map(|p| p.slug)),
        });
        Ok(CachedPage::new(html, HTML))
    })
    .await
}

pub async fn projects(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    listing_page(&state, &headers, "/projects".into(), None).await
}

pub async fn platform(State(state): State<AppState>, headers: HeaderMap, Path(slug): Path<String>) -> AppResult<Response> {
    let pl = platforms::get(&slug).ok_or(AppError::NotFound)?;
    listing_page(&state, &headers, format!("/platform/{}", pl.slug), Some(pl)).await
}

pub async fn platforms_index(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    cached(&state, &headers, "/platforms".into(), || async {
        let all = state.db.run(db::public_projects_owned).await?;
        let threshold = state.config.hide_threshold;
        let counts: Vec<_> = PLATFORMS
            .iter()
            .map(|pl| (pl, all.iter().filter(|p| p.platform == pl.slug && is_visible(p, threshold)).count()))
            .collect();
        let html = views::layout::render(Page {
            title: "Platforms",
            description: "Decompilation projects by platform.",
            image: None,
            body: views::home::platforms_page(&counts),
        });
        Ok(CachedPage::new(html, HTML))
    })
    .await
}

async fn load_public_project(state: &AppState, slug: String) -> AppResult<Project> {
    state.db.run(move |c| db::project_by_slug(c, &slug)).await?.ok_or(AppError::NotFound)
}

pub async fn project(State(state): State<AppState>, headers: HeaderMap, Path(slug): Path<String>) -> AppResult<Response> {
    cached(&state, &headers, format!("/p/{slug}"), || async {
        let p = load_public_project(&state, slug).await?;
        let pid = p.id;
        let (reports, owner_banned) = state
            .db
            .run(move |c| {
                let banned: bool = c.query_row(
                    "SELECT u.is_banned FROM users u JOIN projects p ON p.owner_id = u.id WHERE p.id = ?1",
                    [pid],
                    |r| r.get(0),
                )?;
                Ok((db::reports_for_project(c, pid, 500)?, banned))
            })
            .await?;
        if owner_banned {
            return Err(AppError::NotFound);
        }
        let desc = format!(
            "{} decompilation: {} code decompiled ({} matching).",
            p.name,
            crate::util::fmt_pct(p.code.decompiled_pct()),
            crate::util::fmt_pct(p.code.matching_pct())
        );
        let hidden = !is_visible(&p, state.config.hide_threshold);
        let html = views::layout::render(Page {
            title: &p.name,
            description: &desc,
            image: og_image(&state, &p),
            body: views::project::project(&p, &reports, &state.config.base_url, hidden),
        });
        Ok(CachedPage::new(html, HTML))
    })
    .await
}

pub async fn project_json(State(state): State<AppState>, headers: HeaderMap, Path(slug): Path<String>) -> AppResult<Response> {
    cached(&state, &headers, format!("/p/{slug}/progress.json"), || async {
        let p = load_public_project(&state, slug).await?;
        let body = serde_json::json!({
            "slug": p.slug,
            "name": p.name,
            "platform": p.platform,
            "repo": p.repo,
            "ai_generated": p.ai_generated,
            "unit": p.unit,
            "code": p.code,
            "data": p.data,
            "code_matching_pct": p.code.matching_pct(),
            "code_decompiled_pct": p.code.decompiled_pct(),
            "data_pct": p.data.pct(),
            "areas": p.areas(),
            "last_report_at": p.last_report_at.map(crate::util::iso_time),
            "last_commit": p.last_commit,
            "last_message": p.last_message,
            "updated_at": crate::util::iso_time(p.updated_at),
            "score": p.score(),
        });
        Ok(CachedPage::new(serde_json::to_vec(&body)?, "application/json"))
    })
    .await
}

pub async fn badge(State(state): State<AppState>, headers: HeaderMap, Path(file): Path<String>) -> AppResult<Response> {
    let slug = file.strip_suffix(".svg").ok_or(AppError::NotFound)?.to_string();
    cached(&state, &headers, format!("/badge/{slug}.svg"), || async {
        let p = load_public_project(&state, slug).await?;
        Ok(CachedPage::new(views::svg::badge("decompiled", p.code.decompiled_pct()), "image/svg+xml"))
    })
    .await
}

pub async fn docs(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    cached(&state, &headers, "/docs".into(), || async {
        let html = views::layout::render(Page {
            title: "Progress API",
            description: "How to report decompilation progress from CI.",
            image: None,
            body: views::docs::docs(&state.config.base_url),
        });
        Ok(CachedPage::new(html, HTML))
    })
    .await
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub async fn feed(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    cached(&state, &headers, "/feed.xml".into(), || async {
        let all = state.db.run(db::public_projects_owned).await?;
        let mut list: Vec<&Project> = all
            .iter()
            .filter(|p| p.last_report_at.is_some() && is_visible(p, state.config.hide_threshold))
            .collect();
        list.sort_by_key(|p| -p.last_report_at.unwrap_or(0));
        let base = &state.config.base_url;
        let mut xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><rss version="2.0"><channel><title>decompilati.onl updates</title><link>{base}/</link><description>Recent decompilation progress</description>"#
        );
        for p in list.into_iter().take(50) {
            let ts = p.last_report_at.unwrap_or(0);
            let date = time::OffsetDateTime::from_unix_timestamp(ts)
                .ok()
                .and_then(|t| t.format(&time::format_description::well_known::Rfc2822).ok())
                .unwrap_or_default();
            let title = format!("{}: {} code decompiled", p.name, crate::util::fmt_pct(p.code.decompiled_pct()));
            xml.push_str(&format!(
                "<item><title>{}</title><link>{base}/p/{}</link><guid isPermaLink=\"false\">{}-{ts}</guid><pubDate>{date}</pubDate><description>{}</description></item>",
                xml_escape(&title),
                p.slug,
                p.slug,
                xml_escape(p.last_message.as_deref().unwrap_or(""))
            ));
        }
        xml.push_str("</channel></rss>");
        Ok(CachedPage::new(xml, "application/rss+xml; charset=utf-8"))
    })
    .await
}

pub async fn robots() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain"), (header::CACHE_CONTROL, "public, max-age=86400")],
        "User-agent: *\nDisallow: /dashboard\nDisallow: /admin\nDisallow: /auth\nDisallow: /api\n",
    )
}

pub async fn upload(State(state): State<AppState>, Path(file): Path<String>, headers: HeaderMap) -> Response {
    let valid = file
        .split_once('.')
        .is_some_and(|(name, ext)| name.len() >= 16 && name.bytes().all(|b| b.is_ascii_hexdigit()) && ext == "jpg");
    if !valid {
        return StatusCode::NOT_FOUND.into_response();
    }
    // File names are content hashes, so the ETag is just the name.
    let etag = format!("\"{}\"", &sha256_hex(&file)[..16]);
    if headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    match tokio::fs::read(state.config.uploads_dir().join(&file)).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "image/jpeg".to_string()),
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable".to_string()),
                (header::ETAG, etag),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
