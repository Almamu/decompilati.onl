//! GitHub OAuth login, cookie sessions and request extractors.

use axum::{
    extract::{FromRequestParts, Query, State},
    http::{header, request::Parts, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;

use crate::{
    app::{ApiError, AppError, AppResult, AppState},
    db::{self, User},
    util::{random_token, sha256_hex},
};

const SESSION_COOKIE: &str = "sid";
const STATE_COOKIE: &str = "oauth_state";
const LOGIN_HINT_COOKIE: &str = "li";
const SESSION_TTL: i64 = 30 * 86400;

pub fn get_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

fn cookie(state: &AppState, name: &str, value: &str, max_age: i64) -> String {
    let secure = if state.config.secure_cookies { "; Secure" } else { "" };
    format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")
}

/// The logged-in user, if any.
pub struct MaybeUser(pub Option<SessionUser>);

#[derive(Clone)]
pub struct SessionUser {
    pub user: User,
    pub gh_token: String,
    pub is_admin: bool,
}

/// A logged-in user is required. For state-changing requests it also verifies the
/// `Origin` header (CSRF protection on top of `SameSite=Lax`).
pub struct RequireUser(pub SessionUser);

/// Like `RequireUser` but the user must also be a site admin.
pub struct RequireAdmin(pub SessionUser);

async fn load_user(parts: &Parts, state: &AppState) -> Result<Option<SessionUser>, AppError> {
    let Some(token) = get_cookie(&parts.headers, SESSION_COOKIE) else { return Ok(None) };
    let hash = sha256_hex(token);
    let found = state.db.run(move |c| db::session_user(c, &hash)).await?;
    Ok(found.filter(|(u, _)| !u.is_banned).map(|(user, gh_token)| SessionUser {
        is_admin: state.config.is_admin(&user.login),
        user,
        gh_token,
    }))
}

fn same_origin(parts: &Parts, state: &AppState) -> bool {
    if matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    let base = &state.config.base_url;
    let check = |h| parts.headers.get(h).and_then(|v| v.to_str().ok());
    match check(header::ORIGIN) {
        Some(origin) => origin == base,
        None => check(header::REFERER).is_some_and(|r| r == base || r.starts_with(&format!("{base}/"))),
    }
}

impl FromRequestParts<AppState> for MaybeUser {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        Ok(MaybeUser(load_user(parts, state).await?))
    }
}

pub enum AuthRejection {
    Login(String),
    Error(AppError),
    /// For `/api/` requests: a JSON error the page's script can show.
    Api(StatusCode, &'static str),
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        match self {
            AuthRejection::Login(next) => {
                Redirect::to(&format!("/auth/github?next={}", urlencode(&next))).into_response()
            }
            AuthRejection::Error(e) => e.into_response(),
            AuthRejection::Api(status, msg) => ApiError(status, msg.into()).into_response(),
        }
    }
}

impl FromRequestParts<AppState> for RequireUser {
    type Rejection = AuthRejection;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let api = parts.uri.path().starts_with("/api/");
        if !same_origin(parts, state) {
            return Err(if api {
                AuthRejection::Api(StatusCode::FORBIDDEN, "This request came from another site and was blocked.")
            } else {
                AuthRejection::Error(AppError::Forbidden("Cross-origin request rejected.".into()))
            });
        }
        match load_user(parts, state).await.map_err(AuthRejection::Error)? {
            Some(u) => Ok(RequireUser(u)),
            None if api => Err(AuthRejection::Api(StatusCode::UNAUTHORIZED, "Your session has expired. Please log in again.")),
            None => Err(AuthRejection::Login(
                if parts.method == Method::GET { parts.uri.path().to_string() } else { "/dashboard".into() },
            )),
        }
    }
}

impl FromRequestParts<AppState> for RequireAdmin {
    type Rejection = AuthRejection;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let RequireUser(u) = RequireUser::from_request_parts(parts, state).await?;
        if !u.is_admin {
            return Err(AuthRejection::Error(AppError::Forbidden("Admins only.".into())));
        }
        Ok(RequireAdmin(u))
    }
}

pub fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Only allow local redirect targets.
fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") && !n.contains('\\') => n.to_string(),
        _ => "/dashboard".into(),
    }
}

#[derive(Deserialize)]
pub struct LoginQuery {
    next: Option<String>,
}

pub async fn login(State(state): State<AppState>, Query(q): Query<LoginQuery>) -> AppResult<Response> {
    if state.config.github_client_id.is_empty() {
        return Err(AppError::BadRequest("GitHub login is not configured on this server.".into()));
    }
    let nonce = random_token();
    let next = safe_next(q.next.as_deref());
    let url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&state={}&allow_signup=true",
        urlencode(&state.config.github_client_id),
        urlencode(&format!("{}/auth/github/callback", state.config.base_url)),
        nonce,
    );
    // The state cookie carries the nonce and the post-login destination.
    let value = format!("{nonce}.{}", urlencode(&next));
    Ok((
        [
            (header::SET_COOKIE, cookie(&state, STATE_COOKIE, &value, 600)),
            (header::CACHE_CONTROL, "no-store".into()),
        ],
        Redirect::to(&url),
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct GithubUser {
    id: i64,
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub async fn callback(State(state): State<AppState>, headers: HeaderMap, Query(q): Query<CallbackQuery>) -> AppResult<Response> {
    let cookie_val = get_cookie(&headers, STATE_COOKIE).unwrap_or_default();
    let (nonce, next) = cookie_val.split_once('.').unwrap_or((cookie_val, ""));
    let (Some(code), Some(st)) = (q.code, q.state) else {
        return Err(AppError::BadRequest("Missing OAuth parameters.".into()));
    };
    if nonce.is_empty() || st != nonce {
        return Err(AppError::BadRequest("Login session expired, please try again.".into()));
    }
    let next = safe_next(Some(&percent_decode(next)));

    let token: TokenResponse = state
        .http
        .post("https://github.com/login/oauth/access_token")
        .header(header::ACCEPT, "application/json")
        .form(&[
            ("client_id", state.config.github_client_id.as_str()),
            ("client_secret", state.config.github_client_secret.as_str()),
            ("code", code.as_str()),
            ("redirect_uri", &format!("{}/auth/github/callback", state.config.base_url)),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let Some(gh_token) = token.access_token else {
        return Err(AppError::BadRequest(format!(
            "GitHub login failed: {}",
            token.error_description.unwrap_or_else(|| "unknown error".into())
        )));
    };
    let gh: GithubUser = state
        .http
        .get("https://api.github.com/user")
        .bearer_auth(&gh_token)
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if crate::util::is_reserved_name(&gh.login) {
        return Err(AppError::Forbidden(format!(
            "The GitHub account name \"{}\" is reserved for this site's own pages, so it can't be used to log in.",
            gh.login
        )));
    }
    let session = random_token();
    let session_hash = sha256_hex(&session);
    let user = state
        .db
        .run(move |c| {
            let user = db::upsert_user(c, gh.id, &gh.login, gh.name.as_deref(), gh.avatar_url.as_deref())?;
            if !user.is_banned {
                db::create_session(c, &session_hash, user.id, &gh_token, SESSION_TTL)?;
            }
            Ok(user)
        })
        .await?;
    if user.is_banned {
        return Err(AppError::Forbidden("This account has been suspended.".into()));
    }
    let mut res = Redirect::to(&next).into_response();
    let h = res.headers_mut();
    h.append(header::SET_COOKIE, cookie(&state, SESSION_COOKIE, &session, SESSION_TTL).parse().unwrap());
    h.append(header::SET_COOKIE, cookie(&state, STATE_COOKIE, "", 0).parse().unwrap());
    // Non-HttpOnly hint so cached pages only call /api/me for logged-in visitors.
    h.append(header::SET_COOKIE, cookie(&state, LOGIN_HINT_COOKIE, "1", SESSION_TTL).replace("; HttpOnly", "").parse().unwrap());
    h.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    Ok(res)
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    if let Some(token) = get_cookie(&headers, SESSION_COOKIE) {
        let hash = sha256_hex(token);
        state.db.run(move |c| db::delete_session(c, &hash)).await?;
    }
    let mut res = Redirect::to("/").into_response();
    let h = res.headers_mut();
    h.append(header::SET_COOKIE, cookie(&state, SESSION_COOKIE, "", 0).parse().unwrap());
    h.append(header::SET_COOKIE, cookie(&state, LOGIN_HINT_COOKIE, "", 0).replace("; HttpOnly", "").parse().unwrap());
    h.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    Ok(res)
}

#[derive(Deserialize)]
struct RepoInfo {
    id: i64,
    full_name: String,
    private: bool,
    #[serde(default)]
    permissions: Option<RepoPermissions>,
}

#[derive(Deserialize, Default)]
struct RepoPermissions {
    #[serde(default)]
    admin: bool,
    #[serde(default)]
    maintain: bool,
    #[serde(default)]
    push: bool,
}

pub enum RepoListError {
    /// The stored GitHub token is no longer valid; the user must log in again.
    Expired,
    Failed,
}

/// Public repositories the user can push to (`owner/name`), sorted by most recently pushed.
/// Cached per user for a few minutes so form re-renders don't hit GitHub every time.
/// A repository the user can push to: its current `owner/name` and GitHub's stable id.
#[derive(Clone, Debug)]
pub struct RepoRef {
    pub full_name: String,
    pub id: i64,
}

pub async fn pushable_repos(state: &AppState, user: &SessionUser) -> Result<Vec<RepoRef>, RepoListError> {
    const TTL: i64 = 300;
    let now = crate::util::now();
    if let Some((at, repos)) = state.repo_cache.lock().get(&user.user.id) {
        if now - at < TTL {
            return Ok(repos.clone());
        }
    }
    let mut repos = Vec::new();
    for page in 1..=10 {
        let res = state
            .http
            .get("https://api.github.com/user/repos")
            .query(&[
                ("affiliation", "owner,collaborator,organization_member"),
                ("sort", "pushed"),
                ("per_page", "100"),
                ("page", &page.to_string()),
            ])
            .bearer_auth(&user.gh_token)
            .header(header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .map_err(|_| RepoListError::Failed)?;
        if res.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(RepoListError::Expired);
        }
        let batch: Vec<RepoInfo> = res.error_for_status().map_err(|_| RepoListError::Failed)?.json().await.map_err(|_| RepoListError::Failed)?;
        let done = batch.len() < 100;
        repos.extend(
            batch
                .into_iter()
                .filter(|r| !r.private && r.permissions.as_ref().is_some_and(|p| p.admin || p.maintain || p.push))
                .map(|r| RepoRef { full_name: r.full_name, id: r.id }),
        );
        if done {
            break;
        }
    }
    let mut cache = state.repo_cache.lock();
    cache.retain(|_, (at, _)| now - *at < TTL);
    cache.insert(user.user.id, (now, repos.clone()));
    Ok(repos)
}
