//! Hub storage (ARCHITECTURE.md §10): SQLite in WAL mode with synchronous=FULL, ordered migrations under
//! `user_version` (each after an automatic backup), online backups with `VACUUM INTO`, and a checked restore.
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub type R<T> = Result<T, String>;

pub fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Additive only: add columns and tables, never repurpose them.
const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE users(id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL, revoked INTEGER NOT NULL DEFAULT 0);
CREATE TABLE devices(id TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id), name TEXT NOT NULL,
  secret_hash TEXT NOT NULL, created_at TEXT NOT NULL, last_seen TEXT, revoked INTEGER NOT NULL DEFAULT 0);
CREATE INDEX devices_user ON devices(user_id);
CREATE TABLE workspaces(id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL, deleted INTEGER NOT NULL DEFAULT 0);
CREATE TABLE projects(id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL REFERENCES workspaces(id), name TEXT NOT NULL,
  created_by TEXT NOT NULL, created_at TEXT NOT NULL, deleted INTEGER NOT NULL DEFAULT 0);
CREATE INDEX projects_ws ON projects(workspace_id);
-- scope: NULL = every project, else a JSON array of project ids. grants/denies: JSON arrays of permission names.
CREATE TABLE memberships(workspace_id TEXT NOT NULL REFERENCES workspaces(id), user_id TEXT NOT NULL REFERENCES users(id),
  role TEXT NOT NULL, grants TEXT NOT NULL DEFAULT '[]', denies TEXT NOT NULL DEFAULT '[]', scope TEXT,
  revoked INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, PRIMARY KEY(workspace_id, user_id));
CREATE TABLE invitations(id TEXT PRIMARY KEY, token_hash TEXT NOT NULL UNIQUE, workspace_id TEXT NOT NULL REFERENCES workspaces(id),
  role TEXT NOT NULL, scope TEXT, expires_at TEXT NOT NULL, max_uses INTEGER NOT NULL DEFAULT 1, uses INTEGER NOT NULL DEFAULT 0,
  revoked INTEGER NOT NULL DEFAULT 0, created_by TEXT, created_at TEXT NOT NULL);
CREATE INDEX invitations_ws ON invitations(workspace_id);
CREATE TABLE audit(seq INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL, workspace_id TEXT, actor_user TEXT,
  actor_device TEXT, action TEXT NOT NULL, target TEXT, detail TEXT NOT NULL DEFAULT '{}');
CREATE INDEX audit_ws ON audit(workspace_id, seq);
CREATE TRIGGER audit_no_update BEFORE UPDATE ON audit BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;
CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;
"#, r#"
-- M2 shared records (§7): task, milestone, media. Client-made UUID ids; version bumps on every write; field_versions
-- holds {field: [version, device]} for the last change of each field (per-field merge); deleted = tombstone;
-- seq = change-feed order.
CREATE TABLE records(id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL REFERENCES workspaces(id),
  project_id TEXT NOT NULL REFERENCES projects(id), kind TEXT NOT NULL, fields TEXT NOT NULL DEFAULT '{}',
  field_versions TEXT NOT NULL DEFAULT '{}', version INTEGER NOT NULL, created_by TEXT NOT NULL, updated_by TEXT NOT NULL,
  updated_at TEXT NOT NULL, deleted INTEGER NOT NULL DEFAULT 0, seq INTEGER NOT NULL UNIQUE);
CREATE INDEX records_feed ON records(workspace_id, seq);
-- Idempotency (§7.2): the reply an op got, so a retry gets the same answer. Pruned after 30 days.
CREATE TABLE applied_ops(op_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, workspace_id TEXT NOT NULL, code INTEGER NOT NULL,
  reply TEXT NOT NULL, at TEXT NOT NULL);
CREATE INDEX applied_ops_at ON applied_ops(at);
"#, r#"
-- Which project each uploaded file belongs to. Files are stored once by hash, but a media record may only name a hash
-- that was uploaded into its own workspace and project, so knowing a hash never grants its content.
CREATE TABLE blobs(sha256 TEXT NOT NULL, workspace_id TEXT NOT NULL, project_id TEXT NOT NULL, size INTEGER NOT NULL,
  uploaded_by TEXT NOT NULL, uploaded_at TEXT NOT NULL, PRIMARY KEY(sha256, workspace_id, project_id));
"#];

pub fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub fn hash(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn hash_bytes(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Drops idempotency records older than 30 days (§7.2).
pub fn prune_ops(c: &Connection) -> R<usize> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(30)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
    c.execute("DELETE FROM applied_ops WHERE at < ?1", [cutoff]).map_err(err)
}

pub fn random_hex(bytes: usize) -> R<String> {
    let mut b = vec![0u8; bytes];
    getrandom::getrandom(&mut b).map_err(err)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

pub fn new_id(prefix: &str) -> R<String> {
    Ok(format!("{prefix}_{}", random_hex(16)?))
}

pub fn db_path(data: &Path) -> PathBuf {
    data.join("hub.db")
}

pub fn backups_dir(data: &Path) -> PathBuf {
    data.join("backups")
}

/// Opens (creating if needed) and migrates. `data` = None is an in-memory hub for tests.
pub fn open(data: Option<&Path>) -> R<Connection> {
    let c = match data {
        Some(d) => {
            fs::create_dir_all(d).map_err(err)?;
            Connection::open(db_path(d)).map_err(err)?
        }
        None => Connection::open_in_memory().map_err(err)?,
    };
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;").map_err(err)?;
    migrate(&c, data.map(backups_dir).as_deref())?;
    Ok(c)
}

fn version(c: &Connection) -> R<usize> {
    c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).map(|v| v as usize).map_err(err)
}

pub fn migrate(c: &Connection, backups: Option<&Path>) -> R<()> {
    let v = version(c)?;
    if v > MIGRATIONS.len() {
        return Err(format!("database is version {v}, this hub knows {}: use a newer ptcc-hub", MIGRATIONS.len()));
    }
    if v < MIGRATIONS.len() && v > 0 {
        if let Some(dir) = backups {
            backup(c, dir, &format!("pre-migrate-v{v}-{}.db", now().replace(':', "")))?;
        }
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(v) {
        c.execute_batch(&format!("BEGIN; {sql} PRAGMA user_version={}; COMMIT;", i + 1)).map_err(|e| {
            let _ = c.execute_batch("ROLLBACK");
            format!("migration {} failed: {e}", i + 1)
        })?;
    }
    Ok(())
}

/// Online, consistent copy of the live database.
pub fn backup(c: &Connection, dir: &Path, name: &str) -> R<PathBuf> {
    fs::create_dir_all(dir).map_err(err)?;
    let p = dir.join(name);
    if p.exists() {
        return Err(format!("{} already exists", p.display()));
    }
    c.execute_batch(&format!("VACUUM INTO '{}'", p.to_string_lossy().replace('\'', "''"))).map_err(err)?;
    Ok(p)
}

/// Today's `hub-YYYY-MM-DD.db` if it isn't there yet, then rotation: the newest 14 dailies plus every 1st-of-month copy.
pub fn daily_backup(c: &Connection, dir: &Path) -> R<Option<PathBuf>> {
    let name = format!("hub-{}.db", chrono::Utc::now().format("%Y-%m-%d"));
    let made = if dir.join(&name).exists() { None } else { Some(backup(c, dir, &name)?) };
    let mut dailies: Vec<String> = fs::read_dir(dir)
        .map_err(err)?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("hub-") && n.ends_with(".db") && n.len() == "hub-YYYY-MM-DD.db".len())
        .collect();
    dailies.sort_unstable_by(|a, b| b.cmp(a));
    for old in dailies.iter().skip(14).filter(|n| !n.ends_with("-01.db")) {
        fs::remove_file(dir.join(old)).map_err(err)?;
    }
    Ok(made)
}

/// Held by `serve` for its whole run, so a restore can't swap the file under a live hub.
pub fn lock(data: &Path) -> R<fs::File> {
    fs::create_dir_all(data).map_err(err)?;
    let f = fs::OpenOptions::new().create(true).truncate(false).write(true).open(data.join("hub.lock")).map_err(err)?;
    f.try_lock().map_err(|_| "another ptcc-hub is running on this data folder; stop it first".to_string())?;
    Ok(f)
}

/// Replaces the live database with a checked backup. The current database is backed up first.
pub fn restore(data: &Path, from: &Path) -> R<PathBuf> {
    let _lock = lock(data)?;
    {
        let b = Connection::open_with_flags(from, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
        let ok: String = b.query_row("PRAGMA integrity_check", [], |r| r.get(0)).map_err(err)?;
        if ok != "ok" {
            return Err(format!("{} failed its integrity check: {ok}", from.display()));
        }
        let v = version(&b)?;
        if v == 0 || v > MIGRATIONS.len() {
            return Err(format!("{} is not a ptcc-hub database this version can use (version {v})", from.display()));
        }
    }
    let live = db_path(data);
    let mut saved = PathBuf::new();
    if live.exists() {
        let c = Connection::open(&live).map_err(err)?;
        saved = backup(&c, &backups_dir(data), &format!("pre-restore-{}.db", now().replace(':', "")))?;
        drop(c);
    }
    for ext in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{ext}", live.display()));
        if side.exists() {
            fs::remove_file(side).map_err(err)?;
        }
    }
    fs::copy(from, &live).map_err(err)?;
    Ok(saved)
}
