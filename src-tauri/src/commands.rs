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
pub fn get_activity(st: State<AppState>, project: Option<String>, from: String, to: String) -> R<Vec<Value>> {
    with(&st, |c| {
        let p = project.unwrap_or_default();
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
    })
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
    if !(url.starts_with("https://") || url.starts_with("http://")) || url.contains(['"', ' ', '\n']) {
        return Err("only http(s) links can be opened".into());
    }
    // explorer hands the URL to the default browser; no shell parsing involved.
    std::process::Command::new("explorer").arg(&url).spawn().map_err(|e| e.to_string())?;
    Ok(())
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
    st.worker.send(Job::Refresh);
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

/// Chat slide-out: a separate window docked to the right edge of the dashboard, loading the configured
/// web chat (Discord by default). Remote content gets no IPC access (capabilities cover "main" only).
/// Toggling closes it, which frees its memory.
#[tauri::command(async)]
pub fn toggle_chat(st: State<AppState>, app: tauri::AppHandle) -> R<bool> {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("chat") {
        w.close().map_err(|e| e.to_string())?;
        return Ok(false);
    }
    let url = with(&st, |c| Ok(registry::settings(c)["chat_url"].as_str().unwrap_or("").to_string()))?;
    if !url.starts_with("https://") {
        return Err("chat URL must start with https://".into());
    }
    let main = app.get_webview_window("main").ok_or("main window missing")?;
    let scale = main.scale_factor().unwrap_or(1.0);
    let pos = main.outer_position().map_err(|e| e.to_string())?.to_logical::<f64>(scale);
    let size = main.outer_size().map_err(|e| e.to_string())?.to_logical::<f64>(scale);
    let width = 440.0;
    // Dock outside the right edge when the monitor has room, else overlay the dashboard's right side.
    let monitor_right = main
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| { let p = m.position().to_logical::<f64>(scale); let s = m.size().to_logical::<f64>(scale); p.x + s.width })
        .unwrap_or(f64::MAX);
    let x = if pos.x + size.width + width <= monitor_right { pos.x + size.width } else { pos.x + size.width - width };
    tauri::WebviewWindowBuilder::new(&app, "chat", tauri::WebviewUrl::External(url.parse().map_err(|e| format!("{e}"))?))
        .title("PAPERTRAIL // CHAT")
        // Must match the main window's flags: webviews sharing a profile need identical browser args.
        .additional_browser_args("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --disable-gpu --renderer-process-limit=1 --disable-background-networking")
        .inner_size(width, size.height.max(500.0))
        .position(x, pos.y)
        .background_color(tauri::window::Color(11, 8, 20, 255))
        .build()
        .map_err(|e| e.to_string())?;
    Ok(true)
}
