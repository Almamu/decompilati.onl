//! decomp.dev-compatible "pull" mode, through a GitHub App.
//!
//! Projects install the app on their repository and keep uploading objdiff's `report.json`
//! as a GitHub Actions artifact named `<version>_report`, as they did for decomp.dev. When a
//! workflow run on the default branch completes successfully, GitHub sends a `workflow_run`
//! webhook; we download the report artifact with an installation token and store it like
//! any other progress report.

use std::io::Read;

use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

use crate::{app::AppState, cache::version_paths, db, objdiff};

const MAX_ARTIFACT: u64 = 64 * 1024 * 1024;
const MAX_REPORT: u64 = 128 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct GithubAppConfig {
    pub app_id: String,
    pub slug: String,
    pub webhook_secret: String,
    pub private_key_pem: String,
}

#[derive(Deserialize)]
struct WorkflowRunEvent {
    action: String,
    workflow_run: WorkflowRun,
    repository: Repository,
    installation: Option<Installation>,
}

#[derive(Deserialize)]
struct WorkflowRun {
    id: u64,
    event: String,
    head_branch: Option<String>,
    head_sha: String,
    conclusion: Option<String>,
    head_commit: Option<HeadCommit>,
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Deserialize)]
struct HeadCommit {
    message: String,
}

#[derive(Deserialize)]
struct Repository {
    id: i64,
    full_name: String,
    #[serde(default)]
    default_branch: String,
}

#[derive(Deserialize)]
struct Installation {
    id: u64,
}

#[derive(Deserialize)]
struct ArtifactList {
    artifacts: Vec<Artifact>,
}

#[derive(Deserialize)]
struct Artifact {
    name: String,
    archive_download_url: String,
    size_in_bytes: u64,
    expired: bool,
}

fn verify_signature(secret: &str, body: &[u8], header: Option<&str>) -> bool {
    let Some(sig) = header.and_then(|h| h.strip_prefix("sha256=")).and_then(|h| hex::decode(h).ok()) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else { return false };
    mac.update(body);
    mac.verify_slice(&sig).is_ok()
}

/// `POST /api/github/webhook`
pub async fn webhook(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(app) = state.config.github_app.clone() else {
        return (StatusCode::NOT_FOUND, "GitHub App not configured").into_response();
    };
    let sig = headers.get("x-hub-signature-256").and_then(|v| v.to_str().ok());
    if !verify_signature(&app.webhook_secret, &body, sig) {
        return (StatusCode::UNAUTHORIZED, "bad signature").into_response();
    }
    let event = headers.get("x-github-event").and_then(|v| v.to_str().ok()).unwrap_or("");
    if event == "repository" {
        // Renamed or transferred: keep the stored name current (matched by the stable id).
        #[derive(Deserialize)]
        struct RepositoryEvent {
            action: String,
            repository: Repository,
        }
        if let Ok(ev) = serde_json::from_slice::<RepositoryEvent>(&body) {
            if ev.action == "renamed" || ev.action == "transferred" {
                crate::github::note_repo_name(&state, ev.repository.id, &ev.repository.full_name).await;
            }
        }
        return (StatusCode::OK, "ok").into_response();
    }
    if event != "workflow_run" {
        // `ping`, `installation` and friends need no action.
        return (StatusCode::OK, "ignored").into_response();
    }
    let ev: WorkflowRunEvent = match serde_json::from_slice(&body) {
        Ok(e) => e,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("invalid payload: {e}")).into_response(),
    };
    let run = &ev.workflow_run;
    let on_default_branch = run.head_branch.as_deref() == Some(ev.repository.default_branch.as_str());
    if ev.action != "completed"
        || run.conclusion.as_deref() != Some("success")
        || !on_default_branch
        || run.event == "pull_request"
    {
        return (StatusCode::OK, "ignored").into_response();
    }
    let Some(installation) = ev.installation else {
        return (StatusCode::OK, "ignored: no installation").into_response();
    };
    // GitHub expects a quick answer; the download and import happen in the background.
    tokio::spawn(import_run(state, app, installation.id, ev.repository.id, ev.repository.full_name, ev.workflow_run));
    (StatusCode::ACCEPTED, "queued").into_response()
}

fn app_jwt(app: &GithubAppConfig) -> anyhow::Result<String> {
    #[derive(serde::Serialize)]
    struct Claims<'a> {
        iat: i64,
        exp: i64,
        iss: &'a str,
    }
    let now = crate::util::now();
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(app.private_key_pem.as_bytes())?;
    Ok(jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &Claims { iat: now - 60, exp: now + 540, iss: &app.app_id },
        &key,
    )?)
}

async fn installation_token(state: &AppState, app: &GithubAppConfig, installation: u64) -> anyhow::Result<String> {
    #[derive(Deserialize)]
    struct Token {
        token: String,
    }
    let t: Token = state
        .http
        .post(format!("https://api.github.com/app/installations/{installation}/access_tokens"))
        .bearer_auth(app_jwt(app)?)
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(t.token)
}

/// Reads the report out of an artifact zip: `report.json` if present, otherwise the first
/// `.json` file.
fn read_report(zip_bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))?;
    let names: Vec<String> = zip.file_names().map(str::to_string).collect();
    let name = names
        .iter()
        .find(|n| n.rsplit('/').next() == Some("report.json"))
        .or_else(|| names.iter().find(|n| n.ends_with(".json")))
        .ok_or_else(|| anyhow::anyhow!("no JSON report in the artifact (binary protobuf reports aren't supported)"))?
        .clone();
    let mut out = Vec::new();
    zip.by_name(&name)?.take(MAX_REPORT + 1).read_to_end(&mut out)?;
    anyhow::ensure!((out.len() as u64) <= MAX_REPORT, "report is too large");
    Ok(out)
}

async fn import_run(state: AppState, app: GithubAppConfig, installation: u64, repo_id: i64, repo: String, run: WorkflowRun) {
    if let Err(e) = try_import_run(&state, &app, installation, repo_id, &repo, run).await {
        tracing::warn!(%repo, error = %e, "github app import failed");
    }
}

async fn try_import_run(
    state: &AppState,
    app: &GithubAppConfig,
    installation: u64,
    repo_id: i64,
    repo: &str,
    run: WorkflowRun,
) -> anyhow::Result<()> {
    // The payload has the repository's current name: refresh projects stored under an old one,
    // and give projects matched by name their id.
    crate::github::note_repo_name(state, repo_id, repo).await;
    let r = repo.to_string();
    let projects = state
        .db
        .run(move |c| {
            let by_id = db::projects_by_repo_id(c, repo_id)?;
            let by_name: Vec<db::Project> = db::projects_by_repo(c, &r)?
                .into_iter()
                .filter(|p| p.github_repo_id.is_none())
                .collect();
            for p in &by_name {
                db::set_repo_identity(c, p.id, &r, repo_id)?;
            }
            Ok(by_id.into_iter().chain(by_name).collect::<Vec<_>>())
        })
        .await?;
    let projects: Vec<db::Project> = projects.into_iter().filter(|p| !p.app_artifacts.trim().is_empty()).collect();
    if projects.is_empty() {
        return Ok(());
    }
    let token = installation_token(state, app, installation).await?;
    let list: ArtifactList = state
        .http
        .get(format!("https://api.github.com/repos/{repo}/actions/runs/{}/artifacts?per_page=100", run.id))
        .bearer_auth(&token)
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    // decomp.dev's convention: one `<version>_report` artifact per game version.
    let reports = report_artifacts(&list);
    if reports.is_empty() {
        tracing::info!(%repo, run = run.id, "workflow run has no *_report artifact");
        return Ok(());
    }
    let message = run.head_commit.as_ref().map(|c| c.message.clone());
    for (version, artifact) in reports {
        if let Err(e) = import_artifact(state, &token, repo, &run.head_sha, message.clone(), &version, artifact, &projects).await {
            tracing::warn!(%repo, artifact = %artifact.name, error = %e, "couldn't import report artifact");
        }
    }
    Ok(())
}

/// Downloads a `<version>_report` artifact and converts the objdiff report inside it.
async fn fetch_artifact_report(
    state: &AppState,
    token: &str,
    artifact: &Artifact,
    version: &str,
    head_sha: &str,
    message: Option<String>,
) -> anyhow::Result<crate::report::Report> {
    anyhow::ensure!(artifact.size_in_bytes <= MAX_ARTIFACT, "artifact is too large");
    let zip_bytes = state
        .http
        .get(&artifact.archive_download_url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    anyhow::ensure!((zip_bytes.len() as u64) <= MAX_ARTIFACT, "artifact is too large");
    let (commit, v) = (head_sha.to_string(), version.to_string());
    tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let json = read_report(&zip_bytes)?;
        let r: objdiff::Report = serde_json::from_slice(&json)?;
        let mut input = r.into_input(Some(commit), message);
        input.layout = Some("treemap".into());
        input.version = Some(v);
        input.normalize().map_err(anyhow::Error::msg)
    })
    .await?
}

#[allow(clippy::too_many_arguments)]
async fn import_artifact(
    state: &AppState,
    token: &str,
    repo: &str,
    head_sha: &str,
    message: Option<String>,
    version: &str,
    artifact: &Artifact,
    projects: &[db::Project],
) -> anyhow::Result<()> {
    let report = fetch_artifact_report(state, token, artifact, version, head_sha, message).await?;
    // Only into the projects that take this artifact (several can share a repository).
    for project in projects.iter().filter(|p| p.wants_artifact(&artifact.name)) {
        if !state.limiter.check(&format!("progress:{}", project.id), 120, 3600) {
            continue;
        }
        let (p, r) = (project.clone(), report.clone());
        let applied = state.db.run(move |c| db::apply_report(c, &p, &r, None)).await?;
        tracing::info!(%repo, project = %project.slug, %version, changed = applied.is_some(), "imported report from workflow run");
        if let Some(applied) = applied {
            state.cache.invalidate(version_paths(&project.slug, &project.platform, version));
            crate::github::spawn_commit_fetch(
                state.clone(),
                project.clone(),
                version.to_string(),
                applied.prev_commit,
                Some(head_sha.to_string()),
            );
        }
    }
    Ok(())
}

/// `<version>_report` artifacts of a run (`report` alone is the unnamed version), by version.
fn report_artifacts(list: &ArtifactList) -> Vec<(String, &Artifact)> {
    let mut reports: Vec<(String, &Artifact)> = list
        .artifacts
        .iter()
        .filter(|a| !a.expired)
        .filter_map(|a| {
            let version = if a.name == "report" { Some("") } else { a.name.strip_suffix("_report") }?;
            crate::report::valid_version(version).then(|| (version.to_string(), a))
        })
        .collect();
    reports.sort_by(|a, b| a.0.cmp(&b.0));
    reports
}

async fn gh_get<T: serde::de::DeserializeOwned>(state: &AppState, token: &str, url: &str) -> anyhow::Result<T> {
    Ok(state
        .http
        .get(url)
        .bearer_auth(token)
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

// ---------------------------------------------------------------- history import

/// The app's installation on a repository, if it's installed there.
async fn installation_for_repo(state: &AppState, app: &GithubAppConfig, repo: &str) -> anyhow::Result<Option<u64>> {
    #[derive(Deserialize)]
    struct Inst {
        id: u64,
    }
    let res = state
        .http
        .get(format!("https://api.github.com/repos/{repo}/installation"))
        .bearer_auth(app_jwt(app)?)
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    Ok(Some(res.error_for_status()?.json::<Inst>().await?.id))
}

#[derive(Deserialize)]
struct RunList {
    workflow_runs: Vec<WorkflowRun>,
}

/// Queues an import of a project's history from its past workflow runs. Errors if the app
/// isn't installed on the repository or an import is already queued or running.
pub async fn start_history_import(state: &AppState, project: &db::Project, user_id: i64) -> Result<(), String> {
    let app = state.config.github_app.clone().ok_or("The GitHub App isn't configured on this server.")?;
    if project.app_artifacts.trim().is_empty() {
        return Err("GitHub App imports are turned off for this project; choose which artifacts to import first.".into());
    }
    installation_for_repo(state, &app, &project.repo)
        .await
        .map_err(|_| "Couldn't reach GitHub, please try again.".to_string())?
        .ok_or_else(|| format!("Install the GitHub App on {} first.", project.repo))?;
    crate::imports::queue(state, project, user_id, "github", None).await
}

/// Job body for `github` imports (run by `imports`).
pub async fn import_from_actions(state: &AppState, project: &db::Project, job: i64) -> anyhow::Result<()> {
    let app = state.config.github_app.clone().ok_or_else(|| anyhow::anyhow!("the GitHub App isn't configured"))?;
    let installation = installation_for_repo(state, &app, &project.repo)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the GitHub App isn't installed on {} anymore", project.repo))?;
    import_history(state, &app, installation, project, job).await
}

async fn import_history(state: &AppState, app: &GithubAppConfig, installation: u64, project: &db::Project, job: i64) -> anyhow::Result<()> {
    const MAX_RUNS: usize = 1000;
    let repo = &project.repo;
    let token = installation_token(state, app, installation).await?;
    #[derive(Deserialize)]
    struct RepoInfo {
        default_branch: String,
    }
    let info: RepoInfo = gh_get(state, &token, &format!("https://api.github.com/repos/{repo}")).await?;

    // Successful runs on the default branch, newest first from the API.
    let mut runs = Vec::new();
    for page in 1..=MAX_RUNS / 100 {
        let url = format!(
            "https://api.github.com/repos/{repo}/actions/runs?branch={}&status=success&per_page=100&page={page}",
            crate::auth::urlencode(&info.default_branch)
        );
        let list: RunList = gh_get(state, &token, &url).await?;
        let done = list.workflow_runs.len() < 100;
        runs.extend(list.workflow_runs.into_iter().filter(|r| r.event != "pull_request"));
        if done {
            break;
        }
    }
    runs.reverse();
    let total = runs.len();
    let progress = |done: usize, imported: usize| crate::imports::progress(state, job, total, done, imported);
    progress(0, 0).await;

    let mut imported = 0;
    let mut versions_touched = std::collections::BTreeSet::new();
    for (done, run) in runs.into_iter().enumerate() {
        let at = run.updated_at.as_deref().and_then(crate::util::parse_timestamp);
        let artifacts: ArtifactList = match gh_get(
            state,
            &token,
            &format!("https://api.github.com/repos/{repo}/actions/runs/{}/artifacts?per_page=100", run.id),
        )
        .await
        {
            Ok(a) => a,
            Err(e) => {
                tracing::debug!(%repo, run = run.id, error = %e, "couldn't list artifacts");
                ArtifactList { artifacts: vec![] }
            }
        };
        let message = run.head_commit.as_ref().map(|c| c.message.clone());
        for (version, artifact) in report_artifacts(&artifacts).into_iter().filter(|(_, a)| project.wants_artifact(&a.name)) {
            let report = match fetch_artifact_report(state, &token, artifact, &version, &run.head_sha, message.clone()).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!(%repo, run = run.id, error = %e, "skipping artifact");
                    continue;
                }
            };
            let p = project.clone();
            if state.db.run(move |c| db::apply_report(c, &p, &report, at)).await?.is_some() {
                versions_touched.insert(version);
                imported += 1;
            }
        }
        progress(done + 1, imported).await;
    }
    for version in &versions_touched {
        state.cache.invalidate(version_paths(&project.slug, &project.platform, version));
    }
    tracing::info!(%repo, imported, "history import finished");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature() {
        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(b"body");
        let sig = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        assert!(verify_signature("secret", b"body", Some(&sig)));
        assert!(!verify_signature("secret", b"other", Some(&sig)));
        assert!(!verify_signature("secret", b"body", None));
    }
}
