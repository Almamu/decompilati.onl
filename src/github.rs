//! Fetches the commits behind a progress report so the project page can show what changed.

use axum::http::header;
use serde::{Deserialize, Serialize};

use crate::{app::AppState, cache::version_paths, db};

/// How many commits are kept for display.
const SHOWN: usize = 8;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CommitList {
    #[serde(default)]
    pub commits: Vec<CommitInfo>,
    /// Total commits between the previous and this report (when known), which can be more
    /// than the ones kept.
    #[serde(default)]
    pub total: usize,
    /// `base...head` when the commits come from comparing two reports.
    #[serde(default)]
    pub compare: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub author: Option<String>,
    pub date: Option<String>,
}

#[derive(Deserialize)]
struct GhCommit {
    sha: String,
    commit: GhCommitBody,
    author: Option<GhUser>,
}

#[derive(Deserialize)]
struct GhCommitBody {
    message: String,
    author: Option<GhGitAuthor>,
}

#[derive(Deserialize)]
struct GhGitAuthor {
    name: Option<String>,
    date: Option<String>,
}

#[derive(Deserialize)]
struct GhUser {
    login: String,
}

#[derive(Deserialize)]
struct GhCompare {
    total_commits: usize,
    commits: Vec<GhCommit>,
}

pub fn is_commit_hash(s: &str) -> bool {
    (7..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl From<GhCommit> for CommitInfo {
    fn from(c: GhCommit) -> Self {
        let git_author = c.commit.author;
        CommitInfo {
            sha: c.sha,
            message: c.commit.message.lines().next().unwrap_or("").chars().take(200).collect(),
            author: c.author.map(|a| a.login).or_else(|| git_author.as_ref().and_then(|a| a.name.clone())),
            date: git_author.and_then(|a| a.date),
        }
    }
}

async fn get<T: serde::de::DeserializeOwned>(state: &AppState, url: &str) -> anyhow::Result<T> {
    let mut req = state.http.get(url).header(header::ACCEPT, "application/vnd.github+json");
    if let Some(token) = &state.config.github_api_token {
        req = req.bearer_auth(token);
    }
    Ok(req.send().await?.error_for_status()?.json().await?)
}

async fn fetch(state: &AppState, repo: &str, prev: Option<&str>, head: Option<&str>) -> anyhow::Result<CommitList> {
    // Commits between the previous report and this one, newest first.
    if let (Some(prev), Some(head)) = (prev, head) {
        if prev != head {
            let url = format!("https://api.github.com/repos/{repo}/compare/{prev}...{head}");
            if let Ok(cmp) = get::<GhCompare>(state, &url).await {
                return Ok(CommitList {
                    total: cmp.total_commits,
                    commits: cmp.commits.into_iter().rev().take(SHOWN).map(Into::into).collect(),
                    compare: Some(format!("{prev}...{head}")),
                });
            }
        }
    }
    // Otherwise the latest commits up to this report's commit (or the default branch).
    let mut url = format!("https://api.github.com/repos/{repo}/commits?per_page={SHOWN}");
    if let Some(head) = head {
        url.push_str(&format!("&sha={head}"));
    }
    let commits: Vec<GhCommit> = get(state, &url).await?;
    Ok(CommitList { total: 0, commits: commits.into_iter().map(Into::into).collect(), compare: None })
}

/// Background task run after a report is stored: fetch and save the commits, then refresh
/// the cached pages.
pub fn spawn_commit_fetch(state: AppState, project: db::Project, version: String, prev_commit: Option<String>, report_commit: Option<String>) {
    // Commit lists come from the GitHub API.
    if !project.is_github() {
        return;
    }
    let prev = prev_commit.filter(|c| is_commit_hash(c));
    let head = report_commit.clone().filter(|c| is_commit_hash(c));
    tokio::spawn(async move {
        let list = match fetch(&state, &project.repo, prev.as_deref(), head.as_deref()).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(repo = %project.repo, error = %e, "couldn't fetch commits");
                return;
            }
        };
        let json = match serde_json::to_string(&list) {
            Ok(j) => j,
            Err(_) => return,
        };
        let (id, v) = (project.id, version.clone());
        let saved = state
            .db
            .run(move |c| db::set_commits(c, id, &v, report_commit.as_deref(), &json))
            .await
            .unwrap_or(false);
        if saved {
            state.cache.invalidate(version_paths(&project.slug, &project.platform, &version));
        }
    });
}

#[derive(Deserialize)]
struct RepoIdentity {
    id: i64,
    full_name: String,
}

/// Starts the task that keeps repository names current: right away (which also fills in
/// missing repository ids) and then once a day. GitHub redirects renamed and transferred
/// repositories, so looking them up by id, or by their old name, gives the current name.
pub fn spawn_repo_refresh(state: AppState) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = refresh_repo_names(&state).await {
                tracing::warn!(error = %e, "couldn't refresh repository names");
            }
            tokio::time::sleep(std::time::Duration::from_secs(24 * 3600)).await;
        }
    });
}

async fn refresh_repo_names(state: &AppState) -> anyhow::Result<()> {
    let projects = state.db.run(db::repo_identities).await?;
    let mut renamed = 0;
    for p in projects {
        let url = match p.repo_id {
            Some(id) => format!("https://api.github.com/repositories/{id}"),
            None => format!("https://api.github.com/repos/{}", p.repo),
        };
        // A deleted or private repository (404) or a rate limit just keeps the stored name.
        let Ok(current) = get::<RepoIdentity>(state, &url).await else { continue };
        if p.repo_id == Some(current.id) && p.repo == current.full_name {
            continue;
        }
        let (pid, name, id) = (p.project_id, current.full_name.clone(), current.id);
        state.db.run(move |c| db::set_repo_identity(c, pid, &name, id)).await?;
        if p.repo != current.full_name {
            renamed += 1;
            tracing::info!(project = %p.slug, from = %p.repo, to = %current.full_name, "repository renamed");
            state.cache.invalidate(crate::cache::project_paths(&p.slug, &p.platform));
        }
        // Stay well within the API rate limit.
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    if renamed > 0 {
        tracing::info!(renamed, "refreshed repository names");
    }
    Ok(())
}

/// Called with what GitHub says a repository is currently called (e.g. from a webhook):
/// updates projects registered under its old name.
pub async fn note_repo_name(state: &AppState, repo_id: i64, full_name: &str) {
    let name = full_name.to_string();
    match state.db.run(move |c| db::rename_repo(c, repo_id, &name)).await {
        Ok(changed) => {
            for (slug, platform) in changed {
                tracing::info!(project = %slug, to = %full_name, "repository renamed");
                state.cache.invalidate(crate::cache::project_paths(&slug, &platform));
            }
        }
        Err(e) => tracing::warn!(error = %e, "couldn't record a repository rename"),
    }
}
