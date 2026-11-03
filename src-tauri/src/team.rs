//! Team profiles (name + PIN) and per-project action items, both kept in the shared Book of Secrets vault
//! so every teammate's app sees the same data. A PIN records who did what; it is not access control:
//! anyone who can edit the vault can edit these files.
use crate::db;
use crate::util::now_iso;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

type R<T> = Result<T, String>;

fn vault(conn: &Connection) -> R<PathBuf> {
    db::setting(conn, "vault_path")
        .and_then(|v| v.as_str().map(PathBuf::from))
        .filter(|p| p.is_dir())
        .ok_or_else(|| "No Book of Secrets vault is set. Choose it in Settings first.".into())
}

/// Write via a temp file + rename so a crash or sync never leaves half a file.
fn write_atomic(path: &Path, text: &str) -> R<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/* ---------- profiles ---------- */

#[derive(Serialize, Deserialize, Clone)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub color: String,
    salt: String,
    pin_hash: String,
    pub created: String,
}

impl Profile {
    pub fn public(&self) -> Value {
        json!({"id": self.id, "name": self.name, "color": self.color})
    }
}

fn team_file(conn: &Connection) -> R<PathBuf> {
    let dir = vault(conn)?.join("02 Projects").join("Papertrail Command Center");
    if !dir.is_dir() {
        return Err(format!("Team folder missing: {}", dir.display()));
    }
    Ok(dir.join("TEAM.json"))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn random_hex(n: usize) -> R<String> {
    let mut b = vec![0u8; n];
    getrandom::getrandom(&mut b).map_err(|e| e.to_string())?;
    Ok(hex(&b))
}

fn pin_hash(salt: &str, pin: &str) -> String {
    hex(&Sha256::digest(format!("ptcc:{salt}:{pin}").as_bytes()))
}

fn load_team(path: &Path) -> R<Vec<Profile>> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str::<Value>(&s)
            .ok()
            .and_then(|v| serde_json::from_value(v["profiles"].clone()).ok())
            .ok_or_else(|| format!("{} is not a valid team file", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e.to_string()),
    }
}

pub fn profiles(conn: &Connection) -> R<Vec<Profile>> {
    load_team(&team_file(conn)?)
}

pub fn create_profile(conn: &Connection, name: &str, pin: &str, color: &str) -> R<Profile> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 || name.chars().any(|c| c.is_control()) {
        return Err("Name must be 1–40 characters.".into());
    }
    if !(4..=8).contains(&pin.len()) || !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err("PIN must be 4–8 digits.".into());
    }
    let color = if color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit()) { color } else { "#a78bfa" };
    let path = team_file(conn)?;
    let mut team = load_team(&path)?;
    if team.iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
        return Err(format!("A profile named {name} already exists."));
    }
    let salt = random_hex(16)?;
    let p = Profile { id: random_hex(6)?, name: name.into(), color: color.into(), pin_hash: pin_hash(&salt, pin), salt, created: now_iso() };
    team.push(p.clone());
    write_atomic(&path, &serde_json::to_string_pretty(&json!({"note": "Papertrail Command Center team profiles. PINs are salted hashes.", "profiles": team})).unwrap())?;
    Ok(p)
}

pub fn verify(conn: &Connection, id: &str, pin: &str) -> R<Profile> {
    let p = profiles(conn)?.into_iter().find(|p| p.id == id).ok_or("Unknown profile.")?;
    if pin_hash(&p.salt, pin) == p.pin_hash {
        Ok(p)
    } else {
        Err("Wrong PIN.".into())
    }
}

/* ---------- action items ---------- */

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(default)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub notes: String,
    /// open | doing | done
    pub status: String,
    pub assignee: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub started_by: Option<String>,
    pub started_at: Option<String>,
    pub done_by: Option<String>,
    pub done_at: Option<String>,
    pub updated_by: String,
    pub updated_at: String,
    /// [when, who, what], newest last, capped.
    pub history: Vec<(String, String, String)>,
}

#[derive(Deserialize, Debug)]
pub struct Op {
    pub kind: String,
    pub id: Option<String>,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub assignee: Option<String>,
}

pub const FILE: &str = "ACTION ITEMS.md";
const MARK: &str = "<!-- ai ";

fn day(ts: &Option<String>) -> String {
    ts.as_deref().map(|t| t.chars().take(10).collect()).unwrap_or_default()
}

pub fn parse(md: &str) -> Vec<Item> {
    md.lines()
        .filter_map(|l| {
            let j = &l[l.find(MARK)? + MARK.len()..];
            serde_json::from_str(j[..j.rfind("-->")?].trim()).ok()
        })
        .collect()
}

/// Obsidian shows a normal checklist; the hidden comment on each line is the source of truth.
pub fn render(project: &str, items: &[Item]) -> String {
    let mut md = format!(
        "# ACTION ITEMS — {project}\n\n> Managed by Papertrail Command Center. One line per item; the hidden comment keeps its history. Edit items in the app.\n\n"
    );
    for it in items {
        let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let state = match it.status.as_str() {
            "done" => format!("✔ done by {} {}", it.done_by.clone().unwrap_or_default(), day(&it.done_at)),
            "doing" => format!("▶ started by {} {}", it.started_by.clone().unwrap_or_default(), day(&it.started_at)),
            _ => format!("added by {} {}", it.created_by, day(&Some(it.created_at.clone()))),
        };
        let who = it.assignee.as_deref().map(|a| format!(" · for {}", one_line(a))).unwrap_or_default();
        // `>` escaped so text can never close the comment early; still valid JSON.
        let data = serde_json::to_string(it).unwrap().replace('>', "\\u003e");
        md += &format!("- [{}] {} · {state}{who} {MARK}{data} -->\n", if it.status == "done" { "x" } else { " " }, one_line(&it.title));
    }
    md
}

/// Apply one operation to a fresh read of the file (so a teammate's synced edit is never overwritten wholesale).
pub fn apply(items: &mut Vec<Item>, op: &Op, by: &str, now: &str, new_id: String) -> R<()> {
    let clean = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    if op.kind == "add" {
        let title = clean(&op.title).ok_or("An action item needs a title.")?;
        items.push(Item {
            id: new_id,
            title,
            notes: clean(&op.notes).unwrap_or_default(),
            status: "open".into(),
            assignee: clean(&op.assignee),
            created_by: by.into(),
            created_at: now.into(),
            updated_by: by.into(),
            updated_at: now.into(),
            history: vec![(now.into(), by.into(), "added".into())],
            ..Default::default()
        });
        return Ok(());
    }
    let id = op.id.as_deref().ok_or("missing item id")?;
    let i = items.iter().position(|x| x.id == id).ok_or("That item was removed or changed by someone else. Reload.")?;
    if op.kind == "remove" {
        items.remove(i);
        return Ok(());
    }
    let it = &mut items[i];
    let what = match op.kind.as_str() {
        "edit" => {
            if let Some(t) = clean(&op.title) {
                it.title = t;
            }
            if op.notes.is_some() {
                it.notes = clean(&op.notes).unwrap_or_default();
            }
            if op.assignee.is_some() {
                it.assignee = clean(&op.assignee);
            }
            "edited"
        }
        "start" => {
            it.status = "doing".into();
            it.started_by = Some(by.into());
            it.started_at = Some(now.into());
            "started"
        }
        "complete" => {
            it.status = "done".into();
            it.done_by = Some(by.into());
            it.done_at = Some(now.into());
            "completed"
        }
        "reopen" => {
            it.status = "open".into();
            it.done_by = None;
            it.done_at = None;
            "reopened"
        }
        _ => return Err("unknown action".into()),
    };
    it.updated_by = by.into();
    it.updated_at = now.into();
    it.history.push((now.into(), by.into(), what.into()));
    if it.history.len() > 30 {
        it.history.remove(0);
    }
    Ok(())
}

pub fn file_for(vault_dir: Option<&str>) -> R<PathBuf> {
    vault_dir
        .map(PathBuf::from)
        .filter(|d| d.is_dir())
        .map(|d| d.join(FILE))
        .ok_or_else(|| "This project has no Book of Secrets folder, so it can't hold shared action items.".into())
}

pub fn read(path: &Path) -> R<Vec<Item>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(parse(&s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn run_op(path: &Path, project: &str, op: &Op, by: &str) -> R<Vec<Item>> {
    let mut items = read(path)?;
    apply(&mut items, op, by, &now_iso(), random_hex(5)?)?;
    write_atomic(path, &render(project, &items))?;
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn op(kind: &str, id: Option<&str>, title: Option<&str>) -> Op {
        Op { kind: kind.into(), id: id.map(String::from), title: title.map(String::from), notes: None, assignee: None }
    }
    #[test]
    fn lifecycle_roundtrip() {
        let mut v = vec![];
        apply(&mut v, &op("add", None, Some("Ship --> it <b>")), "Tev", "2026-11-03T10:00:00Z", "a1".into()).unwrap();
        apply(&mut v, &op("add", None, Some("Second")), "Sam", "2026-11-03T10:01:00Z", "b2".into()).unwrap();
        apply(&mut v, &op("start", Some("a1"), None), "Sam", "2026-11-03T11:00:00Z", "x".into()).unwrap();
        apply(&mut v, &op("complete", Some("a1"), None), "Tev", "2026-11-03T12:00:00Z", "x".into()).unwrap();
        apply(&mut v, &op("remove", Some("b2"), None), "Tev", "2026-11-03T12:00:00Z", "x".into()).unwrap();
        assert!(apply(&mut v, &op("add", None, Some("  ")), "Tev", "t", "c".into()).is_err());
        let md = render("Demo", &v);
        assert!(md.contains("- [x] Ship --> it <b> · ✔ done by Tev 2026-11-03"), "{md}");
        assert_eq!(md.matches("-->").count(), 2, "title text cannot close the hidden comment");
        let back = parse(&md);
        assert_eq!(back, v);
        assert_eq!(back[0].started_by.as_deref(), Some("Sam"));
        assert_eq!(back[0].history.len(), 3);
    }
    #[test]
    fn pins() {
        let s = "abc";
        assert_eq!(pin_hash(s, "1234"), pin_hash(s, "1234"));
        assert_ne!(pin_hash(s, "1234"), pin_hash("abd", "1234"));
    }
}
