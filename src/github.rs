//! Fetches the commits behind a progress report so the project page can show what changed.

use axum::http::header;
use serde::{Deserialize, Serialize};

use crate::{app::AppState, cache::project_paths, db};

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
pub fn spawn_commit_fetch(state: AppState, project: db::Project, report_commit: Option<String>) {
    let prev = project.last_commit.clone().filter(|c| is_commit_hash(c));
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
        let id = project.id;
        let saved = state
            .db
            .run(move |c| db::set_commits(c, id, report_commit.as_deref(), &json))
            .await
            .unwrap_or(false);
        if saved {
            state.cache.invalidate(project_paths(&project.slug, &project.platform));
        }
    });
}
