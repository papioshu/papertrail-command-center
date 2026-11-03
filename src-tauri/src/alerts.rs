//! Desktop alerts after a refresh: new blockers, new indexing errors, and once a day the projects that
//! have NOW tasks but no activity for a week. Each alert names what changed; the first run only records
//! a baseline so nobody gets a flood on install.
use crate::db;
use crate::util::{rows, truncate};
use rusqlite::Connection;
use serde_json::{json, Value};

const STALE_DAYS: i64 = 7;

fn names(list: &[String]) -> String {
    let more = list.len().saturating_sub(4);
    let head = list.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
    if more > 0 { format!("{head} and {more} more") } else { head }
}

/// (title, body) pairs to show now. Updates the stored baselines.
pub fn check(conn: &Connection) -> Vec<(String, String)> {
    if db::setting(conn, "notifications").and_then(|v| v.as_bool()) == Some(false) {
        return vec![];
    }
    let mut out = vec![];

    // Blockers: keyed by project + text, so an edited blocker counts as new and a moved one does not.
    let blocked = rows(
        conn,
        "SELECT p.name, t.project_id || '|' || substr(t.text,1,160) k, t.text FROM tasks t JOIN projects p ON p.id=t.project_id
         WHERE t.status='blocked' AND p.hidden=0",
        [],
    )
    .unwrap_or_default();
    let keys: Vec<Value> = blocked.iter().map(|r| r["k"].clone()).collect();
    if let Some(prev) = db::setting(conn, "alert_blocked").and_then(|v| v.as_array().cloned()) {
        let new: Vec<&Value> = blocked.iter().filter(|r| !prev.contains(&r["k"])).collect();
        match new.as_slice() {
            [] => {}
            [one] => out.push((format!("New blocker · {}", one["name"].as_str().unwrap_or("")), truncate(one["text"].as_str().unwrap_or(""), 180))),
            many => {
                let mut ps: Vec<String> = many.iter().filter_map(|r| r["name"].as_str().map(String::from)).collect();
                ps.dedup();
                out.push((format!("{} new blockers", many.len()), names(&ps)));
            }
        }
    }
    let _ = db::set_setting(conn, "alert_blocked", &Value::Array(keys));

    // Indexing errors since the last check.
    let max_err = conn.query_row("SELECT coalesce(max(id),0) FROM index_errors", [], |r| r.get::<_, i64>(0)).unwrap_or(0);
    if let Some(prev) = db::setting(conn, "alert_error_id").and_then(|v| v.as_i64()) {
        let errs = rows(conn, "SELECT scope FROM index_errors WHERE id>?1", [prev]).unwrap_or_default();
        if !errs.is_empty() {
            let mut scopes: Vec<String> = errs.iter().filter_map(|r| r["scope"].as_str().map(String::from)).collect();
            scopes.sort();
            scopes.dedup();
            out.push((format!("{} indexing problem{}", errs.len(), if errs.len() == 1 { "" } else { "s" }), format!("{} · details in Settings › Diagnostics", names(&scopes))));
        }
    }
    let _ = db::set_setting(conn, "alert_error_id", &json!(max_err));

    // Stale projects, at most once per local day.
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    if db::setting(conn, "alert_stale_day").and_then(|v| v.as_str().map(String::from)).as_deref() != Some(today.as_str()) {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(STALE_DAYS)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let stale: Vec<String> = rows(
            conn,
            "SELECT name FROM projects p WHERE hidden=0 AND coalesce(last_activity,'') < ?1
               AND EXISTS(SELECT 1 FROM tasks t WHERE t.project_id=p.id AND t.status='active') ORDER BY name",
            [&cutoff],
        )
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r["name"].as_str().map(String::from))
        .collect();
        if !stale.is_empty() {
            out.push((format!("{} project{} waiting on NOW tasks", stale.len(), if stale.len() == 1 { "" } else { "s" }), format!("No activity in {STALE_DAYS}+ days: {}", names(&stale))));
        }
        let _ = db::set_setting(conn, "alert_stale_day", &json!(today));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn baseline_then_new() {
        let c = Connection::open_in_memory().unwrap();
        db::migrate(&c).unwrap();
        c.execute_batch(
            "INSERT INTO projects(id,name,source,hidden,last_activity) VALUES('a','Alpha','vault',0,'2099-01-01');
             INSERT INTO tasks(project_id,status,text,source_file,line) VALUES('a','blocked','old','f',1);",
        )
        .unwrap();
        assert!(check(&c).is_empty(), "first run is a baseline");
        c.execute("INSERT INTO tasks(project_id,status,text,source_file,line) VALUES('a','blocked','new one','f',2)", []).unwrap();
        db::record_error(&c, "git:a", "boom");
        let a = check(&c);
        assert_eq!(a.len(), 2, "{a:?}");
        assert_eq!(a[0], ("New blocker · Alpha".into(), "new one".into()));
        assert!(a[1].0.starts_with("1 indexing problem"));
        assert!(check(&c).is_empty(), "nothing new");
    }
}
