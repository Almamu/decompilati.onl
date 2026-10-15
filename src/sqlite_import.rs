//! `decompsite import-sqlite <file>`: one-off copy of a database from the SQLite days into
//! an empty PostgreSQL database, keeping every row and its id.

use std::path::Path;

use postgres::types::ToSql;
use rusqlite::{types::ValueRef, Connection, OpenFlags};

use crate::db::Db;

/// The last SQLite schema version; older files must be upgraded by an older build first.
const SQLITE_SCHEMA_VERSION: i64 = 7;

#[derive(Clone, Copy)]
enum Kind {
    Int,
    OptInt,
    Text,
    OptText,
    Bool,
}
use Kind::*;

/// Tables in dependency order, with their columns.
const TABLES: &[(&str, &[(&str, Kind)])] = &[
    ("users", &[("id", Int), ("github_id", Int), ("login", Text), ("name", OptText), ("avatar_url", OptText), ("is_banned", Bool), ("created_at", Int)]),
    ("sessions", &[("token_hash", Text), ("user_id", Int), ("gh_token", Text), ("created_at", Int), ("expires_at", Int)]),
    (
        "projects",
        &[
            ("id", Int), ("slug", Text), ("owner_id", Int), ("name", Text), ("platform", Text), ("repo", Text),
            ("ai_contributions", Text), ("description", Text), ("image", OptText), ("ai_generated", Bool), ("spotlight", Bool),
            ("hidden", Bool), ("api_key_hash", Text), ("api_key_prefix", Text), ("default_version", Text), ("unit", Text),
            ("layout", Text), ("code_total", Int), ("code_matching", Int), ("code_nonmatching", Int), ("data_total", Int),
            ("data_done", Int), ("functions_total", Int), ("functions_matched", Int), ("prev_code_total", Int),
            ("prev_code_matching", Int), ("prev_code_nonmatching", Int), ("prev_data_total", Int), ("prev_data_done", Int),
            ("areas_json", Text), ("last_report_at", OptInt), ("last_commit", OptText), ("last_message", OptText),
            ("commits_json", Text), ("upvotes", Int), ("downvotes", Int), ("created_at", Int), ("updated_at", Int),
        ],
    ),
    (
        "project_versions",
        &[
            ("project_id", Int), ("version", Text), ("unit", Text), ("layout", Text), ("code_total", Int), ("code_matching", Int),
            ("code_nonmatching", Int), ("data_total", Int), ("data_done", Int), ("functions_total", Int), ("functions_matched", Int),
            ("prev_code_total", Int), ("prev_code_matching", Int), ("prev_code_nonmatching", Int), ("prev_data_total", Int),
            ("prev_data_done", Int), ("areas_json", Text), ("last_report_at", OptInt), ("last_commit", OptText),
            ("last_message", OptText), ("commits_json", Text),
        ],
    ),
    (
        "reports",
        &[
            ("id", Int), ("project_id", Int), ("version", Text), ("created_at", Int), ("code_total", Int), ("code_matching", Int),
            ("code_nonmatching", Int), ("data_total", Int), ("data_done", Int), ("commit_hash", OptText), ("message", OptText),
        ],
    ),
    ("votes", &[("user_id", Int), ("project_id", Int), ("value", Int), ("created_at", Int)]),
    (
        "import_jobs",
        &[
            ("id", Int), ("project_id", Int), ("requested_by", OptInt), ("kind", Text), ("source", OptText), ("status", Text),
            ("attempts", Int), ("runs_total", Int), ("runs_done", Int), ("imported", Int), ("error", OptText),
            ("created_at", Int), ("started_at", OptInt), ("finished_at", OptInt),
        ],
    ),
];

/// Tables whose `id` comes from a sequence that must continue after the copied rows.
const SEQUENCES: &[&str] = &["users", "projects", "reports", "import_jobs"];

fn value(v: ValueRef, kind: Kind, column: &str) -> anyhow::Result<Box<dyn ToSql + Sync>> {
    let int = |v: ValueRef| -> anyhow::Result<Option<i64>> {
        match v {
            ValueRef::Null => Ok(None),
            ValueRef::Integer(i) => Ok(Some(i)),
            other => anyhow::bail!("{column}: expected an integer, found {:?}", other.data_type()),
        }
    };
    let text = |v: ValueRef| -> anyhow::Result<Option<String>> {
        match v {
            ValueRef::Null => Ok(None),
            ValueRef::Text(t) => Ok(Some(String::from_utf8_lossy(t).into_owned())),
            other => anyhow::bail!("{column}: expected text, found {:?}", other.data_type()),
        }
    };
    Ok(match kind {
        Int => Box::new(int(v)?.ok_or_else(|| anyhow::anyhow!("{column}: unexpected NULL"))?),
        OptInt => Box::new(int(v)?),
        Text => Box::new(text(v)?.ok_or_else(|| anyhow::anyhow!("{column}: unexpected NULL"))?),
        OptText => Box::new(text(v)?),
        Bool => Box::new(int(v)?.unwrap_or(0) != 0),
    })
}

pub fn run(db: &Db, path: &Path) -> anyhow::Result<()> {
    let src = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let version: i64 = src.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    anyhow::ensure!(
        version == SQLITE_SCHEMA_VERSION,
        "{} has schema version {version}; expected {SQLITE_SCHEMA_VERSION}",
        path.display()
    );

    let mut conn = db.conn()?;
    let mut tx = conn.transaction()?;
    let existing: i64 = tx.query_one("SELECT (SELECT COUNT(*) FROM users) + (SELECT COUNT(*) FROM projects)", &[])?.get(0);
    anyhow::ensure!(existing == 0, "the PostgreSQL database already has data; import only into an empty one");

    for (table, columns) in TABLES {
        let names: Vec<&str> = columns.iter().map(|(n, _)| *n).collect();
        let placeholders: Vec<String> = (1..=columns.len()).map(|i| format!("${i}")).collect();
        let insert = tx.prepare(&format!("INSERT INTO {table} ({}) VALUES ({})", names.join(", "), placeholders.join(", ")))?;
        let mut select = src.prepare(&format!("SELECT {} FROM {table}", names.join(", ")))?;
        let mut rows = select.query([])?;
        let mut count = 0;
        while let Some(row) = rows.next()? {
            let values = columns
                .iter()
                .enumerate()
                .map(|(i, (name, kind))| value(row.get_ref(i)?, *kind, &format!("{table}.{name}")))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let params: Vec<&(dyn ToSql + Sync)> = values.iter().map(|v| v.as_ref()).collect();
            tx.execute(&insert, &params)?;
            count += 1;
        }
        println!("{table}: {count} rows");
    }
    for table in SEQUENCES {
        tx.execute(
            &format!("SELECT setval(pg_get_serial_sequence('{table}', 'id'), COALESCE((SELECT MAX(id) FROM {table}), 0) + 1, false)"),
            &[],
        )?;
    }
    tx.commit()?;
    println!("done");
    Ok(())
}
