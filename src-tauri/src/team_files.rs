//! Project files for the team (owner choice D048). The owner's PC shares a linked project's git-tracked files into its
//! workspace project (secret-looking names never, 25 MB per file, 5,000 files), each as a hub `file` record; teammates
//! download them and send changes back as `proposal` records. A proposal touches the owner's repo only when the owner
//! accepts it on this PC, after a preview, and only at a checked path inside that repo.

use crate::hub_client::{b64_decode, build_snapshot, repo_of, sensitive, sha256_hex, write_atomic, HubState};
use crate::AppState;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tauri::State;

type R<T> = Result<T, String>;

const MAX_FILE: u64 = 25 * 1024 * 1024;
const MAX_FILES: usize = 5000;
const PREVIEW_TEXT: usize = 200 * 1024;

/// Same rule as the hub: relative, forward slashes, no "..", no drive/root, nothing inside .git.
fn rel_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 500
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.contains(':')
        && !p.chars().any(char::is_control)
        && p.split('/').all(|s| {
            // Windows drops trailing dots/spaces (".git." == ".git") and has 8.3 short names (GIT~1) and device names.
            let low = s.to_ascii_lowercase();
            let stem = low.split('.').next().unwrap_or_default();
            !s.is_empty() && !s.ends_with('.') && !s.ends_with(' ') && low != ".git" && !low.starts_with("git~")
                && !matches!(stem, "con" | "prn" | "aux" | "nul" | "com1" | "com2" | "com3" | "com4" | "com5" | "com6" | "com7" | "com8" | "com9" | "lpt1" | "lpt2" | "lpt3" | "lpt4" | "lpt5" | "lpt6" | "lpt7" | "lpt8" | "lpt9")
        })}

/// What a share run does: (path, existing record id if changed) to upload, and record ids to remove.
fn plan(local: &HashMap<String, String>, remote: &HashMap<String, (String, String)>) -> (Vec<(String, Option<String>)>, Vec<String>) {
    let mut up: Vec<(String, Option<String>)> = local
        .iter()
        .filter_map(|(p, sha)| match remote.get(p) {
            None => Some((p.clone(), None)),
            Some((id, s)) if s != sha => Some((p.clone(), Some(id.clone()))),
            _ => None,
        })
        .collect();
    up.sort();
    let gone = remote.iter().filter(|(p, _)| !local.contains_key(*p)).map(|(_, (id, _))| id.clone()).collect();
    (up, gone)
}

/// The workspace + hub project a local project is shared into.
fn link(h: &HubState, project: &str) -> R<(String, String)> {
    let l = h.client.config().map(|c| c["links"][project].clone()).filter(|l| l.is_object()).ok_or("Share this project with a workspace first (TEAM › Share a local project).")?;
    Ok((l["workspace"].as_str().unwrap_or_default().into(), l["project"].as_str().unwrap_or_default().into()))
}

/// The local project shared as this hub project, if it was shared from this PC.
fn local_of(h: &HubState, ws: &str, hub_project: &str) -> R<String> {
    let cfg = h.client.config().unwrap_or_default();
    cfg["links"].as_object().and_then(|l| l.iter().find(|(_, v)| v["workspace"] == ws && v["project"] == hub_project).map(|(k, _)| k.clone()))
        .ok_or_else(|| "Only the PC that shared this project can accept changes to it.".into())
}

struct Scan {
    local: HashMap<String, String>,
    sizes: HashMap<String, u64>,
    too_large: Vec<String>,
    excluded: usize,
    capped: bool,
}

fn scan(repo: &Path) -> R<Scan> {
    let (snap, excluded) = build_snapshot(repo)?;
    let mut s = Scan { local: HashMap::new(), sizes: HashMap::new(), too_large: vec![], excluded, capped: false };
    for f in snap["files"].as_array().into_iter().flatten() {
        let (p, size) = (f[0].as_str().unwrap_or_default(), f[1].as_u64().unwrap_or(0));
        if !rel_path(p) {
            continue;
        }
        if size > MAX_FILE {
            s.too_large.push(p.to_string());
            continue;
        }
        if s.local.len() >= MAX_FILES {
            s.capped = true;
            break;
        }
        let Ok(b) = std::fs::read(repo.join(p)) else { continue };
        s.sizes.insert(p.to_string(), b.len() as u64);
        s.local.insert(p.to_string(), sha256_hex(&b));
    }
    Ok(s)
}

fn remote(h: &HubState, ws: &str, project: &str) -> R<HashMap<String, (String, String)>> {
    Ok(h.client.records_of(ws, project, "file")?.into_iter()
        .map(|r| (r["fields"]["path"].as_str().unwrap_or_default().to_string(), (r["id"].as_str().unwrap_or_default().to_string(), r["fields"]["sha256"].as_str().unwrap_or_default().to_string())))
        .collect())
}

/// Exactly what sharing would send now, before anything is sent.
#[tauri::command(async)]
pub fn hub_files_preview(st: State<AppState>, h: State<HubState>, project: String) -> R<Value> {
    let (ws, hp) = link(&h, &project)?;
    let s = scan(&repo_of(&st, &project)?)?;
    let (up, gone) = plan(&s.local, &remote(&h, &ws, &hp)?);
    let bytes: u64 = up.iter().map(|(p, _)| s.sizes.get(p).copied().unwrap_or(0)).sum();
    Ok(json!({ "files": s.local.len(), "new": up.iter().filter(|u| u.1.is_none()).count(), "changed": up.iter().filter(|u| u.1.is_some()).count(),
               "removed": gone.len(), "upload_bytes": bytes, "excluded": s.excluded, "too_large": s.too_large, "capped": s.capped,
               "never": ["untracked or ignored files", "secret-looking files (.env, keys, credentials)", "files over 25 MB", "chats", "where the folder is on this PC"] }))
}

/// Shares (or refreshes) the project's files: uploads new and changed ones, removes ones no longer tracked.
#[tauri::command(async)]
pub fn hub_files_share(st: State<AppState>, h: State<HubState>, project: String) -> R<Value> {
    let (ws, hp) = link(&h, &project)?;
    let repo = repo_of(&st, &project)?;
    let s = scan(&repo)?;
    let (up, gone) = plan(&s.local, &remote(&h, &ws, &hp)?);
    for (path, rec) in &up {
        let b = std::fs::read(repo.join(path)).map_err(|e| format!("{path}: {e}"))?;
        match rec {
            None => { h.client.publish_file(&ws, &hp, "file", &b, json!({ "path": path }))?; }
            Some(id) => {
                let sha = h.client.put_blob(&ws, &hp, &b)?;
                h.client.edit(&ws, "update", "file", Some(id), &hp, &json!({ "sha256": sha }))?;
            }
        }
    }
    for id in &gone {
        h.client.edit(&ws, "delete", "file", Some(id), &hp, &Value::Null)?;
    }
    h.wake();
    Ok(json!({ "uploaded": up.len(), "removed": gone.len() }))
}

/// The shared files of a workspace project (from this PC's cache).
#[tauri::command(async)]
pub fn hub_files(h: State<HubState>, workspace: String, project: String) -> R<Vec<Value>> {
    h.client.records_of(&workspace, &project, "file")
}

/// A teammate's change (or new file) for the owner to review; `base` is the shared version it started from.
#[tauri::command(async)]
pub fn hub_propose(h: State<HubState>, workspace: String, project: String, path: String, base: Option<String>, note: String, data: String) -> R<String> {
    let path = path.trim().trim_start_matches("./").replace('\\', "/");
    if !rel_path(&path) || sensitive(&path) {
        return Err("Use a path inside the project, like art/logo.png (no .., no secret-looking names).".into());
    }
    let bytes = b64_decode(&data)?;
    let id = h.client.publish_file(&workspace, &project, "proposal", &bytes, json!({ "path": path, "base": base, "note": note.chars().take(1000).collect::<String>(), "status": "open" }))?;
    h.wake();
    Ok(id)
}

/// The repo file a proposal would replace, checked: a valid relative path, not secret-looking, and (after resolving
/// links) inside the repo.
fn target(repo: &Path, path: &str) -> R<PathBuf> {
    if !rel_path(path) || sensitive(path) {
        return Err("That proposal's path isn't allowed.".into());
    }
    let t = path.split('/').fold(repo.to_path_buf(), |p, s| p.join(s));
    let root = repo.canonicalize().map_err(|e| e.to_string())?;
    let mut probe = t.parent().map(Path::to_path_buf).unwrap_or_default();
    while !probe.exists() {
        probe = probe.parent().map(Path::to_path_buf).ok_or("bad path")?;
    }
    let real = probe.canonicalize().map_err(|e| e.to_string())?;
    if !real.starts_with(&root) || t.canonicalize().is_ok_and(|c| !c.starts_with(&root)) {
        return Err("That path leads outside the project.".into());
    }
    // As Windows resolved it (short names, case), nothing may land inside .git.
    let inside = |p: &Path| p.strip_prefix(&root).map(|r| r.components().any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case(".git"))).unwrap_or(true);
    if inside(&real) || t.canonicalize().is_ok_and(|c| inside(&c)) {
        return Err("That path isn't allowed.".into());
    }
    Ok(t)
}

fn proposal(h: &HubState, ws: &str, project: &str, id: &str) -> R<Value> {
    h.client.records_of(ws, project, "proposal")?.into_iter().find(|r| r["id"] == id).ok_or_else(|| "That proposal is gone.".into())
}

fn text(b: &[u8]) -> Option<String> {
    (b.len() <= PREVIEW_TEXT).then(|| String::from_utf8(b.to_vec()).ok()).flatten()
}

/// What accepting would change in the owner's repo.
#[tauri::command(async)]
pub fn hub_proposal_preview(st: State<AppState>, h: State<HubState>, workspace: String, project: String, id: String) -> R<Value> {
    let p = proposal(&h, &workspace, &project, &id)?;
    let path = p["fields"]["path"].as_str().unwrap_or_default().to_string();
    let t = target(&repo_of(&st, &local_of(&h, &workspace, &project)?)?, &path)?;
    let new = h.client.file(&workspace, &id)?;
    let old = std::fs::read(&t).ok();
    let cur = old.as_deref().map(sha256_hex);
    let base = p["fields"]["base"].as_str().map(String::from);
    Ok(json!({ "path": path, "exists": old.is_some(), "base_matches": cur == base, "old_size": old.as_ref().map(Vec::len), "new_size": new.len(),
               "old_text": old.as_deref().and_then(text), "new_text": text(&new), "same": cur.as_deref() == Some(sha256_hex(&new).as_str()) }))
}

/// Accept (writes the file into the repo, updates the shared copy) or decline. If the repo file changed since the
/// version the proposal started from, accepting needs `force`.
#[tauri::command(async)]
pub fn hub_proposal_decide(st: State<AppState>, h: State<HubState>, workspace: String, project: String, id: String, accept: bool, force: bool) -> R<()> {
    let p = proposal(&h, &workspace, &project, &id)?;
    if !accept {
        h.client.edit(&workspace, "update", "proposal", Some(&id), &project, &json!({ "status": "declined" }))?;
        h.wake();
        return Ok(());
    }
    let path = p["fields"]["path"].as_str().unwrap_or_default().to_string();
    let t = target(&repo_of(&st, &local_of(&h, &workspace, &project)?)?, &path)?;
    let new = h.client.file(&workspace, &id)?;
    let cur = std::fs::read(&t).ok().map(|b| sha256_hex(&b));
    if !force && cur.as_deref() != p["fields"]["base"].as_str() {
        return Err("Your copy of this file changed since the proposal was made. Review it, then Accept anyway.".into());
    }
    if let Some(dir) = t.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    write_atomic(&t, &new)?;
    h.client.edit(&workspace, "update", "proposal", Some(&id), &project, &json!({ "status": "accepted" }))?;
    let sha = sha256_hex(&new);
    match remote(&h, &workspace, &project)?.get(&path) {
        Some((rec, s)) if *s != sha => { h.client.edit(&workspace, "update", "file", Some(rec), &project, &json!({ "sha256": sha }))?; }
        Some(_) => {}
        None => { h.client.edit(&workspace, "create", "file", None, &project, &json!({ "path": path, "sha256": sha }))?; }
    }
    h.wake();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_plan_uploads_new_and_changed_and_removes_untracked() {
        let local: HashMap<_, _> = [("a.txt", "1"), ("b.txt", "2"), ("c.txt", "3")].iter().map(|(p, s)| (p.to_string(), s.to_string())).collect();
        let remote: HashMap<_, _> = [("b.txt", ("rb", "2")), ("c.txt", ("rc", "old")), ("gone.txt", ("rg", "9"))]
            .iter().map(|(p, (i, s))| (p.to_string(), (i.to_string(), s.to_string()))).collect();
        let (up, gone) = plan(&local, &remote);
        assert_eq!(up, vec![("a.txt".to_string(), None), ("c.txt".to_string(), Some("rc".to_string()))]);
        assert_eq!(gone, vec!["rg".to_string()]);
    }

    #[test]
    fn proposals_only_land_inside_the_repo() {
        let repo = std::env::temp_dir().join(format!("ptcc-files-{}", std::process::id()));
        std::fs::create_dir_all(repo.join("art")).unwrap();
        assert!(target(&repo, "art/logo.png").unwrap().starts_with(&repo));
        assert!(target(&repo, "new/dir/readme.md").is_ok(), "new folders are fine");
        std::fs::create_dir_all(repo.join(".git").join("hooks")).unwrap();
        for bad in ["../x", "/etc/passwd", "C:/x", ".git/config", "a\\b", ".env", "keys/server.pem", "my secrets.txt", ".git./hooks/pre-commit", ".git /config",
                    "GIT~1/config", "docs/con.txt", "nul", "a/./b", "x/../y", "file.txt."] {
            assert!(target(&repo, bad).is_err(), "{bad} must be refused");
        }
        let _ = std::fs::remove_dir_all(repo);
    }
}
