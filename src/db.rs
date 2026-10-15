//! PostgreSQL storage. All access goes through `Db::run`, which executes on the blocking pool
//! with a pooled connection.

use std::sync::Arc;

use anyhow::Context;
use postgres::{types::ToSql, GenericClient, NoTls, Row};
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;

use crate::report::{Area, CodeStats, DataStats, FunctionStats, Layout, Report};
use crate::util::now;

pub type Conn = r2d2::PooledConnection<PostgresConnectionManager<NoTls>>;
type Params<'a> = &'a [&'a (dyn ToSql + Sync)];

#[derive(Clone)]
pub struct Db {
    pool: Arc<PoolHolder>,
}

type PgPool = Pool<PostgresConnectionManager<NoTls>>;

/// Closing a synchronous PostgreSQL connection blocks on its own small runtime, which panics
/// on an async worker thread. Whichever `Db` clone goes last, the pool (and the connections
/// it holds) is dropped on a plain thread instead.
struct PoolHolder(Option<PgPool>);

impl Drop for PoolHolder {
    fn drop(&mut self) {
        if let Some(pool) = self.0.take() {
            let _ = std::thread::spawn(move || drop(pool)).join();
        }
    }
}

impl std::ops::Deref for PoolHolder {
    type Target = PgPool;
    fn deref(&self) -> &PgPool {
        self.0.as_ref().expect("pool is only taken on drop")
    }
}

/// Schema migrations, applied in order and recorded in `schema_migrations`.
const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE users (
    id          BIGSERIAL PRIMARY KEY,
    github_id   BIGINT NOT NULL UNIQUE,
    login       TEXT NOT NULL,
    name        TEXT,
    avatar_url  TEXT,
    is_banned   BOOLEAN NOT NULL DEFAULT FALSE,
    created_at  BIGINT NOT NULL
);
CREATE TABLE sessions (
    token_hash  TEXT PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    gh_token    TEXT NOT NULL,
    created_at  BIGINT NOT NULL,
    expires_at  BIGINT NOT NULL
);
CREATE INDEX sessions_expiry ON sessions(expires_at);
CREATE INDEX sessions_user ON sessions(user_id);
CREATE TABLE projects (
    id              BIGSERIAL PRIMARY KEY,
    slug            TEXT NOT NULL UNIQUE,
    owner_id        BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    platform        TEXT NOT NULL,
    repo            TEXT NOT NULL,
    ai_contributions TEXT NOT NULL DEFAULT '',
    description     TEXT NOT NULL DEFAULT '',
    image           TEXT,
    ai_generated    BOOLEAN NOT NULL DEFAULT FALSE,
    spotlight       BOOLEAN NOT NULL DEFAULT FALSE,
    hidden          BOOLEAN NOT NULL DEFAULT FALSE,
    api_key_hash    TEXT NOT NULL UNIQUE,
    api_key_prefix  TEXT NOT NULL,
    default_version TEXT NOT NULL DEFAULT '',
    -- Progress of the default version, mirrored from project_versions for listings.
    unit            TEXT NOT NULL DEFAULT 'bytes',
    layout          TEXT NOT NULL DEFAULT 'treemap',
    code_total      BIGINT NOT NULL DEFAULT 0,
    code_matching   BIGINT NOT NULL DEFAULT 0,
    code_nonmatching BIGINT NOT NULL DEFAULT 0,
    data_total      BIGINT NOT NULL DEFAULT 0,
    data_done       BIGINT NOT NULL DEFAULT 0,
    functions_total BIGINT NOT NULL DEFAULT 0,
    functions_matched BIGINT NOT NULL DEFAULT 0,
    prev_code_total      BIGINT NOT NULL DEFAULT 0,
    prev_code_matching   BIGINT NOT NULL DEFAULT 0,
    prev_code_nonmatching BIGINT NOT NULL DEFAULT 0,
    prev_data_total      BIGINT NOT NULL DEFAULT 0,
    prev_data_done       BIGINT NOT NULL DEFAULT 0,
    areas_json      TEXT NOT NULL DEFAULT '[]',
    last_report_at  BIGINT,
    last_commit     TEXT,
    last_message    TEXT,
    commits_json    TEXT NOT NULL DEFAULT '{}',
    upvotes         BIGINT NOT NULL DEFAULT 0,
    downvotes       BIGINT NOT NULL DEFAULT 0,
    created_at      BIGINT NOT NULL,
    updated_at      BIGINT NOT NULL
);
CREATE INDEX projects_owner ON projects(owner_id);
CREATE INDEX projects_platform ON projects(platform);
CREATE INDEX projects_repo ON projects(lower(repo));
CREATE TABLE project_versions (
    project_id      BIGINT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    version         TEXT NOT NULL,
    unit            TEXT NOT NULL DEFAULT 'bytes',
    layout          TEXT NOT NULL DEFAULT 'treemap',
    code_total      BIGINT NOT NULL DEFAULT 0,
    code_matching   BIGINT NOT NULL DEFAULT 0,
    code_nonmatching BIGINT NOT NULL DEFAULT 0,
    data_total      BIGINT NOT NULL DEFAULT 0,
    data_done       BIGINT NOT NULL DEFAULT 0,
    functions_total BIGINT NOT NULL DEFAULT 0,
    functions_matched BIGINT NOT NULL DEFAULT 0,
    prev_code_total      BIGINT NOT NULL DEFAULT 0,
    prev_code_matching   BIGINT NOT NULL DEFAULT 0,
    prev_code_nonmatching BIGINT NOT NULL DEFAULT 0,
    prev_data_total      BIGINT NOT NULL DEFAULT 0,
    prev_data_done       BIGINT NOT NULL DEFAULT 0,
    areas_json      TEXT NOT NULL DEFAULT '[]',
    last_report_at  BIGINT,
    last_commit     TEXT,
    last_message    TEXT,
    commits_json    TEXT NOT NULL DEFAULT '{}',
    PRIMARY KEY (project_id, version)
);
CREATE TABLE reports (
    id              BIGSERIAL PRIMARY KEY,
    project_id      BIGINT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    version         TEXT NOT NULL DEFAULT '',
    created_at      BIGINT NOT NULL,
    code_total      BIGINT NOT NULL,
    code_matching   BIGINT NOT NULL,
    code_nonmatching BIGINT NOT NULL,
    data_total      BIGINT NOT NULL,
    data_done       BIGINT NOT NULL,
    commit_hash     TEXT,
    message         TEXT
);
CREATE INDEX reports_project_version_time ON reports(project_id, version, created_at);
CREATE TABLE votes (
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id  BIGINT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    value       BIGINT NOT NULL CHECK (value IN (-1, 1)),
    created_at  BIGINT NOT NULL,
    PRIMARY KEY (user_id, project_id)
);
CREATE INDEX votes_project ON votes(project_id);
CREATE TABLE import_jobs (
    id              BIGSERIAL PRIMARY KEY,
    project_id      BIGINT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    requested_by    BIGINT REFERENCES users(id) ON DELETE SET NULL,
    kind            TEXT NOT NULL DEFAULT 'github',
    source          TEXT,
    status          TEXT NOT NULL CHECK (status IN ('queued', 'running', 'finished', 'failed')),
    attempts        BIGINT NOT NULL DEFAULT 0,
    runs_total      BIGINT NOT NULL DEFAULT 0,
    runs_done       BIGINT NOT NULL DEFAULT 0,
    imported        BIGINT NOT NULL DEFAULT 0,
    error           TEXT,
    created_at      BIGINT NOT NULL,
    started_at      BIGINT,
    finished_at     BIGINT
);
CREATE INDEX import_jobs_project ON import_jobs(project_id, id);
CREATE INDEX import_jobs_status ON import_jobs(status, id);
"#, r#"
ALTER TABLE projects ADD COLUMN discord TEXT;
"#];

/// Counts are `u64` in Rust and `BIGINT` in the database.
fn big(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn unsigned(r: &Row, idx: usize) -> Result<u64, postgres::Error> {
    Ok(r.try_get::<_, i64>(idx)?.max(0) as u64)
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    #[allow(dead_code)]
    pub github_id: i64,
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub is_banned: bool,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub id: i64,
    pub slug: String,
    pub owner_id: i64,
    pub owner_login: String,
    pub name: String,
    pub platform: String,
    pub repo: String,
    /// Whether the project accepts AI-assisted contributions (see `AI_CONTRIBUTIONS`).
    pub ai_contributions: String,
    pub description: String,
    pub image: Option<String>,
    pub ai_generated: bool,
    pub spotlight: bool,
    pub hidden: bool,
    pub api_key_prefix: String,
    pub unit: String,
    pub layout: Layout,
    pub code: CodeStats,
    pub data: DataStats,
    /// Function counts of the default version (`total == 0`: not reported).
    pub functions: FunctionStats,
    pub prev_code: CodeStats,
    pub prev_data: DataStats,
    pub areas_json: String,
    pub last_report_at: Option<i64>,
    pub last_commit: Option<String>,
    pub last_message: Option<String>,
    pub upvotes: i64,
    pub downvotes: i64,
    pub created_at: i64,
    pub updated_at: i64,
    /// Recent commits fetched from GitHub after the last report (see `github::CommitList`).
    pub commits_json: String,
    /// The version shown on cards and listings; its progress is mirrored into this row.
    pub default_version: String,
    /// Discord invite (`https://discord.gg/...`), if the project has a server.
    pub discord: Option<String>,
}

impl Project {
    pub fn score(&self) -> i64 {
        self.upvotes - self.downvotes
    }
    pub fn areas(&self) -> Vec<Area> {
        serde_json::from_str(&self.areas_json).unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct ReportRow {
    pub created_at: i64,
    pub code: CodeStats,
    pub data: DataStats,
    pub commit: Option<String>,
    pub message: Option<String>,
}

/// Editable project metadata (from the dashboard form).
#[derive(Debug, Clone)]
pub struct ProjectMeta {
    pub slug: String,
    pub name: String,
    pub platform: String,
    pub repo: String,
    pub ai_contributions: String,
    pub discord: Option<String>,
    pub description: String,
    pub ai_generated: bool,
}

const PROJECT_COLS: &str = "p.id, p.slug, p.owner_id, u.login, p.name, p.platform, p.repo, p.ai_contributions, p.description,
    p.image, p.ai_generated, p.spotlight, p.hidden, p.api_key_prefix, p.unit, p.layout,
    p.code_total, p.code_matching, p.code_nonmatching, p.data_total, p.data_done,
    p.prev_code_total, p.prev_code_matching, p.prev_code_nonmatching, p.prev_data_total, p.prev_data_done,
    p.areas_json, p.last_report_at, p.last_commit, p.last_message, p.upvotes, p.downvotes, p.created_at, p.updated_at,
    p.commits_json, p.default_version, p.functions_total, p.functions_matched, p.discord";

fn project_from_row(r: &Row) -> Result<Project, postgres::Error> {
    Ok(Project {
        id: r.try_get(0)?,
        slug: r.try_get(1)?,
        owner_id: r.try_get(2)?,
        owner_login: r.try_get(3)?,
        name: r.try_get(4)?,
        platform: r.try_get(5)?,
        repo: r.try_get(6)?,
        ai_contributions: r.try_get(7)?,
        description: r.try_get(8)?,
        image: r.try_get(9)?,
        ai_generated: r.try_get(10)?,
        spotlight: r.try_get(11)?,
        hidden: r.try_get(12)?,
        api_key_prefix: r.try_get(13)?,
        unit: r.try_get(14)?,
        layout: Layout::parse(r.try_get::<_, &str>(15)?),
        code: CodeStats { total: unsigned(r, 16)?, matching: unsigned(r, 17)?, non_matching: unsigned(r, 18)? },
        data: DataStats { total: unsigned(r, 19)?, done: unsigned(r, 20)? },
        prev_code: CodeStats { total: unsigned(r, 21)?, matching: unsigned(r, 22)?, non_matching: unsigned(r, 23)? },
        prev_data: DataStats { total: unsigned(r, 24)?, done: unsigned(r, 25)? },
        areas_json: r.try_get(26)?,
        last_report_at: r.try_get(27)?,
        last_commit: r.try_get(28)?,
        last_message: r.try_get(29)?,
        upvotes: r.try_get(30)?,
        downvotes: r.try_get(31)?,
        created_at: r.try_get(32)?,
        updated_at: r.try_get(33)?,
        commits_json: r.try_get(34)?,
        default_version: r.try_get(35)?,
        functions: FunctionStats { total: unsigned(r, 36)?, matching: unsigned(r, 37)? },
        discord: r.try_get(38)?,
    })
}

const USER_COLS: &str = "u.id, u.github_id, u.login, u.name, u.avatar_url, u.is_banned";

fn user_from_row(r: &Row) -> Result<User, postgres::Error> {
    Ok(User {
        id: r.try_get(0)?,
        github_id: r.try_get(1)?,
        login: r.try_get(2)?,
        name: r.try_get(3)?,
        avatar_url: r.try_get(4)?,
        is_banned: r.try_get(5)?,
    })
}

impl Db {
    /// Connects with a libpq-style connection string, e.g.
    /// `host=/run/postgresql dbname=decompsite` or `postgres://user:pass@localhost/decompsite`.
    pub fn open(url: &str) -> anyhow::Result<Db> {
        let config: postgres::Config = url.parse().context("parsing DATABASE_URL")?;
        let manager = PostgresConnectionManager::new(config, NoTls);
        let pool = Pool::builder()
            .max_size(16)
            .connection_timeout(std::time::Duration::from_secs(10))
            .build(manager)
            .context("connecting to PostgreSQL")?;
        migrate(&mut pool.get()?)?;
        Ok(Db { pool: Arc::new(PoolHolder(Some(pool))) })
    }

    /// A pooled connection for synchronous use (CLI commands).
    pub fn conn(&self) -> anyhow::Result<Conn> {
        Ok(self.pool.get()?)
    }

    /// Run a closure with a pooled connection on the blocking thread pool.
    pub async fn run<T, F>(&self, f: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Conn) -> anyhow::Result<T> + Send + 'static,
    {
        let pool = self.pool.clone();
        // `pool` (an Arc) may be the last reference; it is dropped on this blocking thread.
        tokio::task::spawn_blocking(move || {
            let mut conn = pool.get()?;
            f(&mut conn)
        })
        .await?
    }
}

fn migrate(c: &mut Conn) -> anyhow::Result<()> {
    c.batch_execute("CREATE TABLE IF NOT EXISTS schema_migrations (version BIGINT PRIMARY KEY, applied_at BIGINT NOT NULL)")?;
    let applied: i64 = c.query_one("SELECT COALESCE(MAX(version), 0) FROM schema_migrations", &[])?.try_get(0)?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(applied as usize) {
        let mut tx = c.transaction()?;
        tx.batch_execute(sql)?;
        tx.execute("INSERT INTO schema_migrations (version, applied_at) VALUES ($1, $2)", &[&(i as i64 + 1), &now()])?;
        tx.commit()?;
        tracing::info!("applied migration {}", i + 1);
    }
    Ok(())
}

// ---------------------------------------------------------------- users & sessions

pub fn upsert_user(c: &mut Conn, github_id: i64, login: &str, name: Option<&str>, avatar: Option<&str>) -> anyhow::Result<User> {
    let row = c.query_one(
        "INSERT INTO users AS u (github_id, login, name, avatar_url, created_at) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (github_id) DO UPDATE SET login = excluded.login, name = excluded.name, avatar_url = excluded.avatar_url
         RETURNING id, github_id, login, name, avatar_url, is_banned",
        &[&github_id, &login, &name, &avatar, &now()],
    )?;
    Ok(user_from_row(&row)?)
}

pub fn create_session(c: &mut Conn, token_hash: &str, user_id: i64, gh_token: &str, ttl: i64) -> anyhow::Result<()> {
    let t = now();
    c.execute("DELETE FROM sessions WHERE expires_at < $1", &[&t])?;
    c.execute(
        "INSERT INTO sessions (token_hash, user_id, gh_token, created_at, expires_at) VALUES ($1, $2, $3, $4, $5)",
        &[&token_hash, &user_id, &gh_token, &t, &(t + ttl)],
    )?;
    Ok(())
}

pub fn session_user(c: &mut Conn, token_hash: &str) -> anyhow::Result<Option<(User, String)>> {
    let sql = format!(
        "SELECT {USER_COLS}, s.gh_token FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = $1 AND s.expires_at > $2"
    );
    match c.query_opt(&sql, &[&token_hash, &now()])? {
        Some(r) => Ok(Some((user_from_row(&r)?, r.try_get(6)?))),
        None => Ok(None),
    }
}

pub fn delete_session(c: &mut Conn, token_hash: &str) -> anyhow::Result<()> {
    c.execute("DELETE FROM sessions WHERE token_hash = $1", &[&token_hash])?;
    Ok(())
}

pub fn list_users(c: &mut Conn) -> anyhow::Result<Vec<User>> {
    let sql = format!("SELECT {USER_COLS} FROM users u ORDER BY u.id DESC");
    Ok(c.query(&sql, &[])?.iter().map(user_from_row).collect::<Result<_, _>>()?)
}

pub fn is_banned(c: &mut Conn, user_id: i64) -> anyhow::Result<bool> {
    Ok(c.query_one("SELECT is_banned FROM users WHERE id = $1", &[&user_id])?.try_get(0)?)
}

pub fn set_banned(c: &mut Conn, user_id: i64, banned: bool) -> anyhow::Result<()> {
    c.execute("UPDATE users SET is_banned = $2 WHERE id = $1", &[&user_id, &banned])?;
    if banned {
        c.execute("DELETE FROM sessions WHERE user_id = $1", &[&user_id])?;
    }
    Ok(())
}

// ---------------------------------------------------------------- projects

fn query_projects(c: &mut Conn, where_sql: &str, params: Params) -> anyhow::Result<Vec<Project>> {
    let sql = format!("SELECT {PROJECT_COLS} FROM projects p JOIN users u ON u.id = p.owner_id {where_sql}");
    Ok(c.query(&sql, params)?.iter().map(project_from_row).collect::<Result<_, _>>()?)
}

/// All projects that may appear in public listings (not admin-hidden, owner not banned,
/// and with the required cover image).
pub fn public_projects(c: &mut Conn) -> anyhow::Result<Vec<Project>> {
    query_projects(c, "WHERE NOT p.hidden AND NOT u.is_banned AND p.image IS NOT NULL", &[])
}

pub fn all_projects(c: &mut Conn) -> anyhow::Result<Vec<Project>> {
    query_projects(c, "ORDER BY p.created_at DESC", &[])
}

pub fn projects_by_owner(c: &mut Conn, owner_id: i64) -> anyhow::Result<Vec<Project>> {
    query_projects(c, "WHERE p.owner_id = $1 ORDER BY lower(p.name)", &[&owner_id])
}

pub fn project_by_slug(c: &mut Conn, slug: &str) -> anyhow::Result<Option<Project>> {
    Ok(query_projects(c, "WHERE p.slug = $1", &[&slug])?.into_iter().next())
}

pub fn project_by_id(c: &mut Conn, id: i64) -> anyhow::Result<Option<Project>> {
    Ok(query_projects(c, "WHERE p.id = $1", &[&id])?.into_iter().next())
}

pub fn project_by_key_hash(c: &mut Conn, key_hash: &str) -> anyhow::Result<Option<Project>> {
    Ok(query_projects(c, "WHERE p.api_key_hash = $1 AND NOT u.is_banned", &[&key_hash])?.into_iter().next())
}

/// Projects registered for a GitHub repository (`owner/name`, case-insensitive).
pub fn projects_by_repo(c: &mut Conn, repo: &str) -> anyhow::Result<Vec<Project>> {
    query_projects(c, "WHERE lower(p.repo) = lower($1) AND NOT u.is_banned", &[&repo])
}

/// `public_projects` with the signature `Db::run` expects.
pub fn public_projects_owned(c: &mut Conn) -> anyhow::Result<Vec<Project>> {
    public_projects(c)
}

pub fn owner_is_banned(c: &mut Conn, project_id: i64) -> anyhow::Result<bool> {
    Ok(c
        .query_one("SELECT u.is_banned FROM users u JOIN projects p ON p.owner_id = u.id WHERE p.id = $1", &[&project_id])?
        .try_get(0)?)
}

pub fn slug_taken(c: &mut Conn, slug: &str, except_id: i64) -> anyhow::Result<bool> {
    Ok(c.query_one("SELECT EXISTS(SELECT 1 FROM projects WHERE slug = $1 AND id <> $2)", &[&slug, &except_id])?.try_get(0)?)
}

/// Whether any project still uses an uploaded image.
pub fn image_in_use(c: &mut Conn, image: &str) -> anyhow::Result<bool> {
    Ok(c.query_one("SELECT EXISTS(SELECT 1 FROM projects WHERE image = $1)", &[&image])?.try_get(0)?)
}

pub fn insert_project(c: &mut Conn, owner_id: i64, m: &ProjectMeta, image: &str, key_hash: &str, key_prefix: &str) -> anyhow::Result<i64> {
    let t = now();
    let row = c.query_one(
        "INSERT INTO projects (slug, owner_id, name, platform, repo, ai_contributions, discord, description, ai_generated, image,
                               api_key_hash, api_key_prefix, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $13, $7, $8, $9, $10, $11, $12, $12) RETURNING id",
        &[&m.slug, &owner_id, &m.name, &m.platform, &m.repo, &m.ai_contributions, &m.description, &m.ai_generated, &image, &key_hash, &key_prefix, &t, &m.discord],
    )?;
    Ok(row.try_get(0)?)
}

pub fn update_project_meta(c: &mut Conn, id: i64, m: &ProjectMeta) -> anyhow::Result<()> {
    c.execute(
        "UPDATE projects SET slug = $2, name = $3, platform = $4, repo = $5, ai_contributions = $6, description = $7, discord = $10,
                             ai_generated = $8, updated_at = $9 WHERE id = $1",
        &[&id, &m.slug, &m.name, &m.platform, &m.repo, &m.ai_contributions, &m.description, &m.ai_generated, &now(), &m.discord],
    )?;
    Ok(())
}

pub fn set_project_image(c: &mut Conn, id: i64, image: Option<&str>) -> anyhow::Result<()> {
    c.execute("UPDATE projects SET image = $2, updated_at = $3 WHERE id = $1", &[&id, &image, &now()])?;
    Ok(())
}

pub fn set_api_key(c: &mut Conn, id: i64, key_hash: &str, key_prefix: &str) -> anyhow::Result<()> {
    c.execute("UPDATE projects SET api_key_hash = $2, api_key_prefix = $3 WHERE id = $1", &[&id, &key_hash, &key_prefix])?;
    Ok(())
}

pub fn delete_project(c: &mut Conn, id: i64) -> anyhow::Result<()> {
    c.execute("DELETE FROM projects WHERE id = $1", &[&id])?;
    Ok(())
}

pub fn set_spotlight(c: &mut Conn, id: i64, on: bool) -> anyhow::Result<()> {
    c.execute("UPDATE projects SET spotlight = $2 WHERE id = $1", &[&id, &on])?;
    Ok(())
}

pub fn set_hidden(c: &mut Conn, id: i64, on: bool) -> anyhow::Result<()> {
    c.execute("UPDATE projects SET hidden = $2 WHERE id = $1", &[&id, &on])?;
    Ok(())
}

// ---------------------------------------------------------------- versions & reports

/// Progress of one game version of a project.
#[derive(Debug, Clone)]
pub struct VersionState {
    pub version: String,
    pub unit: String,
    pub layout: Layout,
    pub code: CodeStats,
    pub data: DataStats,
    pub functions: FunctionStats,
    pub prev_code: CodeStats,
    pub prev_data: DataStats,
    pub areas_json: String,
    pub last_report_at: Option<i64>,
    pub last_commit: Option<String>,
    pub last_message: Option<String>,
    pub commits_json: String,
}

impl VersionState {
    /// The project with this version's progress in place of the default version's.
    pub fn apply_to(&self, p: &Project) -> Project {
        Project {
            unit: self.unit.clone(),
            layout: self.layout,
            code: self.code,
            data: self.data,
            functions: self.functions,
            prev_code: self.prev_code,
            prev_data: self.prev_data,
            areas_json: self.areas_json.clone(),
            last_report_at: self.last_report_at,
            last_commit: self.last_commit.clone(),
            last_message: self.last_message.clone(),
            commits_json: self.commits_json.clone(),
            ..p.clone()
        }
    }
}

const VERSION_COLS: &str = "version, unit, layout, code_total, code_matching, code_nonmatching, data_total, data_done,
    prev_code_total, prev_code_matching, prev_code_nonmatching, prev_data_total, prev_data_done,
    areas_json, last_report_at, last_commit, last_message, commits_json, functions_total, functions_matched";

fn version_from_row(r: &Row) -> Result<VersionState, postgres::Error> {
    Ok(VersionState {
        version: r.try_get(0)?,
        unit: r.try_get(1)?,
        layout: Layout::parse(r.try_get::<_, &str>(2)?),
        code: CodeStats { total: unsigned(r, 3)?, matching: unsigned(r, 4)?, non_matching: unsigned(r, 5)? },
        data: DataStats { total: unsigned(r, 6)?, done: unsigned(r, 7)? },
        prev_code: CodeStats { total: unsigned(r, 8)?, matching: unsigned(r, 9)?, non_matching: unsigned(r, 10)? },
        prev_data: DataStats { total: unsigned(r, 11)?, done: unsigned(r, 12)? },
        areas_json: r.try_get(13)?,
        last_report_at: r.try_get(14)?,
        last_commit: r.try_get(15)?,
        last_message: r.try_get(16)?,
        commits_json: r.try_get(17)?,
        functions: FunctionStats { total: unsigned(r, 18)?, matching: unsigned(r, 19)? },
    })
}

/// All versions of a project that have reported progress, by name.
pub fn versions_for_project(c: &mut Conn, project_id: i64) -> anyhow::Result<Vec<VersionState>> {
    let sql = format!("SELECT {VERSION_COLS} FROM project_versions WHERE project_id = $1 ORDER BY version");
    Ok(c.query(&sql, &[&project_id])?.iter().map(version_from_row).collect::<Result<_, _>>()?)
}

fn version_state(c: &mut impl GenericClient, project_id: i64, version: &str) -> anyhow::Result<Option<VersionState>> {
    let sql = format!("SELECT {VERSION_COLS} FROM project_versions WHERE project_id = $1 AND version = $2");
    Ok(c.query_opt(&sql, &[&project_id, &version])?.as_ref().map(version_from_row).transpose()?)
}

/// Copies a version's progress into the project row, which listings and cards read.
fn mirror_version(c: &mut impl GenericClient, project_id: i64, version: &str) -> anyhow::Result<()> {
    c.execute(
        "UPDATE projects p SET (unit, layout, code_total, code_matching, code_nonmatching, data_total, data_done,
                prev_code_total, prev_code_matching, prev_code_nonmatching, prev_data_total, prev_data_done,
                areas_json, last_report_at, last_commit, last_message, commits_json, functions_total, functions_matched) =
            (v.unit, v.layout, v.code_total, v.code_matching, v.code_nonmatching, v.data_total, v.data_done,
                v.prev_code_total, v.prev_code_matching, v.prev_code_nonmatching, v.prev_data_total, v.prev_data_done,
                v.areas_json, v.last_report_at, v.last_commit, v.last_message, v.commits_json, v.functions_total, v.functions_matched)
         FROM project_versions v
         WHERE p.id = $1 AND v.project_id = p.id AND v.version = $2",
        &[&project_id, &version],
    )?;
    Ok(())
}

/// Makes `version` the one shown on cards and listings.
pub fn set_default_version(c: &mut Conn, project_id: i64, version: &str) -> anyhow::Result<()> {
    let mut tx = c.transaction()?;
    tx.execute("UPDATE projects SET default_version = $2 WHERE id = $1", &[&project_id, &version])?;
    mirror_version(&mut tx, project_id, version)?;
    tx.commit()?;
    Ok(())
}

/// Result of storing a report that changed something.
pub struct Applied {
    /// The commit of the version's previous report, to list the commits in between.
    pub prev_commit: Option<String>,
    /// The report was older than the current state and was only added to the history.
    pub historical: bool,
}

fn insert_report_row(c: &mut impl GenericClient, project_id: i64, r: &Report, at: i64) -> anyhow::Result<()> {
    c.execute(
        "INSERT INTO reports (project_id, version, created_at, code_total, code_matching, code_nonmatching, data_total, data_done, commit_hash, message)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        &[
            &project_id, &r.version, &at, &big(r.code.total), &big(r.code.matching), &big(r.code.non_matching),
            &big(r.data.total), &big(r.data.done), &r.commit, &r.message,
        ],
    )?;
    Ok(())
}

/// Stores a new report for its version. Returns `None` if nothing changed compared to that
/// version's current state (in which case nothing is written and no cache needs purging).
/// The first version a project reports becomes its default.
pub fn apply_report(c: &mut Conn, project: &Project, r: &Report, at: Option<i64>) -> anyhow::Result<Option<Applied>> {
    let version = r.version.as_str();
    let areas_json = serde_json::to_string(&r.areas)?;
    let mut tx = c.transaction()?;
    let current = version_state(&mut tx, project.id, version)?;
    let t = at.unwrap_or_else(now);
    // Older than the current state: it only belongs in the history.
    if current.as_ref().and_then(|v| v.last_report_at).is_some_and(|last| t < last) {
        let added = insert_history(&mut tx, project.id, r, t)?;
        tx.commit()?;
        return Ok(added.then_some(Applied { prev_commit: None, historical: true }));
    }
    if let Some(v) = &current {
        if v.code == r.code && v.data == r.data && v.functions == r.functions && v.areas_json == areas_json && v.unit == r.unit && v.layout == r.layout {
            return Ok(None);
        }
    }
    let prev_commit = current.as_ref().and_then(|v| v.last_commit.clone());
    let prev = current.map(|v| (v.code, v.data)).unwrap_or_default();
    tx.execute(
        "INSERT INTO project_versions (project_id, version, unit, layout, code_total, code_matching, code_nonmatching,
                data_total, data_done, prev_code_total, prev_code_matching, prev_code_nonmatching, prev_data_total,
                prev_data_done, areas_json, last_report_at, last_commit, last_message, commits_json,
                functions_total, functions_matched)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, '{}', $19, $20)
         ON CONFLICT (project_id, version) DO UPDATE SET
            unit = excluded.unit, layout = excluded.layout, code_total = excluded.code_total,
            code_matching = excluded.code_matching, code_nonmatching = excluded.code_nonmatching,
            data_total = excluded.data_total, data_done = excluded.data_done,
            prev_code_total = excluded.prev_code_total, prev_code_matching = excluded.prev_code_matching,
            prev_code_nonmatching = excluded.prev_code_nonmatching, prev_data_total = excluded.prev_data_total,
            prev_data_done = excluded.prev_data_done, areas_json = excluded.areas_json,
            last_report_at = excluded.last_report_at, last_commit = excluded.last_commit,
            last_message = excluded.last_message, commits_json = '{}',
            functions_total = excluded.functions_total, functions_matched = excluded.functions_matched",
        &[
            &project.id, &version, &r.unit, &r.layout.as_str(), &big(r.code.total), &big(r.code.matching),
            &big(r.code.non_matching), &big(r.data.total), &big(r.data.done), &big(prev.0.total), &big(prev.0.matching),
            &big(prev.0.non_matching), &big(prev.1.total), &big(prev.1.done), &areas_json, &t, &r.commit, &r.message,
            &big(r.functions.total), &big(r.functions.matching),
        ],
    )?;
    insert_report_row(&mut tx, project.id, r, t)?;
    // Fall back to this version as the default if the current default has never reported.
    let default: String = tx.query_one("SELECT default_version FROM projects WHERE id = $1", &[&project.id])?.try_get(0)?;
    let default_exists: bool = tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM project_versions WHERE project_id = $1 AND version = $2)",
            &[&project.id, &default],
        )?
        .try_get(0)?;
    let default = if default_exists {
        default
    } else {
        tx.execute("UPDATE projects SET default_version = $2 WHERE id = $1", &[&project.id, &version])?;
        version.to_string()
    };
    if default == version {
        mirror_version(&mut tx, project.id, version)?;
    }
    tx.commit()?;
    Ok(Some(Applied { prev_commit, historical: false }))
}

/// Adds a past report to a version's history without touching its current state. Returns
/// `false` if that commit (or that exact time) is already in the history.
fn insert_history(c: &mut impl GenericClient, project_id: i64, r: &Report, at: i64) -> anyhow::Result<bool> {
    let exists: bool = c
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM reports WHERE project_id = $1 AND version = $2
                           AND (created_at = $3 OR ($4::TEXT IS NOT NULL AND commit_hash = $4)))",
            &[&project_id, &r.version, &at, &r.commit],
        )?
        .try_get(0)?;
    if exists {
        return Ok(false);
    }
    insert_report_row(c, project_id, r, at)?;
    Ok(true)
}

pub fn reports_for_project(c: &mut Conn, project_id: i64, version: &str, limit: usize) -> anyhow::Result<Vec<ReportRow>> {
    let rows = c.query(
        "SELECT created_at, code_total, code_matching, code_nonmatching, data_total, data_done, commit_hash, message
         FROM reports WHERE project_id = $1 AND version = $2 ORDER BY created_at DESC LIMIT $3",
        &[&project_id, &version, &(limit as i64)],
    )?;
    let mut out = rows
        .iter()
        .map(|r| {
            Ok(ReportRow {
                created_at: r.try_get(0)?,
                code: CodeStats { total: unsigned(r, 1)?, matching: unsigned(r, 2)?, non_matching: unsigned(r, 3)? },
                data: DataStats { total: unsigned(r, 4)?, done: unsigned(r, 5)? },
                commit: r.try_get(6)?,
                message: r.try_get(7)?,
            })
        })
        .collect::<Result<Vec<_>, postgres::Error>>()?;
    out.reverse();
    Ok(out)
}

/// Stores the commits fetched for a version's report, unless a newer report has arrived
/// meanwhile.
pub fn set_commits(c: &mut Conn, project_id: i64, version: &str, for_commit: Option<&str>, commits_json: &str) -> anyhow::Result<bool> {
    let mut tx = c.transaction()?;
    let updated = tx.execute(
        "UPDATE project_versions SET commits_json = $4
         WHERE project_id = $1 AND version = $2 AND last_commit IS NOT DISTINCT FROM $3",
        &[&project_id, &version, &for_commit, &commits_json],
    )? > 0;
    let is_default: bool = tx
        .query_one("SELECT default_version = $2 FROM projects WHERE id = $1", &[&project_id, &version])?
        .try_get(0)?;
    if updated && is_default {
        mirror_version(&mut tx, project_id, version)?;
    }
    tx.commit()?;
    Ok(updated)
}

// ---------------------------------------------------------------- votes

/// Set a user's vote (`value` of -1, 0 or 1) and refresh the project's counters.
pub fn vote(c: &mut Conn, user_id: i64, project_id: i64, value: i64) -> anyhow::Result<(i64, i64)> {
    let mut tx = c.transaction()?;
    if value == 0 {
        tx.execute("DELETE FROM votes WHERE user_id = $1 AND project_id = $2", &[&user_id, &project_id])?;
    } else {
        tx.execute(
            "INSERT INTO votes (user_id, project_id, value, created_at) VALUES ($1, $2, $3, $4)
             ON CONFLICT (user_id, project_id) DO UPDATE SET value = excluded.value",
            &[&user_id, &project_id, &value.signum(), &now()],
        )?;
    }
    let row = tx.query_one(
        "SELECT COUNT(*) FILTER (WHERE value = 1), COUNT(*) FILTER (WHERE value = -1) FROM votes WHERE project_id = $1",
        &[&project_id],
    )?;
    let (up, down): (i64, i64) = (row.try_get(0)?, row.try_get(1)?);
    tx.execute("UPDATE projects SET upvotes = $2, downvotes = $3 WHERE id = $1", &[&project_id, &up, &down])?;
    tx.commit()?;
    Ok((up, down))
}

pub fn user_votes(c: &mut Conn, user_id: i64) -> anyhow::Result<Vec<(i64, i64)>> {
    Ok(c
        .query("SELECT project_id, value FROM votes WHERE user_id = $1", &[&user_id])?
        .iter()
        .map(|r| Ok((r.try_get(0)?, r.try_get(1)?)))
        .collect::<Result<_, postgres::Error>>()?)
}

// ---------------------------------------------------------------- import jobs

/// A history import job (see `imports`).
#[derive(Debug, Clone)]
pub struct ImportJob {
    pub id: i64,
    #[allow(dead_code)]
    pub project_id: i64,
    pub project_slug: String,
    pub project_name: String,
    pub status: String,
    pub attempts: i64,
    pub runs_total: i64,
    pub runs_done: i64,
    pub imported: i64,
    pub error: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    /// `github` (workflow artifacts) or `decompdev` (decomp.dev history).
    pub kind: String,
    /// For `decompdev`: the project's decomp.dev URL.
    pub source: Option<String>,
    /// Jobs queued ahead of this one (only meaningful while queued).
    pub queue_position: i64,
}

impl ImportJob {
    pub fn is_active(&self) -> bool {
        self.status == "queued" || self.status == "running"
    }
}

const JOB_COLS: &str = "j.id, j.project_id, p.slug, p.name, j.status, j.attempts, j.runs_total, j.runs_done, j.imported,
    j.error, j.created_at, j.started_at, j.finished_at,
    (SELECT COUNT(*) FROM import_jobs q WHERE q.status = 'queued' AND q.id < j.id), j.kind, j.source";

fn job_from_row(r: &Row) -> Result<ImportJob, postgres::Error> {
    Ok(ImportJob {
        id: r.try_get(0)?,
        project_id: r.try_get(1)?,
        project_slug: r.try_get(2)?,
        project_name: r.try_get(3)?,
        status: r.try_get(4)?,
        attempts: r.try_get(5)?,
        runs_total: r.try_get(6)?,
        runs_done: r.try_get(7)?,
        imported: r.try_get(8)?,
        error: r.try_get(9)?,
        created_at: r.try_get(10)?,
        started_at: r.try_get(11)?,
        finished_at: r.try_get(12)?,
        queue_position: r.try_get(13)?,
        kind: r.try_get(14)?,
        source: r.try_get(15)?,
    })
}

/// Queues an import for a project, or returns `None` if one is already queued or running.
pub fn create_import_job(c: &mut Conn, project_id: i64, user_id: i64, kind: &str, source: Option<&str>) -> anyhow::Result<Option<i64>> {
    let mut tx = c.transaction()?;
    // Serialise job creation per project so two requests can't both pass the check.
    tx.execute("SELECT pg_advisory_xact_lock($1)", &[&project_id])?;
    let active: bool = tx
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM import_jobs WHERE project_id = $1 AND status IN ('queued', 'running'))",
            &[&project_id],
        )?
        .try_get(0)?;
    if active {
        return Ok(None);
    }
    let id: i64 = tx
        .query_one(
            "INSERT INTO import_jobs (project_id, requested_by, status, created_at, kind, source)
             VALUES ($1, $2, 'queued', $3, $4, $5) RETURNING id",
            &[&project_id, &user_id, &now(), &kind, &source],
        )?
        .try_get(0)?;
    tx.commit()?;
    Ok(Some(id))
}

pub fn latest_import_job(c: &mut Conn, project_id: i64) -> anyhow::Result<Option<ImportJob>> {
    let sql = format!(
        "SELECT {JOB_COLS} FROM import_jobs j JOIN projects p ON p.id = j.project_id WHERE j.project_id = $1 ORDER BY j.id DESC LIMIT 1"
    );
    Ok(c.query_opt(&sql, &[&project_id])?.as_ref().map(job_from_row).transpose()?)
}

pub fn recent_import_jobs(c: &mut Conn, limit: usize) -> anyhow::Result<Vec<ImportJob>> {
    let sql = format!("SELECT {JOB_COLS} FROM import_jobs j JOIN projects p ON p.id = j.project_id ORDER BY j.id DESC LIMIT $1");
    Ok(c.query(&sql, &[&(limit as i64)])?.iter().map(job_from_row).collect::<Result<_, _>>()?)
}

/// After a restart: jobs that were running are queued again, and every unfinished job is
/// returned (oldest first) so it can be started.
pub fn unfinished_import_jobs(c: &mut Conn) -> anyhow::Result<Vec<(i64, i64)>> {
    c.execute("UPDATE import_jobs SET status = 'queued' WHERE status = 'running'", &[])?;
    Ok(c
        .query("SELECT id, project_id FROM import_jobs WHERE status = 'queued' ORDER BY id", &[])?
        .iter()
        .map(|r| Ok((r.try_get(0)?, r.try_get(1)?)))
        .collect::<Result<_, postgres::Error>>()?)
}

/// Marks a job as running and returns how many times it has been started, this one included.
pub fn start_import_job(c: &mut Conn, id: i64) -> anyhow::Result<i64> {
    Ok(c
        .query_one(
            "UPDATE import_jobs SET status = 'running', attempts = attempts + 1, started_at = $2, error = NULL
             WHERE id = $1 RETURNING attempts",
            &[&id, &now()],
        )?
        .try_get(0)?)
}

pub fn import_job_progress(c: &mut Conn, id: i64, runs_total: usize, runs_done: usize, imported: usize) -> anyhow::Result<()> {
    c.execute(
        "UPDATE import_jobs SET runs_total = $2, runs_done = $3, imported = $4 WHERE id = $1",
        &[&id, &(runs_total as i64), &(runs_done as i64), &(imported as i64)],
    )?;
    Ok(())
}

pub fn finish_import_job(c: &mut Conn, id: i64, error: Option<&str>) -> anyhow::Result<()> {
    c.execute(
        "UPDATE import_jobs SET status = $2, error = $3, finished_at = $4 WHERE id = $1",
        &[&id, &if error.is_some() { "failed" } else { "finished" }, &error, &now()],
    )?;
    Ok(())
}

/// What a job imports: its kind and source (see `ImportJob`).
pub fn import_job_source(c: &mut Conn, id: i64) -> anyhow::Result<(String, Option<String>)> {
    let r = c.query_one("SELECT kind, source FROM import_jobs WHERE id = $1", &[&id])?;
    Ok((r.try_get(0)?, r.try_get(1)?))
}

/// Values of `projects.ai_contributions`, with their labels. Empty means "not stated".
pub const AI_CONTRIBUTIONS: &[(&str, &str)] = &[
    ("", "Not stated"),
    ("welcome", "Welcome"),
    ("disclosed", "Accepted if disclosed"),
    ("forbidden", "Not accepted"),
];

// ---------------------------------------------------------------- feeds

/// A progress report for RSS feeds, with the previous report of the same version (if any).
#[derive(Debug, Clone)]
pub struct FeedItem {
    pub report_id: i64,
    pub created_at: i64,
    pub version: String,
    pub code: CodeStats,
    pub data: DataStats,
    pub commit: Option<String>,
    pub message: Option<String>,
    pub slug: String,
    pub name: String,
    pub repo: String,
    pub prev_code: Option<CodeStats>,
}

const FEED_SQL: &str = "SELECT r.id, r.created_at, r.version, r.code_total, r.code_matching, r.code_nonmatching,
        r.data_total, r.data_done, r.commit_hash, r.message, p.slug, p.name, p.repo,
        prev.code_total, prev.code_matching, prev.code_nonmatching
    FROM reports r
    JOIN projects p ON p.id = r.project_id
    JOIN users u ON u.id = p.owner_id
    LEFT JOIN LATERAL (
        SELECT q.code_total, q.code_matching, q.code_nonmatching FROM reports q
        WHERE q.project_id = r.project_id AND q.version = r.version AND q.created_at < r.created_at
        ORDER BY q.created_at DESC LIMIT 1
    ) prev ON TRUE";

fn feed_item_from_row(r: &Row) -> Result<FeedItem, postgres::Error> {
    let prev_total: Option<i64> = r.try_get(13)?;
    Ok(FeedItem {
        report_id: r.try_get(0)?,
        created_at: r.try_get(1)?,
        version: r.try_get(2)?,
        code: CodeStats { total: unsigned(r, 3)?, matching: unsigned(r, 4)?, non_matching: unsigned(r, 5)? },
        data: DataStats { total: unsigned(r, 6)?, done: unsigned(r, 7)? },
        commit: r.try_get(8)?,
        message: r.try_get(9)?,
        slug: r.try_get(10)?,
        name: r.try_get(11)?,
        repo: r.try_get(12)?,
        prev_code: match prev_total {
            Some(total) => Some(CodeStats {
                total: total.max(0) as u64,
                matching: unsigned(r, 14)?,
                non_matching: unsigned(r, 15)?,
            }),
            None => None,
        },
    })
}

/// Latest reports of every listed project (same rules as the public listings).
pub fn recent_feed_items(c: &mut Conn, hide_threshold: i64, limit: usize) -> anyhow::Result<Vec<FeedItem>> {
    let sql = format!(
        "{FEED_SQL}
         WHERE NOT p.hidden AND NOT u.is_banned AND p.image IS NOT NULL AND p.upvotes - p.downvotes > $1
         ORDER BY r.created_at DESC, r.id DESC LIMIT $2"
    );
    Ok(c.query(&sql, &[&hide_threshold, &(limit as i64)])?.iter().map(feed_item_from_row).collect::<Result<_, _>>()?)
}

/// Latest reports of one project, every version.
pub fn project_feed_items(c: &mut Conn, project_id: i64, limit: usize) -> anyhow::Result<Vec<FeedItem>> {
    let sql = format!("{FEED_SQL} WHERE r.project_id = $1 ORDER BY r.created_at DESC, r.id DESC LIMIT $2");
    Ok(c.query(&sql, &[&project_id, &(limit as i64)])?.iter().map(feed_item_from_row).collect::<Result<_, _>>()?)
}

/// Puts a job back in the queue to be retried, keeping the reason visible.
pub fn requeue_import_job(c: &mut Conn, id: i64, message: &str) -> anyhow::Result<()> {
    c.execute("UPDATE import_jobs SET status = 'queued', error = $2 WHERE id = $1", &[&id, &message])?;
    Ok(())
}
