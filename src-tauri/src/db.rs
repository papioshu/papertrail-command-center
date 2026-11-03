//! SQLite store. The app's own cache: every row can be rebuilt from the source files.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::path::Path;

const MIGRATIONS: &[&str] = &[
    // v1
    r#"
CREATE TABLE projects(
  id TEXT PRIMARY KEY, name TEXT NOT NULL, workspace TEXT, workspace_override TEXT,
  repo_path TEXT, vault_dir TEXT, source TEXT NOT NULL, hidden INTEGER NOT NULL DEFAULT 0,
  branch TEXT, head TEXT, dirty INTEGER, ahead INTEGER, behind INTEGER, git_error TEXT,
  summary TEXT, next_action TEXT, last_activity TEXT, updated_at TEXT);
CREATE TABLE tasks(
  id INTEGER PRIMARY KEY, project_id TEXT NOT NULL, section TEXT, status TEXT NOT NULL,
  text TEXT NOT NULL, author TEXT, source_file TEXT NOT NULL, line INTEGER);
CREATE INDEX tasks_project ON tasks(project_id, status);
CREATE TABLE events(
  id INTEGER PRIMARY KEY, project_id TEXT, ts TEXT NOT NULL, day TEXT, kind TEXT NOT NULL,
  source TEXT NOT NULL, agent TEXT, title TEXT NOT NULL, detail TEXT, ref TEXT NOT NULL,
  evidence TEXT, workstream TEXT, source_file TEXT, UNIQUE(source, ref));
CREATE INDEX events_project_ts ON events(project_id, ts);
CREATE INDEX events_ts ON events(ts);
CREATE TABLE commit_files(
  project_id TEXT NOT NULL, sha TEXT NOT NULL, path TEXT NOT NULL, added INTEGER, deleted INTEGER,
  workstream TEXT, PRIMARY KEY(project_id, sha, path));
CREATE TABLE sessions(
  id TEXT PRIMARY KEY, agent TEXT NOT NULL, project_id TEXT, map_method TEXT, cwd TEXT, title TEXT,
  branch TEXT, started TEXT, updated TEXT, prompts INTEGER NOT NULL DEFAULT 0,
  edits INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0, path TEXT);
CREATE INDEX sessions_project ON sessions(project_id, updated);
CREATE TABLE session_days(
  session_id TEXT NOT NULL, day TEXT NOT NULL, prompts INTEGER NOT NULL DEFAULT 0,
  edits INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(session_id, day));
CREATE TABLE session_files(
  session_id TEXT NOT NULL, path TEXT NOT NULL, edits INTEGER NOT NULL DEFAULT 0, last_ts TEXT,
  PRIMARY KEY(session_id, path));
CREATE TABLE file_state(key TEXT PRIMARY KEY, size INTEGER, mtime INTEGER, offset INTEGER, extra TEXT);
CREATE TABLE launches(
  id INTEGER PRIMARY KEY, ts TEXT NOT NULL, project_id TEXT, agent TEXT, mode TEXT,
  handoff_path TEXT, command TEXT, ok INTEGER, error TEXT);
CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE index_errors(id INTEGER PRIMARY KEY, ts TEXT, scope TEXT, message TEXT);
CREATE TABLE discoveries(
  id TEXT PRIMARY KEY, name TEXT, url TEXT, description TEXT, license TEXT, pricing TEXT,
  stars INTEGER, created_at TEXT, pushed_at TEXT, topics TEXT, projects TEXT, reason TEXT,
  score REAL, status TEXT NOT NULL DEFAULT 'new', source TEXT, fetched_at TEXT, project_link TEXT);
CREATE TABLE review_tasks(
  id INTEGER PRIMARY KEY, ts TEXT, project_id TEXT, text TEXT, discovery_id TEXT, done INTEGER DEFAULT 0);
CREATE TABLE links(project_id TEXT NOT NULL, target_id TEXT NOT NULL, source_file TEXT NOT NULL, count INTEGER,
  PRIMARY KEY(project_id, target_id, source_file));
CREATE VIRTUAL TABLE search USING fts5(text, project_id UNINDEXED, kind UNINDEXED, ref UNINDEXED);
"#,
];

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
    )?;
    Ok(conn)
}

/// Open + migrate. A corrupt file is moved aside and rebuilt (it is only a cache).
pub fn open_migrated(path: &Path) -> Result<Connection, String> {
    let attempt = || -> rusqlite::Result<Connection> {
        let conn = open(path)?;
        let ok: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if ok != "ok" {
            return Err(rusqlite::Error::InvalidQuery);
        }
        migrate(&conn)?;
        Ok(conn)
    };
    match attempt() {
        Ok(c) => Ok(c),
        Err(e) => {
            crate::util::log("error", format!("database unusable ({e}); rebuilding cache"));
            let bad = path.with_extension(format!("corrupt-{}.db", chrono::Utc::now().timestamp()));
            let _ = std::fs::rename(path, bad);
            for ext in ["db-wal", "db-shm"] {
                let _ = std::fs::remove_file(path.with_extension(ext));
            }
            attempt().map_err(|e| e.to_string())
        }
    }
}

pub(crate) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let v: usize = conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as usize;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(v) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.execute_batch(&format!("PRAGMA user_version={}", i + 1))?;
        tx.commit()?;
    }
    Ok(())
}

pub fn setting(conn: &Connection, key: &str) -> Option<Value> {
    conn.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
}

pub fn set_setting(conn: &Connection, key: &str, v: &Value) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, v.to_string()],
    )?;
    Ok(())
}

pub fn record_error(conn: &Connection, scope: &str, msg: &str) {
    crate::util::log("warn", format!("{scope}: {msg}"));
    let _ = conn.execute(
        "INSERT INTO index_errors(ts,scope,message) VALUES(?1,?2,?3)",
        params![crate::util::now_iso(), scope, msg],
    );
    // Keep the error table bounded.
    let _ = conn.execute("DELETE FROM index_errors WHERE id <= (SELECT MAX(id) FROM index_errors) - 200", []);
}

#[cfg(test)]
mod tests {
    #[test]
    fn migrates_in_memory() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        super::migrate(&c).unwrap();
        super::migrate(&c).unwrap();
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, super::MIGRATIONS.len());
    }
}

#[cfg(test)]
pub fn tests_migrate(c: &Connection) {
    migrate(c).unwrap();
}
