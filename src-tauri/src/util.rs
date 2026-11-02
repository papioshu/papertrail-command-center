//! Small shared helpers: logging, bounded subprocesses, path normalization, row -> JSON.
use rusqlite::{types::ValueRef, Connection, Params};
use serde_json::{Map, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn init_log(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let p = dir.join("app.log");
    // shortcut: single-file rotation at 4 MB keeps one previous copy; enough for a personal tool.
    if std::fs::metadata(&p).map(|m| m.len() > 4 << 20).unwrap_or(false) {
        let _ = std::fs::rename(&p, dir.join("app.prev.log"));
    }
    let _ = LOG_PATH.set(p);
}

pub fn log(level: &str, msg: impl AsRef<str>) {
    let line = format!("{} [{}] {}\n", now_iso(), level, msg.as_ref());
    if let Some(p) = LOG_PATH.get() {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = f.write_all(line.as_bytes());
        }
    }
    #[cfg(debug_assertions)]
    eprint!("{line}");
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Local calendar day for an RFC3339 timestamp.
pub fn local_day(ts: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
}

pub fn unix_to_iso(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// Lowercase, backslash-separated, no `\\?\` prefix, no trailing slash. For comparisons only.
pub fn norm(p: &str) -> String {
    let s = p.trim().trim_start_matches(r"\\?\").replace('/', "\\").to_lowercase();
    s.trim_end_matches('\\').to_string()
}

/// True if `child` equals `parent` or sits beneath it (both normalized).
pub fn under(child: &str, parent: &str) -> bool {
    let (c, p) = (norm(child), norm(parent));
    !p.is_empty() && (c == p || c.starts_with(&format!("{p}\\")))
}

pub fn mtime_secs(p: &Path) -> Option<i64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(m.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64)
}

pub fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Users\Default"))
}

/// Resolve an executable on PATH (Windows: tries .exe/.cmd). No shell involved.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    dirs.push(home().join(".local").join("bin"));
    for d in dirs {
        for ext in ["exe", "cmd"] {
            let c = d.join(format!("{name}.{ext}"));
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

pub struct Output {
    pub ok: bool,
    pub stdout: String,
}

/// Run a program with structured args, no console window, killed after `timeout`.
pub fn run(program: &str, args: &[&str], cwd: Option<&Path>, timeout: Duration) -> Result<Output, String> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let mut child = cmd.spawn().map_err(|e| format!("{program}: {e}"))?;
    let mut out = child.stdout.take().unwrap();
    // Drain stdout on a thread so a large output can't deadlock the pipe.
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} timed out after {}s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let buf = reader.join().unwrap_or_default();
    Ok(Output { ok: status.success(), stdout: String::from_utf8_lossy(&buf).into_owned() })
}

/// Run a query and return rows as JSON objects keyed by column name.
pub fn rows<P: Params>(conn: &Connection, sql: &str, params: P) -> Result<Vec<Value>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut q = stmt.query(params).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(r) = q.next().map_err(|e| e.to_string())? {
        let mut m = Map::new();
        for (i, n) in names.iter().enumerate() {
            let v = match r.get_ref(i).map_err(|e| e.to_string())? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(x) => Value::from(x),
                ValueRef::Real(x) => Value::from(x),
                ValueRef::Text(t) => Value::from(String::from_utf8_lossy(t).into_owned()),
                ValueRef::Blob(_) => Value::Null,
            };
            m.insert(n.clone(), v);
        }
        out.push(Value::Object(m));
    }
    Ok(out)
}

pub fn truncate(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n { format!("{}…", t.trim_end()) } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths() {
        assert!(under(r"\\?\C:\Projectwork\OARA\src", r"c:/projectwork/oara"));
        assert!(!under(r"C:\Projectwork\OARA-TEST-ENV", r"C:\Projectwork\OARA"));
        assert_eq!(truncate("abcdef", 3), "abc…");
    }
}
