//! Imports a project's progress history from decomp.dev.
//!
//! decomp.dev serves, per project, a summary (`/{owner}/{repo}.json`, with the versions), the
//! history of each version (`/{owner}/{repo}/{version}.json?mode=history`: totals per commit,
//! newest first) and the latest full objdiff report (`?mode=report`). The history becomes our
//! history; the newest entry uses the full report so the current progress gets its areas.

use std::collections::BTreeSet;

use serde::Deserialize;

use crate::{app::AppState, cache::version_paths, db, imports, objdiff, report::valid_version};

const MAX_RESPONSE: usize = 64 * 1024 * 1024;

fn valid_name(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && s.len() <= 100 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// Parses `https://decomp.dev/{owner}/{repo}` (optionally with `.json` or more path after it).
pub fn parse_url(url: &str) -> Option<(String, String)> {
    let rest = url.trim().strip_prefix("https://decomp.dev/").or_else(|| url.trim().strip_prefix("decomp.dev/"))?;
    let rest = rest.split(['?', '#']).next()?;
    let mut parts = rest.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?.trim_end_matches(".json");
    (valid_name(owner) && valid_name(repo)).then(|| (owner.to_string(), repo.to_string()))
}

/// Queues an import from decomp.dev. The decomp.dev project must be the same repository.
pub async fn start(state: &AppState, project: &db::Project, url: &str, user_id: i64) -> Result<(), String> {
    let (owner, repo) = parse_url(url).ok_or("Enter the project's decomp.dev URL, like https://decomp.dev/owner/repo")?;
    if !format!("{owner}/{repo}").eq_ignore_ascii_case(&project.repo) {
        return Err(format!("That decomp.dev page is for {owner}/{repo}, but this project's repository is {}.", project.repo));
    }
    imports::queue(state, project, user_id, "decompdev", Some(format!("https://decomp.dev/{owner}/{repo}"))).await
}

/// Errors that mean "decomp.dev is unreachable or overloaded right now": the job retries later.
fn transient(e: reqwest::Error) -> anyhow::Error {
    let temporary = e.is_connect()
        || e.is_timeout()
        || e.status().is_some_and(|s| s.is_server_error() || s == reqwest::StatusCode::TOO_MANY_REQUESTS);
    if temporary { anyhow::Error::new(imports::Retryable(format!("decomp.dev is unreachable ({e})"))) } else { e.into() }
}

async fn get_json<T: serde::de::DeserializeOwned + Send + 'static>(state: &AppState, url: &str) -> anyhow::Result<T> {
    let body = state
        .http
        .get(url)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(transient)?
        .error_for_status()
        .map_err(transient)?
        .bytes()
        .await
        .map_err(transient)?;
    anyhow::ensure!(body.len() <= MAX_RESPONSE, "response from {url} is too large");
    let url = url.to_string();
    tokio::task::spawn_blocking(move || serde_json::from_slice(&body).map_err(|e| anyhow::anyhow!("unexpected response from {url}: {e}"))).await?
}

#[derive(Deserialize)]
struct Summary {
    #[serde(default)]
    report_versions: Vec<String>,
    default_version: Option<String>,
}

#[derive(Deserialize)]
struct HistoryEntry {
    timestamp: String,
    commit_sha: Option<String>,
    commit_message: Option<String>,
    measures: objdiff::Measures,
}

/// Job body for `decompdev` imports (run by `imports`).
pub async fn import(state: &AppState, project: &db::Project, url: &str, job: i64) -> anyhow::Result<()> {
    let (owner, repo) = parse_url(url).ok_or_else(|| anyhow::anyhow!("invalid decomp.dev URL"))?;
    let base = format!("https://decomp.dev/{owner}/{repo}");
    let summary: Summary = get_json(state, &format!("{base}.json")).await?;
    let versions: Vec<String> = summary.report_versions.into_iter().filter(|v| valid_version(v)).collect();
    anyhow::ensure!(!versions.is_empty(), "decomp.dev has no versions for {owner}/{repo}");

    let pid = project.id;
    let had_versions = !state.db.run(move |c| db::versions_for_project(c, pid)).await?.is_empty();

    // Fetch everything first so the progress counter knows the total.
    let mut histories = Vec::new();
    for version in &versions {
        let mut entries: Vec<HistoryEntry> = get_json(state, &format!("{base}/{version}.json?mode=history")).await?;
        entries.reverse(); // oldest first
        // The full report of the newest entry, for the areas. Optional: totals still work without it.
        let latest: Option<objdiff::Report> = get_json(state, &format!("{base}/{version}.json?mode=report")).await.ok();
        histories.push((version.clone(), entries, latest));
    }
    let total: usize = histories.iter().map(|(_, e, _)| e.len()).sum();
    imports::progress(state, job, total, 0, 0).await;

    let (mut done, mut imported) = (0, 0);
    let mut touched = BTreeSet::new();
    for (version, entries, latest) in histories {
        let mut latest = latest;
        let last = entries.len().saturating_sub(1);
        for (i, entry) in entries.into_iter().enumerate() {
            done += 1;
            let Some(at) = crate::util::parse_timestamp(&entry.timestamp) else { continue };
            let mut input = match latest.take().filter(|_| i == last) {
                Some(report) => report.into_input(entry.commit_sha, entry.commit_message),
                None => entry.measures.into_input(entry.commit_sha, entry.commit_message),
            };
            input.version = Some(version.clone());
            input.layout = Some("treemap".into());
            let Ok(report) = input.normalize() else { continue };
            let p = project.clone();
            if state.db.run(move |c| db::apply_report(c, &p, &report, Some(at))).await?.is_some() {
                imported += 1;
                touched.insert(version.clone());
            }
            if done % 25 == 0 {
                imports::progress(state, job, total, done, imported).await;
            }
        }
    }
    imports::progress(state, job, total, done, imported).await;

    // A project that had nothing before takes decomp.dev's default version.
    if let Some(default) = summary.default_version.filter(|v| !had_versions && touched.contains(v)) {
        state.db.run(move |c| db::set_default_version(c, pid, &default)).await?;
    }
    for version in &touched {
        state.cache.invalidate(version_paths(&project.slug, &project.platform, version));
    }
    tracing::info!(repo = %project.repo, imported, "decomp.dev import finished");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_urls() {
        let want = Some(("Almamu".to_string(), "CrashBandicootXS-decomp".to_string()));
        assert_eq!(parse_url("https://decomp.dev/Almamu/CrashBandicootXS-decomp.json"), want);
        assert_eq!(parse_url("https://decomp.dev/Almamu/CrashBandicootXS-decomp"), want);
        assert_eq!(parse_url("decomp.dev/Almamu/CrashBandicootXS-decomp/europe?mode=history"), want);
        assert_eq!(parse_url("https://evil.example/Almamu/x"), None);
        assert_eq!(parse_url("https://decomp.dev/../etc"), None);
    }
}

/// Whether decomp.dev has progress for a repository (`owner/name`).
async fn has_project(state: &AppState, owner: &str, name: &str) -> anyhow::Result<bool> {
    let res = state
        .http
        .get(format!("https://decomp.dev/{owner}/{name}.json"))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(transient)?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    let summary: Summary = res.error_for_status().map_err(transient)?.json().await.map_err(transient)?;
    Ok(summary.report_versions.iter().any(|v| valid_version(v)))
}

/// Queues the job that, for a newly created project, imports its decomp.dev history if
/// decomp.dev has it. Never contacts decomp.dev itself, so creating a project doesn't
/// depend on it being up.
pub async fn queue_auto_import(state: &AppState, project: &db::Project, user_id: i64) -> Result<(), String> {
    imports::queue(state, project, user_id, "decompdev-auto", Some(format!("https://decomp.dev/{}", project.repo))).await
}

/// Job body for `decompdev-auto`: check decomp.dev, then import if the project is there.
/// Returns `false` when decomp.dev has nothing for it.
pub async fn auto_import(state: &AppState, project: &db::Project, url: &str, job: i64) -> anyhow::Result<bool> {
    let (owner, repo) = parse_url(url).ok_or_else(|| anyhow::anyhow!("invalid decomp.dev URL"))?;
    if !has_project(state, &owner, &repo).await? {
        return Ok(false);
    }
    import(state, project, url, job).await?;
    Ok(true)
}
