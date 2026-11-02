//! Discovery radar. Two public, unauthenticated sources, fetched on a schedule or on demand:
//!  - GitHub repository search: new repos (created in the last 180 days) matching the
//!    dependencies the projects actually use.
//!  - npm registry: newer major versions of the projects' own direct dependencies.
//! Untrusted content: stored as text, rendered as text, never executed.
use crate::db;
use crate::indexer::load_projects;
use crate::util::{log, now_iso, truncate};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Duration;

const UA: &str = "papertrail-command-center/0.1 (local desktop app)";
/// Dependencies too generic to say anything about a project.
const GENERIC: &[&str] = &[
    "react", "react-dom", "typescript", "vite", "vitest", "eslint", "prettier", "postcss", "autoprefixer", "tailwindcss",
    "jest", "ts-node", "tsx", "rimraf", "cross-env", "dotenv", "zod", "clsx", "concurrently", "nodemon", "@vitejs/plugin-react",
    "eslint-config-next", "typescript-eslint", "globals", "serde", "serde_json", "tokio", "anyhow", "thiserror", "lucide-react",
    "class-variance-authority", "tailwind-merge", "@eslint/js", "jsdom", "playwright", "@playwright/test", "npm-run-all",
];
const OSI: &[&str] = &["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "MPL-2.0", "GPL-2.0", "GPL-3.0", "LGPL-2.1", "LGPL-3.0", "AGPL-3.0", "Unlicense", "0BSD", "Zlib", "CC0-1.0"];

/// (dependency, declared version) for a repo's package.json.
fn npm_deps(repo: &Path) -> Vec<(String, String)> {
    let Ok(s) = std::fs::read_to_string(repo.join("package.json")) else { return vec![] };
    let Ok(v) = serde_json::from_str::<Value>(&s) else { return vec![] };
    let mut out = Vec::new();
    for k in ["dependencies", "devDependencies"] {
        if let Some(m) = v[k].as_object() {
            for (name, ver) in m {
                if !GENERIC.contains(&name.as_str()) && !name.starts_with("@types/") && !name.starts_with("eslint-") {
                    out.push((name.clone(), ver.as_str().unwrap_or("").to_string()));
                }
            }
        }
    }
    out
}

/// Search term for a dependency: scope and noise removed ("@dimforge/rapier3d-compat" -> "rapier3d").
fn term(dep: &str) -> String {
    let base = dep.rsplit('/').next().unwrap_or(dep);
    base.trim_end_matches("-compat").trim_end_matches("-js").trim_end_matches(".js").to_string()
}

fn major(v: &str) -> Option<u64> {
    v.trim_start_matches(|c: char| !c.is_ascii_digit()).split('.').next()?.parse().ok()
}

fn get(agent: &ureq::Agent, conn: &Connection, url: &str) -> Result<Option<Value>, String> {
    let key = format!("etag:{url}");
    let etag: Option<String> = conn.query_row("SELECT extra FROM file_state WHERE key=?1", [&key], |r| r.get(0)).ok().flatten();
    let mut req = agent.get(url).set("User-Agent", UA).set("Accept", "application/json");
    if let Some(e) = &etag {
        req = req.set("If-None-Match", e);
    }
    match req.call() {
        Ok(r) if r.status() == 304 => Ok(None),
        Ok(r) => {
            if let Some(e) = r.header("ETag") {
                let _ = conn.execute(
                    "INSERT INTO file_state(key,extra) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET extra=excluded.extra",
                    params![key, e],
                );
            }
            r.into_json::<Value>().map(Some).map_err(|e| e.to_string())
        }
        Err(ureq::Error::Status(304, _)) => Ok(None),
        Err(ureq::Error::Status(code, _)) => Err(format!("HTTP {code} from {url}")),
        Err(e) => Err(format!("network: {e}")),
    }
}

pub fn due(conn: &Connection) -> bool {
    let hours = crate::registry::settings(conn)["discovery_refresh_hours"].as_f64().unwrap_or(24.0);
    let last = db::setting(conn, "discovery_last").and_then(|v| v.as_str().map(str::to_string));
    match last.and_then(|l| chrono::DateTime::parse_from_rfc3339(&l).ok()) {
        Some(t) => (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_minutes() as f64 > hours * 60.0,
        None => true,
    }
}

pub fn refresh(conn: &Connection) -> Result<usize, String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(20)).build();
    let projects = load_projects(conn);
    // Recently active projects first, so the capped request budget goes where work is happening.
    let activity: HashMap<String, String> = {
        let mut s = conn.prepare("SELECT id, coalesce(last_activity,'') FROM projects").map_err(|e| e.to_string())?;
        let v = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?.flatten().collect();
        v
    };
    let mut ordered = projects.clone();
    ordered.sort_by(|a, b| activity.get(&b.id).cmp(&activity.get(&a.id)));

    let mut terms: BTreeMap<String, Vec<String>> = BTreeMap::new(); // term -> project ids
    let mut deps: Vec<(String, String, String)> = Vec::new(); // (project, dep, declared)
    for p in &ordered {
        let Some(repo) = p.repo.as_deref() else { continue };
        for (d, v) in npm_deps(Path::new(repo)) {
            terms.entry(term(&d)).or_default().push(p.id.clone());
            deps.push((p.id.clone(), d, v));
        }
    }
    let mut n = 0;
    let mut errors = Vec::new();

    // npm: newer majors of dependencies in use (one request per unique package, capped).
    let mut seen: HashMap<String, Option<Value>> = HashMap::new();
    for (pid, dep, declared) in deps.iter() {
        if seen.len() >= 40 && !seen.contains_key(dep) {
            continue;
        }
        let latest = match seen.get(dep) {
            Some(v) => v.clone(),
            None => {
                let url = format!("https://registry.npmjs.org/{}/latest", dep.replace('/', "%2F"));
                let v = match get(&agent, conn, &url) {
                    Ok(v) => v,
                    Err(e) => {
                        errors.push(e);
                        None
                    }
                };
                seen.insert(dep.clone(), v.clone());
                v
            }
        };
        let Some(latest) = latest else { continue };
        let lv = latest["version"].as_str().unwrap_or("");
        if let (Some(a), Some(b)) = (major(declared), major(lv)) {
            if b > a {
                let id = format!("npm:{dep}@{b}");
                let license = latest["license"].as_str().unwrap_or("unknown").to_string();
                let repo_url = latest["repository"]["url"].as_str().unwrap_or("").replace("git+", "").trim_end_matches(".git").to_string();
                let url = if repo_url.starts_with("http") { repo_url } else { format!("https://www.npmjs.com/package/{dep}") };
                upsert(conn, &json!({
                    "id": id, "name": format!("{dep} {lv}"), "url": url,
                    "description": truncate(latest["description"].as_str().unwrap_or(""), 300),
                    "license": license, "pricing": pricing(&license), "stars": Value::Null,
                    "created_at": Value::Null, "pushed_at": Value::Null, "topics": "release",
                    "project": pid, "reason": format!("{pid} declares {dep} {declared}; major {b} is published"),
                    "score": 3.0, "source": "npm"
                }))?;
                n += 1;
            }
        }
    }

    // GitHub search: new repos around the most-shared terms. Unauthenticated limit is 10/min.
    let mut ranked: Vec<(&String, &Vec<String>)> = terms.iter().collect();
    ranked.sort_by_key(|(_, ps)| std::cmp::Reverse(ps.len()));
    let since = (chrono::Utc::now() - chrono::Duration::days(180)).format("%Y-%m-%d").to_string();
    for (i, (t, ps)) in ranked.into_iter().take(8).enumerate() {
        if i > 0 {
            std::thread::sleep(Duration::from_secs(7));
        }
        let url = format!(
            "https://api.github.com/search/repositories?q={}+in:name,description,topics+created:%3E{}+stars:%3E40&sort=stars&order=desc&per_page=8",
            t.replace(' ', "+"),
            since
        );
        let res = match get(&agent, conn, &url) {
            Ok(Some(v)) => v,
            Ok(None) => continue,
            Err(e) => {
                errors.push(e);
                if errors.iter().any(|e| e.contains("403") || e.contains("429")) {
                    break; // rate limited: stop and try next schedule
                }
                continue;
            }
        };
        let mut uniq: Vec<String> = ps.clone();
        uniq.dedup();
        for item in res["items"].as_array().cloned().unwrap_or_default() {
            let license = item["license"]["spdx_id"].as_str().filter(|l| *l != "NOASSERTION").unwrap_or("unknown").to_string();
            let stars = item["stargazers_count"].as_i64().unwrap_or(0);
            let pushed = item["pushed_at"].as_str().unwrap_or("").to_string();
            let recent = chrono::DateTime::parse_from_rfc3339(&pushed)
                .map(|d| (chrono::Utc::now() - d.with_timezone(&chrono::Utc)).num_days() < 30)
                .unwrap_or(false);
            let score = uniq.len() as f64 * 2.0 + (stars.max(1) as f64).log10() + if recent { 1.0 } else { 0.0 };
            upsert(conn, &json!({
                "id": format!("gh:{}", item["full_name"].as_str().unwrap_or("")),
                "name": item["full_name"], "url": item["html_url"],
                "description": truncate(item["description"].as_str().unwrap_or(""), 300),
                "license": license, "pricing": pricing(&license), "stars": stars,
                "created_at": item["created_at"], "pushed_at": pushed,
                "topics": item["topics"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
                "project": uniq.join(","),
                "reason": format!("matches \"{t}\" used by {} · {stars}★ · score = 2×{} projects + log10(stars){}",
                    uniq.join(", "), uniq.len(), if recent { " + 1 (pushed <30d)" } else { "" }),
                "score": score, "source": "github"
            }))?;
            n += 1;
        }
    }
    let _ = db::set_setting(conn, "discovery_last", &json!(now_iso()));
    let _ = db::set_setting(conn, "discovery_errors", &json!(errors));
    log("info", format!("discovery: {n} items, {} errors", errors.len()));
    if n == 0 && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(n)
}

fn pricing(license: &str) -> String {
    if OSI.contains(&license) {
        format!("Free · open source ({license})")
    } else {
        "Unknown, verify before use".into()
    }
}

fn upsert(conn: &Connection, d: &Value) -> Result<(), String> {
    let projects: Vec<&str> = d["project"].as_str().unwrap_or("").split(',').filter(|s| !s.is_empty()).collect();
    conn.execute(
        "INSERT INTO discoveries(id,name,url,description,license,pricing,stars,created_at,pushed_at,topics,projects,reason,score,source,fetched_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
         ON CONFLICT(id) DO UPDATE SET description=excluded.description, license=excluded.license, pricing=excluded.pricing,
           stars=excluded.stars, pushed_at=excluded.pushed_at, topics=excluded.topics, projects=excluded.projects,
           reason=excluded.reason, score=excluded.score, fetched_at=excluded.fetched_at",
        params![
            d["id"].as_str(), d["name"].as_str(), d["url"].as_str(), d["description"].as_str(), d["license"].as_str(),
            d["pricing"].as_str(), d["stars"].as_i64(), d["created_at"].as_str(), d["pushed_at"].as_str(),
            d["topics"].as_str(), json!(projects).to_string(), d["reason"].as_str(), d["score"].as_f64(),
            d["source"].as_str(), now_iso()
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn terms_and_majors() {
        assert_eq!(super::term("@dimforge/rapier3d-compat"), "rapier3d");
        assert_eq!(super::major("^0.170.0"), Some(0));
        assert_eq!(super::major("~16.2"), Some(16));
        assert!(super::pricing("MIT").starts_with("Free"));
        assert!(super::pricing("unknown").starts_with("Unknown"));
    }
}
