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
}

#[derive(Deserialize)]
struct HeadCommit {
    message: String,
}

#[derive(Deserialize)]
struct Repository {
    full_name: String,
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
    tokio::spawn(import_run(state, app, installation.id, ev.repository.full_name, ev.workflow_run));
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

async fn import_run(state: AppState, app: GithubAppConfig, installation: u64, repo: String, run: WorkflowRun) {
    if let Err(e) = try_import_run(&state, &app, installation, &repo, run).await {
        tracing::warn!(%repo, error = %e, "github app import failed");
    }
}

async fn try_import_run(state: &AppState, app: &GithubAppConfig, installation: u64, repo: &str, run: WorkflowRun) -> anyhow::Result<()> {
    let r = repo.to_string();
    let projects = state.db.run(move |c| db::projects_by_repo(c, &r)).await?;
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
    let report = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let json = read_report(&zip_bytes)?;
        let r: objdiff::Report = serde_json::from_slice(&json)?;
        let mut input = r.into_input(Some(commit), message);
        input.layout = Some("treemap".into());
        input.version = Some(v);
        input.normalize().map_err(anyhow::Error::msg)
    })
    .await??;

    for project in projects {
        if !state.limiter.check(&format!("progress:{}", project.id), 120, 3600) {
            continue;
        }
        let (p, r) = (project.clone(), report.clone());
        let applied = state.db.run(move |c| db::apply_report(c, &p, &r)).await?;
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
