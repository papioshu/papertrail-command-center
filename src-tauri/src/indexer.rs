//! Background indexing worker: one thread, one job at a time, bounded queue, panic-contained.
use crate::registry::{self, Config, Project};
use crate::util::{log, mtime_secs, now_iso};
use crate::{agents, continuity, db, discovery, gitscan};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

pub enum Job {
    Refresh,
    Discovery,
}

pub struct Worker {
    tx: SyncSender<Job>,
}

impl Worker {
    /// Queue a job. A full queue means the same work is already pending, so dropping is correct.
    pub fn send(&self, j: Job) {
        let _ = self.tx.try_send(j);
    }
}

pub static STATUS: Mutex<Option<Value>> = Mutex::new(None);

pub type Emit<'a> = &'a dyn Fn(&str, Value);

fn set_status(emit: Emit, v: Value) {
    *STATUS.lock().unwrap() = Some(v.clone());
    emit("index-status", v);
}

pub fn spawn(app: AppHandle, db_path: PathBuf) -> Worker {
    let (tx, rx) = sync_channel::<Job>(2);
    std::thread::Builder::new()
        .name("ptcc-indexer".into())
        .spawn(move || {
            let emit_fn = |ev: &str, v: Value| {
                let _ = app.emit(ev, v);
            };
            let emit: Emit = &emit_fn;
            let conn = match db::open(&db_path) {
                Ok(c) => c,
                Err(e) => {
                    log("error", format!("indexer cannot open db: {e}"));
                    return;
                }
            };
            loop {
                let cfg = Config::load(&conn);
                let (job, scheduled) = match rx.recv_timeout(Duration::from_secs(cfg.refresh_minutes * 60)) {
                    Ok(j) => (j, false),
                    Err(RecvTimeoutError::Timeout) => (Job::Refresh, true),
                    Err(RecvTimeoutError::Disconnected) => return,
                };
                // Scheduled refreshes also run discovery when its interval has elapsed.
                let job = if scheduled && discovery::due(&conn) { Job::Discovery } else { job };
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
                    Job::Refresh => refresh(&conn, emit),
                    Job::Discovery => {
                        if scheduled {
                            refresh(&conn, emit);
                        }
                        set_status(emit, json!({"state":"discovery","at":now_iso()}));
                        let r = discovery::refresh(&conn);
                        if let Err(e) = &r {
                            db::record_error(&conn, "discovery", e);
                        }
                        emit("discovery-updated", json!(r.is_ok()));
                        set_status(emit, json!({"state":"idle","at":now_iso()}));
                    }
                }));
                if res.is_err() {
                    db::record_error(&conn, "indexer", "a job panicked; worker continues");
                    set_status(emit, json!({"state":"error","at":now_iso(),"message":"indexing job failed, see Settings › Diagnostics"}));
                }
            }
        })
        .expect("spawn indexer");
    Worker { tx }
}

fn changed(conn: &Connection, key: &str, path: &Path) -> bool {
    let size = std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(-1);
    let mtime = mtime_secs(path).unwrap_or(-1);
    let prev: Option<(i64, i64)> = conn
        .query_row("SELECT size, mtime FROM file_state WHERE key=?1", [key], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .ok()
        .flatten();
    if prev == Some((size, mtime)) {
        return false;
    }
    let _ = conn.execute(
        "INSERT INTO file_state(key,size,mtime) VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET size=excluded.size, mtime=excluded.mtime",
        params![key, size, mtime],
    );
    true
}

/// Continuity files: the vault folder first, else the repo root (spaced or underscore names).
pub fn continuity_files(p: &Project) -> Vec<(&'static str, PathBuf)> {
    let kinds = [("continuity", "CONTINUITY LOG.md", "CONTINUITY_LOG.md"), ("tasks", "TASKS.md", "TASKS.md"),
                 ("log", "PROJECT LOG.md", "PROJECT_LOG.md"), ("decisions", "DECISIONS.md", "DECISIONS.md")];
    let dirs: Vec<PathBuf> = p.vault_dir.iter().chain(p.repo.iter()).map(PathBuf::from).collect();
    kinds
        .iter()
        .filter_map(|(k, a, b)| {
            dirs.iter().flat_map(|d| [d.join(a), d.join(b)]).find(|f| f.is_file()).map(|f| (*k, f))
        })
        .collect()
}

fn index_continuity(conn: &Connection, p: &Project, all: &[Project]) -> Result<(), String> {
    for (kind, path) in continuity_files(p) {
        let key = format!("cont:{}:{}", p.id, path.display());
        if !changed(conn, &key, &path) {
            continue;
        }
        let md = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file = path.to_string_lossy().into_owned();
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM tasks WHERE project_id=?1 AND source_file=?2", params![p.id, file]).map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM events WHERE project_id=?1 AND source_file=?2", params![p.id, file]).map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM links WHERE project_id=?1 AND source_file=?2", params![p.id, file]).map_err(|e| e.to_string())?;
        for (target, n) in mentions(&md, p, all) {
            tx.execute("INSERT INTO links(project_id,target_id,source_file,count) VALUES(?1,?2,?3,?4)", params![p.id, target, file, n])
                .map_err(|e| e.to_string())?;
        }
        match kind {
            "continuity" => {
                let (s, n) = continuity::parse_continuity(&md);
                tx.execute("UPDATE projects SET summary=?2, next_action=?3 WHERE id=?1", params![p.id, s, n])
                    .map_err(|e| e.to_string())?;
            }
            "tasks" => {
                for t in continuity::parse_tasks(&md) {
                    tx.execute(
                        "INSERT INTO tasks(project_id,section,status,text,author,source_file,line) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                        params![p.id, t.section, t.status, t.text, t.author, file, t.line as i64],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            _ => {
                let decisions = kind == "decisions";
                for e in continuity::parse_entries(&md, decisions) {
                    let title = match &e.code {
                        Some(c) if !e.title.starts_with(c.as_str()) => format!("{c} {}", e.title),
                        _ => e.title.clone(),
                    };
                    tx.execute(
                        "INSERT OR REPLACE INTO events(project_id,ts,day,kind,source,agent,title,detail,ref,evidence,source_file)
                         VALUES(?1,?2,?2,?3,?3,?4,?5,?6,?7,?8,?9)",
                        params![
                            p.id,
                            e.date.clone().unwrap_or_default(),
                            if decisions { "decision" } else { "log" },
                            e.author,
                            title,
                            e.detail,
                            format!("{}#L{}", file, e.line),
                            format!("{}:{}", file, e.line),
                            file
                        ],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Explicit references to other projects (vault folder path or repo path) in a document.
fn mentions(md: &str, me: &Project, all: &[Project]) -> Vec<(String, i64)> {
    let low = md.to_lowercase().replace("%20", " ").replace('\\', "/");
    all.iter()
        .filter(|q| q.id != me.id)
        .filter_map(|q| {
            let mut needles: Vec<String> = Vec::new();
            if let Some(v) = &q.vault_dir {
                let v = v.replace('\\', "/").to_lowercase();
                if let Some(i) = v.find("02 projects/") {
                    needles.push(v[i..].to_string());
                }
            }
            if let Some(r) = &q.repo {
                needles.push(r.replace('\\', "/").to_lowercase());
            }
            // Don't count a parent folder that is a prefix of my own paths.
            let mine: Vec<String> = me.vault_dir.iter().chain(me.repo.iter()).map(|x| x.replace('\\', "/").to_lowercase()).collect();
            let n: usize = needles
                .iter()
                .filter(|nd| !mine.iter().any(|m| m.contains(nd.as_str())))
                .map(|nd| low.matches(nd.as_str()).count())
                .sum();
            (n > 0).then(|| (q.id.clone(), n as i64))
        })
        .collect()
}

/// Newest Markdown note in the project's vault folder, as one upserted timeline event (real file mtime).
/// Bounded walk: depth 3, 3000 entries; skips repos and build folders.
fn index_vault_activity(conn: &Connection, p: &Project) -> Result<(), String> {
    let Some(dir) = p.vault_dir.as_deref().map(Path::new) else { return Ok(()) };
    let mut best: Option<(i64, PathBuf)> = None;
    let mut budget = 3000usize;
    fn walk(d: &Path, depth: usize, budget: &mut usize, best: &mut Option<(i64, PathBuf)>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            if *budget == 0 { return; }
            *budget -= 1;
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if depth > 0 && !name.starts_with('.') && name != "repo" && !crate::registry::EXCLUDED_DIRS.contains(&name.as_str()) {
                    walk(&path, depth - 1, budget, best);
                }
            } else if name.ends_with(".md") {
                if let Some(m) = mtime_secs(&path) {
                    if best.as_ref().map_or(true, |(b, _)| m > *b) { *best = Some((m, path)); }
                }
            }
        }
    }
    walk(dir, 3, &mut budget, &mut best);
    let Some((m, file)) = best else { return Ok(()) };
    let ts = crate::util::unix_to_iso(m);
    let rel = file.strip_prefix(dir).map(|r| r.display().to_string()).unwrap_or_else(|_| file.display().to_string());
    conn.execute(
        "INSERT INTO events(project_id,ts,day,kind,source,agent,title,detail,ref,evidence,source_file)
         VALUES(?1,?2,?3,'note','vault',NULL,?4,?5,?6,?7,?7)
         ON CONFLICT(source,ref) DO UPDATE SET ts=excluded.ts, day=excluded.day, title=excluded.title, detail=excluded.detail,
           evidence=excluded.evidence, source_file=excluded.source_file",
        params![p.id, ts, crate::util::local_day(&ts), format!("Vault note updated: {rel}"), "newest Markdown file in the project's Book of Secrets folder", format!("vault:{}", p.id), file.display().to_string()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn index_git(conn: &Connection, p: &Project) -> Result<(), String> {
    let Some(repo) = p.repo.as_deref().map(Path::new).filter(|r| r.join(".git").exists()) else { return Ok(()) };
    // Repo config that could make git run a program: don't run any git command in this repo (fail closed).
    if let Some(why) = gitscan::risky_config(repo) {
        conn.execute("UPDATE projects SET git_error=?2 WHERE id=?1", params![p.id, format!("git skipped for safety: {why}")])
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    let st = gitscan::status(repo);
    let head = gitscan::head(repo);
    let (branch, dirty, ahead, behind, err) = match &st {
        Ok(s) => (s.branch.clone(), Some(s.dirty), s.ahead, s.behind, None),
        Err(e) => (None, None, 0, 0, Some(e.clone())),
    };
    conn.execute(
        "UPDATE projects SET branch=?2, dirty=?3, ahead=?4, behind=?5, git_error=?6, head=?7 WHERE id=?1",
        params![p.id, branch, dirty, ahead, behind, err, head],
    )
    .map_err(|e| e.to_string())?;
    let Some(head) = head else { return Ok(()) };
    let key = format!("git:{}", p.id);
    let prev: Option<String> = conn
        .query_row("SELECT extra FROM file_state WHERE key=?1", [&key], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();
    if prev.as_deref() == Some(head.as_str()) {
        return Ok(());
    }
    let commits = match &prev {
        Some(old) => gitscan::log(repo, Some(&format!("{old}..HEAD")), 5000).or_else(|_| gitscan::log(repo, None, 3000)),
        None => gitscan::log(repo, None, 3000),
    }?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    for c in &commits {
        let mut ws: HashMap<String, i64> = HashMap::new();
        let (mut add, mut del) = (0, 0);
        for (path, a, d) in &c.files {
            let w = gitscan::workstream(path);
            *ws.entry(w.clone()).or_default() += 1;
            add += a;
            del += d;
            tx.execute(
                "INSERT OR IGNORE INTO commit_files(project_id,sha,path,added,deleted,workstream) VALUES(?1,?2,?3,?4,?5,?6)",
                params![p.id, c.sha, path, a, d, w],
            )
            .map_err(|e| e.to_string())?;
        }
        let top = ws.into_iter().max_by_key(|(_, n)| *n).map(|(w, _)| w);
        let day = crate::util::local_day(&c.ts);
        tx.execute(
            "INSERT OR IGNORE INTO events(project_id,ts,day,kind,source,agent,title,detail,ref,evidence,workstream)
             VALUES(?1,?2,?3,'commit','git',?4,?5,?6,?7,?8,?9)",
            params![
                p.id,
                c.ts,
                day,
                c.agent,
                c.subject,
                format!("{} · {} files · +{add} −{del}", c.author, c.files.len()),
                format!("{}:{}", p.id, c.sha),
                c.sha,
                top
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.execute(
        "INSERT INTO file_state(key,extra) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET extra=excluded.extra",
        params![key, head],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

fn upsert_projects(conn: &Connection, projects: &[Project]) -> Result<(), String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    for p in projects {
        tx.execute(
            "INSERT INTO projects(id,name,workspace,repo_path,vault_dir,source,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, workspace=excluded.workspace, repo_path=excluded.repo_path,
               vault_dir=excluded.vault_dir, source=excluded.source, updated_at=excluded.updated_at",
            params![p.id, p.name, p.workspace, p.repo, p.vault_dir, p.source, now_iso()],
        )
        .map_err(|e| e.to_string())?;
    }
    let ids: Vec<String> = projects.iter().map(|p| p.id.clone()).collect();
    let existing: Vec<String> = {
        let mut s = tx.prepare("SELECT id FROM projects").map_err(|e| e.to_string())?;
        let v = s.query_map([], |r| r.get(0)).map_err(|e| e.to_string())?.flatten().collect();
        v
    };
    for gone in existing.iter().filter(|i| !ids.contains(i)) {
        tx.execute("DELETE FROM projects WHERE id=?1", [gone]).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}

fn rebuild_search(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "BEGIN;
         DELETE FROM search;
         INSERT INTO search(text,project_id,kind,ref) SELECT name || ' ' || coalesce(summary,'') || ' ' || coalesce(next_action,''), id, 'project', id FROM projects;
         INSERT INTO search(text,project_id,kind,ref) SELECT text, project_id, 'task', id FROM tasks;
         INSERT INTO search(text,project_id,kind,ref) SELECT title || ' ' || coalesce(detail,''), project_id, kind, id FROM events;
         COMMIT;",
    )
    .map_err(|e| e.to_string())
}

/// Bump when a continuity parser changes so already-indexed files are parsed again.
const PARSER_VERSION: i64 = 3;

pub fn refresh(conn: &Connection, emit: Emit) {
    let started = Instant::now();
    if db::setting(conn, "parser_version").and_then(|v| v.as_i64()) != Some(PARSER_VERSION) {
        let _ = conn.execute_batch(
            "DELETE FROM file_state WHERE key LIKE 'cont:%'; DELETE FROM tasks; DELETE FROM links;
             DELETE FROM events WHERE source IN ('log','decision');",
        );
        let _ = db::set_setting(conn, "parser_version", &json!(PARSER_VERSION));
    }
    let cfg = Config::load(conn);
    let step = |stage: &str, n: usize, total: usize| {
        set_status(emit, json!({"state":"indexing","stage":stage,"done":n,"total":total,"at":now_iso()}));
    };
    step("projects", 0, 0);
    let (projects, errs) = registry::discover(&cfg);
    for e in errs {
        db::record_error(conn, "registry", &e);
    }
    if let Err(e) = upsert_projects(conn, &projects) {
        db::record_error(conn, "registry", &e);
    }
    emit("projects-updated", Value::Null);
    for (i, p) in projects.iter().enumerate() {
        step("project files + git", i, projects.len());
        if let Err(e) = index_continuity(conn, p, &projects) {
            db::record_error(conn, &format!("continuity:{}", p.id), &e);
        }
        if let Err(e) = index_vault_activity(conn, p) {
            db::record_error(conn, &format!("vault:{}", p.id), &e);
        }
        if let Err(e) = index_git(conn, p) {
            db::record_error(conn, &format!("git:{}", p.id), &e);
        }
    }
    step("claude sessions", 0, 0);
    if let Err(e) = agents::index_claude(conn, &cfg.claude_dir) {
        db::record_error(conn, "claude", &e);
    }
    step("codex sessions", 0, 0);
    if let Err(e) = agents::index_codex(conn, &cfg.codex_dir) {
        db::record_error(conn, "codex", &e);
    }
    if let Err(e) = agents::map_sessions(conn, &projects) {
        db::record_error(conn, "sessions", &e);
    }
    let _ = conn.execute(
        "UPDATE projects SET last_activity=(SELECT max(ts) FROM events WHERE events.project_id=projects.id AND ts<>'')",
        [],
    );
    if let Err(e) = rebuild_search(conn) {
        db::record_error(conn, "search", &e);
    }
    let _ = db::set_setting(conn, "last_refresh", &json!(now_iso()));
    log("info", format!("refresh done in {} ms ({} projects)", started.elapsed().as_millis(), projects.len()));
    set_status(emit, json!({"state":"idle","at":now_iso(),"ms":started.elapsed().as_millis() as u64}));
    emit("projects-updated", Value::Null);
}

/// Rows of the current projects as registry::Project (for modules that need paths).
pub fn load_projects(conn: &Connection) -> Vec<Project> {
    let mut s = match conn.prepare("SELECT id,name,coalesce(workspace_override,workspace),repo_path,vault_dir,source FROM projects") {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let v = s
        .query_map([], |r| {
            let src: String = r.get(5)?;
            Ok(Project {
                id: r.get(0)?,
                name: r.get(1)?,
                workspace: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                repo: r.get(3)?,
                vault_dir: r.get(4)?,
                source: match src.as_str() { "registry" => "registry", "manual" => "manual", "vault" => "vault", _ => "scan" },
            })
        })
        .map(|it| it.flatten().collect())
        .unwrap_or_default();
    v
}

#[cfg(test)]
mod tests {
    /// Full index against this machine's real data into a throwaway DB. Run explicitly:
    /// cargo test --lib real_index -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_index() {
        let path = std::env::temp_dir().join("ptcc-real-index.db");
        let _ = std::fs::remove_file(&path);
        let conn = crate::db::open_migrated(&path).unwrap();
        let t = std::time::Instant::now();
        super::refresh(&conn, &|_, _| {});
        let first = t.elapsed();
        let t = std::time::Instant::now();
        super::refresh(&conn, &|_, _| {});
        let second = t.elapsed();
        for (q, label) in [
            ("SELECT count(*) FROM projects", "projects"), ("SELECT count(*) FROM tasks", "tasks"),
            ("SELECT count(*) FROM events WHERE kind='commit'", "commits"), ("SELECT count(*) FROM events WHERE kind='log'", "log entries"),
            ("SELECT count(*) FROM events WHERE kind='decision'", "decisions"), ("SELECT count(*) FROM sessions WHERE agent='claude'", "claude sessions"),
            ("SELECT count(*) FROM sessions WHERE agent='codex'", "codex sessions"), ("SELECT count(*) FROM sessions WHERE project_id IS NULL", "unmapped sessions"),
            ("SELECT count(*) FROM links", "links"), ("SELECT count(*) FROM index_errors", "errors"),
        ] {
            let n: i64 = conn.query_row(q, [], |r| r.get(0)).unwrap();
            println!("{label}: {n}");
        }
        for r in crate::util::rows(&conn, "SELECT id, workspace, source, branch, dirty, (SELECT count(*) FROM tasks t WHERE t.project_id=p.id) tasks, last_activity, substr(coalesce(next_action,''),1,60) next FROM projects p ORDER BY last_activity DESC", []).unwrap() {
            println!("{r}");
        }
        for r in crate::util::rows(&conn, "SELECT agent, project_id, map_method, substr(title,1,50) t FROM sessions ORDER BY updated DESC LIMIT 12", []).unwrap() {
            println!("{r}");
        }
        for r in crate::util::rows(&conn, "SELECT scope, substr(message,1,160) m FROM index_errors LIMIT 15", []).unwrap() {
            println!("ERR {r}");
        }
        println!("first refresh {:?}, second (incremental) {:?}, db {} KB", first, second, std::fs::metadata(&path).unwrap().len() / 1024);
    }
}
