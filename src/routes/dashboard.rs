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
    auth::{verify_repo_access, RequireAdmin, RequireUser, SessionUser},
    cache::project_paths,
    db::{self, Project, ProjectMeta},
    images, platforms,
    util::{clean_url, new_api_key, slugify, valid_repo, valid_slug},
    views::{
        self,
        dashboard::ProjectForm,
        layout::Page,
    },
};

const MAX_PROJECTS_PER_USER: usize = 25;

fn page(title: &str, body: Markup) -> Response {
    Html(views::layout::render(Page { title, description: "", image: None, body })).into_response()
}

pub async fn index(State(state): State<AppState>, RequireUser(u): RequireUser) -> AppResult<Response> {
    let uid = u.user.id;
    let projects = state.db.run(move |c| db::projects_by_owner(c, uid)).await?;
    Ok(page("Your projects", views::dashboard::dashboard(&u, &projects)))
}

pub async fn new_form(RequireUser(_u): RequireUser) -> Response {
    page("Add a project", views::dashboard::new_project(&ProjectForm::default(), None))
}

fn normalize_repo(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("http://github.com/"))
        .or_else(|| s.strip_prefix("github.com/"))
        .unwrap_or(s);
    s.trim_end_matches('/').trim_end_matches(".git").to_string()
}

/// Validates the form; checks GitHub access when the repository is new or changed.
async fn validate(state: &AppState, u: &SessionUser, f: &ProjectForm, existing: Option<&Project>) -> Result<ProjectMeta, String> {
    let name = f.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 100 {
        return Err("Name must be between 1 and 100 characters.".into());
    }
    let slug = if f.slug.trim().is_empty() { slugify(&name) } else { f.slug.trim().to_ascii_lowercase() };
    if !valid_slug(&slug) {
        return Err("The slug must be 2–64 lowercase letters, digits or dashes.".into());
    }
    if matches!(slug.as_str(), "new" | "admin" | "api" | "static") {
        return Err("That slug is reserved.".into());
    }
    let except = existing.map(|p| p.id).unwrap_or(0);
    let s = slug.clone();
    if state.db.run(move |c| db::slug_taken(c, &s, except)).await.map_err(|e| e.to_string())? {
        return Err(format!("The slug \"{slug}\" is already taken."));
    }
    if platforms::get(&f.platform).is_none() {
        return Err("Pick a platform.".into());
    }
    let mut repo = normalize_repo(&f.repo);
    if !valid_repo(&repo) {
        return Err("Repository must look like owner/name.".into());
    }
    let repo_changed = existing.is_none_or(|p| !p.repo.eq_ignore_ascii_case(&repo));
    if repo_changed {
        repo = verify_repo_access(state, u, &repo).await?;
    } else if let Some(p) = existing {
        repo = p.repo.clone();
    }
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
        return Ok(page("Add a project", views::dashboard::new_project(&f, Some("You've reached the maximum number of projects."))));
    }
    let meta = match validate(&state, &u, &f, None).await {
        Ok(m) => m,
        Err(e) => return Ok(page("Add a project", views::dashboard::new_project(&f, Some(&e)))),
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
    Ok(edit_page(&state, &u, &project, None, Some("Project created! Now set up your CI to report progress."), Some(&key)))
}

fn edit_page(state: &AppState, u: &SessionUser, p: &Project, error: Option<&str>, notice: Option<&str>, key: Option<&str>) -> Response {
    let f = ProjectForm::from_project(p);
    page(&p.name, views::dashboard::edit_project(u, p, &f, error, notice, key, &state.config.base_url))
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
    Ok(edit_page(&state, &u, &p, None, None, None))
}

pub async fn update(State(state): State<AppState>, RequireUser(u): RequireUser, Path(id): Path<i64>, Form(f): Form<ProjectForm>) -> AppResult<Response> {
    let p = owned(&state, &u, id).await?;
    let meta = match validate(&state, &u, &f, Some(&p)).await {
        Ok(m) => m,
        Err(e) => {
            return Ok(page(&p.name, views::dashboard::edit_project(&u, &p, &f, Some(&e), None, None, &state.config.base_url)));
        }
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
    // Both the old and new slug/platform pages are affected.
    state.cache.invalidate(project_paths(&p.slug, &p.platform));
    state.cache.invalidate(project_paths(&updated.slug, &updated.platform));
    Ok(edit_page(&state, &u, &updated, None, Some("Saved."), None))
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
        return Ok(edit_page(&state, &u, &p, Some("No image was uploaded."), None, None));
    };
    let dir = state.config.uploads_dir();
    let stored = tokio::task::spawn_blocking(move || images::store_cover(&data, &dir)).await?;
    match stored {
        Err(e) => Ok(edit_page(&state, &u, &p, Some(&e), None, None)),
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
            Ok(edit_page(&state, &u, &updated, None, Some("Image updated."), None))
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
    Ok(edit_page(&state, &u, &updated, None, Some("Image removed."), None))
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
    Ok(edit_page(&state, &u, &updated, None, Some("A new API key was generated."), Some(&key)))
}

#[derive(Deserialize)]
pub struct DeleteForm {
    confirm: String,
}

pub async fn delete(State(state): State<AppState>, RequireUser(u): RequireUser, Path(id): Path<i64>, Form(f): Form<DeleteForm>) -> AppResult<Response> {
    let p = owned(&state, &u, id).await?;
    if f.confirm.trim() != p.slug {
        return Ok(edit_page(&state, &u, &p, Some("The confirmation didn't match the slug; nothing was deleted."), None, None));
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
