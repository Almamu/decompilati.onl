//! SQLite storage. All access goes through `Db::run`, which executes on the blocking pool.

use std::path::Path;

use anyhow::Context;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::report::{Area, CodeStats, DataStats, Layout, Report};
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
    p.commits_json";

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

/// Store a new report. Returns `false` if nothing changed compared to the current state
/// (in which case nothing is written and no cache needs purging).
pub fn apply_report(c: &mut Conn, project: &Project, r: &Report) -> anyhow::Result<bool> {
    let areas_json = serde_json::to_string(&r.areas)?;
    let unchanged = project.code == r.code
        && project.data == r.data
        && project.areas_json == areas_json
        && project.unit == r.unit
        && project.layout == r.layout;
    if unchanged {
        return Ok(false);
    }
    let t = now();
    let tx = c.transaction()?;
    tx.execute(
        "UPDATE projects SET
            prev_code_total = code_total, prev_code_matching = code_matching, prev_code_nonmatching = code_nonmatching,
            prev_data_total = data_total, prev_data_done = data_done,
            code_total = ?2, code_matching = ?3, code_nonmatching = ?4, data_total = ?5, data_done = ?6,
            unit = ?7, layout = ?8, areas_json = ?9, last_report_at = ?10, last_commit = ?11, last_message = ?12,
            commits_json = '{}'
         WHERE id = ?1",
        params![
            project.id, r.code.total, r.code.matching, r.code.non_matching, r.data.total, r.data.done,
            r.unit, r.layout.as_str(), areas_json, t, r.commit, r.message
        ],
    )?;
    tx.execute(
        "INSERT INTO reports (project_id, created_at, code_total, code_matching, code_nonmatching, data_total, data_done, commit_hash, message)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![project.id, t, r.code.total, r.code.matching, r.code.non_matching, r.data.total, r.data.done, r.commit, r.message],
    )?;
    tx.commit()?;
    Ok(true)
}

pub fn reports_for_project(c: &Conn, project_id: i64, limit: usize) -> anyhow::Result<Vec<ReportRow>> {
    let mut st = c.prepare_cached(
        "SELECT created_at, code_total, code_matching, code_nonmatching, data_total, data_done, commit_hash, message
         FROM reports WHERE project_id = ?1 ORDER BY created_at DESC LIMIT ?2",
    )?;
    let mut rows: Vec<ReportRow> = st
        .query_map(params![project_id, limit as i64], |r| {
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

/// For every project, the code stats it had at `since` (the latest report at or before it,
/// or its first report if it was created afterwards). Used for "most progress".
pub fn baselines_since(c: &Conn, since: i64) -> anyhow::Result<Vec<(i64, CodeStats)>> {
    let mut st = c.prepare_cached(
        "SELECT p.id,
            COALESCE(
              (SELECT r.id FROM reports r WHERE r.project_id = p.id AND r.created_at <= ?1 ORDER BY r.created_at DESC LIMIT 1),
              (SELECT r.id FROM reports r WHERE r.project_id = p.id ORDER BY r.created_at ASC LIMIT 1)
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

/// Stores the commits fetched for a report, unless a newer report has arrived meanwhile.
pub fn set_commits(c: &Conn, project_id: i64, for_commit: Option<&str>, commits_json: &str) -> anyhow::Result<bool> {
    Ok(c.execute(
        "UPDATE projects SET commits_json = ?3 WHERE id = ?1 AND last_commit IS ?2",
        params![project_id, for_commit, commits_json],
    )? > 0)
}
