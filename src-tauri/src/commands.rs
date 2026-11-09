//! Tauri commands. Read paths use the UI connection; writes to external files need an explicit call.
use crate::indexer::{Job, STATUS};
use crate::launcher::{self, LaunchReq};
use crate::util::{rows, under};
use crate::{db, registry, AppState};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tauri::State;
use tauri_plugin_autostart::ManagerExt;

type R<T> = Result<T, String>;

fn with<T>(st: &State<AppState>, f: impl FnOnce(&rusqlite::Connection) -> R<T>) -> R<T> {
    let c = st.db.lock().map_err(|_| "database lock poisoned")?;
    f(&c)
}

const PROJECT_COLS: &str = "p.id, p.name, coalesce(p.workspace_override,p.workspace) workspace, p.repo_path, p.vault_dir,
  p.source, p.hidden, p.branch, p.head, p.dirty, p.ahead, p.behind, p.git_error, p.summary, p.next_action, p.last_activity,
  (SELECT count(*) FROM tasks t WHERE t.project_id=p.id AND t.status='active') active_tasks,
  (SELECT count(*) FROM tasks t WHERE t.project_id=p.id AND t.status='blocked') blocked_tasks,
  (SELECT count(*) FROM tasks t WHERE t.project_id=p.id AND t.status='done') done_tasks,
  (SELECT count(*) FROM tasks t WHERE t.project_id=p.id AND t.status='pending') pending_tasks,
  (SELECT count(*) FROM tasks t WHERE t.project_id=p.id AND t.status='deferred') deferred_tasks,
  (SELECT count(*) FROM tasks t WHERE t.project_id=p.id) all_tasks,
  (SELECT json_object('id',id,'title',title,'updated',updated,'cwd',cwd) FROM sessions s WHERE s.project_id=p.id AND s.agent='claude' ORDER BY updated DESC LIMIT 1) last_claude,
  (SELECT json_object('id',id,'title',title,'updated',updated,'cwd',cwd) FROM sessions s WHERE s.project_id=p.id AND s.agent='codex' ORDER BY updated DESC LIMIT 1) last_codex";

#[tauri::command(async)]
pub fn get_status(st: State<AppState>) -> R<Value> {
    let s = STATUS.lock().unwrap().clone().unwrap_or(json!({"state":"idle"}));
    with(&st, |c| {
        let last = db::setting(c, "last_refresh");
        Ok(json!({"status": s, "last_refresh": last, "since": st.briefing_since}))
    })
}

#[tauri::command(async)]
pub fn get_projects(st: State<AppState>, include_hidden: Option<bool>) -> R<Vec<Value>> {
    with(&st, |c| {
        let sql = format!(
            "SELECT {PROJECT_COLS} FROM projects p {} ORDER BY coalesce(p.last_activity,'') DESC",
            if include_hidden == Some(true) { "" } else { "WHERE p.hidden=0" }
        );
        rows(c, &sql, [])
    })
}

/// Deterministic briefing since the previous app start (min 24 h). Every line names its evidence.
#[tauri::command(async)]
pub fn get_briefing(st: State<AppState>) -> R<Value> {
    let since = st.briefing_since.clone();
    with(&st, |c| {
        let per_project = rows(
            c,
            "SELECT p.id, p.name,
               sum(e.kind='commit') commits, sum(e.kind='commit' AND e.agent='claude') claude_commits,
               sum(e.kind='commit' AND e.agent='codex') codex_commits,
               sum(e.kind='log') logs, sum(e.kind='decision') decisions,
               (SELECT group_concat(w, ', ') FROM (SELECT workstream w FROM events x WHERE x.project_id=p.id AND x.kind='commit'
                  AND x.ts>=?1 AND workstream IS NOT NULL GROUP BY w ORDER BY count(*) DESC LIMIT 3)) workstreams,
               (SELECT title FROM events x WHERE x.project_id=p.id AND x.kind='decision' AND x.ts>=substr(?1,1,10) ORDER BY ts DESC LIMIT 1) last_decision
             FROM projects p JOIN events e ON e.project_id=p.id
             WHERE p.hidden=0 AND e.ts >= substr(?1,1,10) AND e.kind IN ('commit','log','decision')
             GROUP BY p.id ORDER BY max(e.ts) DESC",
            [&since],
        )?;
        let sessions = rows(
            c,
            "SELECT s.agent, s.title, s.updated, s.prompts, s.edits, p.name project, p.id project_id FROM sessions s
             LEFT JOIN projects p ON p.id=s.project_id WHERE s.updated >= ?1 ORDER BY s.updated DESC LIMIT 12",
            [&since],
        )?;
        let blocked = rows(
            c,
            "SELECT p.id, p.name, count(*) n, min(t.text) sample FROM tasks t JOIN projects p ON p.id=t.project_id
             WHERE t.status='blocked' AND p.hidden=0 GROUP BY p.id ORDER BY n DESC",
            [],
        )?;
        let unpushed = rows(
            c,
            "SELECT id, name, dirty, ahead, branch FROM projects WHERE hidden=0 AND (dirty>0 OR ahead>0) ORDER BY ahead DESC, dirty DESC",
            [],
        )?;
        let stale = rows(
            c,
            "SELECT p.id, p.name, p.last_activity, (SELECT count(*) FROM tasks t WHERE t.project_id=p.id AND t.status='active') open
             FROM projects p WHERE p.hidden=0 AND p.last_activity IS NOT NULL AND p.last_activity < date('now','-14 day')
               AND (SELECT count(*) FROM tasks t WHERE t.project_id=p.id AND t.status='active') > 0
             ORDER BY p.last_activity DESC LIMIT 6",
            [],
        )?;
        let done_now = rows(c, "SELECT project_id, count(*) n FROM tasks WHERE status='done' GROUP BY project_id", [])?;
        let snap = &st.task_snapshot;
        let completed: Vec<Value> = done_now
            .iter()
            .filter_map(|r| {
                let id = r["project_id"].as_str()?;
                let before = snap[id].as_i64()?;
                let now = r["n"].as_i64()?;
                (now > before).then(|| json!({"project_id": id, "n": now - before}))
            })
            .collect();
        let discoveries: i64 = c
            .query_row("SELECT count(*) FROM discoveries WHERE status='new' AND fetched_at >= ?1", [&since], |r| r.get(0))
            .unwrap_or(0);
        Ok(json!({"since": since, "projects": per_project, "sessions": sessions, "blocked": blocked,
                  "unpushed": unpushed, "stale": stale, "completed": completed, "new_discoveries": discoveries}))
    })
}

#[tauri::command(async)]
pub fn get_project(st: State<AppState>, id: String) -> R<Value> {
    with(&st, |c| {
        let p = rows(c, &format!("SELECT {PROJECT_COLS} FROM projects p WHERE p.id=?1"), [&id])?
            .into_iter()
            .next()
            .ok_or("unknown project")?;
        let tasks = rows(c, "SELECT id, section, status, text, author, source_file, line FROM tasks WHERE project_id=?1 ORDER BY id", [&id])?;
        let sessions = rows(c, "SELECT * FROM sessions WHERE project_id=?1 ORDER BY updated DESC LIMIT 300", [&id])?;
        let workstreams = rows(
            c,
            "SELECT workstream, count(DISTINCT sha) commits, count(*) files, sum(added) added, sum(deleted) deleted
             FROM commit_files WHERE project_id=?1 GROUP BY workstream ORDER BY commits DESC",
            [&id],
        )?;
        let files = rows(
            c,
            "SELECT f.path, count(*) commits, sum(f.added) added, sum(f.deleted) deleted, max(e.ts) last_ts
             FROM commit_files f LEFT JOIN events e ON e.source='git' AND e.ref = f.project_id || ':' || f.sha
             WHERE f.project_id=?1 GROUP BY f.path ORDER BY commits DESC LIMIT 5000",
            [&id],
        )?;
        let agent_files = rows(
            c,
            "SELECT sf.path, s.agent, sum(sf.edits) edits, max(sf.last_ts) last_ts, count(DISTINCT s.id) sessions
             FROM session_files sf JOIN sessions s ON s.id=sf.session_id WHERE s.project_id=?1
             GROUP BY sf.path, s.agent ORDER BY last_ts DESC LIMIT 5000",
            [&id],
        )?;
        let links = rows(
            c,
            "SELECT l.target_id, p.name, sum(l.count) n, group_concat(DISTINCT l.source_file) files FROM links l
             JOIN projects p ON p.id=l.target_id WHERE l.project_id=?1 GROUP BY l.target_id ORDER BY n DESC",
            [&id],
        )?;
        let reviews = rows(c, "SELECT * FROM review_tasks WHERE project_id=?1 ORDER BY id DESC", [&id])?;
        let launches = rows(c, "SELECT * FROM launches WHERE project_id=?1 ORDER BY id DESC LIMIT 50", [&id])?;
        Ok(json!({"project": p, "tasks": tasks, "sessions": sessions, "workstreams": workstreams, "files": files,
                  "agent_files": agent_files, "links": links, "reviews": reviews, "launches": launches}))
    })
}

/// Per-day counts for charts. Every series is a count of indexed records.
#[tauri::command(async)]
pub fn get_activity(st: State<AppState>, project: Option<String>, from: String, to: String, workspace: Option<String>) -> R<Vec<Value>> {
    with(&st, |c| {
        let p = project.unwrap_or_default();
        // A workspace aggregates its projects; the queries below filter on ?1 = project id or ''.
        if let Some(w) = workspace.filter(|w| !w.is_empty()) {
            let ids = rows(c, "SELECT id FROM projects WHERE coalesce(workspace_override,workspace)=?1 AND hidden=0", [&w])?;
            let mut by: std::collections::BTreeMap<String, serde_json::Map<String, Value>> = Default::default();
            for r in ids {
                let id = r["id"].as_str().unwrap_or("").to_string();
                for d in activity_rows(c, &id, &from, &to)? {
                    let day = d["day"].as_str().unwrap_or("").to_string();
                    let e = by.entry(day.clone()).or_insert_with(|| { let mut m = serde_json::Map::new(); m.insert("day".into(), json!(day)); m });
                    for (k, v) in d.as_object().unwrap() {
                        if k != "day" { let cur = e.get(k).and_then(|x| x.as_i64()).unwrap_or(0); e.insert(k.clone(), json!(cur + v.as_i64().unwrap_or(0))); }
                    }
                }
            }
            return Ok(by.into_values().map(Value::Object).collect());
        }
        activity_rows(c, &p, &from, &to)
    })
}

fn activity_rows(c: &rusqlite::Connection, p: &str, from: &str, to: &str) -> R<Vec<Value>> {
    {
        rows(
            c,
            // Single-pass aggregates joined by day (correlated subqueries over CTEs took ~9 s).
            "WITH ev AS (SELECT day, sum(kind='commit') commits, sum(kind='commit' AND agent='claude') commits_claude,
                           sum(kind='commit' AND agent='codex') commits_codex, sum(kind='log') log_entries, sum(kind='decision') decisions
                         FROM events WHERE day BETWEEN ?2 AND ?3 AND (?1='' OR project_id=?1) GROUP BY day),
                  sd AS (SELECT d.day, sum(CASE WHEN s.agent='claude' THEN d.prompts ELSE 0 END) claude_prompts,
                           sum(CASE WHEN s.agent='claude' THEN d.edits ELSE 0 END) claude_edits,
                           count(DISTINCT CASE WHEN s.agent='claude' THEN d.session_id END) claude_sessions,
                           count(DISTINCT CASE WHEN s.agent='codex' THEN d.session_id END) codex_sessions
                         FROM session_days d JOIN sessions s ON s.id=d.session_id
                         WHERE d.day BETWEEN ?2 AND ?3 AND (?1='' OR s.project_id=?1) GROUP BY d.day),
                  cf AS (SELECT e.day, count(*) n FROM events e JOIN commit_files f ON f.project_id=e.project_id AND f.sha=e.evidence
                         WHERE e.source='git' AND e.day BETWEEN ?2 AND ?3 AND (?1='' OR e.project_id=?1) GROUP BY e.day),
                  days AS (SELECT day FROM ev UNION SELECT day FROM sd)
             SELECT days.day, coalesce(ev.commits,0) commits, coalesce(ev.commits_claude,0) commits_claude,
               coalesce(ev.commits_codex,0) commits_codex, coalesce(ev.log_entries,0) log_entries, coalesce(ev.decisions,0) decisions,
               coalesce(cf.n,0) files_changed, coalesce(sd.claude_prompts,0) claude_prompts, coalesce(sd.claude_edits,0) claude_edits,
               coalesce(sd.claude_sessions,0) claude_sessions, coalesce(sd.codex_sessions,0) codex_sessions
             FROM days LEFT JOIN ev ON ev.day=days.day LEFT JOIN sd ON sd.day=days.day LEFT JOIN cf ON cf.day=days.day
             WHERE days.day IS NOT NULL ORDER BY days.day",
            rusqlite::params![p, from, to],
        )
    }
}

#[derive(Deserialize)]
pub struct TimelineQ {
    project: Option<String>,
    agent: Option<String>,
    kind: Option<String>,
    workstream: Option<String>,
    from: Option<String>,
    to: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[tauri::command(async)]
pub fn get_timeline(st: State<AppState>, f: TimelineQ) -> R<Value> {
    with(&st, |c| {
        let q = f.q.unwrap_or_default().trim().to_string();
        // FTS match on quoted tokens only: user text never becomes FTS syntax.
        let fts = q.split_whitespace().map(|w| format!("\"{}\"", w.replace('"', ""))).collect::<Vec<_>>().join(" ");
        let where_ = "e.ts<>'' AND (?1='' OR e.project_id=?1) AND (?2='' OR coalesce(e.agent,'none')=?2) AND (?3='' OR e.kind=?3)
             AND (?4='' OR e.workstream=?4) AND (?5='' OR e.day>=?5) AND (?6='' OR e.day<=?6)
             AND (?7='' OR e.id IN (SELECT CAST(ref AS INTEGER) FROM search WHERE search MATCH ?7 AND kind IN ('commit','log','decision','session','launch')))";
        let params = rusqlite::params![
            f.project.unwrap_or_default(), f.agent.unwrap_or_default(), f.kind.unwrap_or_default(), f.workstream.unwrap_or_default(),
            f.from.unwrap_or_default(), f.to.unwrap_or_default(), fts, f.limit.unwrap_or(200).clamp(1, 2000), f.offset.unwrap_or(0).max(0)
        ];
        let items = rows(
            c,
            &format!("SELECT e.*, p.name project FROM events e LEFT JOIN projects p ON p.id=e.project_id WHERE {where_} ORDER BY e.ts DESC, e.id DESC LIMIT ?8 OFFSET ?9"),
            params,
        )?;
        Ok(json!({"items": items}))
    })
}

#[tauri::command(async)]
pub fn get_sessions(st: State<AppState>, agent: Option<String>) -> R<Vec<Value>> {
    with(&st, |c| {
        rows(
            c,
            "SELECT s.*, p.name project FROM sessions s LEFT JOIN projects p ON p.id=s.project_id
             WHERE (?1='' OR s.agent=?1) ORDER BY s.updated DESC LIMIT 1000",
            [agent.unwrap_or_default()],
        )
    })
}

#[tauri::command(async)]
pub fn get_session_files(st: State<AppState>, id: String) -> R<Vec<Value>> {
    with(&st, |c| rows(c, "SELECT path, edits, last_ts FROM session_files WHERE session_id=?1 ORDER BY last_ts DESC", [&id]))
}

#[tauri::command(async)]
pub fn get_graph(st: State<AppState>) -> R<Value> {
    with(&st, |c| {
        let projects = rows(c, "SELECT id, name, coalesce(workspace_override,workspace) workspace, last_activity, vault_dir, repo_path, source,
            (SELECT count(*) FROM tasks t WHERE t.project_id=projects.id AND t.status='active') active
            FROM projects WHERE hidden=0", [])?;
        let workstreams = rows(
            c,
            "SELECT project_id, workstream, count(DISTINCT sha) commits FROM commit_files GROUP BY project_id, workstream",
            [],
        )?;
        let links = rows(c, "SELECT project_id, target_id, sum(count) n, group_concat(DISTINCT source_file) files FROM links GROUP BY project_id, target_id", [])?;
        Ok(json!({"projects": projects, "workstreams": workstreams, "links": links}))
    })
}

#[tauri::command(async)]
pub fn search(st: State<AppState>, q: String) -> R<Vec<Value>> {
    let fts = q.split_whitespace().map(|w| format!("\"{}\"*", w.replace('"', ""))).collect::<Vec<_>>().join(" ");
    if fts.is_empty() {
        return Ok(vec![]);
    }
    with(&st, |c| {
        rows(c, "SELECT s.kind, s.ref, s.project_id, p.name project, snippet(search, 0, '[', ']', '…', 12) snip
                 FROM search s LEFT JOIN projects p ON p.id=s.project_id WHERE search MATCH ?1 ORDER BY rank LIMIT 40", [fts])
    })
}

#[tauri::command(async)]
pub fn refresh_now(st: State<AppState>) {
    st.worker.send(Job::Refresh);
}

#[tauri::command(async)]
pub fn discovery_refresh(st: State<AppState>) {
    st.worker.send(Job::Discovery);
}

#[tauri::command(async)]
pub fn get_discoveries(st: State<AppState>) -> R<Value> {
    with(&st, |c| {
        let items = rows(c, "SELECT * FROM discoveries ORDER BY status='dismissed', score DESC, fetched_at DESC LIMIT 1000", [])?;
        Ok(json!({"items": items, "last": db::setting(c, "discovery_last"), "errors": db::setting(c, "discovery_errors")}))
    })
}

#[tauri::command(async)]
pub fn set_discovery(st: State<AppState>, id: String, status: Option<String>, project_link: Option<String>) -> R<()> {
    if let Some(s) = &status {
        if !["new", "saved", "dismissed"].contains(&s.as_str()) {
            return Err("bad status".into());
        }
    }
    with(&st, |c| {
        c.execute(
            "UPDATE discoveries SET status=coalesce(?2,status), project_link=coalesce(?3,project_link) WHERE id=?1",
            rusqlite::params![id, status, project_link],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

#[tauri::command(async)]
pub fn add_review_task(st: State<AppState>, project_id: String, text: String, discovery_id: Option<String>) -> R<()> {
    with(&st, |c| {
        c.execute(
            "INSERT INTO review_tasks(ts,project_id,text,discovery_id) VALUES(?1,?2,?3,?4)",
            rusqlite::params![crate::util::now_iso(), project_id, crate::util::truncate(&text, 500), discovery_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

#[tauri::command(async)]
pub fn toggle_review_task(st: State<AppState>, id: i64) -> R<()> {
    with(&st, |c| {
        c.execute("UPDATE review_tasks SET done=1-done WHERE id=?1", [id]).map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// The exact text that "Add to BoS" would append, and where. Nothing is written here.
fn bos_target(c: &rusqlite::Connection, discovery_id: &str, project_id: &str) -> R<(PathBuf, String)> {
    let d = rows(c, "SELECT * FROM discoveries WHERE id=?1", [discovery_id])?.into_iter().next().ok_or("unknown discovery")?;
    let p = rows(c, "SELECT name, vault_dir FROM projects WHERE id=?1", [project_id])?.into_iter().next().ok_or("unknown project")?;
    let dir = p["vault_dir"].as_str().ok_or("this project has no Book of Secrets folder")?;
    let path = Path::new(dir).join("Discoveries.md");
    let clean = |k: &str| d[k].as_str().unwrap_or("").replace(['\n', '\r'], " ");
    let text = format!(
        "\n## {} 🟦 CLAUDE — {}\n\n- Source: {}\n- What: {}\n- License: {} · Pricing: {}\n- Why: {}\n- Added from Papertrail Command Center (Discovery); review before adopting.\n",
        chrono::Local::now().format("%Y-%m-%d"),
        clean("name"), clean("url"), clean("description"), clean("license"), clean("pricing"), clean("reason")
    );
    Ok((path, text))
}

#[tauri::command(async)]
pub fn bos_preview(st: State<AppState>, discovery_id: String, project_id: String) -> R<Value> {
    with(&st, |c| {
        let (path, text) = bos_target(c, &discovery_id, &project_id)?;
        Ok(json!({"path": path, "exists": path.is_file(), "text": text}))
    })
}

#[tauri::command(async)]
pub fn bos_append(st: State<AppState>, discovery_id: String, project_id: String) -> R<String> {
    with(&st, |c| {
        let (path, text) = bos_target(c, &discovery_id, &project_id)?;
        use std::io::Write;
        let new = !path.is_file();
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| e.to_string())?;
        if new {
            f.write_all(b"# Discoveries\n\nTools and resources saved from Papertrail Command Center. Append-only.\n").map_err(|e| e.to_string())?;
        }
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        crate::util::log("info", format!("BoS append: {}", path.display()));
        Ok(path.display().to_string())
    })
}

#[tauri::command(async)]
pub fn preview_handoff(st: State<AppState>, req: LaunchReq) -> R<String> {
    with(&st, |c| launcher::build_handoff(c, &req))
}

#[tauri::command(async)]
pub fn launch_agent(st: State<AppState>, req: LaunchReq) -> R<Value> {
    with(&st, |c| launcher::launch(c, &st.data_dir, &req))
}

#[tauri::command(async)]
pub fn get_cli_info() -> Value {
    let probe = |name: &str, args: &[&str]| -> Value {
        match crate::util::which(name) {
            Some(p) => {
                let v = crate::util::run(&p.display().to_string(), args, None, std::time::Duration::from_secs(10))
                    .map(|o| o.stdout.lines().next().unwrap_or("").trim().to_string())
                    .unwrap_or_else(|e| format!("version check failed: {e}"));
                json!({"path": p, "version": v})
            }
            None => json!(null),
        }
    };
    json!({"claude": probe("claude", &["--version"]), "codex": probe("codex", &["--version"]),
           "wt": crate::util::which("wt"), "git": crate::util::which("git")})
}

/// Open a folder/file in Explorer or its default app. Only paths inside a registered project,
/// the vault, or this app's data folder are allowed.
#[tauri::command(async)]
pub fn open_path(st: State<AppState>, path: String) -> R<()> {
    let ok = with(&st, |c| {
        let mut roots: Vec<String> = rows(c, "SELECT repo_path r FROM projects UNION SELECT vault_dir FROM projects", [])?
            .iter()
            .filter_map(|r| r["r"].as_str().map(str::to_string))
            .collect();
        if let Some(v) = registry::Config::load(c).vault {
            roots.push(v.display().to_string());
        }
        roots.push(st.data_dir.display().to_string());
        Ok(roots.iter().any(|r| under(&path, r)))
    })?;
    if !ok || !Path::new(&path).exists() {
        return Err("path is not inside a registered project, the vault, or app data".into());
    }
    std::process::Command::new("explorer").arg(&path).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn open_url(url: String) -> R<()> {
    // Parse, allow only http(s) with a host, and hand the re-serialized (percent-encoded) URL to the shell's
    // default handler directly: no explorer.exe argument parsing involved.
    let u = tauri::Url::parse(url.trim()).map_err(|_| "not a valid URL")?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().map_or(true, |h| h.is_empty()) {
        return Err("only http(s) links with a host can be opened".into());
    }
    shell_open(u.as_str())
}

#[cfg(windows)]
pub(crate) fn shell_open(target: &str) -> R<()> {
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(hwnd: *mut core::ffi::c_void, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> isize;
    }
    let w = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (op, file) = (w("open"), w(target));
    // SAFETY: valid NUL-terminated UTF-16 buffers that outlive the call; null params/dir are allowed.
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
    if r > 32 { Ok(()) } else { Err(format!("could not open link (ShellExecute {r})")) }
}

#[cfg(not(windows))]
pub(crate) fn shell_open(_target: &str) -> R<()> {
    Err("unsupported platform".into())
}

#[tauri::command(async)]
pub fn get_settings(st: State<AppState>, app: tauri::AppHandle) -> R<Value> {
    let autostart = app.autolaunch().is_enabled().unwrap_or(false);
    with(&st, |c| {
        let mut s = registry::settings(c);
        s["autostart"] = json!(autostart);
        Ok(s)
    })
}

#[tauri::command(async)]
pub fn save_settings(st: State<AppState>, app: tauri::AppHandle, patch: Value) -> R<Value> {
    let allowed = registry::defaults();
    let obj = patch.as_object().ok_or("settings patch must be an object")?;
    if let Some(a) = obj.get("autostart").and_then(|v| v.as_bool()) {
        let m = app.autolaunch();
        if a { m.enable() } else { m.disable() }.map_err(|e| format!("autostart: {e}"))?;
    }
    with(&st, |c| {
        for (k, v) in obj {
            if k == "autostart" {
                continue;
            }
            if allowed.get(k).is_none() {
                return Err(format!("unknown setting {k}"));
            }
            // Type must match the default's type (null vault allowed).
            let d = &allowed[k];
            let same = std::mem::discriminant(d) == std::mem::discriminant(v) || (k == "vault_path" && (v.is_string() || v.is_null()));
            if !same {
                return Err(format!("setting {k} has the wrong type"));
            }
            db::set_setting(c, k, v).map_err(|e| e.to_string())?;
        }
        Ok(())
    })?;
    // Saved chats etc. don't change what gets indexed.
    if obj.keys().any(|k| !k.starts_with("chat_")) {
        st.worker.send(Job::Refresh);
    }
    get_settings(st, app)
}

#[tauri::command(async)]
pub fn set_project_meta(st: State<AppState>, id: String, hidden: Option<bool>, workspace: Option<String>) -> R<()> {
    with(&st, |c| {
        if let Some(h) = hidden {
            c.execute("UPDATE projects SET hidden=?2 WHERE id=?1", rusqlite::params![id, h as i64]).map_err(|e| e.to_string())?;
        }
        if let Some(w) = workspace {
            let w = w.trim().to_string();
            c.execute("UPDATE projects SET workspace_override=?2 WHERE id=?1", rusqlite::params![id, if w.is_empty() { None } else { Some(w) }])
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })
}

#[tauri::command(async)]
pub fn register_project(st: State<AppState>, name: String, path: String, workspace: Option<String>) -> R<()> {
    if name.trim().is_empty() || !Path::new(&path).is_dir() {
        return Err("name required and path must be an existing folder".into());
    }
    with(&st, |c| {
        let mut list = registry::settings(c)["manual_projects"].as_array().cloned().unwrap_or_default();
        list.push(json!({"name": name.trim(), "path": path, "workspace": workspace}));
        db::set_setting(c, "manual_projects", &json!(list)).map_err(|e| e.to_string())
    })?;
    st.worker.send(Job::Refresh);
    Ok(())
}

#[tauri::command(async)]
pub fn get_diagnostics(st: State<AppState>) -> R<Value> {
    let db_size = std::fs::metadata(st.data_dir.join("papertrail.db")).map(|m| m.len()).unwrap_or(0);
    with(&st, |c| {
        let errors = rows(c, "SELECT * FROM index_errors ORDER BY id DESC LIMIT 100", [])?;
        let counts = rows(c, "SELECT (SELECT count(*) FROM projects) projects, (SELECT count(*) FROM events) events,
            (SELECT count(*) FROM sessions) sessions, (SELECT count(*) FROM tasks) tasks, (SELECT count(*) FROM commit_files) commit_files,
            (SELECT count(*) FROM discoveries) discoveries", [])?;
        Ok(json!({"errors": errors, "counts": counts.into_iter().next(), "db_bytes": db_size, "data_dir": st.data_dir,
                  "pid": std::process::id()}))
    })
}

#[tauri::command(async)]
pub fn backup_db(st: State<AppState>) -> R<String> {
    let dest = st.data_dir.join("backups").join(format!("papertrail-{}.db", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    with(&st, |c| {
        c.execute("VACUUM INTO ?1", [dest.display().to_string()]).map_err(|e| e.to_string())?;
        Ok(dest.display().to_string())
    })
}

/// Per-project daily activity for sparklines: commits, log entries, decisions and agent sessions per day.
#[tauri::command(async)]
pub fn get_sparks(st: State<AppState>, days: i64) -> R<Vec<Value>> {
    let since = (chrono::Local::now() - chrono::Duration::days(days.clamp(1, 120) - 1)).format("%Y-%m-%d").to_string();
    with(&st, |c| {
        rows(
            c,
            "SELECT project_id, day, count(*) n FROM events WHERE project_id IS NOT NULL AND day >= ?1
               AND kind IN ('commit','log','decision','session') GROUP BY project_id, day",
            [since],
        )
    })
}

/// Chat sidebar width in logical px (0 = hidden).
static CHAT_W: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Discord's unread count (the "(N)" its page title starts with) now, and when the panel was last open.
static CHAT_COUNT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
static CHAT_SEEN: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

/// Tells the dashboard whether the Chat button's unread dot shows: Discord's count rose since the panel was last
/// open. Opening the panel, or reading elsewhere (count drops), clears it.
fn chat_unread(app: &tauri::AppHandle, count: Option<i64>) {
    use std::sync::atomic::Ordering::Relaxed;
    use tauri::Emitter;
    let n = count.unwrap_or_else(|| CHAT_COUNT.load(Relaxed));
    CHAT_COUNT.store(n, Relaxed);
    let seen = CHAT_SEEN.load(Relaxed);
    let seen = if seen < 0 || CHAT_W.load(Relaxed) > 0 { n } else { seen.min(n) };
    CHAT_SEEN.store(seen, Relaxed);
    let _ = app.emit_to("main", "chat-unread", n > seen);
}

/// "(12) Discord | general" → 12; no count → 0.
fn title_count(t: &str) -> i64 {
    t.strip_prefix('(').and_then(|r| r.split(')').next()).and_then(|n| n.parse().ok()).unwrap_or(0)
}

/// Builds the chat webview inside the main window (hidden when `width` is 0: loaded at launch for the unread dot).
pub fn create_chat(app: &tauri::AppHandle, url: &str, width: u32) -> R<tauri::Webview> {
    use tauri::Manager;
    let e = |e: tauri::Error| e.to_string();
    if !url.starts_with("https://") {
        return Err("chat URL must start with https://".into());
    }
    let win = app.get_window("main").ok_or("main window missing")?;
    let builder = tauri::webview::WebviewBuilder::new("chat", tauri::WebviewUrl::External(url.parse().map_err(|e| format!("{e}"))?))
        // Must match the main webview's flags: webviews sharing a profile need identical browser args.
        .additional_browser_args("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --renderer-process-limit=1 --disable-gpu-rasterization --disable-background-networking")
        .background_color(tauri::window::Color(11, 8, 20, 255))
        .initialization_script(CHAT_CSS)
        // Loaded pages read "(N) Discord | <chat>"; the bare "Discord" while loading would fake a zero baseline.
        .on_document_title_changed(|w, t| if t.contains(" | ") { chat_unread(w.app_handle(), Some(title_count(&t))) });
    CHAT_W.store(width, std::sync::atomic::Ordering::Relaxed);
    let chat = win.add_child(builder, tauri::LogicalPosition::new(0.0, 0.0), tauri::LogicalSize::new(width.max(1) as f64, 500.0)).map_err(e)?;
    if width == 0 {
        chat.hide().map_err(e)?;
    }
    layout_chat(app);
    Ok(chat)
}

/// Injected into every Discord page: chat only (server + channel sidebar hidden; prefix matches survive Discord's
/// class-hash changes), the dashboard's violet palette (styles.css :root), and on the DMs home just the DM list
/// (no Friends page, no Nitro/Shop/Quests). The data attribute follows Discord's in-page navigation.
const CHAT_CSS: &str = r#"(()=>{const f=()=>document.documentElement.toggleAttribute("data-ptcc-dms",location.pathname==="/channels/@me");
for(const k of["pushState","replaceState"]){const o=history[k];history[k]=function(...a){const r=o.apply(this,a);f();return r}}
addEventListener("popstate",f);f();
addEventListener("DOMContentLoaded",()=>{const s=document.createElement("style");s.textContent=`
[class^="content_"]>[class^="sidebar_"]{display:none!important}
.theme-dark,.theme-darker{--background-base-lowest:#07050f!important;--background-base-lower:#0b0814!important;--background-base-low:#110c20!important;--background-surface-high:#16102b!important;--background-surface-higher:#1d1638!important;--background-surface-highest:#2c2350!important;--chat-background:#16102b!important;--chat-background-default:#16102b!important;--background-secondary-alt:#110c20!important;--bg-surface-raised:#1d1638!important;--chat-border:#2c2350!important;--background-brand:#8b5cf6!important;--background-accent:#3a2f68!important}
html[data-ptcc-dms] [class^="content_"]>[class^="sidebar_"]{display:flex!important;flex-direction:column!important;width:100%!important;max-width:none!important;flex:1 1 auto!important}
html[data-ptcc-dms] [class^="content_"]>[class^="sidebar_"]>:not([class^="sidebarList_"]){display:none!important}
html[data-ptcc-dms] [class^="sidebarList_"]{width:100%!important;flex:1 1 auto!important}
html[data-ptcc-dms] [class^="content_"]>[class^="sidebar_"]~*{display:none!important}
html[data-ptcc-dms] [class^="content_"]{grid-template-columns:100%!important;grid-template-areas:none!important}
html[data-ptcc-dms] [class^="content_"]>[class^="sidebar_"]{grid-area:auto!important;grid-column:1/-1!important}
html[data-ptcc-dms] nav[class^="privateChannels_"] *:has(> a[href="/store"]),html[data-ptcc-dms] nav[class^="privateChannels_"] *:has(> a[href="/shop"]),html[data-ptcc-dms] nav[class^="privateChannels_"] *:has(> a[href^="/quest"]){display:none!important}`;document.head.append(s)})})()"#;

/// Pins the chat sidebar to the main window's right edge, full height. Called on toggle and on every resize.
pub fn layout_chat(app: &tauri::AppHandle) {
    use tauri::Manager;
    let (Some(win), Some(chat)) = (app.get_window("main"), app.get_webview("chat")) else { return };
    let Ok(size) = win.inner_size() else { return };
    let size = size.to_logical::<f64>(win.scale_factor().unwrap_or(1.0));
    let w = (CHAT_W.load(std::sync::atomic::Ordering::Relaxed) as f64).min(size.width);
    if size.width < 1.0 {
        return; // minimized
    }
    // The dashboard's own view gets narrower (not covered), so its layout breakpoints see the real room left.
    if let Some(main) = app.get_webview("main") {
        let _ = main.set_bounds(tauri::Rect {
            position: tauri::LogicalPosition::new(0.0, 0.0).into(),
            size: tauri::LogicalSize::new(size.width - w, size.height).into(),
        });
    }
    if w == 0.0 {
        return; // hidden
    }
    let _ = chat.set_bounds(tauri::Rect {
        position: tauri::LogicalPosition::new(size.width - w, 0.0).into(),
        size: tauri::LogicalSize::new(w, size.height).into(),
    });
}

/// Toggles the chat sidebar: web chat (Discord by default) docked inside the main window on the right, a second
/// webview in the same window; the dashboard's view narrows to make room. Returns its width (0 = closed). Remote content gets no
/// IPC access (capabilities cover the "main" webview only). Hidden rather than closed: tearing down a webview that
/// shares the dashboard's renderer left the dashboard drawing black (2026-11-09). `qr`: wider, because Discord only
/// shows its scan-with-your-phone QR code on pages wider than 830px. `url`: go straight to a saved chat.
#[tauri::command(async)]
pub fn toggle_chat(st: State<AppState>, app: tauri::AppHandle, qr: Option<bool>, url: Option<String>) -> R<u32> {
    use std::sync::atomic::Ordering::Relaxed;
    use tauri::Manager;
    let e = |e: tauri::Error| e.to_string();
    let qr = qr == Some(true);
    let target = if qr { Some("https://discord.com/login".to_string()) } else { url };
    if target.as_ref().is_some_and(|u| !u.starts_with("https://")) {
        return Err("chat URL must start with https://".into());
    }
    let width = if qr { 860 } else { 440 };
    if let Some(chat) = app.get_webview("chat") {
        if target.is_none() && CHAT_W.load(Relaxed) > 0 {
            CHAT_W.store(0, Relaxed);
            chat.hide().map_err(e)?;
            layout_chat(&app);
            return Ok(0);
        }
        // Opening goes to the DM list; a saved chat jumps straight there. Inside Discord, move in-page (Discord's own
        // router follows popstate), which is instant; a full load takes seconds.
        let go = if qr { target } else { Some(target.unwrap_or_else(|| "https://discord.com/channels/@me".into())) };
        match go.as_deref().and_then(|u| u.strip_prefix("https://discord.com")) {
            Some(path) if !qr && chat.url().is_ok_and(|u| u.host_str() == Some("discord.com")) => {
                let path = serde_json::to_string(path).map_err(|e| e.to_string())?;
                chat.eval(format!("history.pushState(null,'',{path});dispatchEvent(new PopStateEvent('popstate'))")).map_err(e)?;
            }
            _ => {
                if let Some(u) = go {
                    chat.navigate(u.parse().map_err(|e| format!("{e}"))?).map_err(e)?;
                }
            }
        }
        CHAT_W.store(width, Relaxed);
        layout_chat(&app);
        chat.show().map_err(e)?;
        chat_unread(&app, None);
        let _ = chat.set_focus();
        return Ok(width);
    }
    let url = match target {
        Some(u) => u,
        None => with(&st, |c| Ok(registry::settings(c)["chat_url"].as_str().unwrap_or("").to_string()))?,
    };
    let chat = create_chat(&app, &url, width)?;
    chat_unread(&app, None);
    let _ = chat.set_focus();
    Ok(width)
}

/// The chat panel's current page, so it can be saved as a named chat (a group DM, a channel).
#[tauri::command]
pub fn chat_current(app: tauri::AppHandle) -> R<String> {
    use tauri::Manager;
    let w = app.get_webview("chat").ok_or("Open the Chat panel on the chat you want to save first.")?;
    Ok(w.url().map_err(|e| e.to_string())?.to_string())
}

/// Folder map for a project: every real folder (repo depth <= 2, vault depth <= 2) plus folders seen in git
/// history, with what was recorded in each: commits touching it, lines changed, Claude edit calls, last change.
#[tauri::command(async)]
pub fn get_folders(st: State<AppState>, id: String) -> R<Vec<Value>> {
    use std::collections::{BTreeMap, HashSet};
    #[derive(Default)]
    struct F { commits: HashSet<String>, changes: i64, edits: i64, last: String, exists: bool, kind: &'static str }
    let (repo, vault, git_rows, edit_rows) = with(&st, |c| {
        let p = rows(c, "SELECT repo_path, vault_dir FROM projects WHERE id=?1", [&id])?.into_iter().next().ok_or("unknown project")?;
        let git = rows(c, "SELECT f.path, f.sha, coalesce(f.added,0)+coalesce(f.deleted,0) ch, coalesce(e.ts,'') ts FROM commit_files f
                           LEFT JOIN events e ON e.source='git' AND e.ref = f.project_id || ':' || f.sha WHERE f.project_id=?1", [&id])?;
        let edits = rows(c, "SELECT sf.path, sf.edits, coalesce(sf.last_ts,'') ts FROM session_files sf JOIN sessions s ON s.id=sf.session_id
                             WHERE s.project_id=?1", [&id])?;
        Ok((p["repo_path"].as_str().map(str::to_string), p["vault_dir"].as_str().map(str::to_string), git, edits))
    })?;
    let mut map: BTreeMap<(String, String), F> = BTreeMap::new(); // (kind, rel path)
    let ancestors = |rel: &str| -> Vec<String> {
        let segs: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        if segs.len() <= 1 { return vec!["(root files)".into()]; }
        (1..segs.len().min(3)).map(|i| segs[..i].join("/")).collect()
    };
    // real folders on disk
    fn walk(base: &Path, rel: String, depth: usize, out: &mut Vec<String>, budget: &mut usize) {
        let Ok(rd) = std::fs::read_dir(base.join(&rel)) else { return };
        for e in rd.flatten() {
            if *budget == 0 { return; }
            let n = e.file_name().to_string_lossy().into_owned();
            if !e.path().is_dir() || n.starts_with('.') || crate::registry::EXCLUDED_DIRS.contains(&n.as_str()) { continue; }
            *budget -= 1;
            let r = if rel.is_empty() { n } else { format!("{rel}/{n}") };
            out.push(r.clone());
            if depth > 1 { walk(base, r, depth - 1, out, budget); }
        }
    }
    for (kind, base) in [("repo", repo.as_deref()), ("vault", vault.as_deref())] {
        let Some(b) = base else { continue };
        let mut dirs = Vec::new();
        let mut budget = 1500;
        walk(Path::new(b), String::new(), 2, &mut dirs, &mut budget);
        for d in dirs {
            let f = map.entry((kind.into(), d.clone())).or_default();
            f.exists = true;
            f.kind = kind;
            if kind == "vault" {
                // newest note directly in this folder
                if let Ok(rd) = std::fs::read_dir(Path::new(b).join(&d)) {
                    for e in rd.flatten() {
                        if e.file_name().to_string_lossy().ends_with(".md") {
                            if let Some(m) = crate::util::mtime_secs(&e.path()) {
                                let ts = crate::util::unix_to_iso(m);
                                if ts > f.last { f.last = ts; }
                            }
                        }
                    }
                }
            }
        }
    }
    for g in &git_rows {
        let path = g["path"].as_str().unwrap_or("");
        for a in ancestors(path) {
            let f = map.entry(("repo".into(), a)).or_default();
            f.kind = "repo";
            f.commits.insert(g["sha"].as_str().unwrap_or("").to_string());
            f.changes += g["ch"].as_i64().unwrap_or(0);
            let ts = g["ts"].as_str().unwrap_or("");
            if ts > f.last.as_str() { f.last = ts.to_string(); }
        }
    }
    if let Some(r) = repo.as_deref() {
        let rn = crate::util::norm(r);
        for e in &edit_rows {
            let abs = crate::util::norm(e["path"].as_str().unwrap_or(""));
            let Some(rel) = abs.strip_prefix(&format!("{rn}\\")) else { continue };
            for a in ancestors(&rel.replace('\\', "/")) {
                // match the on-disk casing if we know the folder; norm() lowercased it
                let key = map.keys().find(|(k, p)| k == "repo" && p.to_lowercase() == a).map(|(_, p)| p.clone()).unwrap_or(a);
                let f = map.entry(("repo".into(), key)).or_default();
                f.kind = "repo";
                f.edits += e["edits"].as_i64().unwrap_or(0);
                let ts = e["ts"].as_str().unwrap_or("");
                if ts > f.last.as_str() { f.last = ts.to_string(); }
            }
        }
    }
    let base_exists = |kind: &str, p: &str| -> bool {
        let b = if kind == "repo" { repo.as_deref() } else { vault.as_deref() };
        p == "(root files)" || b.map_or(false, |b| Path::new(b).join(p).is_dir())
    };
    Ok(map
        .into_iter()
        .map(|((kind, path), f)| {
            let exists = f.exists || base_exists(&kind, &path);
            json!({"kind": kind, "path": path.clone(), "depth": path.matches('/').count(), "commits": f.commits.len(),
                   "changes": f.changes, "edits": f.edits, "last_ts": if f.last.is_empty() { Value::Null } else { json!(f.last) }, "exists": exists})
        })
        .collect())
}

/* ---------- TASKS.md ticking (previewed write) ---------- */

/// The exact TASKS.md line a tick would change. Refuses when the file moved on since indexing.
fn task_line(c: &rusqlite::Connection, id: i64) -> R<(String, PathBuf, usize, String, String)> {
    let r = rows(c, "SELECT project_id, source_file, line, text FROM tasks WHERE id=?1", [id])?.into_iter().next().ok_or("unknown task")?;
    let file = PathBuf::from(r["source_file"].as_str().unwrap_or_default());
    if file.file_name().and_then(|n| n.to_str()) != Some("TASKS.md") {
        return Err("Only TASKS.md items can be ticked here.".into());
    }
    let n = r["line"].as_i64().unwrap_or(0) as usize;
    let md = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let before = md.lines().nth(n.wrapping_sub(1)).unwrap_or_default().to_string();
    let lt = crate::continuity::line_text(&before);
    if lt.is_empty() || !r["text"].as_str().unwrap_or_default().starts_with(&lt) {
        return Err("TASKS.md changed since it was read. Refresh and try again.".into());
    }
    let after = crate::continuity::toggle_line(&before).ok_or("This item is a paragraph, not a bullet. Open TASKS.md to change it.")?;
    Ok((r["project_id"].as_str().unwrap_or_default().to_string(), file, n, before, after))
}

#[tauri::command(async)]
pub fn task_toggle_preview(st: State<AppState>, id: i64) -> R<Value> {
    with(&st, |c| {
        let (_, file, line, before, after) = task_line(c, id)?;
        Ok(json!({"file": file, "line": line, "before": before, "after": after}))
    })
}

#[tauri::command(async)]
pub fn task_toggle(st: State<AppState>, id: i64, before: String) -> R<()> {
    with(&st, |c| {
        let (pid, file, line, now, after) = task_line(c, id)?;
        if now != before {
            return Err("TASKS.md changed after the preview. Refresh and try again.".into());
        }
        let md = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
        // Replace only that line, keeping each line's own ending (LF or CRLF).
        let out: String = md
            .split_inclusive('\n')
            .enumerate()
            .map(|(i, l)| if i + 1 == line { format!("{after}{}", &l[l.trim_end_matches(['\r', '\n']).len()..]) } else { l.to_string() })
            .collect();
        let tmp = file.with_extension("md.tmp");
        std::fs::write(&tmp, out).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &file).map_err(|e| e.to_string())?;
        crate::util::log("info", format!("task ticked: {}:{line}", file.display()));
        crate::indexer::reindex_project(c, &pid)
    })
}

/* ---------- team login + action items ---------- */

#[tauri::command(async)]
pub fn team_profiles(st: State<AppState>) -> R<Value> {
    let me = st.user.lock().unwrap().clone();
    with(&st, |c| Ok(json!({"profiles": crate::team::profiles(c)?.iter().map(|p| p.public()).collect::<Vec<_>>(), "me": me})))
}

#[tauri::command(async)]
pub fn team_create(st: State<AppState>, name: String, pin: String, color: String) -> R<Value> {
    let p = with(&st, |c| crate::team::create_profile(c, &name, &pin, &color))?;
    *st.user.lock().unwrap() = Some(p.public());
    Ok(p.public())
}

#[tauri::command(async)]
pub fn team_login(st: State<AppState>, id: String, pin: String) -> R<Value> {
    match with(&st, |c| crate::team::verify(c, &id, &pin)) {
        Ok(p) => {
            *st.user.lock().unwrap() = Some(p.public());
            Ok(p.public())
        }
        Err(e) => {
            std::thread::sleep(std::time::Duration::from_millis(700)); // slows PIN guessing
            Err(e)
        }
    }
}

#[tauri::command(async)]
pub fn team_logout(st: State<AppState>) {
    *st.user.lock().unwrap() = None;
}

/// Action items for one project, or for every project that has a vault folder.
#[tauri::command(async)]
pub fn actions_list(st: State<AppState>, project_id: Option<String>) -> R<Vec<Value>> {
    with(&st, |c| {
        let ps = rows(c, "SELECT id, name, vault_dir FROM projects WHERE hidden=0 AND vault_dir IS NOT NULL AND (?1 IS NULL OR id=?1)", [&project_id])?;
        let mut out = vec![];
        for p in ps {
            let Ok(path) = crate::team::file_for(p["vault_dir"].as_str()) else { continue };
            for it in crate::team::read(&path)? {
                let mut v = serde_json::to_value(it).unwrap();
                v["project_id"] = p["id"].clone();
                v["project"] = p["name"].clone();
                out.push(v);
            }
        }
        Ok(out)
    })
}

#[tauri::command(async)]
pub fn action_op(st: State<AppState>, project_id: String, op: crate::team::Op) -> R<()> {
    let me = st.user.lock().unwrap().clone().ok_or("Sign in first.")?;
    let by = me["name"].as_str().unwrap_or_default().to_string();
    with(&st, |c| {
        let p = rows(c, "SELECT name, vault_dir FROM projects WHERE id=?1", [&project_id])?.into_iter().next().ok_or("unknown project")?;
        let path = crate::team::file_for(p["vault_dir"].as_str())?;
        crate::team::run_op(&path, p["name"].as_str().unwrap_or_default(), &op, &by)?;
        crate::util::log("info", format!("action {} by {by} in {project_id}", op.kind));
        Ok(())
    })
}

/* ---------- CHAT (read transcripts) ---------- */

#[tauri::command(async)]
pub fn chat_page(st: State<AppState>, id: String, before: Option<u64>, limit: Option<usize>) -> R<Value> {
    let (path, agent, mut session) = with(&st, |c| crate::chat::transcript(c, &id))?;
    with(&st, |c| Ok(crate::chat::add_card(c, &id, &mut session)?))?;
    let page = crate::chat::read_page(&path, &agent, before, limit.unwrap_or(120).clamp(1, 400))?;
    Ok(json!({"session": session, "page": page}))
}

#[tauri::command(async)]
pub fn chat_tail(st: State<AppState>, id: String, from: u64) -> R<crate::chat::Page> {
    let (path, agent, _) = with(&st, |c| crate::chat::transcript(c, &id))?;
    crate::chat::read_tail(&path, &agent, from)
}

#[tauri::command(async)]
pub fn chat_entry(st: State<AppState>, id: String, off: u64) -> R<Vec<crate::chat::ChatEntry>> {
    let (path, agent, _) = with(&st, |c| crate::chat::transcript(c, &id))?;
    crate::chat::read_entry(&path, &agent, off)
}

#[tauri::command(async)]
pub fn chat_projects(st: State<AppState>) -> R<Vec<Value>> {
    with(&st, |c| Ok(crate::chat::project_cards(c)?))
}

#[tauri::command(async)]
pub fn chat_list(st: State<AppState>, agent: Option<String>, project: String) -> R<Vec<Value>> {
    let mut list = with(&st, |c| {
        rows(
            c,
            &format!(
                "SELECT s.*, p.name project, tp.name temp_copy {} WHERE (?2='' OR s.agent=?2) AND coalesce(p.id,'')=?3 ORDER BY s.updated DESC LIMIT 1000",
                crate::chat::CARD_JOIN
            ),
            [crate::chat::temp_copies(c)?, agent.unwrap_or_default(), project],
        )
    })?;
    // Snippets only for the newest 60: each costs one small read at the end of a transcript.
    for r in list.iter_mut().take(60) {
        let snippet = match (r["path"].as_str(), r["agent"].as_str()) {
            (Some(p), Some(a)) => crate::chat::last_reply(Path::new(p), a),
            _ => String::new(),
        };
        r["last_reply"] = json!(snippet);
    }
    Ok(list)
}

#[cfg(test)]
mod chat_tests {
    #[test]
    fn title_count() {
        assert_eq!(super::title_count("(2179) Discord | Friends"), 2179);
        assert_eq!(super::title_count("Discord | general"), 0);
        assert_eq!(super::title_count("(x) Discord"), 0);
    }
}
