use std::collections::HashMap;

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use bytes::Bytes;
use serde::Deserialize;

use crate::{
    app::{ApiError, AppState},
    auth::{MaybeUser, RequireUser},
    cache::project_paths,
    db,
    objdiff,
    report::ReportInput,
    util::sha256_hex,
};

#[derive(Deserialize)]
pub struct ProgressQuery {
    /// `objdiff` forces objdiff's report format; by default it's detected from the body.
    format: Option<String>,
    /// Commit and message for report formats that don't carry them (objdiff).
    commit: Option<String>,
    message: Option<String>,
}

/// `POST /api/v1/progress` — the progress webhook. Accepts our own report format or an
/// objdiff `report.json` (optionally gzip-compressed with `Content-Encoding: gzip`).
pub async fn progress(State(state): State<AppState>, Query(q): Query<ProgressQuery>, headers: HeaderMap, body: Bytes) -> Result<Response, ApiError> {
    let key = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|k| k.starts_with("dk_"))
        .ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "missing or malformed `Authorization: Bearer <key>` header".into()))?;
    let hash = sha256_hex(key);
    let project = state
        .db
        .run(move |c| db::project_by_key_hash(c, &hash))
        .await?
        .ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "invalid API key".into()))?;

    if !state.limiter.check(&format!("progress:{}", project.id), 120, 3600) {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded (120 reports per hour)".into()));
    }

    let gzip = headers.get(header::CONTENT_ENCODING).is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"gzip"));

    // Parsing a multi-megabyte objdiff report is CPU work; keep it off the async workers.
    let report = tokio::task::spawn_blocking(move || -> Result<_, String> {
        let body = if gzip { gunzip(&body)? } else { body.to_vec() };
        let input = if q.format.as_deref() == Some("objdiff") || objdiff::looks_like_objdiff(&body) {
            let r: objdiff::Report = serde_json::from_slice(&body).map_err(|e| format!("invalid objdiff report: {e}"))?;
            let mut input = r.into_input(q.commit, q.message);
            input.layout = Some("treemap".into());
            input
        } else {
            let mut input: ReportInput = serde_json::from_slice(&body).map_err(|e| format!("invalid JSON: {e}"))?;
            input.commit = input.commit.or(q.commit);
            input.message = input.message.or(q.message);
            input
        };
        input.normalize()
    })
    .await
    .map_err(|e| anyhow::anyhow!(e))?
    .map_err(ApiError::bad)?;

    let commit = report.commit.clone();
    let before = project.clone();
    let changed = state.db.run(move |c| db::apply_report(c, &project, &report)).await?;
    if changed {
        state.cache.invalidate(project_paths(&before.slug, &before.platform));
        crate::github::spawn_commit_fetch(state.clone(), before.clone(), commit);
    }
    Ok(Json(serde_json::json!({
        "status": if changed { "updated" } else { "unchanged" },
        "project": before.slug,
    }))
    .into_response())
}

/// `GET /api/me` — who is logged in and how they voted. Never cached.
pub async fn me(State(state): State<AppState>, MaybeUser(user): MaybeUser) -> Result<Response, ApiError> {
    let body = match user {
        None => serde_json::json!({ "user": null }),
        Some(u) => {
            let uid = u.user.id;
            let votes: HashMap<String, i64> = state
                .db
                .run(move |c| db::user_votes(c, uid))
                .await?
                .into_iter()
                .map(|(pid, v)| (pid.to_string(), v))
                .collect();
            serde_json::json!({
                "user": { "id": uid, "login": u.user.login, "avatar_url": u.user.avatar_url, "is_admin": u.is_admin },
                "votes": votes,
            })
        }
    };
    Ok(([(header::CACHE_CONTROL, "private, no-store")], Json(body)).into_response())
}

#[derive(Deserialize)]
pub struct VoteInput {
    project_id: i64,
    value: i64,
}

/// `POST /api/v1/vote` — `{"project_id": 1, "value": 1 | 0 | -1}`.
pub async fn vote(State(state): State<AppState>, RequireUser(u): RequireUser, Json(v): Json<VoteInput>) -> Result<Response, ApiError> {
    if !(-1..=1).contains(&v.value) {
        return Err(ApiError::bad("value must be -1, 0 or 1"));
    }
    if !state.limiter.check(&format!("vote:{}", u.user.id), 30, 60) {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "You're voting too fast. Please wait a minute and try again.".into()));
    }
    let uid = u.user.id;
    let project = state
        .db
        .run(move |c| db::project_by_id(c, v.project_id))
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "This project no longer exists.".into()))?;
    if project.owner_id == uid {
        return Err(ApiError(StatusCode::FORBIDDEN, "You can't vote on your own project.".into()));
    }
    let pid = project.id;
    let (up, down) = state.db.run(move |c| db::vote(c, uid, pid, v.value)).await?;
    state.cache.invalidate(project_paths(&project.slug, &project.platform));
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({ "up": up, "down": down, "score": up - down, "value": v.value })),
    )
        .into_response())
}

/// Decompresses a gzip request body, refusing anything that inflates past 128 MB.
fn gunzip(data: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    const MAX: u64 = 128 * 1024 * 1024;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(data)
        .take(MAX + 1)
        .read_to_end(&mut out)
        .map_err(|_| "invalid gzip body".to_string())?;
    if out.len() as u64 > MAX {
        return Err("decompressed body is too large".into());
    }
    Ok(out)
}
