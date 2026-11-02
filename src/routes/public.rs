//! Public, edge-cacheable pages. These must never depend on the visitor (no cookies read,
//! none set): user-specific bits are loaded client-side from `/api/me`.

use std::{collections::HashMap, future::Future, sync::Arc};

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

use crate::{
    app::{AppError, AppResult, AppState},
    cache::CachedPage,
    db::{self, Project},
    platforms::{self, PLATFORMS},
    util::sha256_hex,
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
        let all = state.db.run(db::public_projects_owned).await?;
        let threshold = state.config.hide_threshold;
        let mut visible: Vec<&Project> = all.iter().filter(|p| is_visible(p, threshold)).collect();
        hot_sort(&mut visible);

        let html = views::layout::render(Page {
            title: "",
            description: &format!(
                "Track the progress of {} video game decompilation projects: matching and partially matching code, data, areas and recent changes, reported by each project's CI.",
                visible.len()
            ),
            image: None,
            body: views::home::home(&visible, visible.len()),
            canonical: Some("/".into()),
            json_ld: Some(serde_json::json!({
                "@context": "https://schema.org",
                "@type": "WebSite",
                "name": views::layout::SITE_NAME,
                "url": format!("{}/", state.config.base_url),
                "description": "Progress tracker for video game decompilation projects.",
            })),
            og_type: None,
            feed: None,
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
        // The platform chips cover every listed project, not just this page's.
        let listed: Vec<&Project> = all.iter().filter(|p| is_visible(p, threshold)).collect();
        let (title, intro, desc) = match platform {
            Some(pl) => (pl.name.to_string(), None, format!("Decompilation projects for the {}.", pl.name)),
            None => ("All projects".to_string(), None, "Every decompilation project tracked on the site.".to_string()),
        };
        let html = views::layout::render(Page {
            title: &title,
            description: &desc,
            image: None,
            body: views::home::listing(&title, intro, &visible, &hidden, &listed, platform.map(|p| p.slug)),
            canonical: Some(platform.map_or("/projects".to_string(), |p| format!("/platform/{}", p.slug))),
            ..Default::default()
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
            canonical: Some("/platforms".into()),
            ..Default::default()
        });
        Ok(CachedPage::new(html, HTML))
    })
    .await
}

/// `GET /ports`: every listed project's ports, grouped by project.
pub async fn ports(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    cached(&state, &headers, "/ports".into(), || async {
        let (projects, ports) = state.db.run(|c| Ok((db::public_projects_owned(c)?, db::all_ports(c)?))).await?;
        let threshold = state.config.hide_threshold;
        let mut groups: Vec<(&Project, Vec<db::Port>)> = projects
            .iter()
            .filter(|p| is_visible(p, threshold))
            .map(|p| (p, ports.iter().filter(|(pid, _)| *pid == p.id).map(|(_, port)| port.clone()).collect::<Vec<_>>()))
            .filter(|(_, ports)| !ports.is_empty())
            .collect();
        groups.sort_by_key(|(p, _)| p.name.to_lowercase());
        let html = views::layout::render(Page {
            title: "Ports",
            description: "Native versions and other ports built from video game decompilation projects.",
            image: None,
            body: views::home::ports_page(&groups),
            canonical: Some("/ports".into()),
            ..Default::default()
        });
        Ok(CachedPage::new(html, HTML))
    })
    .await
}

async fn load_public_project(state: &AppState, slug: String) -> AppResult<Project> {
    state.db.run(move |c| db::project_by_slug(c, &slug)).await?.ok_or(AppError::NotFound)
}

#[derive(serde::Deserialize, Default)]
pub struct PageQuery {
    page: Option<usize>,
}

impl PageQuery {
    fn page(&self) -> usize {
        self.page.unwrap_or(1).max(1)
    }
}

pub async fn project(State(state): State<AppState>, headers: HeaderMap, Path(slug): Path<String>, Query(q): Query<PageQuery>) -> AppResult<Response> {
    let key = format!("/projects/{slug}");
    let page = q.page();
    if page > 1 {
        let key = format!("{key}?page={page}");
        return cached_view(&state, &headers, key, || render_project(&state, slug, None, None, page)).await;
    }
    cached(&state, &headers, key, || render_project(&state, slug, None, None, 1)).await
}

pub async fn project_version(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((slug, version)): Path<(String, String)>,
    Query(q): Query<PageQuery>,
) -> AppResult<Response> {
    let key = format!("/projects/{slug}/versions/{version}");
    let version = match version.as_str() {
        "~" => String::new(),
        v if crate::report::valid_version(v) && !v.is_empty() => version,
        _ => return Err(AppError::NotFound),
    };
    let page = q.page();
    if page > 1 {
        let key = format!("{key}?page={page}");
        return cached_view(&state, &headers, key, || render_project(&state, slug, Some(version), None, page)).await;
    }
    cached(&state, &headers, key, || render_project(&state, slug, Some(version), None, 1)).await
}

/// A project page for one version (`None`: the default version), as of its latest report or,
/// with `at`, as of an earlier one (a key from `views::project::report_keys`).
async fn render_project(state: &AppState, slug: String, version: Option<String>, at: Option<String>, page: usize) -> AppResult<Arc<CachedPage>> {
    let p = load_public_project(state, slug).await?;
    let pid = p.id;
    let active = version.unwrap_or_else(|| p.default_version.clone());
    let v = active.clone();
    // A past commit can be anywhere in the history; the latest page only charts the recent part.
    let limit = if at.is_some() { 1_000_000 } else { 20_000 };
    let (mut reports, versions, ports, owner_banned) = state
        .db
        .run(move |c| {
            let banned = db::owner_is_banned(c, pid)?;
            Ok((db::reports_for_project(c, pid, &v, limit)?, db::versions_for_project(c, pid)?, db::ports_for_project(c, pid)?, banned))
        })
        .await?;
    if owner_banned {
        return Err(AppError::NotFound);
    }
    // Show the requested version's progress; the default version is already in `p`.
    let mut shown = match versions.iter().find(|v| v.version == active) {
        Some(v) => v.apply_to(&p),
        None if active == p.default_version => p.clone(),
        None => return Err(AppError::NotFound),
    };
    let names: Vec<String> = versions.iter().map(|v| v.version.clone()).collect();
    let base = if active == p.default_version { format!("/projects/{}", p.slug) } else { crate::util::version_page(&p.slug, &active) };
    let keys = views::project::report_keys(&reports);
    let mut links: Vec<String> = keys.iter().map(|k| format!("{base}/commit/{k}")).collect();

    let mut at_commit = None;
    if let Some(key) = at {
        let idx = views::project::find_report(&reports, &key).ok_or(AppError::NotFound)?;
        let r = reports[idx].clone();
        let latest = idx + 1 == reports.len();
        let areas = if r.has_areas {
            let id = r.id;
            state.db.run(move |c| db::report_areas(c, pid, id)).await?
        } else {
            None
        };
        // What the page compares with: the last earlier report that moved the figures.
        let prev = reports[..idx].iter().rev().find(|q| q.code != r.code || q.data != r.data);
        shown = db::Project {
            code: r.code,
            data: r.data,
            functions: if r.functions.total == 0 && latest { shown.functions } else { r.functions },
            prev_code: prev.map_or(r.code, |q| q.code),
            prev_data: prev.map_or(r.data, |q| q.data),
            areas_json: areas.clone().unwrap_or_else(|| "[]".into()),
            last_report_at: Some(r.created_at),
            last_commit: r.commit.clone(),
            last_message: r.message.clone(),
            commits_json: "{}".into(),
            ..shown
        };
        at_commit = Some(views::project::AtCommit {
            report: r,
            older: idx.checked_sub(1).map(|i| links[i].clone()),
            newer: links.get(idx + 1).cloned(),
            latest: Some(base.clone()),
            here: links[idx].clone(),
            past: true,
            areas_missing: areas.is_none(),
        });
        reports.truncate(idx + 1);
        links.truncate(idx + 1);
    } else if let Some(r) = reports.last() {
        // The latest report, with the way back through the earlier ones.
        at_commit = Some(views::project::AtCommit {
            report: r.clone(),
            older: reports.len().checked_sub(2).map(|i| links[i].clone()),
            newer: None,
            latest: None,
            here: base.clone(),
            past: false,
            areas_missing: false,
        });
    }
    let past = at_commit.as_ref().is_some_and(|a| a.past);

    let mut title = if active != p.default_version { format!("{} ({active})", p.name) } else { p.name.clone() };
    if let Some(a) = at_commit.as_ref().filter(|a| a.past) {
        let when = &crate::util::human_date(a.report.created_at)[..10];
        title = match a.report.commit.as_deref() {
            Some(c) => format!("{title} at {} ({when})", &c[..7.min(c.len())]),
            None => format!("{title} on {when}"),
        };
    }
    let desc = format!(
        "{title} decompilation: {} code decompiled ({} matching).",
        crate::util::fmt_pct(shown.code.decompiled_pct()),
        crate::util::fmt_pct(shown.code.matching_pct())
    );
    let hidden = !is_visible(&p, state.config.hide_threshold);
    let html = views::layout::render(Page {
        title: &title,
        description: &desc,
        image: og_image(state, &p),
        body: views::project::project(&shown, &reports, &links, at_commit.as_ref(), page, &state.config.base_url, hidden, &names, &active, &ports),
        canonical: (!past).then(|| base.clone()),
        og_type: Some("article"),
        feed: Some(format!("/projects/{}/feed.xml", p.slug)),
        json_ld: (!past).then(|| project_json_ld(state, &shown, &base, &desc)),
    });
    Ok(CachedPage::new(html, HTML))
}

/// `GET /projects/{slug}/commit/{key}`: the project page as of an earlier report.
pub async fn project_commit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((slug, key)): Path<(String, String)>,
    Query(q): Query<PageQuery>,
) -> AppResult<Response> {
    commit_response(&state, &headers, slug, None, key, q.page()).await
}

/// `GET /projects/{slug}/versions/{version}/commit/{key}`
pub async fn project_version_commit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((slug, version, key)): Path<(String, String, String)>,
    Query(q): Query<PageQuery>,
) -> AppResult<Response> {
    let version = match version.as_str() {
        "~" => String::new(),
        v if crate::report::valid_version(v) && !v.is_empty() => version,
        _ => return Err(AppError::NotFound),
    };
    commit_response(&state, &headers, slug, Some(version), key, q.page()).await
}

async fn commit_response(state: &AppState, headers: &HeaderMap, slug: String, version: Option<String>, key: String, page: usize) -> AppResult<Response> {
    if key.len() > 64 || !key.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(AppError::NotFound);
    }
    let base = match &version {
        Some(v) => crate::util::version_page(&slug, v),
        None => format!("/projects/{slug}"),
    };
    let cache_key = if page > 1 { format!("{base}/commit/{key}?page={page}") } else { format!("{base}/commit/{key}") };
    cached_view(state, headers, cache_key, || render_project(state, slug, version, Some(key), page)).await
}

/// Responds with a page left out of the page cache, which browsers and the edge keep briefly.
/// Like `cached`, for views of a project page that can't be purged from the edge by URL (past
/// commits, later pages of the report list). They're cached in memory only and dropped with
/// their project page (see `PageCache::views`); browsers and the edge revalidate them every time.
async fn cached_view<F, Fut>(state: &AppState, headers: &HeaderMap, key: String, render: F) -> AppResult<Response>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = AppResult<Arc<CachedPage>>>,
{
    if let Some(page) = state.cache.get_view(&key) {
        return Ok(state.cache.respond_view(&page, headers));
    }
    let page = render().await?;
    state.cache.put_view(key, page.clone());
    Ok(state.cache.respond_view(&page, headers))
}

/// The old `/history` list: the project page now shows any commit itself.
pub async fn project_history_page(Path(slug): Path<String>) -> Response {
    permanent_redirect(format!("/projects/{slug}"))
}

pub async fn project_version_history_page(Path((slug, version)): Path<(String, String)>) -> Response {
    permanent_redirect(crate::util::version_page(&slug, &version))
}

fn permanent_redirect(target: String) -> Response {
    (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, target), (header::CACHE_CONTROL, "public, max-age=86400".to_string())]).into_response()
}

pub async fn project_json(State(state): State<AppState>, headers: HeaderMap, Path(slug): Path<String>) -> AppResult<Response> {
    cached(&state, &headers, format!("/projects/{slug}/progress.json"), || async {
        let p = load_public_project(&state, slug).await?;
        let pid = p.id;
        let (versions, ports) = state.db.run(move |c| Ok((db::versions_for_project(c, pid)?, db::ports_for_project(c, pid)?))).await?;
        let versions: serde_json::Map<String, serde_json::Value> = versions
            .iter()
            .map(|v| {
                (v.version.clone(), serde_json::json!({
                    "code": v.code,
                    "data": v.data,
                    "code_matching_pct": v.code.matching_pct(),
                    "code_decompiled_pct": v.code.decompiled_pct(),
                    "data_pct": v.data.pct(),
                    "functions": v.functions,
                    "last_report_at": v.last_report_at.map(crate::util::iso_time),
                    "last_commit": v.last_commit,
                }))
            })
            .collect();
        let body = serde_json::json!({
            "slug": p.slug,
            "name": p.name,
            "platform": p.platform,
            "repo": (!p.repo_private).then_some(&p.repo),
            "repo_host": p.repo_host,
            "repo_url": p.public_repo_link(),
            "repo_private": p.repo_private,
            "ai_generated": p.ai_generated,
            "ai_contributions": if p.ai_contributions.is_empty() { None } else { Some(&p.ai_contributions) },
            "discord": p.discord,
            "unit": p.unit,
            "code": p.code,
            "data": p.data,
            "code_matching_pct": p.code.matching_pct(),
            "code_decompiled_pct": p.code.decompiled_pct(),
            "code_linked_pct": p.code.linked_pct(),
            "data_pct": p.data.pct(),
            "functions": p.functions,
            "areas": p.areas(),
            "last_report_at": p.last_report_at.map(crate::util::iso_time),
            "last_commit": p.last_commit,
            "last_message": p.last_message,
            "updated_at": crate::util::iso_time(p.updated_at),
            "score": p.score(),
            "default_version": p.default_version,
            "versions": versions,
            "ports": ports,
        });
        Ok(CachedPage::new(serde_json::to_vec(&body)?, "application/json"))
    })
    .await
}

/// `GET /projects/{slug}/report.json`: the default version's progress as an objdiff report.
pub async fn project_report(State(state): State<AppState>, headers: HeaderMap, Path(slug): Path<String>) -> AppResult<Response> {
    let key = format!("/projects/{slug}/report.json");
    cached(&state, &headers, key, || render_report(&state, slug, None)).await
}

/// `GET /projects/{slug}/versions/{version}/report.json`
pub async fn project_version_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((slug, version)): Path<(String, String)>,
) -> AppResult<Response> {
    let key = format!("{}/report.json", crate::util::version_page(&slug, &version));
    let version = match version.as_str() {
        "~" => String::new(),
        v if crate::report::valid_version(v) && !v.is_empty() => version,
        _ => return Err(AppError::NotFound),
    };
    cached(&state, &headers, key, || render_report(&state, slug, Some(version))).await
}

/// `GET /projects/{slug}/history.json`: the default version's history in decomp.dev's format.
pub async fn project_history(State(state): State<AppState>, headers: HeaderMap, Path(slug): Path<String>) -> AppResult<Response> {
    let key = format!("/projects/{slug}/history.json");
    cached(&state, &headers, key, || render_history(&state, slug, None)).await
}

/// `GET /projects/{slug}/versions/{version}/history.json`
pub async fn project_version_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((slug, version)): Path<(String, String)>,
) -> AppResult<Response> {
    let key = format!("{}/history.json", crate::util::version_page(&slug, &version));
    let version = match version.as_str() {
        "~" => String::new(),
        v if crate::report::valid_version(v) && !v.is_empty() => version,
        _ => return Err(AppError::NotFound),
    };
    cached(&state, &headers, key, || render_history(&state, slug, Some(version))).await
}

/// The project with the given version's progress (`None`: the default version), for the exports.
async fn export_version(state: &AppState, slug: String, version: Option<String>) -> AppResult<(Project, String)> {
    let p = load_public_project(state, slug).await?;
    let pid = p.id;
    let (versions, owner_banned) =
        state.db.run(move |c| Ok((db::versions_for_project(c, pid)?, db::owner_is_banned(c, pid)?))).await?;
    if owner_banned {
        return Err(AppError::NotFound);
    }
    let active = version.unwrap_or_else(|| p.default_version.clone());
    let shown = match versions.iter().find(|v| v.version == active) {
        Some(v) => v.apply_to(&p),
        None if active == p.default_version => p,
        None => return Err(AppError::NotFound),
    };
    Ok((shown, active))
}

async fn render_report(state: &AppState, slug: String, version: Option<String>) -> AppResult<Arc<CachedPage>> {
    let (shown, _) = export_version(state, slug, version).await?;
    let body = crate::objdiff::export(&shown.code, &shown.data, &shown.functions, &shown.areas());
    Ok(CachedPage::new(serde_json::to_vec(&body)?, "application/json"))
}

async fn render_history(state: &AppState, slug: String, version: Option<String>) -> AppResult<Arc<CachedPage>> {
    let (shown, active) = export_version(state, slug, version).await?;
    let pid = shown.id;
    let reports = state.db.run(move |c| db::reports_for_project(c, pid, &active, 100_000)).await?;
    // Newest first, like decomp.dev. Reports from before function counts were kept per report
    // have none, except the latest.
    let latest = reports.last().map(|r| r.id);
    let body: Vec<serde_json::Value> = reports
        .iter()
        .rev()
        .map(|r| {
            let functions = if r.functions.total == 0 && Some(r.id) == latest { &shown.functions } else { &r.functions };
            crate::objdiff::history_entry(r.created_at, r.commit.as_deref(), r.message.as_deref(), &r.code, &r.data, functions)
        })
        .collect();
    Ok(CachedPage::new(serde_json::to_vec(&body)?, "application/json"))
}

/// `GET /badge/{slug}.svg`: decompiled code of the default version.
pub async fn badge(State(state): State<AppState>, headers: HeaderMap, Path(file): Path<String>) -> AppResult<Response> {
    let slug = file.strip_suffix(".svg").ok_or(AppError::NotFound)?.to_string();
    badge_response(&state, &headers, slug, "decompiled").await
}

/// `GET /badge/{slug}/{matching,data,functions,linked}.svg`
pub async fn badge_kind(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((slug, file)): Path<(String, String)>,
) -> AppResult<Response> {
    let kind = match file.as_str() {
        "matching.svg" => "matching",
        "data.svg" => "data",
        "functions.svg" => "functions",
        "linked.svg" => "linked",
        _ => return Err(AppError::NotFound),
    };
    badge_response(&state, &headers, slug, kind).await
}

async fn badge_response(state: &AppState, headers: &HeaderMap, slug: String, kind: &'static str) -> AppResult<Response> {
    let key = if kind == "decompiled" { format!("/badge/{slug}.svg") } else { format!("/badge/{slug}/{kind}.svg") };
    cached(state, headers, key, || async {
        let p = load_public_project(state, slug).await?;
        let svg = match kind {
            "matching" => views::svg::badge("matching", &crate::util::fmt_pct(p.code.matching_pct()), Some(p.code.matching_pct())),
            "linked" if p.code.linked > 0 => views::svg::badge("fully linked", &crate::util::fmt_pct(p.code.linked_pct()), Some(p.code.linked_pct())),
            "linked" => views::svg::badge("fully linked", "n/a", None),
            "data" if p.data.total > 0 => views::svg::badge("data", &crate::util::fmt_pct(p.data.pct()), Some(p.data.pct())),
            "functions" if p.functions.total > 0 => views::svg::badge(
                "functions",
                &format!("{}/{}", crate::util::fmt_count(p.functions.matching), crate::util::fmt_count(p.functions.total)),
                Some(p.functions.pct()),
            ),
            "data" | "functions" => views::svg::badge(kind, "n/a", None),
            _ => views::svg::badge("decompiled", &crate::util::fmt_pct(p.code.decompiled_pct()), Some(p.code.decompiled_pct())),
        };
        Ok(CachedPage::new(svg, "image/svg+xml"))
    })
    .await
}

pub async fn docs(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    docs_response(&state, &headers, "").await
}

pub async fn docs_page(State(state): State<AppState>, headers: HeaderMap, Path(slug): Path<String>) -> AppResult<Response> {
    if slug.is_empty() {
        return Err(AppError::NotFound);
    }
    docs_response(&state, &headers, &slug).await
}

async fn docs_response(state: &AppState, headers: &HeaderMap, slug: &str) -> AppResult<Response> {
    let page = views::docs::find(slug).ok_or(AppError::NotFound)?;
    let path = views::docs::path(page);
    cached(state, headers, path.clone(), || async {
        let title = if page.slug.is_empty() { "Documentation".to_string() } else { format!("{} · Docs", page.title) };
        let html = views::layout::render(Page {
            title: &title,
            description: page.summary,
            body: views::docs::page(page, &state.config.base_url, state.config.github_app.as_ref().map(|a| a.slug.as_str())),
            canonical: Some(path),
            ..Default::default()
        });
        Ok(CachedPage::new(html, HTML))
    })
    .await
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `GET /feed.xml`: the latest progress reports of every listed project.
pub async fn feed(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    cached(&state, &headers, "/feed.xml".into(), || async {
        let threshold = state.config.hide_threshold;
        let items = state.db.run(move |c| db::recent_feed_items(c, threshold, 100)).await?;
        let xml = views::feed::rss(
            &state.config.base_url,
            "decompilati.onl: decompilation progress",
            "Progress updates from every decompilation project on decompilati.onl.",
            "/",
            "/feed.xml",
            &items,
        );
        Ok(CachedPage::new(xml, "application/rss+xml; charset=utf-8"))
    })
    .await
}

/// `GET /projects/{slug}/feed.xml`: one project's progress reports, every version.
pub async fn project_feed(State(state): State<AppState>, headers: HeaderMap, Path(slug): Path<String>) -> AppResult<Response> {
    cached(&state, &headers, format!("/projects/{slug}/feed.xml"), || async {
        let p = load_public_project(&state, slug).await?;
        let pid = p.id;
        let items = state.db.run(move |c| db::project_feed_items(c, pid, 100)).await?;
        let xml = views::feed::rss(
            &state.config.base_url,
            &format!("{}: decompilation progress", p.name),
            &format!("Progress updates from the {} decompilation.", p.name),
            &format!("/projects/{}", p.slug),
            &format!("/projects/{}/feed.xml", p.slug),
            &items,
        );
        Ok(CachedPage::new(xml, "application/rss+xml; charset=utf-8"))
    })
    .await
}

pub async fn robots() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain"), (header::CACHE_CONTROL, "public, max-age=86400")],
        format!(
            "User-agent: *\nDisallow: /dashboard\nDisallow: /admin\nDisallow: /auth\nDisallow: /api\n\nSitemap: {}/sitemap.xml\n",
            views::layout::base_url()
        ),
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

/// `/p/...` was the original project URL prefix; send it to `/projects/...` permanently.
pub async fn old_project_url(Path(rest): Path<String>) -> Response {
    let target = format!("/projects/{}", rest.trim_start_matches('/'));
    (
        StatusCode::MOVED_PERMANENTLY,
        [(header::LOCATION, target), (header::CACHE_CONTROL, "public, max-age=86400".to_string())],
    )
        .into_response()
}

/// schema.org description of a project page: source code about a video game.
fn project_json_ld(state: &AppState, p: &Project, path: &str, desc: &str) -> serde_json::Value {
    let platform = platforms::get_or_other(&p.platform);
    let mut ld = serde_json::json!({
        "@context": "https://schema.org",
        "@type": "SoftwareSourceCode",
        "name": format!("{} decompilation", p.name),
        "description": desc,
        "url": format!("{}{path}", state.config.base_url),
        "author": { "@type": "Person", "name": p.owner_login, "url": format!("https://github.com/{}", p.owner_login) },
        "about": { "@type": "VideoGame", "name": p.name, "gamePlatform": platform.name },
        "dateCreated": crate::util::iso_time(p.created_at),
    });
    if let Some(repo) = p.public_repo_link() {
        ld["codeRepository"] = repo.into();
    }
    if let Some(t) = p.last_report_at {
        ld["dateModified"] = crate::util::iso_time(t).into();
    }
    if let Some(img) = og_image(state, p) {
        ld["image"] = img.into();
    }
    ld
}

/// `GET /sitemap.xml`: every public page, with the last time its content changed.
pub async fn sitemap(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    cached(&state, &headers, "/sitemap.xml".into(), || async {
        let all = state.db.run(db::public_projects_owned).await?;
        let threshold = state.config.hide_threshold;
        let listed: Vec<&Project> = all.iter().filter(|p| is_visible(p, threshold)).collect();
        let ids: Vec<i64> = listed.iter().map(|p| p.id).collect();
        let versions = state
            .db
            .run(move |c| {
                ids.into_iter().map(|id| Ok((id, db::versions_for_project(c, id)?))).collect::<anyhow::Result<HashMap<_, _>>>()
            })
            .await?;
        let base = &state.config.base_url;
        let latest = listed.iter().filter_map(|p| p.last_report_at).max();
        let mut xml = String::from(r#"<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">"#);
        let mut url = |path: &str, modified: Option<i64>| {
            xml.push_str(&format!("<url><loc>{base}{}</loc>", xml_escape(path)));
            if let Some(t) = modified {
                xml.push_str(&format!("<lastmod>{}</lastmod>", crate::util::iso_time(t)));
            }
            xml.push_str("</url>");
        };
        url("/", latest);
        url("/projects", latest);
        url("/highlights", latest);
        url("/platforms", latest);
        url("/ports", None);
        for page in views::docs::PAGES {
            url(&views::docs::path(page), None);
        }
        let mut platforms: Vec<&str> = listed.iter().map(|p| p.platform.as_str()).collect();
        platforms.sort_unstable();
        platforms.dedup();
        for pl in platforms {
            let modified = listed.iter().filter(|p| p.platform == pl).filter_map(|p| p.last_report_at).max();
            url(&format!("/platform/{pl}"), modified);
        }
        for p in &listed {
            url(&format!("/projects/{}", p.slug), Some(p.last_report_at.unwrap_or(p.updated_at)));
            for v in versions.get(&p.id).into_iter().flatten() {
                if v.version != p.default_version && !v.version.is_empty() {
                    url(&format!("/projects/{}/versions/{}", p.slug, v.version), v.last_report_at);
                }
            }
        }
        xml.push_str("</urlset>");
        Ok(CachedPage::new(xml, "application/xml; charset=utf-8"))
    })
    .await
}

/// Fallback for unknown paths: decomp.dev-style URLs (`/{owner}/{repo}`, optionally with a
/// version, `.json` or `.svg`) redirect to the project registered for that repository, so
/// links only need their domain changed. Anything else is a 404.
pub async fn legacy_or_not_found(State(state): State<AppState>, uri: axum::http::Uri) -> AppResult<Response> {
    let segments: Vec<&str> = uri.path().trim_matches('/').split('/').collect();
    let (owner, repo, version) = match segments.as_slice() {
        [owner, repo] => (*owner, *repo, None),
        [owner, repo, version] => (*owner, *repo, Some(*version)),
        _ => return Err(AppError::NotFound),
    };
    if crate::util::is_reserved_name(owner) {
        return Err(AppError::NotFound);
    }
    // The extension sits on the last segment: /{owner}/{repo}.json or /{owner}/{repo}/{version}.svg
    let last = version.unwrap_or(repo);
    let (stem, ext) = match last.rsplit_once('.') {
        Some((stem, ext @ ("json" | "svg" | "png"))) => (stem, Some(ext)),
        _ => (last, None),
    };
    let (repo, version) = if version.is_some() { (repo, Some(stem)) } else { (stem, None) };
    let full = format!("{owner}/{repo}");
    let mut projects = state.db.run(move |c| db::projects_by_repo(c, &full)).await?;
    projects.sort_by_key(|p| p.created_at);
    let p = projects.into_iter().next().ok_or(AppError::NotFound)?;

    let target = match ext {
        Some("json") => format!("/projects/{}/progress.json", p.slug),
        Some(_) => {
            // decomp.dev shields: ?mode=shield&measure=code|data|functions|…
            let measure = uri
                .query()
                .unwrap_or("")
                .split('&')
                .find_map(|kv| kv.strip_prefix("measure="))
                .unwrap_or("code");
            match measure {
                "data" | "matched_data" | "complete_data" => format!("/badge/{}/data.svg", p.slug),
                "functions" | "matched_functions" => format!("/badge/{}/functions.svg", p.slug),
                "complete_code" | "linked_code" => format!("/badge/{}/linked.svg", p.slug),
                "fuzzy" | "fuzzy_match" => format!("/badge/{}.svg", p.slug),
                _ => format!("/badge/{}/matching.svg", p.slug),
            }
        }
        None => match version.filter(|v| crate::report::valid_version(v) && !v.is_empty() && *v != p.default_version) {
            Some(v) => format!("/projects/{}/versions/{v}", p.slug),
            None => format!("/projects/{}", p.slug),
        },
    };
    Ok((
        StatusCode::MOVED_PERMANENTLY,
        [(header::LOCATION, target), (header::CACHE_CONTROL, "public, max-age=3600".to_string())],
    )
        .into_response())
}

/// Highlighted projects: admin-pinned ones first, then the most upvoted.
fn pick_highlights<'a>(visible: &[&'a Project], max: usize) -> Vec<&'a Project> {
    let mut picked: Vec<&Project> = visible.iter().copied().filter(|p| p.spotlight).collect();
    let mut rated: Vec<&Project> = visible.iter().copied().filter(|p| !p.spotlight && p.score() > 0).collect();
    rated.sort_by_key(|p| (-p.score(), -p.upvotes, -(p.last_report_at.unwrap_or(0))));
    picked.extend(rated);
    picked.truncate(max);
    picked
}

/// `GET /highlights`
pub async fn highlights(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    cached(&state, &headers, "/highlights".into(), || async {
        let all = state.db.run(db::public_projects_owned).await?;
        let threshold = state.config.hide_threshold;
        let visible: Vec<&Project> = all.iter().filter(|p| is_visible(p, threshold)).collect();
        let picked = pick_highlights(&visible, 12);
        let html = views::layout::render(Page {
            title: "Highlights",
            description: "Decompilation projects picked by the admins and the most upvoted by the community.",
            body: views::home::highlights(&picked),
            canonical: Some("/highlights".into()),
            ..Default::default()
        });
        Ok(CachedPage::new(html, HTML))
    })
    .await
}
