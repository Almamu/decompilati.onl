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
    cache::project_paths,
    db::{self, Project, ProjectMeta},
    images, platforms,
    util::{clean_url, new_api_key, slugify, valid_slug},
    views::{
        self,
        dashboard::{ProjectForm, RepoOptions},
        layout::Page,
    },
};

const MAX_PROJECTS_PER_USER: usize = 25;
const RESERVED_SLUGS: &[&str] = &["new", "admin", "api", "static"];

fn page(title: &str, body: Markup) -> Response {
    Html(views::layout::render(Page { title, description: "", image: None, body })).into_response()
}

async fn repo_options(state: &AppState, u: &SessionUser, next: &str) -> RepoOptions {
    match pushable_repos(state, u).await {
        Ok(list) => RepoOptions::List(list),
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
    let selected = f.repo.trim();
    let repo = match existing {
        Some(p) if selected.is_empty() || p.repo.eq_ignore_ascii_case(selected) => p.repo.clone(),
        _ => {
            if selected.is_empty() {
                return Err("Pick a repository.".into());
            }
            let repos = match pushable_repos(state, u).await {
                Ok(r) => r,
                Err(RepoListError::Expired) => return Err("Your GitHub session has expired. Please log in again.".into()),
                Err(RepoListError::Failed) => return Err("Couldn't reach GitHub, please try again.".into()),
            };
            repos
                .into_iter()
                .find(|r| r.eq_ignore_ascii_case(selected))
                .ok_or("Pick one of your public repositories from the list.")?
        }
    };
    let website = match f.website.trim() {
        "" => None,
        w => Some(clean_url(w).ok_or("Website must be an http(s) URL.")?),
    };
    let description: String = f
        .description
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(2000)
        .collect::<String>()
        .trim()
        .to_string();
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
        website,
        description,
        ai_generated: f.ai_generated.is_some(),
    })
}

pub async fn create(State(state): State<AppState>, RequireUser(u): RequireUser, Form(f): Form<ProjectForm>) -> AppResult<Response> {
    let uid = u.user.id;
    let count = state.db.run(move |c| Ok(db::projects_by_owner(c, uid)?.len())).await?;
    if count >= MAX_PROJECTS_PER_USER {
        return Ok(new_page(&state, &u, &f, Some("You've reached the maximum number of projects.")).await);
    }
    let meta = match validate(&state, &u, &f, None).await {
        Ok(m) => m,
        Err(e) => return Ok(new_page(&state, &u, &f, Some(&e)).await),
    };
    let (key, hash, prefix) = new_api_key();
    let m = meta.clone();
    let project = state
        .db
        .run(move |c| {
            let id = db::insert_project(c, uid, &m, &hash, &prefix)?;
            db::project_by_id(c, id)
        })
        .await?
        .ok_or(AppError::NotFound)?;
    state.cache.invalidate(project_paths(&project.slug, &project.platform));
    Ok(edit_page(&state, &u, &project, None, Some("Project created! Now set up your CI to report progress."), Some(&key)).await)
}

async fn edit_form_page(state: &AppState, u: &SessionUser, p: &Project, f: &ProjectForm, error: Option<&str>, notice: Option<&str>, key: Option<&str>) -> Response {
    let repos = repo_options(state, u, &format!("/dashboard/p/{}", p.id)).await;
    page(&p.name, views::dashboard::edit_project(u, p, f, &repos, error, notice, key, &state.config.base_url))
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
    let updated = state
        .db
        .run(move |c| {
            db::update_project_meta(c, id, &m)?;
            db::project_by_id(c, id)
        })
        .await?
        .ok_or(AppError::NotFound)?;
    // The platform may have changed, so both the old and new listing pages are affected.
    state.cache.invalidate(project_paths(&p.slug, &p.platform));
    state.cache.invalidate(project_paths(&updated.slug, &updated.platform));
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
            state.cache.invalidate(project_paths(&p.slug, &p.platform));
            Ok(edit_page(&state, &u, &updated, None, Some("Image updated."), None).await)
        }
    }
}

async fn remove_old_image(state: &AppState, old: Option<&str>, new: &str) {
    if let Some(old) = old.filter(|o| *o != new) {
        let o = old.to_string();
        let still_used: bool = state
            .db
            .run(move |c| Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM projects WHERE image = ?1)", [&o], |r| r.get(0))?))
            .await
            .unwrap_or(true);
        if !still_used {
            let _ = tokio::fs::remove_file(state.config.uploads_dir().join(old)).await;
        }
    }
}

pub async fn delete_image(State(state): State<AppState>, RequireUser(u): RequireUser, Path(id): Path<i64>) -> AppResult<Response> {
    let p = owned(&state, &u, id).await?;
    let updated = state
        .db
        .run(move |c| {
            db::set_project_image(c, id, None)?;
            db::project_by_id(c, id)
        })
        .await?
        .ok_or(AppError::NotFound)?;
    remove_old_image(&state, p.image.as_deref(), "").await;
    state.cache.invalidate(project_paths(&p.slug, &p.platform));
    Ok(edit_page(&state, &u, &updated, None, Some("Image removed."), None).await)
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
    state.cache.invalidate(project_paths(&p.slug, &p.platform));
    Ok(Redirect::to("/dashboard").into_response())
}

// ---------------------------------------------------------------- admin

pub async fn admin(State(state): State<AppState>, RequireAdmin(_u): RequireAdmin) -> AppResult<Response> {
    let (projects, users) = state.db.run(|c| Ok((db::all_projects(c)?, db::list_users(c)?))).await?;
    Ok(page("Admin", views::dashboard::admin(&projects, &users)))
}

async fn toggle(state: &AppState, id: i64, f: fn(&db::Conn, i64, bool) -> anyhow::Result<()>, get: fn(&Project) -> bool) -> AppResult<Response> {
    let p = state.db.run(move |c| db::project_by_id(c, id)).await?.ok_or(AppError::NotFound)?;
    let on = !get(&p);
    state.db.run(move |c| f(c, id, on)).await?;
    state.cache.invalidate(project_paths(&p.slug, &p.platform));
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
            let banned: bool = c.query_row("SELECT is_banned FROM users WHERE id = ?1", [uid], |r| r.get(0))?;
            db::set_banned(c, uid, !banned)?;
            db::projects_by_owner(c, uid)
        })
        .await?;
    for p in projects {
        state.cache.invalidate(project_paths(&p.slug, &p.platform));
    }
    Ok(Redirect::to("/admin").into_response())
}
