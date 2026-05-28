//! SQLite persistence (via bundled `rusqlite`).
//!
//! Holds durable state that should survive restarts: the structured log stream,
//! task queue, session history, workspace snapshots, and orchestrator config.
//! The live `Connection` is owned by `Core` behind a mutex; these are free
//! functions operating on a borrowed connection so call sites stay explicit
//! about when they hold the DB lock.

use crate::model::*;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

/// Open (creating if needed) the database at `path` and apply the schema.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(path)?;
    // Pragmas tuned for a write-heavy, single-writer log workload.
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    init_schema(&conn)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS config (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS sessions (
            id                 TEXT PRIMARY KEY,
            name               TEXT NOT NULL,
            mode               TEXT NOT NULL,
            cwd                TEXT NOT NULL,
            model              TEXT,
            agent_session_id  TEXT,
            status             TEXT NOT NULL,
            usage_json         TEXT NOT NULL,
            cost_usd           REAL NOT NULL,
            turns              INTEGER NOT NULL,
            created_at         INTEGER NOT NULL,
            last_activity      INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS tasks (
            id              TEXT PRIMARY KEY,
            prompt          TEXT NOT NULL,
            cwd             TEXT NOT NULL,
            model           TEXT,
            permission_mode TEXT,
            allowed_tools   TEXT,
            status          TEXT NOT NULL,
            session_id      TEXT,
            priority        INTEGER NOT NULL,
            attempts        INTEGER NOT NULL,
            max_attempts    INTEGER NOT NULL,
            created_at      INTEGER NOT NULL,
            started_at      INTEGER,
            finished_at     INTEGER,
            result_summary  TEXT,
            error           TEXT
         );
         CREATE TABLE IF NOT EXISTS logs (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  TEXT NOT NULL,
            ts          INTEGER NOT NULL,
            kind        TEXT NOT NULL,
            text        TEXT NOT NULL,
            data        TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_logs_session ON logs(session_id, id);
         CREATE TABLE IF NOT EXISTS snapshots (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            created_at  INTEGER NOT NULL,
            data_json   TEXT NOT NULL
         );",
    )?;
    // Migrate databases from a previous build that used the old column name.
    // ALTER ... RENAME COLUMN has been in SQLite since 3.25; bundled rusqlite
    // ships a newer build. The statement is a no-op when the column has already
    // been renamed (or never existed), so we silently swallow that error.
    let _ = conn.execute(
        "ALTER TABLE sessions RENAME COLUMN claude_session_id TO agent_session_id",
        [],
    );
    Ok(())
}

// ----- config ---------------------------------------------------------------

pub fn load_config(conn: &Connection) -> OrchestratorConfig {
    let row: Option<String> = conn
        .query_row("SELECT value FROM config WHERE key = 'orchestrator'", [], |r| r.get(0))
        .optional()
        .ok()
        .flatten();
    row.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_config(conn: &Connection, cfg: &OrchestratorConfig) -> Result<()> {
    let value = serde_json::to_string(cfg)?;
    conn.execute(
        "INSERT INTO config(key, value) VALUES('orchestrator', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![value],
    )?;
    Ok(())
}

// ----- logs -----------------------------------------------------------------

/// Insert a log line and return its assigned row id (0 on failure).
pub fn insert_log(conn: &Connection, session_id: &str, kind: &str, text: &str, data: Option<&str>) -> i64 {
    match conn.execute(
        "INSERT INTO logs(session_id, ts, kind, text, data) VALUES(?1, ?2, ?3, ?4, ?5)",
        params![session_id, now_ms(), kind, text, data],
    ) {
        Ok(_) => conn.last_insert_rowid(),
        Err(_) => 0,
    }
}

/// Most recent `limit` log lines for a session, returned oldest-first.
pub fn recent_logs(conn: &Connection, session_id: &str, limit: i64) -> Vec<LogEntry> {
    let mut stmt = match conn.prepare(
        "SELECT id, session_id, ts, kind, text, data FROM logs
         WHERE session_id = ?1 ORDER BY id DESC LIMIT ?2",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(params![session_id, limit], |r| {
        Ok(LogEntry {
            id: r.get(0)?,
            session_id: r.get(1)?,
            ts: r.get(2)?,
            kind: r.get(3)?,
            text: r.get(4)?,
            data: r.get(5)?,
        })
    });
    let mut out: Vec<LogEntry> = match rows {
        Ok(it) => it.filter_map(Result::ok).collect(),
        Err(_) => Vec::new(),
    };
    out.reverse();
    out
}

// ----- sessions (history) ---------------------------------------------------

pub fn upsert_session(conn: &Connection, s: &SessionInfo) {
    let usage = serde_json::to_string(&s.usage).unwrap_or_else(|_| "{}".into());
    let mode = serde_json::to_string(&s.mode).unwrap_or_default();
    let status = serde_json::to_string(&s.status).unwrap_or_default();
    let _ = conn.execute(
        "INSERT INTO sessions(id,name,mode,cwd,model,agent_session_id,status,usage_json,cost_usd,turns,created_at,last_activity)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
         ON CONFLICT(id) DO UPDATE SET
            name=excluded.name, status=excluded.status, agent_session_id=excluded.agent_session_id,
            usage_json=excluded.usage_json, cost_usd=excluded.cost_usd, turns=excluded.turns,
            last_activity=excluded.last_activity",
        params![
            s.id, s.name, mode, s.cwd, s.model, s.agent_session_id, status,
            usage, s.cost_usd, s.turns, s.created_at, s.last_activity
        ],
    );
}

// ----- tasks ----------------------------------------------------------------

pub fn upsert_task(conn: &Connection, t: &Task) {
    let status = serde_json::to_string(&t.status).unwrap_or_default();
    let _ = conn.execute(
        "INSERT INTO tasks(id,prompt,cwd,model,permission_mode,allowed_tools,status,session_id,priority,attempts,max_attempts,created_at,started_at,finished_at,result_summary,error)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
         ON CONFLICT(id) DO UPDATE SET
            status=excluded.status, session_id=excluded.session_id, attempts=excluded.attempts,
            started_at=excluded.started_at, finished_at=excluded.finished_at,
            result_summary=excluded.result_summary, error=excluded.error",
        params![
            t.id, t.prompt, t.cwd, t.model, t.permission_mode, t.allowed_tools, status,
            t.session_id, t.priority, t.attempts, t.max_attempts, t.created_at,
            t.started_at, t.finished_at, t.result_summary, t.error
        ],
    );
}

// ----- snapshots ------------------------------------------------------------

pub fn save_snapshot(conn: &Connection, id: &str, name: &str, data_json: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO snapshots(id,name,created_at,data_json) VALUES(?1,?2,?3,?4)",
        params![id, name, now_ms(), data_json],
    )?;
    Ok(())
}

pub fn load_snapshot(conn: &Connection, id: &str) -> Option<String> {
    conn.query_row(
        "SELECT data_json FROM snapshots WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// `(id, name, created_at)` for every saved snapshot, newest first.
pub fn list_snapshots(conn: &Connection) -> Vec<(String, String, i64)> {
    let mut stmt = match conn.prepare("SELECT id, name, created_at FROM snapshots ORDER BY created_at DESC") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)));
    match rows {
        Ok(it) => it.filter_map(Result::ok).collect(),
        Err(_) => Vec::new(),
    }
}
