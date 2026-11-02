//! Configuration + project discovery.
//! Sources, in priority order: the Book of Secrets registry table, manual registrations,
//! then git repos found one/two levels under the configured project roots.
use crate::db;
use crate::util::{home, norm, under};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const EXCLUDED_DIRS: &[&str] = &[
    "node_modules", ".git", "dist", "build", ".next", ".cache", ".venv", "target", "out", "coverage", ".turbo",
];

#[derive(Clone, Debug)]
pub struct Config {
    pub vault: Option<PathBuf>,
    pub roots: Vec<PathBuf>,
    pub manual: Vec<Value>,
    pub claude_dir: PathBuf,
    pub codex_dir: PathBuf,
    pub refresh_minutes: u64,
    pub close_to_tray: bool,
    pub workspace_groups: Vec<(String, String)>,
}

pub fn defaults() -> Value {
    let vault = PathBuf::from(r"C:\Projectwork\Book_Of_Secrets");
    let root = PathBuf::from(r"C:\Projectwork");
    json!({
        "vault_path": if vault.join("MASTER_PASSDOWN.md").is_file() { json!(vault) } else { Value::Null },
        "project_roots": if root.is_dir() { json!([root]) } else { json!([]) },
        "manual_projects": [],
        "claude_dir": home().join(".claude"),
        "codex_dir": home().join(".codex"),
        "refresh_minutes": 15,
        "close_to_tray": true,
        "workstream_aliases": {},
        // Owner-defined grouping (2026-11-02): workspace or project name -> the workspace it belongs to.
        "workspace_groups": {
            "OARA": "Outsider Collective", "GH05tkin": "Outsider Collective",
            "MeshChat-FictoChat": "Encrypted Chat Apps",
            "Nimbus Generator": "FLY WING GENESIS", "ONE FOR ALL": "FLY WING GENESIS",
            "Solo RPG Engine": "RPG Engine", "ThingsBook Visual Library": "ThingsBook"
        },
        "discovery_refresh_hours": 24,
        "chat_url": "https://discord.com/app",
        "presets": [
            {"id":"continue","name":"Continue last task","agent":"any","project":null,
             "task":"Resolve the latest handoff and next logical action, review the worktree, and resume that work."},
            {"id":"audit","name":"Audit recent changes","agent":"any","project":null,
             "task":"Audit the recent commits and uncommitted changes listed in the brief. Report risks before changing anything."},
            {"id":"build","name":"Investigate build errors","agent":"any","project":null,
             "task":"Run the project's build and tests, then investigate and fix any failures."},
            {"id":"oara-mech","name":"Mechanics & systems","agent":"claude","project":"oara",
             "task":"Continue OARA gameplay systems: movement, combat, physics, game systems and integration."},
            {"id":"oara-assets","name":"Assets & implementation","agent":"codex","project":"oara",
             "task":"Continue OARA asset work: acquisition, model integration, textures, animation implementation, scene assets."}
        ]
    })
}

/// Settings = defaults overlaid with stored values.
pub fn settings(conn: &Connection) -> Value {
    let mut v = defaults();
    if let Value::Object(m) = &mut v {
        let keys: Vec<String> = m.keys().cloned().collect();
        for k in keys {
            if let Some(s) = db::setting(conn, &k) {
                m.insert(k, s);
            }
        }
    }
    v
}

impl Config {
    pub fn load(conn: &Connection) -> Config {
        let s = settings(conn);
        let p = |k: &str| s[k].as_str().map(PathBuf::from);
        Config {
            vault: p("vault_path").filter(|v| v.is_dir()),
            roots: s["project_roots"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(PathBuf::from)).collect()).unwrap_or_default(),
            manual: s["manual_projects"].as_array().cloned().unwrap_or_default(),
            claude_dir: p("claude_dir").unwrap_or_else(|| home().join(".claude")),
            codex_dir: p("codex_dir").unwrap_or_else(|| home().join(".codex")),
            refresh_minutes: s["refresh_minutes"].as_u64().unwrap_or(15).clamp(5, 24 * 60),
            close_to_tray: s["close_to_tray"].as_bool().unwrap_or(true),
            workspace_groups: s["workspace_groups"].as_object().map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect()).unwrap_or_default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub workspace: String,
    pub repo: Option<String>,
    pub vault_dir: Option<String>,
    pub source: &'static str,
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(x) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(x);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Workspace = the part before " (" — "FLY WING GENESIS (Media)" -> "FLY WING GENESIS".
fn workspace_of(name: &str) -> String {
    name.split(" (").next().unwrap_or(name).trim().to_string()
}

/// Parse the `<!-- project-registry:start -->` table of MASTER_PASSDOWN.md.
pub fn parse_registry(passdown: &str, vault: &Path) -> Vec<Project> {
    let body = match (passdown.find("project-registry:start"), passdown.find("project-registry:end")) {
        (Some(a), Some(b)) if b > a => &passdown[a..b],
        _ => return vec![],
    };
    let mut out: Vec<Project> = Vec::new();
    for line in body.lines() {
        let cells: Vec<&str> = line.trim().trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() < 3 || cells[0] == "Project" || cells[0].starts_with("---") {
            continue;
        }
        let name = cells[0].to_string();
        let repo = cells[1]
            .split('`')
            .nth(1)
            .map(str::to_string)
            .filter(|r| Path::new(r).is_dir());
        // First markdown link target in the continuity column -> its folder.
        let vault_dir = cells[2]
            .split("](")
            .nth(1)
            .and_then(|t| t.split(')').next())
            .map(url_decode)
            .and_then(|rel| Path::new(&rel).parent().map(|p| vault.join(p)))
            .filter(|p| p.is_dir())
            .map(|p| p.to_string_lossy().replace('/', "\\"));
        out.push(Project { id: slug(&name), workspace: workspace_of(&name), name, repo, vault_dir, source: "registry" });
    }
    // "BOMBSQUAD Bounty Board" belongs to the BOMBSQUAD workspace when that project exists.
    let names: Vec<String> = out.iter().map(|p| p.name.clone()).collect();
    for p in out.iter_mut() {
        if let Some(parent) = names.iter().filter(|n| p.name.starts_with(&format!("{n} "))).max_by_key(|n| n.len()) {
            p.workspace = workspace_of(parent);
        }
    }
    out
}

/// First git repo inside a vault folder (e.g. `PaperTrail OS/Papertrail OS/repo`), bounded depth.
fn find_repo(dir: &Path, depth: usize) -> Option<String> {
    if is_repo(dir) {
        return Some(dir.to_string_lossy().into_owned());
    }
    if depth == 0 {
        return None;
    }
    let rd = std::fs::read_dir(dir).ok()?;
    let mut subs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    subs.sort();
    subs.into_iter()
        .filter(|p| { let n = p.file_name().unwrap().to_string_lossy(); !n.starts_with('.') && !EXCLUDED_DIRS.contains(&n.as_ref()) })
        .find_map(|p| find_repo(&p, depth - 1))
}

fn is_repo(p: &Path) -> bool {
    p.join(".git").exists()
}

fn looks_like_project(p: &Path) -> bool {
    is_repo(p)
        || ["package.json", "Cargo.toml", "pyproject.toml", "foundry.toml", "go.mod"].iter().any(|f| p.join(f).is_file())
}

/// Git repos/projects directly under each root, plus nested repos one level deeper.
fn scan_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let skip = |n: &str| n.starts_with('.') || EXCLUDED_DIRS.contains(&n);
    for root in roots {
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let n = e.file_name().to_string_lossy().into_owned();
            if !p.is_dir() || skip(&n) || n.to_lowercase().contains("backup") || n.eq_ignore_ascii_case("Book_Of_Secrets") {
                continue;
            }
            if looks_like_project(&p) {
                out.push(p.clone());
            }
            if let Ok(rd2) = std::fs::read_dir(&p) {
                for e2 in rd2.flatten() {
                    let p2 = e2.path();
                    let n2 = e2.file_name().to_string_lossy().into_owned();
                    if p2.is_dir() && !skip(&n2) && is_repo(&p2) {
                        out.push(p2);
                    }
                }
            }
        }
    }
    out
}

pub fn discover(cfg: &Config) -> (Vec<Project>, Vec<String>) {
    let mut errors = Vec::new();
    let mut projects: Vec<Project> = Vec::new();
    if let Some(v) = &cfg.vault {
        match std::fs::read_to_string(v.join("MASTER_PASSDOWN.md")) {
            Ok(s) => projects.extend(parse_registry(&s, v)),
            Err(e) => errors.push(format!("MASTER_PASSDOWN.md unreadable: {e}")),
        }
    }
    // Book of Secrets project folders that the registry table doesn't list yet (vault-only projects included).
    if let Some(v) = &cfg.vault {
        let root = v.join("02 Projects");
        if let Ok(rd) = std::fs::read_dir(&root) {
            let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            dirs.sort();
            for d in dirs {
                let ds = d.to_string_lossy().replace('/', "\\");
                // Skip folders already covered: a registered project's vault folder, or the parent of one.
                if projects.iter().any(|p| p.vault_dir.as_deref().map_or(false, |vd| norm(vd) == norm(&ds) || under(vd, &ds))) {
                    continue;
                }
                let name = d.file_name().unwrap().to_string_lossy().into_owned();
                let mut id = slug(&name);
                if id.is_empty() {
                    continue;
                }
                if projects.iter().any(|p| p.id == id) {
                    id = format!("{id}-vault");
                }
                projects.push(Project { id, workspace: workspace_of(&name), name, repo: find_repo(&d, 3), vault_dir: Some(ds), source: "vault" });
            }
            // Nested project folders that carry their own code repo (e.g. `Papertrail Command Center/PaperTrail OS`)
            // join the parent folder's workspace. Plain note subfolders stay part of their parent.
            let tops: Vec<PathBuf> = std::fs::read_dir(&root).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
            for top in tops {
                let parent_ws = projects
                    .iter()
                    .find(|p| p.vault_dir.as_deref().map_or(false, |vd| norm(vd) == norm(&top.to_string_lossy())))
                    .map(|p| p.workspace.clone())
                    .unwrap_or_else(|| workspace_of(&top.file_name().unwrap().to_string_lossy()));
                let Ok(rd) = std::fs::read_dir(&top) else { continue };
                let mut subs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
                subs.sort();
                for sub in subs {
                    let ss = sub.to_string_lossy().replace('/', "\\");
                    if projects.iter().any(|p| p.vault_dir.as_deref().map_or(false, |vd| norm(vd) == norm(&ss) || under(vd, &ss))) {
                        continue;
                    }
                    let Some(repo) = find_repo(&sub, 2) else { continue };
                    if projects.iter().any(|p| p.repo.as_deref().map(norm) == Some(norm(&repo))) {
                        continue;
                    }
                    // Same convention as the registry: "Parent (Sub)", e.g. "Zathena Ventures (website)".
                    let parent_name = top.file_name().unwrap().to_string_lossy().into_owned();
                    let sub_name = sub.file_name().unwrap().to_string_lossy().into_owned();
                    // "BOMBSQUAD/BOMBSQUAD" would read as a copy of the BOMBSQUAD project on the map.
                    let label = if sub_name.eq_ignore_ascii_case(&parent_name) { "vault repo".to_string() } else { sub_name };
                    let name = format!("{parent_name} ({label})");
                    let mut id = slug(&name);
                    if projects.iter().any(|p| p.id == id) {
                        id = format!("{id}-vault");
                    }
                    projects.push(Project { id, workspace: parent_ws.clone(), name, repo: Some(repo), vault_dir: Some(ss), source: "vault" });
                }
            }
        }
    }
    for m in &cfg.manual {
        let (Some(name), Some(path)) = (m["name"].as_str(), m["path"].as_str()) else { continue };
        if !Path::new(path).is_dir() {
            errors.push(format!("manual project path missing: {path}"));
            continue;
        }
        if projects.iter().any(|p| p.repo.as_deref().map(norm) == Some(norm(path))) {
            continue;
        }
        projects.push(Project {
            id: slug(name),
            name: name.into(),
            workspace: m["workspace"].as_str().map(str::to_string).unwrap_or_else(|| workspace_of(name)),
            repo: Some(path.into()),
            vault_dir: None,
            source: "manual",
        });
    }
    for p in scan_roots(&cfg.roots) {
        let ps = p.to_string_lossy().into_owned();
        // Skip if already registered, or if it is a parent of a registered repo with no repo itself.
        if projects.iter().any(|x| x.repo.as_deref().map(norm) == Some(norm(&ps))) {
            continue;
        }
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        // Nested repo under a registered project inherits its workspace.
        let workspace = projects
            .iter()
            .find(|x| x.repo.as_deref().map(|r| under(&ps, r)).unwrap_or(false))
            .map(|x| x.workspace.clone())
            .unwrap_or_else(|| "UNSORTED".into());
        let mut id = slug(&name);
        if projects.iter().any(|x| x.id == id) {
            id = format!("{id}-repo");
        }
        projects.push(Project { id, name, workspace, repo: Some(ps), vault_dir: None, source: "scan" });
    }
    consolidate_workspaces(&mut projects);
    apply_groups(&mut projects, &cfg.workspace_groups);
    (projects, errors)
}

/// Owner's explicit grouping, matched (case/spacing-insensitive) on the project's workspace or its name.
/// Follows chains (A -> B -> C) up to 4 hops.
pub fn apply_groups(projects: &mut [Project], groups: &[(String, String)]) {
    let find = |name: &str| groups.iter().find(|(k, _)| ws_key(k) == ws_key(name)).map(|(_, v)| v.clone());
    for p in projects.iter_mut() {
        let mut ws = find(&p.workspace).or_else(|| find(&p.name));
        let mut hops = 0;
        while let Some(w) = ws.clone() {
            p.workspace = w.clone();
            hops += 1;
            ws = if hops < 4 { find(&w).filter(|n| ws_key(n) != ws_key(&w)) } else { None };
        }
    }
}

fn ws_key(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}
fn ws_initials(w: &str) -> String {
    w.split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).filter_map(|s| s.chars().next()).collect::<String>().to_lowercase()
}

/// Merge workspaces that are the same thing under different spellings, so the map and decks show one cluster:
/// same letters ("PaperShoots" / "Papershoots"), one name containing another ("Caedis" / "CaedisAerial",
/// "OARA" / "OARA and Loomward", "RPG Engine" / "Solo RPG Engine"), or initials ("ZV" / "Zathena Ventures"). Registry names win as the display name.
pub fn consolidate_workspaces(projects: &mut [Project]) {
    let mut names: Vec<String> = projects.iter().map(|p| p.workspace.clone()).filter(|w| w != "UNSORTED").collect();
    names.sort_by_key(|w| (ws_key(w).len(), w.clone()));
    names.dedup();
    let registry: Vec<String> = projects.iter().filter(|p| p.source == "registry").map(|p| p.workspace.clone()).collect();
    let mut target: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (i, w) in names.iter().enumerate() {
        let k = ws_key(w);
        let base = names[..i].iter().find(|o| {
            let ok = ws_key(o);
            ok == k || (ok.len() >= 4 && k.contains(ok.as_str())) || (ok.len() >= 2 && ws_initials(w) == ok && ws_initials(w).len() >= 2)
        });
        if let Some(b) = base {
            let root = target.get(b).cloned().unwrap_or_else(|| b.clone());
            target.insert(w.clone(), root);
        }
    }
    // Pick the display name per group: a registry workspace if any, else for an initials match the spelled-out name.
    let mut display: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (w, root) in &target {
        let cur = display.get(root).cloned().unwrap_or_else(|| root.clone());
        let better = (registry.contains(w) && !registry.contains(&cur)) || (ws_initials(w) == ws_key(&cur) && !registry.contains(&cur));
        if better { display.insert(root.clone(), w.clone()); }
    }
    for p in projects.iter_mut() {
        let root = target.get(&p.workspace).cloned().unwrap_or_else(|| p.workspace.clone());
        if let Some(d) = display.get(&root) { p.workspace = d.clone(); } else { p.workspace = root; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspaces_merge() {
        let mk = |n: &str, w: &str, src: &'static str| Project { id: slug(n), name: n.into(), workspace: w.into(), repo: None, vault_dir: None, source: src };
        let mut ps = vec![mk("ZV", "ZV", "vault"), mk("Zathena Ventures", "Zathena Ventures", "vault"), mk("Caedis", "CaedisAerial", "vault"),
            mk("OARA", "OARA", "registry"), mk("OARA and Loomward", "OARA and Loomward", "vault"), mk("Solo RPG Engine", "Solo RPG Engine", "vault"),
            mk("RPG Engine", "RPG Engine", "vault"), mk("PaperShoots", "PaperShoots", "vault"), mk("p", "Papershoots", "vault"), mk("x", "UNSORTED", "scan")];
        consolidate_workspaces(&mut ps);
        let ws = |n: &str| ps.iter().find(|p| p.name == n).unwrap().workspace.clone();
        assert_eq!(ws("ZV"), "Zathena Ventures");
        assert_eq!(ws("OARA and Loomward"), "OARA");
        assert_eq!(ws("Solo RPG Engine"), "RPG Engine"); // owner: similarly named workspaces are one
        assert_eq!(ws("p"), ws("PaperShoots"));
        assert_eq!(ws("x"), "UNSORTED");
        let mut g = vec![mk("OARA", "OARA", "registry"), mk("GH05tkin", "GH05tkin", "registry"), mk("Nimbus Generator", "Nimbus Generator", "registry")];
        apply_groups(&mut g, &[("OARA".into(), "Outsider Collective".into()), ("gh05tkin".into(), "OARA".into()), ("Nimbus Generator".into(), "FLY WING GENESIS".into())]);
        assert_eq!(g[0].workspace, "Outsider Collective");
        assert_eq!(g[1].workspace, "Outsider Collective"); // chain: GH05tkin -> OARA -> Outsider Collective
        assert_eq!(g[2].workspace, "FLY WING GENESIS");
    }
    #[test]
    fn registry_table() {
        let md = "x\n<!-- project-registry:start -->\n| Project | Repo root | Continuity files | Passdown entry |\n|---|---|---|---|\n| FLY WING GENESIS (Media) | `Z:\\nope` | [CONTINUITY LOG](02%20Projects/FLY%20WING%20GENESIS/Media/CONTINUITY%20LOG.md) | x |\n| BOMBSQUAD | none | x | x |\n| BOMBSQUAD Bounty Board | none | x | x |\n<!-- project-registry:end -->";
        let r = parse_registry(md, Path::new("Z:\\vault"));
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].id, "fly-wing-genesis-media");
        assert_eq!(r[0].workspace, "FLY WING GENESIS");
        assert_eq!(r[0].repo, None); // path doesn't exist -> not trusted
        assert_eq!(r[2].workspace, "BOMBSQUAD");
        assert_eq!(url_decode("a%20b"), "a b");
    }
}
