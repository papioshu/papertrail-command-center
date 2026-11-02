//! Claude Code + Codex session adapters. Read-only.
//! Claude: incremental JSONL parsing from persisted byte offsets (~/.claude/projects/*/*.jsonl).
//! Codex: the `threads` table of its local state DB (rollout files are too large to parse routinely).
use crate::registry::Project;
use crate::util::{local_day, mtime_secs, norm, truncate, under, unix_to_iso};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Max bytes parsed per file per pass, so a huge transcript can't stall a refresh.
const CHUNK: u64 = 48 << 20;
const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

#[derive(Default)]
struct Acc {
    cwd: Option<String>,
    branch: Option<String>,
    first: Option<String>,
    last: Option<String>,
    prompt_title: Option<String>,
    ai_title: Option<String>,
    prompts: i64,
    edits: i64,
    days: HashMap<String, (i64, i64)>,
    files: HashMap<String, (i64, String)>,
    bad_lines: usize,
}

fn is_real_prompt(text: &str) -> bool {
    let t = text.trim_start();
    !t.is_empty() && !t.starts_with('<') && !t.starts_with("Caveat:") && !t.starts_with("[Request interrupted")
}

fn absorb(acc: &mut Acc, v: &Value) {
    let ty = v["type"].as_str().unwrap_or("");
    if ty == "ai-title" {
        if let Some(t) = v["aiTitle"].as_str() {
            acc.ai_title = Some(truncate(t, 140));
        }
        return;
    }
    if ty != "user" && ty != "assistant" {
        return;
    }
    let Some(ts) = v["timestamp"].as_str() else { return };
    if acc.cwd.is_none() {
        acc.cwd = v["cwd"].as_str().map(str::to_string);
    }
    if let Some(b) = v["gitBranch"].as_str().filter(|b| !b.is_empty()) {
        acc.branch = Some(b.to_string());
    }
    if acc.first.as_deref().map_or(true, |f| ts < f) {
        acc.first = Some(ts.to_string());
    }
    if acc.last.as_deref().map_or(true, |l| ts > l) {
        acc.last = Some(ts.to_string());
    }
    let day = local_day(ts).unwrap_or_else(|| ts.chars().take(10).collect());
    let content = &v["message"]["content"];
    if ty == "user" {
        if v["isMeta"].as_bool() == Some(true) || v["isSidechain"].as_bool() == Some(true) {
            return;
        }
        let text = match content {
            Value::String(s) => Some(s.as_str()),
            Value::Array(a) => a.iter().find(|c| c["type"] == "text").and_then(|c| c["text"].as_str()),
            _ => None,
        };
        if let Some(t) = text.filter(|t| is_real_prompt(t)) {
            acc.prompts += 1;
            acc.days.entry(day).or_default().0 += 1;
            if acc.prompt_title.is_none() {
                acc.prompt_title = Some(truncate(t.lines().next().unwrap_or(t), 140));
            }
        }
    } else if let Value::Array(items) = content {
        for c in items {
            let name = c["name"].as_str().unwrap_or("");
            if c["type"] == "tool_use" && EDIT_TOOLS.contains(&name) {
                let f = c["input"]["file_path"].as_str().or_else(|| c["input"]["notebook_path"].as_str());
                if let Some(f) = f {
                    acc.edits += 1;
                    acc.days.entry(day.clone()).or_default().1 += 1;
                    let e = acc.files.entry(f.to_string()).or_insert((0, String::new()));
                    e.0 += 1;
                    e.1 = ts.to_string();
                }
            }
        }
    }
}

fn file_state(conn: &Connection, key: &str) -> Option<(i64, i64, i64)> {
    conn.query_row("SELECT size, mtime, offset FROM file_state WHERE key=?1", [key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()
        .ok()
        .flatten()
}

/// Parse new bytes of one transcript. Returns bytes consumed.
fn index_claude_file(conn: &Connection, path: &Path) -> Result<u64, String> {
    let key = format!("claude:{}", path.display());
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let size = meta.len() as i64;
    let mtime = mtime_secs(path).unwrap_or(0);
    let session = path.file_stem().unwrap().to_string_lossy().into_owned();
    let (mut offset, prev) = match file_state(conn, &key) {
        Some((s, m, o)) if s == size && m == mtime && o >= size => return Ok(0),
        Some((_, _, o)) => (o, true),
        None => (0, false),
    };
    if prev && offset > size {
        // Truncated/rewritten: start over for this session.
        offset = 0;
        conn.execute("DELETE FROM sessions WHERE id=?1", [&session]).map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM session_days WHERE session_id=?1", [&session]).map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM session_files WHERE session_id=?1", [&session]).map_err(|e| e.to_string())?;
    }
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(offset as u64)).map_err(|e| e.to_string())?;
    let mut r = BufReader::new(f);
    let mut acc = Acc::default();
    let mut consumed: u64 = 0;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = r.read_until(b'\n', &mut buf).map_err(|e| e.to_string())?;
        if n == 0 || buf.last() != Some(&b'\n') {
            break; // EOF or a partial line still being written: leave it for next pass.
        }
        consumed += n as u64;
        match serde_json::from_slice::<Value>(&buf) {
            Ok(v) => absorb(&mut acc, &v),
            Err(_) => acc.bad_lines += 1,
        }
        if consumed >= CHUNK {
            break;
        }
    }
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    if acc.last.is_some() || acc.ai_title.is_some() {
        tx.execute(
            "INSERT INTO sessions(id,agent,cwd,title,branch,started,updated,prompts,edits,path)
             VALUES(?1,'claude',?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(id) DO UPDATE SET
               cwd=coalesce(sessions.cwd,excluded.cwd),
               title=CASE WHEN ?10 IS NOT NULL THEN ?10 ELSE coalesce(sessions.title,excluded.title) END,
               branch=coalesce(excluded.branch,sessions.branch),
               started=coalesce(sessions.started,excluded.started),
               updated=max(coalesce(sessions.updated,''),coalesce(excluded.updated,'')),
               prompts=sessions.prompts+excluded.prompts, edits=sessions.edits+excluded.edits",
            params![
                session,
                acc.cwd,
                acc.ai_title.clone().or(acc.prompt_title.clone()),
                acc.branch,
                acc.first,
                acc.last,
                acc.prompts,
                acc.edits,
                path.to_string_lossy(),
                acc.ai_title
            ],
        )
        .map_err(|e| e.to_string())?;
        for (day, (p, e)) in &acc.days {
            tx.execute(
                "INSERT INTO session_days(session_id,day,prompts,edits) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(session_id,day) DO UPDATE SET prompts=prompts+excluded.prompts, edits=edits+excluded.edits",
                params![session, day, p, e],
            )
            .map_err(|e| e.to_string())?;
        }
        for (file, (n, ts)) in &acc.files {
            tx.execute(
                "INSERT INTO session_files(session_id,path,edits,last_ts) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(session_id,path) DO UPDATE SET edits=edits+excluded.edits, last_ts=max(last_ts,excluded.last_ts)",
                params![session, file, n, ts],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    let new_off = offset + consumed as i64;
    tx.execute(
        "INSERT INTO file_state(key,size,mtime,offset) VALUES(?1,?2,?3,?4)
         ON CONFLICT(key) DO UPDATE SET size=excluded.size, mtime=excluded.mtime, offset=excluded.offset",
        params![key, size, mtime, new_off],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    if acc.bad_lines > 0 {
        crate::db::record_error(conn, "claude", &format!("{} unparseable line(s) skipped in {}", acc.bad_lines, path.display()));
    }
    Ok(consumed)
}

pub fn index_claude(conn: &Connection, claude_dir: &Path) -> Result<usize, String> {
    let root = claude_dir.join("projects");
    let rd = std::fs::read_dir(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    let mut changed = 0;
    for d in rd.flatten().filter(|d| d.path().is_dir()) {
        let Ok(files) = std::fs::read_dir(d.path()) else { continue };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            match index_claude_file(conn, &p) {
                Ok(n) if n > 0 => changed += 1,
                Ok(_) => {}
                Err(e) => crate::db::record_error(conn, "claude", &format!("{}: {e}", p.display())),
            }
        }
    }
    Ok(changed)
}

fn codex_db(codex_dir: &Path) -> Option<PathBuf> {
    let mut dbs: Vec<PathBuf> = std::fs::read_dir(codex_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy();
            n.starts_with("state_") && n.ends_with(".sqlite")
        })
        .collect();
    dbs.sort();
    dbs.pop()
}

pub fn index_codex(conn: &Connection, codex_dir: &Path) -> Result<usize, String> {
    let Some(dbp) = codex_db(codex_dir) else { return Err(format!("no state_*.sqlite in {}", codex_dir.display())) };
    let src = Connection::open_with_flags(&dbp, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| e.to_string())?;
    src.busy_timeout(std::time::Duration::from_secs(3)).ok();
    let mut stmt = src
        .prepare(
            "SELECT id, cwd, title, created_at, updated_at, git_branch, archived, rollout_path,
                    substr(coalesce(first_user_message,''),1,4000) FROM threads",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut n = 0;
    for row in rows.flatten() {
        let (id, cwd, title, created, updated, branch, archived, path, first_msg) = row;
        let cwd = cwd.trim_start_matches(r"\\?\").to_string();
        // Title holds the whole first prompt; keep the first line. The full prompt stays in Codex.
        let t = truncate(title.lines().find(|l| !l.trim().is_empty()).unwrap_or(""), 140);
        let created_iso = unix_to_iso(created);
        let updated_iso = unix_to_iso(updated);
        tx.execute(
            "INSERT INTO sessions(id,agent,cwd,title,branch,started,updated,prompts,edits,archived,path)
             VALUES(?1,'codex',?2,?3,?4,?5,?6,0,0,?7,?8)
             ON CONFLICT(id) DO UPDATE SET cwd=excluded.cwd,title=excluded.title,branch=excluded.branch,
               updated=excluded.updated,archived=excluded.archived",
            params![id, cwd, t, branch, created_iso, updated_iso, archived, path],
        )
        .map_err(|e| e.to_string())?;
        // Keep a capped hint for project mapping by mention (not shown in the UI).
        tx.execute(
            "INSERT INTO file_state(key,extra) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET extra=excluded.extra",
            params![format!("codexhint:{id}"), truncate(&format!("{title}\n{first_msg}"), 4000)],
        )
        .map_err(|e| e.to_string())?;
        for day in [local_day(&created_iso), local_day(&updated_iso)].into_iter().flatten() {
            tx.execute(
                "INSERT OR IGNORE INTO session_days(session_id,day,prompts,edits) VALUES(?1,?2,0,0)",
                params![id, day],
            )
            .map_err(|e| e.to_string())?;
        }
        n += 1;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

/// Map sessions to projects with recorded evidence: cwd (strongest), then edited-file paths
/// (Claude), then an explicit mention of the repo path in the Codex thread's opening prompt.
pub fn map_sessions(conn: &Connection, projects: &[Project]) -> Result<(), String> {
    let repos: Vec<(&str, &str)> = projects.iter().filter_map(|p| p.repo.as_deref().map(|r| (p.id.as_str(), r))).collect();
    let best = |path: &str| -> Option<String> {
        repos.iter().filter(|(_, r)| under(path, r)).max_by_key(|(_, r)| r.len()).map(|(id, _)| id.to_string())
    };
    let sessions: Vec<(String, String, Option<String>)> = {
        let mut s = conn.prepare("SELECT id, agent, cwd FROM sessions").map_err(|e| e.to_string())?;
        let v = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(|e| e.to_string())?.flatten().collect();
        v
    };
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    for (id, agent, cwd) in sessions {
        let mut found = cwd.as_deref().and_then(best).map(|p| (p, "cwd"));
        if found.is_none() && agent == "claude" {
            let mut s = tx
                .prepare("SELECT path, edits FROM session_files WHERE session_id=?1 ORDER BY edits DESC LIMIT 50")
                .map_err(|e| e.to_string())?;
            let mut votes: HashMap<String, i64> = HashMap::new();
            for (path, n) in s.query_map([&id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).map_err(|e| e.to_string())?.flatten() {
                if let Some(p) = best(&path) {
                    *votes.entry(p).or_default() += n;
                }
            }
            found = votes.into_iter().max_by_key(|(_, n)| *n).map(|(p, _)| (p, "edited files"));
        }
        if found.is_none() && agent == "codex" {
            let hint: Option<String> = tx
                .query_row("SELECT extra FROM file_state WHERE key=?1", [format!("codexhint:{id}")], |r| r.get(0))
                .optional()
                .ok()
                .flatten();
            if let Some(h) = hint.map(|h| norm(&h)) {
                // Longest repo path (or its last two segments) mentioned in the prompt.
                found = repos
                    .iter()
                    .filter(|(_, r)| {
                        let n = norm(r);
                        let tail: Vec<&str> = n.rsplit('\\').take(2).collect();
                        h.contains(&n) || (tail.len() == 2 && h.contains(&format!("{}\\{}", tail[1], tail[0])))
                    })
                    .max_by_key(|(_, r)| r.len())
                    .map(|(id, _)| (id.to_string(), "path in prompt"));
                // Then a project name (4+ chars, as written) in the thread title's first line.
                if found.is_none() {
                    let title: String = tx
                        .query_row("SELECT coalesce(title,'') FROM sessions WHERE id=?1", [&id], |r| r.get(0))
                        .unwrap_or_default();
                    found = projects
                        .iter()
                        .filter(|p| p.name.len() >= 4 && p.repo.is_some())
                        .filter(|p| title.split(|c: char| !c.is_alphanumeric()).any(|w| w == p.name))
                        .max_by_key(|p| p.name.len())
                        .map(|p| (p.id.clone(), "name in title"));
                }
            }
        }
        let (pid, method) = match found {
            Some((p, m)) => (Some(p), Some(m)),
            None => (None, None),
        };
        tx.execute("UPDATE sessions SET project_id=?2, map_method=?3 WHERE id=?1", params![id, pid, method])
            .map_err(|e| e.to_string())?;
    }
    // One timeline event per session (upserted as it grows).
    tx.execute(
        "INSERT INTO events(project_id,ts,day,kind,source,agent,title,detail,ref,evidence)
         SELECT project_id, updated, substr(updated,1,10), 'session', agent, agent,
                coalesce(title,'(untitled session)'),
                CASE agent WHEN 'claude' THEN prompts || ' prompts · ' || edits || ' recorded edits'
                           ELSE 'thread' || CASE archived WHEN 1 THEN ' (archived)' ELSE '' END END,
                id, path
         FROM sessions WHERE updated IS NOT NULL AND updated <> ''
         ON CONFLICT(source,ref) DO UPDATE SET project_id=excluded.project_id, ts=excluded.ts, day=excluded.day,
           title=excluded.title, detail=excluded.detail",
        [],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incremental_claude_parse() {
        let dir = std::env::temp_dir().join(format!("ptcc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("sess-1.jsonl");
        let l1 = r#"{"type":"user","timestamp":"2026-11-01T10:00:00Z","cwd":"C:\\Projectwork\\OARA","message":{"role":"user","content":"Fix the board"}}"#;
        let l2 = r#"{"type":"assistant","timestamp":"2026-11-01T10:01:00Z","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"C:\\Projectwork\\OARA\\src\\a.ts"}}]}}"#;
        std::fs::write(&p, format!("{l1}\n{l2}\nnot json\n{{\"partial\":")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        crate::db::tests_migrate(&conn);
        index_claude_file(&conn, &p).unwrap();
        let (pr, ed): (i64, i64) = conn.query_row("SELECT prompts, edits FROM sessions WHERE id='sess-1'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((pr, ed), (1, 1));
        // Second pass with an appended prompt only adds the new line.
        std::fs::write(&p, format!("{l1}\n{l2}\nnot json\n{l1}\n")).unwrap();
        index_claude_file(&conn, &p).unwrap();
        let pr: i64 = conn.query_row("SELECT prompts FROM sessions WHERE id='sess-1'", [], |r| r.get(0)).unwrap();
        assert_eq!(pr, 2);
        let projects = vec![Project { id: "oara".into(), name: "OARA".into(), workspace: "OARA".into(), repo: Some(r"C:\Projectwork\OARA".into()), vault_dir: None, source: "registry" }];
        map_sessions(&conn, &projects).unwrap();
        let pid: String = conn.query_row("SELECT project_id FROM sessions WHERE id='sess-1'", [], |r| r.get(0)).unwrap();
        assert_eq!(pid, "oara");
        let _ = std::fs::remove_dir_all(dir);
    }
}
