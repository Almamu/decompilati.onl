//! SQLite storage. All access goes through `Db::run`, which executes on the blocking pool.

use std::path::Path;

use anyhow::Context;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::report::{Area, CodeStats, DataStats, FunctionStats, Layout, Report};
use crate::util::now;

pub type Conn = r2d2::PooledConnection<SqliteConnectionManager>;

#[derive(Clone)]
pub struct Db {
    pool: Pool<SqliteConnectionManager>,
}

const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE users (
    id          INTEGER PRIMARY KEY,
    github_id   INTEGER NOT NULL UNIQUE,
    login       TEXT NOT NULL,
    name        TEXT,
    avatar_url  TEXT,
    is_banned   INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL
);
CREATE TABLE sessions (
    token_hash  TEXT PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    gh_token    TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL
);
CREATE INDEX sessions_expiry ON sessions(expires_at);
CREATE TABLE projects (
    id              INTEGER PRIMARY KEY,
    slug            TEXT NOT NULL UNIQUE,
    owner_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    platform        TEXT NOT NULL,
    repo            TEXT NOT NULL,
    website         TEXT,
    description     TEXT NOT NULL DEFAULT '',
    image           TEXT,
    ai_generated    INTEGER NOT NULL DEFAULT 0,
    spotlight       INTEGER NOT NULL DEFAULT 0,
    hidden          INTEGER NOT NULL DEFAULT 0,
    api_key_hash    TEXT NOT NULL UNIQUE,
    api_key_prefix  TEXT NOT NULL,
    unit            TEXT NOT NULL DEFAULT 'bytes',
    layout          TEXT NOT NULL DEFAULT 'treemap',
    code_total      INTEGER NOT NULL DEFAULT 0,
    code_matching   INTEGER NOT NULL DEFAULT 0,
    code_nonmatching INTEGER NOT NULL DEFAULT 0,
    data_total      INTEGER NOT NULL DEFAULT 0,
    data_done       INTEGER NOT NULL DEFAULT 0,
    prev_code_total      INTEGER NOT NULL DEFAULT 0,
    prev_code_matching   INTEGER NOT NULL DEFAULT 0,
    prev_code_nonmatching INTEGER NOT NULL DEFAULT 0,
    prev_data_total      INTEGER NOT NULL DEFAULT 0,
    prev_data_done       INTEGER NOT NULL DEFAULT 0,
    areas_json      TEXT NOT NULL DEFAULT '[]',
    last_report_at  INTEGER,
    last_commit     TEXT,
    last_message    TEXT,
    upvotes         INTEGER NOT NULL DEFAULT 0,
    downvotes       INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE INDEX projects_owner ON projects(owner_id);
CREATE INDEX projects_platform ON projects(platform);
CREATE TABLE reports (
    id              INTEGER PRIMARY KEY,
    project_id      INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    created_at      INTEGER NOT NULL,
    code_total      INTEGER NOT NULL,
    code_matching   INTEGER NOT NULL,
    code_nonmatching INTEGER NOT NULL,
    data_total      INTEGER NOT NULL,
    data_done       INTEGER NOT NULL,
    commit_hash     TEXT,
    message         TEXT
);
CREATE INDEX reports_project_time ON reports(project_id, created_at);
CREATE TABLE votes (
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    value       INTEGER NOT NULL CHECK (value IN (-1, 1)),
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (user_id, project_id)
) WITHOUT ROWID;
CREATE INDEX votes_project ON votes(project_id);
"#, r#"
ALTER TABLE projects ADD COLUMN commits_json TEXT NOT NULL DEFAULT '{}';
"#, r#"
CREATE TABLE project_versions (
    project_id      INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    version         TEXT NOT NULL,
    unit            TEXT NOT NULL DEFAULT 'bytes',
    layout          TEXT NOT NULL DEFAULT 'treemap',
    code_total      INTEGER NOT NULL DEFAULT 0,
    code_matching   INTEGER NOT NULL DEFAULT 0,
    code_nonmatching INTEGER NOT NULL DEFAULT 0,
    data_total      INTEGER NOT NULL DEFAULT 0,
    data_done       INTEGER NOT NULL DEFAULT 0,
    prev_code_total      INTEGER NOT NULL DEFAULT 0,
    prev_code_matching   INTEGER NOT NULL DEFAULT 0,
    prev_code_nonmatching INTEGER NOT NULL DEFAULT 0,
    prev_data_total      INTEGER NOT NULL DEFAULT 0,
    prev_data_done       INTEGER NOT NULL DEFAULT 0,
    areas_json      TEXT NOT NULL DEFAULT '[]',
    last_report_at  INTEGER,
    last_commit     TEXT,
    last_message    TEXT,
    commits_json    TEXT NOT NULL DEFAULT '{}',
    PRIMARY KEY (project_id, version)
) WITHOUT ROWID;
ALTER TABLE reports ADD COLUMN version TEXT NOT NULL DEFAULT '';
CREATE INDEX reports_project_version_time ON reports(project_id, version, created_at);
ALTER TABLE projects ADD COLUMN default_version TEXT NOT NULL DEFAULT '';
-- Existing progress becomes the unnamed version.
INSERT INTO project_versions
    SELECT id, '', unit, layout, code_total, code_matching, code_nonmatching, data_total, data_done,
           prev_code_total, prev_code_matching, prev_code_nonmatching, prev_data_total, prev_data_done,
           areas_json, last_report_at, last_commit, last_message, commits_json
    FROM projects WHERE last_report_at IS NOT NULL;
"#, r#"
ALTER TABLE project_versions ADD COLUMN functions_total INTEGER NOT NULL DEFAULT 0;
ALTER TABLE project_versions ADD COLUMN functions_matched INTEGER NOT NULL DEFAULT 0;
ALTER TABLE projects ADD COLUMN functions_total INTEGER NOT NULL DEFAULT 0;
ALTER TABLE projects ADD COLUMN functions_matched INTEGER NOT NULL DEFAULT 0;
"#, r#"
CREATE TABLE import_jobs (
    id              INTEGER PRIMARY KEY,
    project_id      INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    requested_by    INTEGER REFERENCES users(id) ON DELETE SET NULL,
    status          TEXT NOT NULL CHECK (status IN ('queued', 'running', 'finished', 'failed')),
    attempts        INTEGER NOT NULL DEFAULT 0,
    runs_total      INTEGER NOT NULL DEFAULT 0,
    runs_done       INTEGER NOT NULL DEFAULT 0,
    imported        INTEGER NOT NULL DEFAULT 0,
    error           TEXT,
    created_at      INTEGER NOT NULL,
    started_at      INTEGER,
    finished_at     INTEGER
);
CREATE INDEX import_jobs_project ON import_jobs(project_id, id);
CREATE INDEX import_jobs_status ON import_jobs(status, id);
"#, r#"
ALTER TABLE import_jobs ADD COLUMN kind TEXT NOT NULL DEFAULT 'github';
ALTER TABLE import_jobs ADD COLUMN source TEXT;
"#];

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
    pub website: Option<String>,
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
    pub website: Option<String>,
    pub description: String,
    pub ai_generated: bool,
}

const PROJECT_COLS: &str = "p.id, p.slug, p.owner_id, u.login, p.name, p.platform, p.repo, p.website, p.description,
    p.image, p.ai_generated, p.spotlight, p.hidden, p.api_key_prefix, p.unit, p.layout,
    p.code_total, p.code_matching, p.code_nonmatching, p.data_total, p.data_done,
    p.prev_code_total, p.prev_code_matching, p.prev_code_nonmatching, p.prev_data_total, p.prev_data_done,
    p.areas_json, p.last_report_at, p.last_commit, p.last_message, p.upvotes, p.downvotes, p.created_at, p.updated_at,
    p.commits_json, p.default_version, p.functions_total, p.functions_matched";

fn project_from_row(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        slug: r.get(1)?,
        owner_id: r.get(2)?,
        owner_login: r.get(3)?,
        name: r.get(4)?,
        platform: r.get(5)?,
        repo: r.get(6)?,
        website: r.get(7)?,
        description: r.get(8)?,
        image: r.get(9)?,
        ai_generated: r.get(10)?,
        spotlight: r.get(11)?,
        hidden: r.get(12)?,
        api_key_prefix: r.get(13)?,
        unit: r.get(14)?,
        layout: Layout::parse(&r.get::<_, String>(15)?),
        code: CodeStats { total: r.get(16)?, matching: r.get(17)?, non_matching: r.get(18)? },
        data: DataStats { total: r.get(19)?, done: r.get(20)? },
        prev_code: CodeStats { total: r.get(21)?, matching: r.get(22)?, non_matching: r.get(23)? },
        prev_data: DataStats { total: r.get(24)?, done: r.get(25)? },
        areas_json: r.get(26)?,
        last_report_at: r.get(27)?,
        last_commit: r.get(28)?,
        last_message: r.get(29)?,
        upvotes: r.get(30)?,
        downvotes: r.get(31)?,
        created_at: r.get(32)?,
        updated_at: r.get(33)?,
        commits_json: r.get(34)?,
        default_version: r.get(35)?,
        functions: FunctionStats { total: r.get(36)?, matching: r.get(37)? },
    })
}

fn user_from_row(r: &Row) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get(0)?,
        github_id: r.get(1)?,
        login: r.get(2)?,
        name: r.get(3)?,
        avatar_url: r.get(4)?,
        is_banned: r.get(5)?,
    })
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Db> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // WAL is persistent, so switch it once up front instead of racing in every pooled connection.
        Connection::open(path)?.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        let manager = SqliteConnectionManager::file(path).with_init(|c| {
            c.execute_batch(
                "PRAGMA busy_timeout = 5000;
                 PRAGMA synchronous = NORMAL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA temp_store = MEMORY;
                 PRAGMA cache_size = -16000;",
            )
        });
        let pool = Pool::builder().max_size(8).build(manager).context("opening database")?;
        migrate(&mut *pool.get()?)?;
        Ok(Db { pool })
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
        tokio::task::spawn_blocking(move || {
            let mut conn = pool.get()?;
            f(&mut conn)
        })
        .await?
    }
}

fn migrate(conn: &mut Connection) -> anyhow::Result<()> {
    let version: usize = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i + 1)?;
        tx.commit()?;
        tracing::info!("applied migration {}", i + 1);
    }
    Ok(())
}

// ---------------------------------------------------------------- users & sessions

pub fn upsert_user(c: &Conn, github_id: i64, login: &str, name: Option<&str>, avatar: Option<&str>) -> anyhow::Result<User> {
    c.execute(
        "INSERT INTO users (github_id, login, name, avatar_url, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(github_id) DO UPDATE SET login = excluded.login, name = excluded.name, avatar_url = excluded.avatar_url",
        params![github_id, login, name, avatar, now()],
    )?;
    Ok(c.query_row(
        "SELECT id, github_id, login, name, avatar_url, is_banned FROM users WHERE github_id = ?1",
        [github_id],
        user_from_row,
    )?)
}

pub fn create_session(c: &Conn, token_hash: &str, user_id: i64, gh_token: &str, ttl: i64) -> anyhow::Result<()> {
    let t = now();
    c.execute("DELETE FROM sessions WHERE expires_at < ?1", [t])?;
    c.execute(
        "INSERT INTO sessions (token_hash, user_id, gh_token, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![token_hash, user_id, gh_token, t, t + ttl],
    )?;
    Ok(())
}

pub fn session_user(c: &Conn, token_hash: &str) -> anyhow::Result<Option<(User, String)>> {
    Ok(c.query_row(
        "SELECT u.id, u.github_id, u.login, u.name, u.avatar_url, u.is_banned, s.gh_token
         FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = ?1 AND s.expires_at > ?2",
        params![token_hash, now()],
        |r| Ok((user_from_row(r)?, r.get(6)?)),
    )
    .optional()?)
}

pub fn delete_session(c: &Conn, token_hash: &str) -> anyhow::Result<()> {
    c.execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash])?;
    Ok(())
}

pub fn list_users(c: &Conn) -> anyhow::Result<Vec<User>> {
    let mut st = c.prepare("SELECT id, github_id, login, name, avatar_url, is_banned FROM users ORDER BY id DESC")?;
    let rows = st.query_map([], user_from_row)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

pub fn set_banned(c: &Conn, user_id: i64, banned: bool) -> anyhow::Result<()> {
    c.execute("UPDATE users SET is_banned = ?2 WHERE id = ?1", params![user_id, banned])?;
    if banned {
        c.execute("DELETE FROM sessions WHERE user_id = ?1", [user_id])?;
    }
    Ok(())
}

// ---------------------------------------------------------------- projects

fn query_projects(c: &Conn, where_sql: &str, p: impl rusqlite::Params) -> anyhow::Result<Vec<Project>> {
    let sql = format!("SELECT {PROJECT_COLS} FROM projects p JOIN users u ON u.id = p.owner_id {where_sql}");
    let mut st = c.prepare_cached(&sql)?;
    let rows = st.query_map(p, project_from_row)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

/// All projects that may appear in public listings (not admin-hidden, owner not banned,
/// and with the required cover image).
pub fn public_projects(c: &Conn) -> anyhow::Result<Vec<Project>> {
    query_projects(c, "WHERE p.hidden = 0 AND u.is_banned = 0 AND p.image IS NOT NULL", [])
}

pub fn all_projects(c: &Conn) -> anyhow::Result<Vec<Project>> {
    query_projects(c, "ORDER BY p.created_at DESC", [])
}

pub fn projects_by_owner(c: &Conn, owner_id: i64) -> anyhow::Result<Vec<Project>> {
    query_projects(c, "WHERE p.owner_id = ?1 ORDER BY p.name COLLATE NOCASE", [owner_id])
}

pub fn project_by_slug(c: &Conn, slug: &str) -> anyhow::Result<Option<Project>> {
    Ok(query_projects(c, "WHERE p.slug = ?1", [slug])?.into_iter().next())
}

pub fn project_by_id(c: &Conn, id: i64) -> anyhow::Result<Option<Project>> {
    Ok(query_projects(c, "WHERE p.id = ?1", [id])?.into_iter().next())
}

pub fn project_by_key_hash(c: &Conn, key_hash: &str) -> anyhow::Result<Option<Project>> {
    Ok(query_projects(c, "WHERE p.api_key_hash = ?1 AND u.is_banned = 0", [key_hash])?.into_iter().next())
}

pub fn slug_taken(c: &Conn, slug: &str, except_id: i64) -> anyhow::Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM projects WHERE slug = ?1 AND id != ?2)", params![slug, except_id], |r| r.get(0))?)
}

pub fn insert_project(c: &Conn, owner_id: i64, m: &ProjectMeta, image: &str, key_hash: &str, key_prefix: &str) -> anyhow::Result<i64> {
    let t = now();
    c.execute(
        "INSERT INTO projects (slug, owner_id, name, platform, repo, website, description, ai_generated, image,
                               api_key_hash, api_key_prefix, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
        params![m.slug, owner_id, m.name, m.platform, m.repo, m.website, m.description, m.ai_generated, image, key_hash, key_prefix, t],
    )?;
    Ok(c.last_insert_rowid())
}

pub fn update_project_meta(c: &Conn, id: i64, m: &ProjectMeta) -> anyhow::Result<()> {
    c.execute(
        "UPDATE projects SET slug = ?2, name = ?3, platform = ?4, repo = ?5, website = ?6, description = ?7,
                             ai_generated = ?8, updated_at = ?9 WHERE id = ?1",
        params![id, m.slug, m.name, m.platform, m.repo, m.website, m.description, m.ai_generated, now()],
    )?;
    Ok(())
}

pub fn set_project_image(c: &Conn, id: i64, image: Option<&str>) -> anyhow::Result<()> {
    c.execute("UPDATE projects SET image = ?2, updated_at = ?3 WHERE id = ?1", params![id, image, now()])?;
    Ok(())
}

pub fn set_api_key(c: &Conn, id: i64, key_hash: &str, key_prefix: &str) -> anyhow::Result<()> {
    c.execute("UPDATE projects SET api_key_hash = ?2, api_key_prefix = ?3 WHERE id = ?1", params![id, key_hash, key_prefix])?;
    Ok(())
}

pub fn delete_project(c: &Conn, id: i64) -> anyhow::Result<()> {
    c.execute("DELETE FROM projects WHERE id = ?1", [id])?;
    Ok(())
}

pub fn set_spotlight(c: &Conn, id: i64, on: bool) -> anyhow::Result<()> {
    c.execute("UPDATE projects SET spotlight = ?2 WHERE id = ?1", params![id, on])?;
    Ok(())
}

pub fn set_hidden(c: &Conn, id: i64, on: bool) -> anyhow::Result<()> {
    c.execute("UPDATE projects SET hidden = ?2 WHERE id = ?1", params![id, on])?;
    Ok(())
}

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

fn version_from_row(r: &Row) -> rusqlite::Result<VersionState> {
    Ok(VersionState {
        version: r.get(0)?,
        unit: r.get(1)?,
        layout: Layout::parse(&r.get::<_, String>(2)?),
        code: CodeStats { total: r.get(3)?, matching: r.get(4)?, non_matching: r.get(5)? },
        data: DataStats { total: r.get(6)?, done: r.get(7)? },
        prev_code: CodeStats { total: r.get(8)?, matching: r.get(9)?, non_matching: r.get(10)? },
        prev_data: DataStats { total: r.get(11)?, done: r.get(12)? },
        areas_json: r.get(13)?,
        last_report_at: r.get(14)?,
        last_commit: r.get(15)?,
        last_message: r.get(16)?,
        commits_json: r.get(17)?,
        functions: FunctionStats { total: r.get(18)?, matching: r.get(19)? },
    })
}

/// All versions of a project that have reported progress, by name.
pub fn versions_for_project(c: &Conn, project_id: i64) -> anyhow::Result<Vec<VersionState>> {
    let sql = format!("SELECT {VERSION_COLS} FROM project_versions WHERE project_id = ?1 ORDER BY version");
    let mut st = c.prepare_cached(&sql)?;
    let rows = st.query_map([project_id], version_from_row)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

fn version_state(c: &Conn, project_id: i64, version: &str) -> anyhow::Result<Option<VersionState>> {
    let sql = format!("SELECT {VERSION_COLS} FROM project_versions WHERE project_id = ?1 AND version = ?2");
    Ok(c.query_row(&sql, params![project_id, version], version_from_row).optional()?)
}

/// Copies a version's progress into the project row, which listings and cards read.
fn mirror_version(c: &Connection, project_id: i64, version: &str) -> anyhow::Result<()> {
    c.execute(
        "UPDATE projects SET (unit, layout, code_total, code_matching, code_nonmatching, data_total, data_done,
                prev_code_total, prev_code_matching, prev_code_nonmatching, prev_data_total, prev_data_done,
                areas_json, last_report_at, last_commit, last_message, commits_json, functions_total, functions_matched) =
            (SELECT unit, layout, code_total, code_matching, code_nonmatching, data_total, data_done,
                prev_code_total, prev_code_matching, prev_code_nonmatching, prev_data_total, prev_data_done,
                areas_json, last_report_at, last_commit, last_message, commits_json, functions_total, functions_matched
             FROM project_versions v WHERE v.project_id = projects.id AND v.version = ?2)
         WHERE id = ?1 AND EXISTS (SELECT 1 FROM project_versions WHERE project_id = ?1 AND version = ?2)",
        params![project_id, version],
    )?;
    Ok(())
}

/// Makes `version` the one shown on cards and listings.
pub fn set_default_version(c: &mut Conn, project_id: i64, version: &str) -> anyhow::Result<()> {
    let tx = c.transaction()?;
    tx.execute("UPDATE projects SET default_version = ?2 WHERE id = ?1", params![project_id, version])?;
    mirror_version(&tx, project_id, version)?;
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

/// Stores a new report for its version. Returns `None` if nothing changed compared to that
/// version's current state (in which case nothing is written and no cache needs purging).
/// The first version a project reports becomes its default.
pub fn apply_report(c: &mut Conn, project: &Project, r: &Report, at: Option<i64>) -> anyhow::Result<Option<Applied>> {
    let version = r.version.as_str();
    let areas_json = serde_json::to_string(&r.areas)?;
    let current = version_state(c, project.id, version)?;
    let t = at.unwrap_or_else(now);
    // Older than the current state: it only belongs in the history.
    if current.as_ref().and_then(|v| v.last_report_at).is_some_and(|last| t < last) {
        return Ok(insert_history(c, project.id, r, t)?.then_some(Applied { prev_commit: None, historical: true }));
    }
    if let Some(v) = &current {
        if v.code == r.code && v.data == r.data && v.functions == r.functions && v.areas_json == areas_json && v.unit == r.unit && v.layout == r.layout {
            return Ok(None);
        }
    }
    let prev_commit = current.as_ref().and_then(|v| v.last_commit.clone());
    let prev = current.map(|v| (v.code, v.data)).unwrap_or_default();
    let tx = c.transaction()?;
    tx.execute(
        "INSERT INTO project_versions (project_id, version, unit, layout, code_total, code_matching, code_nonmatching,
                data_total, data_done, prev_code_total, prev_code_matching, prev_code_nonmatching, prev_data_total,
                prev_data_done, areas_json, last_report_at, last_commit, last_message, commits_json,
                functions_total, functions_matched)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, '{}', ?19, ?20)
         ON CONFLICT(project_id, version) DO UPDATE SET
            unit = excluded.unit, layout = excluded.layout, code_total = excluded.code_total,
            code_matching = excluded.code_matching, code_nonmatching = excluded.code_nonmatching,
            data_total = excluded.data_total, data_done = excluded.data_done,
            prev_code_total = excluded.prev_code_total, prev_code_matching = excluded.prev_code_matching,
            prev_code_nonmatching = excluded.prev_code_nonmatching, prev_data_total = excluded.prev_data_total,
            prev_data_done = excluded.prev_data_done, areas_json = excluded.areas_json,
            last_report_at = excluded.last_report_at, last_commit = excluded.last_commit,
            last_message = excluded.last_message, commits_json = '{}',
            functions_total = excluded.functions_total, functions_matched = excluded.functions_matched",
        params![
            project.id, version, r.unit, r.layout.as_str(), r.code.total, r.code.matching, r.code.non_matching,
            r.data.total, r.data.done, prev.0.total, prev.0.matching, prev.0.non_matching, prev.1.total, prev.1.done,
            areas_json, t, r.commit, r.message, r.functions.total, r.functions.matching
        ],
    )?;
    tx.execute(
        "INSERT INTO reports (project_id, version, created_at, code_total, code_matching, code_nonmatching, data_total, data_done, commit_hash, message)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![project.id, version, t, r.code.total, r.code.matching, r.code.non_matching, r.data.total, r.data.done, r.commit, r.message],
    )?;
    // Fall back to this version as the default if the current default has never reported.
    let default: String = tx.query_row("SELECT default_version FROM projects WHERE id = ?1", [project.id], |r| r.get(0))?;
    let default_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM project_versions WHERE project_id = ?1 AND version = ?2)",
        params![project.id, default],
        |r| r.get(0),
    )?;
    let default = if default_exists {
        default
    } else {
        tx.execute("UPDATE projects SET default_version = ?2 WHERE id = ?1", params![project.id, version])?;
        version.to_string()
    };
    if default == version {
        mirror_version(&tx, project.id, version)?;
    }
    tx.commit()?;
    Ok(Some(Applied { prev_commit, historical: false }))
}

/// Adds a past report to a version's history without touching its current state. Returns
/// `false` if that commit (or that exact time) is already in the history.
fn insert_history(c: &mut Conn, project_id: i64, r: &Report, at: i64) -> anyhow::Result<bool> {
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM reports WHERE project_id = ?1 AND version = ?2
                       AND (created_at = ?3 OR (?4 IS NOT NULL AND commit_hash = ?4)))",
        params![project_id, r.version, at, r.commit],
        |row| row.get(0),
    )?;
    if exists {
        return Ok(false);
    }
    c.execute(
        "INSERT INTO reports (project_id, version, created_at, code_total, code_matching, code_nonmatching, data_total, data_done, commit_hash, message)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![project_id, r.version, at, r.code.total, r.code.matching, r.code.non_matching, r.data.total, r.data.done, r.commit, r.message],
    )?;
    Ok(true)
}

pub fn reports_for_project(c: &Conn, project_id: i64, version: &str, limit: usize) -> anyhow::Result<Vec<ReportRow>> {
    let mut st = c.prepare_cached(
        "SELECT created_at, code_total, code_matching, code_nonmatching, data_total, data_done, commit_hash, message
         FROM reports WHERE project_id = ?1 AND version = ?2 ORDER BY created_at DESC LIMIT ?3",
    )?;
    let mut rows: Vec<ReportRow> = st
        .query_map(params![project_id, version, limit as i64], |r| {
            Ok(ReportRow {
                created_at: r.get(0)?,
                code: CodeStats { total: r.get(1)?, matching: r.get(2)?, non_matching: r.get(3)? },
                data: DataStats { total: r.get(4)?, done: r.get(5)? },
                commit: r.get(6)?,
                message: r.get(7)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    rows.reverse();
    Ok(rows)
}

/// For every project, the code stats its default version had at `since` (the latest report
/// at or before it, or its first report if it was created afterwards). Used for "most progress".
pub fn baselines_since(c: &Conn, since: i64) -> anyhow::Result<Vec<(i64, CodeStats)>> {
    let mut st = c.prepare_cached(
        "SELECT p.id,
            COALESCE(
              (SELECT r.id FROM reports r WHERE r.project_id = p.id AND r.version = p.default_version
                 AND r.created_at <= ?1 ORDER BY r.created_at DESC LIMIT 1),
              (SELECT r.id FROM reports r WHERE r.project_id = p.id AND r.version = p.default_version
                 ORDER BY r.created_at ASC LIMIT 1)
            ) AS rid
         FROM projects p WHERE p.last_report_at >= ?1",
    )?;
    let ids: Vec<(i64, Option<i64>)> = st.query_map([since], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let mut get = c.prepare_cached("SELECT code_total, code_matching, code_nonmatching FROM reports WHERE id = ?1")?;
    let mut out = Vec::with_capacity(ids.len());
    for (pid, rid) in ids {
        if let Some(rid) = rid {
            let s = get.query_row([rid], |r| Ok(CodeStats { total: r.get(0)?, matching: r.get(1)?, non_matching: r.get(2)? }))?;
            out.push((pid, s));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- votes

/// Set a user's vote (`value` of -1, 0 or 1) and refresh the project's counters.
pub fn vote(c: &mut Conn, user_id: i64, project_id: i64, value: i64) -> anyhow::Result<(i64, i64)> {
    let tx = c.transaction()?;
    if value == 0 {
        tx.execute("DELETE FROM votes WHERE user_id = ?1 AND project_id = ?2", params![user_id, project_id])?;
    } else {
        tx.execute(
            "INSERT INTO votes (user_id, project_id, value, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id, project_id) DO UPDATE SET value = excluded.value",
            params![user_id, project_id, value.signum(), now()],
        )?;
    }
    let (up, down): (i64, i64) = tx.query_row(
        "SELECT COALESCE(SUM(value = 1), 0), COALESCE(SUM(value = -1), 0) FROM votes WHERE project_id = ?1",
        [project_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    tx.execute("UPDATE projects SET upvotes = ?2, downvotes = ?3 WHERE id = ?1", params![project_id, up, down])?;
    tx.commit()?;
    Ok((up, down))
}

pub fn user_votes(c: &Conn, user_id: i64) -> anyhow::Result<Vec<(i64, i64)>> {
    let mut st = c.prepare_cached("SELECT project_id, value FROM votes WHERE user_id = ?1")?;
    let rows = st.query_map([user_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    Ok(rows)
}

/// `public_projects` with the signature `Db::run` expects.
pub fn public_projects_owned(c: &mut Conn) -> anyhow::Result<Vec<Project>> {
    public_projects(c)
}

/// Stores the commits fetched for a version's report, unless a newer report has arrived
/// meanwhile.
pub fn set_commits(c: &mut Conn, project_id: i64, version: &str, for_commit: Option<&str>, commits_json: &str) -> anyhow::Result<bool> {
    let tx = c.transaction()?;
    let updated = tx.execute(
        "UPDATE project_versions SET commits_json = ?4 WHERE project_id = ?1 AND version = ?2 AND last_commit IS ?3",
        params![project_id, version, for_commit, commits_json],
    )? > 0;
    let is_default: bool = tx.query_row(
        "SELECT default_version = ?2 FROM projects WHERE id = ?1",
        params![project_id, version],
        |r| r.get(0),
    )?;
    if updated && is_default {
        mirror_version(&tx, project_id, version)?;
    }
    tx.commit()?;
    Ok(updated)
}

/// Projects registered for a GitHub repository (`owner/name`, case-insensitive).
pub fn projects_by_repo(c: &mut Conn, repo: &str) -> anyhow::Result<Vec<Project>> {
    query_projects(c, "WHERE lower(p.repo) = lower(?1) AND u.is_banned = 0", [repo])
}

// ---------------------------------------------------------------- import jobs

/// A history import job (see `github_app::start_history_import`).
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

fn job_from_row(r: &Row) -> rusqlite::Result<ImportJob> {
    Ok(ImportJob {
        id: r.get(0)?,
        project_id: r.get(1)?,
        project_slug: r.get(2)?,
        project_name: r.get(3)?,
        status: r.get(4)?,
        attempts: r.get(5)?,
        runs_total: r.get(6)?,
        runs_done: r.get(7)?,
        imported: r.get(8)?,
        error: r.get(9)?,
        created_at: r.get(10)?,
        started_at: r.get(11)?,
        finished_at: r.get(12)?,
        queue_position: r.get(13)?,
        kind: r.get(14)?,
        source: r.get(15)?,
    })
}

/// Queues an import for a project, or returns `None` if one is already queued or running.
pub fn create_import_job(c: &mut Conn, project_id: i64, user_id: i64, kind: &str, source: Option<&str>) -> anyhow::Result<Option<i64>> {
    let tx = c.transaction()?;
    let active: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM import_jobs WHERE project_id = ?1 AND status IN ('queued', 'running'))",
        [project_id],
        |r| r.get(0),
    )?;
    if active {
        return Ok(None);
    }
    tx.execute(
        "INSERT INTO import_jobs (project_id, requested_by, status, created_at, kind, source) VALUES (?1, ?2, 'queued', ?3, ?4, ?5)",
        params![project_id, user_id, now(), kind, source],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(Some(id))
}

pub fn latest_import_job(c: &Conn, project_id: i64) -> anyhow::Result<Option<ImportJob>> {
    let sql = format!("SELECT {JOB_COLS} FROM import_jobs j JOIN projects p ON p.id = j.project_id WHERE j.project_id = ?1 ORDER BY j.id DESC LIMIT 1");
    Ok(c.query_row(&sql, [project_id], job_from_row).optional()?)
}

pub fn recent_import_jobs(c: &Conn, limit: usize) -> anyhow::Result<Vec<ImportJob>> {
    let sql = format!("SELECT {JOB_COLS} FROM import_jobs j JOIN projects p ON p.id = j.project_id ORDER BY j.id DESC LIMIT ?1");
    let mut st = c.prepare_cached(&sql)?;
    let rows = st.query_map([limit as i64], job_from_row)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

/// After a restart: jobs that were running are queued again, and every unfinished job is
/// returned (oldest first) so it can be started.
pub fn unfinished_import_jobs(c: &Conn) -> anyhow::Result<Vec<(i64, i64)>> {
    c.execute("UPDATE import_jobs SET status = 'queued' WHERE status = 'running'", [])?;
    let mut st = c.prepare("SELECT id, project_id FROM import_jobs WHERE status = 'queued' ORDER BY id")?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Marks a job as running and returns how many times it has been started, this one included.
pub fn start_import_job(c: &Conn, id: i64) -> anyhow::Result<i64> {
    c.execute(
        "UPDATE import_jobs SET status = 'running', attempts = attempts + 1, started_at = ?2, error = NULL WHERE id = ?1",
        params![id, now()],
    )?;
    Ok(c.query_row("SELECT attempts FROM import_jobs WHERE id = ?1", [id], |r| r.get(0))?)
}

pub fn import_job_progress(c: &Conn, id: i64, runs_total: usize, runs_done: usize, imported: usize) -> anyhow::Result<()> {
    c.execute(
        "UPDATE import_jobs SET runs_total = ?2, runs_done = ?3, imported = ?4 WHERE id = ?1",
        params![id, runs_total as i64, runs_done as i64, imported as i64],
    )?;
    Ok(())
}

pub fn finish_import_job(c: &Conn, id: i64, error: Option<&str>) -> anyhow::Result<()> {
    c.execute(
        "UPDATE import_jobs SET status = ?2, error = ?3, finished_at = ?4 WHERE id = ?1",
        params![id, if error.is_some() { "failed" } else { "finished" }, error, now()],
    )?;
    Ok(())
}

/// What a job imports: its kind and source (see `ImportJob`).
pub fn import_job_source(c: &Conn, id: i64) -> anyhow::Result<(String, Option<String>)> {
    Ok(c.query_row("SELECT kind, source FROM import_jobs WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?)
}
