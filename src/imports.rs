//! Queue of history import jobs (from GitHub Actions artifacts or from decomp.dev).
//!
//! Jobs are stored in the `import_jobs` table, so their outcome is kept and unfinished ones
//! resume after a restart. At most `MAX_CONCURRENT_IMPORTS` run at the same time.

use crate::{app::AppState, db};

/// How many history imports may run at the same time; the rest wait in the queue.
pub const MAX_CONCURRENT_IMPORTS: usize = 10;
/// A job that keeps getting interrupted or retried is given up after this many starts.
const MAX_ATTEMPTS: i64 = 8;
/// Waits (in minutes) before retrying a job whose source was temporarily unreachable.
const RETRY_DELAYS_MIN: &[u64] = &[1, 5, 15, 60];

/// An error worth retrying later (e.g. decomp.dev being down), rather than failing the job.
#[derive(Debug)]
pub struct Retryable(pub String);

impl std::fmt::Display for Retryable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Retryable {}

/// Queues an import job for a project. Errors if one is already queued or running.
pub async fn queue(state: &AppState, project: &db::Project, user_id: i64, kind: &'static str, source: Option<String>) -> Result<(), String> {
    let pid = project.id;
    let job = state
        .db
        .run(move |c| db::create_import_job(c, pid, user_id, kind, source.as_deref()))
        .await
        .map_err(|e| e.to_string())?
        .ok_or("An import is already queued or running for this project.")?;
    spawn_job(state.clone(), job, project.id);
    Ok(())
}

/// Restarts the jobs a previous run of the server didn't finish. Call once at startup.
pub async fn resume(state: &AppState) -> anyhow::Result<()> {
    let jobs = state.db.run(|c| db::unfinished_import_jobs(c)).await?;
    if !jobs.is_empty() {
        tracing::info!(count = jobs.len(), "resuming history imports");
    }
    for (job, project) in jobs {
        spawn_job(state.clone(), job, project);
    }
    Ok(())
}

/// Records how far a job got.
pub async fn progress(state: &AppState, job: i64, total: usize, done: usize, imported: usize) {
    let _ = state.db.run(move |c| db::import_job_progress(c, job, total, done, imported)).await;
}

/// Runs a queued job once one of the `MAX_CONCURRENT_IMPORTS` slots is free. Jobs failing
/// with a `Retryable` error go back to the queue and run again after a delay, without
/// holding a slot while they wait.
fn spawn_job(state: AppState, job: i64, project_id: i64) {
    tokio::spawn(async move {
        let mut retries = 0;
        loop {
            let result = {
                let Ok(_slot) = state.import_slots.clone().acquire_owned().await else { return };
                run_job(&state, job, project_id).await
            };
            if let Err(e) = &result {
                if e.downcast_ref::<Retryable>().is_some() && retries < RETRY_DELAYS_MIN.len() {
                    let minutes = RETRY_DELAYS_MIN[retries];
                    retries += 1;
                    let message = format!("{e}; retrying in {minutes} min");
                    tracing::info!(job, project_id, %message, "history import will be retried");
                    let _ = state.db.run(move |c| db::requeue_import_job(c, job, &message)).await;
                    tokio::time::sleep(std::time::Duration::from_secs(minutes * 60)).await;
                    continue;
                }
            }
            let error = result.err().map(|e| e.to_string());
            if let Some(e) = &error {
                tracing::warn!(job, project_id, error = %e, "history import failed");
            }
            if let Err(e) = state.db.run(move |c| db::finish_import_job(c, job, error.as_deref())).await {
                tracing::error!(job, error = %e, "couldn't record the end of a history import");
            }
            return;
        }
    });
}

async fn run_job(state: &AppState, job: i64, project_id: i64) -> anyhow::Result<()> {
    let attempts = state.db.run(move |c| db::start_import_job(c, job)).await?;
    anyhow::ensure!(attempts <= MAX_ATTEMPTS, "gave up after {MAX_ATTEMPTS} interrupted attempts");
    let (project, (kind, source)) = state
        .db
        .run(move |c| Ok((db::project_by_id(c, project_id)?, db::import_job_source(c, job)?)))
        .await?;
    let project = project.ok_or_else(|| anyhow::anyhow!("the project no longer exists"))?;
    match kind.as_str() {
        "decompdev-auto" => {
            let url = source.ok_or_else(|| anyhow::anyhow!("missing decomp.dev URL"))?;
            crate::decompdev::auto_import(state, &project, &url, job).await.map(|_| ())
        }
        "decompdev" => {
            let url = source.ok_or_else(|| anyhow::anyhow!("missing decomp.dev URL"))?;
            crate::decompdev::import(state, &project, &url, job).await
        }
        _ => crate::github_app::import_from_actions(state, &project, job).await,
    }
}
