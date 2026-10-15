//! Logged-in project management and admin moderation. Responses are private (`no-store`).

use axum::{
    extract::{Multipart, Path, State},
    response::{Html, IntoResponse, Redirect, Response},
    Form,
};
use maud::Markup;
use serde::Deserialize;

use crate::{
    app::{AppError, AppResult, AppState},
    auth::{pushable_repos, RepoListError, RequireAdmin, RequireUser, SessionUser},
    cache::membership_paths,
    db::{self, Project, ProjectMeta},
    images, platforms,
    util::{new_api_key, slugify, valid_slug},
    views::{
        self,
        dashboard::{ProjectForm, RepoOptions},
        layout::Page,
    },
};

const MAX_PROJECTS_PER_USER: usize = 25;
const RESERVED_SLUGS: &[&str] = &["new", "admin", "api", "static"];

fn page(title: &str, body: Markup) -> Response {
    // Private pages: no canonical URL, so they render with `noindex`.
    Html(views::layout::render(Page { title, body, ..Default::default() })).into_response()
}

async fn repo_options(state: &AppState, u: &SessionUser, next: &str) -> RepoOptions {
    match pushable_repos(state, u).await {
        Ok(list) => RepoOptions::List(list.into_iter().map(|r| r.full_name).collect()),
        Err(RepoListError::Expired) => RepoOptions::Expired { next: next.to_string() },
        Err(RepoListError::Failed) => RepoOptions::Failed,
    }
}

async fn new_page(state: &AppState, u: &SessionUser, f: &ProjectForm, error: Option<&str>) -> Response {
    let repos = repo_options(state, u, "/dashboard/new").await;
    page("Add a project", views::dashboard::new_project(f, &repos, error))
}

pub async fn index(State(state): State<AppState>, RequireUser(u): RequireUser) -> AppResult<Response> {
    let uid = u.user.id;
    let projects = state.db.run(move |c| db::projects_by_owner(c, uid)).await?;
    Ok(page("Your projects", views::dashboard::dashboard(&u, &projects)))
}

pub async fn new_form(State(state): State<AppState>, RequireUser(u): RequireUser) -> Response {
    new_page(&state, &u, &ProjectForm::default(), None).await
}

/// Picks a free slug derived from the project name (or the repository name if the
/// project name has no usable characters), adding `-2`, `-3`… when taken.
async fn unique_slug(state: &AppState, name: &str, repo: &str) -> Result<String, String> {
    let mut base = slugify(name);
    if !valid_slug(&base) {
        base = slugify(repo.rsplit('/').next().unwrap_or(""));
    }
    if !valid_slug(&base) {
        base = "project".into();
    }
    base.truncate(56);
    let base = base.trim_end_matches('-').to_string();
    let result = state
        .db
        .run(move |c| {
            for n in 1..1000 {
                let candidate = if n == 1 { base.clone() } else { format!("{base}-{n}") };
                if !RESERVED_SLUGS.contains(&candidate.as_str()) && !db::slug_taken(c, &candidate, 0)? {
                    return Ok(Some(candidate));
                }
            }
            Ok(None)
        })
        .await
        .map_err(|e| e.to_string())?;
    result.ok_or_else(|| "Couldn't generate a URL for this project name.".into())
}

/// Validates the form. The repository must be one of the user's pushable repositories,
/// unless it's unchanged on an existing project.
async fn validate(state: &AppState, u: &SessionUser, f: &ProjectForm, existing: Option<&Project>) -> Result<ProjectMeta, String> {
    let name = f.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 100 {
        return Err("Name must be between 1 and 100 characters.".into());
    }
    if platforms::get(&f.platform).is_none() {
        return Err("Pick a platform.".into());
    }
    // Outside GitHub there's no push-access check and no GitHub integration: progress comes
    // through the API only.
    let external = f.repo_host == "external";
    let (repo, github_repo_id, repo_url) = if external {
        let (display, url) = crate::util::normalize_external_repo(&f.repo_url)?;
        (display, None, Some(url))
    } else {
        let selected = f.repo.trim();
        // The repository id is only known (and only changes) when one is picked from the list.
        match existing {
            Some(p) if p.is_github() && (selected.is_empty() || p.repo.eq_ignore_ascii_case(selected)) => (p.repo.clone(), None, None),
            _ => {
                if selected.is_empty() {
                    return Err("Pick a repository.".into());
                }
                let repos = match pushable_repos(state, u).await {
                    Ok(r) => r,
                    Err(RepoListError::Expired) => return Err("Your GitHub session has expired. Please log in again.".into()),
                    Err(RepoListError::Failed) => return Err("Couldn't reach GitHub, please try again.".into()),
                };
                let (name, id) = repos
                    .into_iter()
                    .find(|r| r.full_name.eq_ignore_ascii_case(selected))
                    .map(|r| (r.full_name, Some(r.id)))
                    .ok_or("Pick one of your public repositories from the list.")?;
                // decomp.dev-style /{owner}/{repo} links redirect to projects, so owners can't
                // shadow site pages.
                if name.split('/').next().is_some_and(crate::util::is_reserved_name) {
                    return Err("Repositories owned by an account with a name reserved for this site's pages can't be registered.".into());
                }
                (name, id, None)
            }
        }
    };
    let ai_contributions = f.ai_contributions.trim().to_string();
    if !crate::db::AI_CONTRIBUTIONS.iter().any(|(v, _)| *v == ai_contributions) {
        return Err("Pick an option for AI contributions.".into());
    }
    let description: String = f
        .description
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(2000)
        .collect::<String>()
        .trim()
        .to_string();
    let discord = crate::util::normalize_discord_invite(&f.discord)?;
    // The slug is fixed once created so links and badges keep working after a rename.
    let slug = match existing {
        Some(p) => p.slug.clone(),
        None => unique_slug(state, &name, &repo).await?,
    };
    Ok(ProjectMeta {
        slug,
        name,
        platform: f.platform.clone(),
        repo,
        github_repo_id,
        repo_host: if external { "external".into() } else { "github".into() },
        repo_url,
        ai_contributions,
        discord,
        description,
        ai_generated: f.ai_generated.is_some(),
    })
}

/// Reads the new-project form: text fields plus the required cover image.
async fn read_new_project_form(mut mp: Multipart) -> AppResult<(ProjectForm, Option<bytes::Bytes>)> {
    let mut f = ProjectForm::default();
    let mut image = None;
    while let Some(field) = mp.next_field().await.map_err(|e| AppError::BadRequest(e.to_string()))? {
        let name = field.name().unwrap_or_default().to_string();
        if name == "image" {
            let data = field.bytes().await.map_err(|_| AppError::BadRequest("Upload failed or file too large.".into()))?;
            image = Some(data).filter(|d| !d.is_empty());
            continue;
        }
        let value = field.text().await.map_err(|e| AppError::BadRequest(e.to_string()))?;
        match name.as_str() {
            "name" => f.name = value,
            "platform" => f.platform = value,
            "repo" => f.repo = value,
            "repo_host" => f.repo_host = value,
            "repo_url" => f.repo_url = value,
            "ai_contributions" => f.ai_contributions = value,
            "discord" => f.discord = value,
            "description" => f.description = value,
            "ai_generated" => f.ai_generated = Some(value),
            _ => {}
        }
    }
    Ok((f, image))
}

pub async fn create(State(state): State<AppState>, RequireUser(u): RequireUser, mp: Multipart) -> AppResult<Response> {
    let (f, image) = read_new_project_form(mp).await?;
    let uid = u.user.id;
    let count = state.db.run(move |c| Ok(db::projects_by_owner(c, uid)?.len())).await?;
    if count >= MAX_PROJECTS_PER_USER {
        return Ok(new_page(&state, &u, &f, Some("You've reached the maximum number of projects.")).await);
    }
    let meta = match validate(&state, &u, &f, None).await {
        Ok(m) => m,
        Err(e) => return Ok(new_page(&state, &u, &f, Some(&e)).await),
    };
    let Some(image) = image else {
        return Ok(new_page(&state, &u, &f, Some("A cover image is required.")).await);
    };
    let dir = state.config.uploads_dir();
    let stored = match tokio::task::spawn_blocking(move || images::store_cover(&image, &dir)).await? {
        Ok(name) => name,
        Err(e) => return Ok(new_page(&state, &u, &f, Some(&e)).await),
    };
    let (key, hash, prefix) = new_api_key();
    let m = meta.clone();
    let project = state
        .db
        .run(move |c| {
            let id = db::insert_project(c, uid, &m, &stored, &hash, &prefix)?;
            db::project_by_id(c, id)
        })
        .await?
        .ok_or(AppError::NotFound)?;
    state.cache.invalidate(membership_paths(&project.slug, &project.platform));
    // A repository can hold several games: if another project already uses this one, the
    // GitHub App doesn't import its artifacts here until the owner picks which ones.
    let pid = project.id;
    let shared = project.is_github() && state.db.run(move |c| db::repo_shared(c, pid)).await.unwrap_or(false);
    if shared {
        state.db.run(move |c| db::set_app_artifacts(c, pid, "")).await?;
    }
    // Projects that were on decomp.dev bring their history along, through a background job
    // so creating the project doesn't depend on decomp.dev being up.
    let auto_import = if project.is_github() { crate::decompdev::queue_auto_import(&state, &project, uid).await } else { Err(String::new()) };
    let notice = match auto_import {
        Ok(()) => "Project created! If it was on decomp.dev, its progress history is imported in the background (see \"Import history\" below). Now set up your CI to keep reporting.",
        Err(_) => "Project created! Now set up your CI to report progress.",
    };
    Ok(edit_page(&state, &u, &project, None, Some(notice), Some(&key)).await)
}

async fn edit_form_page(state: &AppState, u: &SessionUser, p: &Project, f: &ProjectForm, error: Option<&str>, notice: Option<&str>, key: Option<&str>) -> Response {
    let repos = repo_options(state, u, &format!("/dashboard/p/{}", p.id)).await;
    let pid = p.id;
    let versions: Vec<String> = state
        .db
        .run(move |c| Ok(db::versions_for_project(c, pid)?.into_iter().map(|v| v.version).collect()))
        .await
        .unwrap_or_default();
    let app_slug = state.config.github_app.as_ref().map(|a| a.slug.as_str());
    let import = state.db.run(move |c| db::latest_import_job(c, pid)).await.ok().flatten();
    page(&p.name, views::dashboard::edit_project(u, p, f, &repos, error, notice, key, &state.config.base_url, app_slug, &versions, import.as_ref()))
}

async fn edit_page(state: &AppState, u: &SessionUser, p: &Project, error: Option<&str>, notice: Option<&str>, key: Option<&str>) -> Response {
    edit_form_page(state, u, p, &ProjectForm::from_project(p), error, notice, key).await
}

/// Loads a project the user may manage (its owner, or an admin).
async fn owned(state: &AppState, u: &SessionUser, id: i64) -> AppResult<Project> {
    let p = state.db.run(move |c| db::project_by_id(c, id)).await?.ok_or(AppError::NotFound)?;
    if p.owner_id != u.user.id && !u.is_admin {
        return Err(AppError::NotFound);
    }
    Ok(p)
}

pub async fn edit(State(state): State<AppState>, RequireUser(u): RequireUser, Path(id): Path<i64>) -> AppResult<Response> {
    let p = owned(&state, &u, id).await?;
    Ok(edit_page(&state, &u, &p, None, None, None).await)
}

pub async fn update(State(state): State<AppState>, RequireUser(u): RequireUser, Path(id): Path<i64>, Form(f): Form<ProjectForm>) -> AppResult<Response> {
    let p = owned(&state, &u, id).await?;
    let meta = match validate(&state, &u, &f, Some(&p)).await {
        Ok(m) => m,
        Err(e) => return Ok(edit_form_page(&state, &u, &p, &f, Some(&e), None, None).await),
    };
    let m = meta.clone();
    let new_default = f.default_version.clone().filter(|v| *v != p.default_version);
    let (updated, versions) = state
        .db
        .run(move |c| {
            db::update_project_meta(c, id, &m)?;
            let versions: Vec<String> = db::versions_for_project(c, id)?.into_iter().map(|v| v.version).collect();
            if let Some(v) = new_default.filter(|v| versions.contains(v)) {
                db::set_default_version(c, id, &v)?;
            }
            Ok((db::project_by_id(c, id)?, versions))
        })
        .await?;
    let updated = updated.ok_or(AppError::NotFound)?;
    // The platform may have changed, so both the old and new listing pages are affected; the
    // version pages show which version is the default.
    state.cache.invalidate(membership_paths(&p.slug, &p.platform));
    state.cache.invalidate(membership_paths(&updated.slug, &updated.platform));
    state.cache.invalidate(versions.iter().filter(|v| !v.is_empty()).map(|v| format!("/projects/{}/versions/{v}", updated.slug)));
    Ok(edit_page(&state, &u, &updated, None, Some("Saved."), None).await)
}

pub async fn upload_image(State(state): State<AppState>, RequireUser(u): RequireUser, Path(id): Path<i64>, mut mp: Multipart) -> AppResult<Response> {
    let p = owned(&state, &u, id).await?;
    let mut data = None;
    while let Some(field) = mp.next_field().await.map_err(|e| AppError::BadRequest(e.to_string()))? {
        if field.name() == Some("image") {
            data = Some(field.bytes().await.map_err(|_| AppError::BadRequest("Upload failed or file too large.".into()))?);
        }
    }
    let Some(data) = data.filter(|d| !d.is_empty()) else {
        return Ok(edit_page(&state, &u, &p, Some("No image was uploaded."), None, None).await);
    };
    let dir = state.config.uploads_dir();
    let stored = tokio::task::spawn_blocking(move || images::store_cover(&data, &dir)).await?;
    match stored {
        Err(e) => Ok(edit_page(&state, &u, &p, Some(&e), None, None).await),
        Ok(name) => {
            let n = name.clone();
            let updated = state
                .db
                .run(move |c| {
                    db::set_project_image(c, id, Some(&n))?;
                    db::project_by_id(c, id)
                })
                .await?
                .ok_or(AppError::NotFound)?;
            remove_old_image(&state, p.image.as_deref(), &name).await;
            state.cache.invalidate(membership_paths(&p.slug, &p.platform));
            Ok(edit_page(&state, &u, &updated, None, Some("Image updated."), None).await)
        }
    }
}

async fn remove_old_image(state: &AppState, old: Option<&str>, new: &str) {
    if let Some(old) = old.filter(|o| *o != new) {
        let o = old.to_string();
        let still_used: bool = state
            .db
            .run(move |c| db::image_in_use(c, &o))
            .await
            .unwrap_or(true);
        if !still_used {
            let _ = tokio::fs::remove_file(state.config.uploads_dir().join(old)).await;
        }
    }
}

pub async fn regenerate_key(State(state): State<AppState>, RequireUser(u): RequireUser, Path(id): Path<i64>) -> AppResult<Response> {
    owned(&state, &u, id).await?;
    let (key, hash, prefix) = new_api_key();
    let updated = state
        .db
        .run(move |c| {
            db::set_api_key(c, id, &hash, &prefix)?;
            db::project_by_id(c, id)
        })
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(edit_page(&state, &u, &updated, None, Some("A new API key was generated."), Some(&key)).await)
}

#[derive(Deserialize)]
pub struct DeleteForm {
    confirm: String,
}

pub async fn delete(State(state): State<AppState>, RequireUser(u): RequireUser, Path(id): Path<i64>, Form(f): Form<DeleteForm>) -> AppResult<Response> {
    let p = owned(&state, &u, id).await?;
    if f.confirm.trim() != p.slug {
        return Ok(edit_page(&state, &u, &p, Some("The confirmation didn't match the slug; nothing was deleted."), None, None).await);
    }
    state.db.run(move |c| db::delete_project(c, id)).await?;
    remove_old_image(&state, p.image.as_deref(), "").await;
    state.cache.invalidate(membership_paths(&p.slug, &p.platform));
    Ok(Redirect::to("/dashboard").into_response())
}

// ---------------------------------------------------------------- admin

pub async fn admin(State(state): State<AppState>, RequireAdmin(_u): RequireAdmin) -> AppResult<Response> {
    let (projects, users, jobs) = state
        .db
        .run(|c| Ok((db::all_projects(c)?, db::list_users(c)?, db::recent_import_jobs(c, 50)?)))
        .await?;
    Ok(page("Admin", views::dashboard::admin(&projects, &users, &jobs)))
}

async fn toggle(state: &AppState, id: i64, f: fn(&mut db::Conn, i64, bool) -> anyhow::Result<()>, get: fn(&Project) -> bool) -> AppResult<Response> {
    let p = state.db.run(move |c| db::project_by_id(c, id)).await?.ok_or(AppError::NotFound)?;
    let on = !get(&p);
    state.db.run(move |c| f(c, id, on)).await?;
    state.cache.invalidate(membership_paths(&p.slug, &p.platform));
    Ok(Redirect::to("/admin").into_response())
}

pub async fn admin_spotlight(State(state): State<AppState>, RequireAdmin(_u): RequireAdmin, Path(id): Path<i64>) -> AppResult<Response> {
    toggle(&state, id, db::set_spotlight, |p| p.spotlight).await
}

pub async fn admin_hide(State(state): State<AppState>, RequireAdmin(_u): RequireAdmin, Path(id): Path<i64>) -> AppResult<Response> {
    toggle(&state, id, db::set_hidden, |p| p.hidden).await
}

pub async fn admin_ban(State(state): State<AppState>, RequireAdmin(_u): RequireAdmin, Path(uid): Path<i64>) -> AppResult<Response> {
    let projects = state
        .db
        .run(move |c| {
            let banned = db::is_banned(c, uid)?;
            db::set_banned(c, uid, !banned)?;
            db::projects_by_owner(c, uid)
        })
        .await?;
    for p in projects {
        state.cache.invalidate(membership_paths(&p.slug, &p.platform));
    }
    Ok(Redirect::to("/admin").into_response())
}

/// Starts importing the project's history from its past GitHub Actions runs.
pub async fn import_history(State(state): State<AppState>, RequireUser(u): RequireUser, Path(id): Path<i64>) -> AppResult<Response> {
    let p = owned(&state, &u, id).await?;
    match crate::github_app::start_history_import(&state, &p, u.user.id).await {
        Ok(()) => Ok(edit_page(&state, &u, &p, None, Some("History import queued. It runs in the background; reload this page to follow it."), None).await),
        Err(e) => Ok(edit_page(&state, &u, &p, Some(&e), None, None).await),
    }
}

#[derive(Deserialize)]
pub struct DecompDevForm {
    url: String,
}

/// Queues an import of the project's history from decomp.dev.
pub async fn import_decompdev(
    State(state): State<AppState>,
    RequireUser(u): RequireUser,
    Path(id): Path<i64>,
    Form(f): Form<DecompDevForm>,
) -> AppResult<Response> {
    let p = owned(&state, &u, id).await?;
    match crate::decompdev::start(&state, &p, &f.url, u.user.id).await {
        Ok(()) => Ok(edit_page(&state, &u, &p, None, Some("decomp.dev import queued. It runs in the background; reload this page to follow it."), None).await),
        Err(e) => Ok(edit_page(&state, &u, &p, Some(&e), None, None).await),
    }
}

#[derive(Deserialize)]
pub struct AppArtifactsForm {
    mode: String,
    #[serde(default)]
    names: String,
}

/// Normalises the GitHub App artifact setting: `*`, empty, or a comma-separated list of
/// `<name>_report` artifact names.
fn app_artifacts_value(f: &AppArtifactsForm) -> Result<String, &'static str> {
    match f.mode.as_str() {
        "all" => Ok("*".into()),
        "off" => Ok(String::new()),
        _ => {
            let names: Vec<&str> = f.names.split([',', ' ', '\n']).map(str::trim).filter(|n| !n.is_empty()).collect();
            if names.is_empty() {
                return Err("List at least one artifact name, or choose another option.");
            }
            for n in &names {
                let version = if *n == "report" { Some("") } else { n.strip_suffix("_report") };
                if !version.is_some_and(crate::report::valid_version) {
                    return Err("Artifact names must look like <version>_report, e.g. europe_report.");
                }
            }
            Ok(names.join(","))
        }
    }
}

/// Chooses which GitHub Actions artifacts the GitHub App imports into the project.
pub async fn set_app_artifacts(
    State(state): State<AppState>,
    RequireUser(u): RequireUser,
    Path(id): Path<i64>,
    Form(f): Form<AppArtifactsForm>,
) -> AppResult<Response> {
    let p = owned(&state, &u, id).await?;
    let value = match app_artifacts_value(&f) {
        Ok(v) => v,
        Err(e) => return Ok(edit_page(&state, &u, &p, Some(e), None, None).await),
    };
    let v = value.clone();
    let updated = state
        .db
        .run(move |c| {
            db::set_app_artifacts(c, id, &v)?;
            db::project_by_id(c, id)
        })
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(edit_page(&state, &u, &updated, None, Some("Saved which artifacts the GitHub App imports."), None).await)
}
