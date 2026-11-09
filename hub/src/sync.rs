//! M2 shared records and sync (ARCHITECTURE.md §7): tasks, milestones and media in linked projects.
//! One op per request, applied in a transaction: authorize at apply time, validate fields against a per-kind allowlist,
//! merge disjoint fields, refuse same-field edits with 409 + the current record, delete beats edit, and remember the
//! reply per op_id so retries are idempotent. The change feed is scoped in SQL by the caller's projects and kinds.
use crate::api::{bad, db, fail, internal, log, member_role, membership, not_found, ok, Caller, Hub, Reply, Res};
use crate::store::{hash_bytes, now, random_hex};
use ptcc_access::{can_edit_item, Decision, Membership, Perm, Target};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};

pub const MAX_BLOB: usize = 25 * 1024 * 1024;
/// Must stay a subset of `ptcc_access::publishable` (checked in `op`).
const KINDS: [&str; 6] = ["task", "milestone", "media", "snapshot", "board", "board_el"];
/// One canvas element (D044) is about this size at most: a long freehand stroke or a big text block.
const BOARD_EL_MAX: usize = 256 * 1024;
/// Kinds whose content is an uploaded file named by `sha256`.
const FILE_KINDS: [&str; 2] = ["media", "snapshot"];
const FEED_MAX: i64 = 500;

/// Client-made ids are UUIDs (§7.1): 36 chars of hex and dashes.
fn uuid(s: &str) -> bool {
    s.len() == 36 && s.chars().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() })
}

fn sha_ok(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn text(v: &Value, min: usize, max: usize) -> bool {
    v.as_str().is_some_and(|s| (min..=max).contains(&s.trim().chars().count()) && !s.chars().any(|c| c.is_control() && c != '\n' && c != '\t'))
}

fn date_or_null(v: &Value) -> bool {
    v.is_null() || v.as_str().is_some_and(|s| s.len() == 10 && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok())
}

struct Row {
    ws: String,
    project: String,
    kind: String,
    fields: Map<String, Value>,
    fv: Map<String, Value>,
    version: i64,
    created_by: String,
    deleted: bool,
}

fn load(c: &Connection, id: &str) -> Result<Option<Row>, Reply> {
    db(c.query_row(
        "SELECT workspace_id, project_id, kind, fields, field_versions, version, created_by, deleted FROM records WHERE id = ?1",
        [id],
        |r| {
            let obj = |s: String| serde_json::from_str::<Map<String, Value>>(&s).unwrap_or_default();
            Ok(Row { ws: r.get(0)?, project: r.get(1)?, kind: r.get(2)?, fields: obj(r.get(3)?), fv: obj(r.get(4)?), version: r.get(5)?, created_by: r.get(6)?, deleted: r.get(7)? })
        },
    )
    .optional())
}

const RECORD_COLS: &str = "r.id, r.kind, r.project_id, r.version, r.fields, r.created_by, r.updated_by, r.updated_at, r.deleted, r.seq";

fn record_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": r.get::<_, String>(0)?, "kind": r.get::<_, String>(1)?, "project_id": r.get::<_, String>(2)?,
        "version": r.get::<_, i64>(3)?, "fields": serde_json::from_str::<Value>(&r.get::<_, String>(4)?).unwrap_or_default(),
        "created_by": r.get::<_, String>(5)?, "updated_by": r.get::<_, String>(6)?, "updated_at": r.get::<_, String>(7)?,
        "deleted": r.get::<_, bool>(8)?, "seq": r.get::<_, i64>(9)?,
    }))
}

fn current(c: &Connection, id: &str) -> Result<Value, Reply> {
    db(c.query_row(&format!("SELECT {RECORD_COLS} FROM records r WHERE r.id = ?1"), [id], record_json))
}

/// Field-level allowlist per kind (§3 rule 2): anything not listed is refused, so nothing private rides along.
fn validate(c: &Connection, ws: &str, project: &str, kind: &str, f: &Map<String, Value>, create: bool) -> Result<(), Reply> {
    for (k, v) in f {
        let good = match (kind, k.as_str()) {
            (_, "title") => text(v, 1, 200),
            ("task", "status") => matches!(v.as_str(), Some("todo" | "doing" | "blocked" | "done")),
            ("task", "assignee") => v.is_null() || v.as_str().is_some_and(|u| member_role(c, ws, u).ok().flatten().is_some()),
            ("task" | "milestone", "due") => date_or_null(v),
            ("task", "notes") => text(v, 0, 4000),
            ("task", "milestone") => {
                v.is_null()
                    || v.as_str().is_some_and(|m| {
                        load(c, m).ok().flatten().is_some_and(|r| r.kind == "milestone" && r.ws == ws && r.project == project && !r.deleted)
                    })
            }
            ("milestone", "status") => matches!(v.as_str(), Some("open" | "done")),
            ("milestone", "progress") => v.as_i64().is_some_and(|p| (0..=100).contains(&p)),
            ("media", "sha256") => create && v.as_str().is_some_and(sha_ok),
            ("media", "name") => text(v, 1, 200),
            ("media", "caption") => text(v, 0, 1000),
            ("media", "folder") => text(v, 0, 80),
            // Snapshot (D035): the file list + map live in the uploaded JSON; the record carries only counts.
            ("snapshot", "sha256") => create && v.as_str().is_some_and(sha_ok),
            ("snapshot", "files" | "dirs") => v.as_i64().is_some_and(|n| (0..=1_000_000).contains(&n)),
            ("snapshot", "note") => text(v, 0, 500),
            // Boards (D044): a board names its background; each canvas element is its own record holding the
            // element's JSON, in a board of the same project.
            ("board", "bg") => matches!(v.as_str(), Some("grid" | "blackboard")),
            ("board_el", "board") => {
                create
                    && v.as_str().is_some_and(|b| load(c, b).ok().flatten().is_some_and(|r| r.kind == "board" && r.ws == ws && r.project == project && !r.deleted))
            }
            ("board_el", "data") => v.as_str().is_some_and(|d| d.len() <= BOARD_EL_MAX && serde_json::from_str::<Value>(d).is_ok_and(|j| j.is_object())),
            _ => return Err(bad(&format!("field {k} is not allowed on a {kind}"))),
        };
        if !good {
            return Err(bad(&format!("invalid {k}")));
        }
    }
    let required: &[&str] = match (kind, create) {
        ("media", true) => &["sha256", "name"],
        ("snapshot", true) => &["sha256", "files", "dirs"],
        ("board_el", true) => &["board", "data"],
        (_, true) => &["title"],
        _ => &[],
    };
    match required.iter().find(|k| !f.contains_key(**k)) {
        Some(k) => Err(bad(&format!("{k} is required"))),
        None => Ok(()),
    }
}

impl Hub {
    fn allow(&self, c: &Connection, who: &Caller, ws: &str, d: Decision, action: &str) -> Result<(), Reply> {
        match d {
            Decision::Allow => Ok(()),
            d => Err(self.refuse(c, who, ws, action, d)),
        }
    }

    /// Checked at apply time, never at queue time (§7.5), so a revoked or re-scoped member's queued ops fail here.
    #[allow(clippy::too_many_arguments)]
    fn authorize(&self, c: &Connection, who: &Caller, m: Option<&Membership>, ws: &str, project: &str, kind: &str, created_by: Option<&str>, f: &Map<String, Value>) -> Result<(), Reply> {
        let proj = Target::Project { workspace: ws, project };
        let ck = |p: Perm, t: Target, a: &str| self.check(c, who, m, p, t, a);
        match (kind, created_by) {
            ("milestone", _) => ck(Perm::MilestoneEdit, proj, "milestone.edit")?,
            ("snapshot", _) => ck(Perm::DocPublish, proj, "doc.publish")?,
            // Anyone who may add media to the project draws on its boards, on anyone's elements: a shared canvas.
            ("board_el", _) => ck(Perm::MediaUpload, proj, "board.draw")?,
            ("task", None) => ck(Perm::TaskCreate, proj, "task.create")?,
            (_, None) => ck(Perm::MediaUpload, proj, "media.upload")?,
            (_, Some(by)) => {
                if kind == "media" || kind == "board" {
                    ck(Perm::MediaUpload, proj, "media.upload")?;
                }
                self.allow(c, who, ws, can_edit_item(m, ws, project, by), "task.edit")?;
            }
        }
        if kind == "task" && f.contains_key("assignee") {
            ck(Perm::TaskAssign, Target::Item { workspace: ws, project, created_by: created_by.unwrap_or(&who.user) }, "task.assign")?;
        }
        Ok(())
    }

    pub(crate) fn op(&self, c: &mut Connection, who: &Caller, ws: &str, b: &Value) -> Res {
        let m = membership(c, ws, &who.user);
        self.check(c, who, m.as_ref(), Perm::WorkspaceRead, Target::Workspace(ws), "record.write")?;
        let op_id = b["op_id"].as_str().filter(|s| uuid(s)).ok_or_else(|| bad("op_id must be a UUID"))?;
        let seen: Option<(String, u16, String)> =
            db(c.query_row("SELECT user_id, code, reply FROM applied_ops WHERE op_id = ?1", [op_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional())?;
        if let Some((user, code, reply)) = seen {
            if user != who.user {
                return Err(fail(409, "op_id_taken"));
            }
            return Ok(Reply { code, body: serde_json::from_str(&reply).unwrap_or_default(), raw: None });
        }
        let kind = b["kind"].as_str().filter(|k| KINDS.contains(k) && ptcc_access::publishable(k).is_some()).ok_or_else(|| bad("unknown kind"))?;
        let id = b["record_id"].as_str().filter(|s| uuid(s)).ok_or_else(|| bad("record_id must be a UUID"))?;
        let empty = Map::new();
        let f = match &b["fields"] {
            Value::Null => &empty,
            Value::Object(o) => o,
            _ => return Err(bad("fields must be an object")),
        };
        // Phase 1, reads only (the hub's single connection lock makes this race-free): authorize, validate and decide
        // the write. Refusals are audited outside any transaction, so they aren't rolled back.
        enum Write {
            Insert(String, Map<String, Value>),
            Update(Map<String, Value>, Map<String, Value>, i64),
            Tombstone(String),
            Nothing,
        }
        let existing = load(c, id)?;
        let (write, conflict) = match b["action"].as_str() {
            Some("create") => {
                let project = b["project_id"].as_str().unwrap_or("");
                self.authorize(c, who, m.as_ref(), ws, project, kind, None, f)?;
                let n: i64 = db(c.query_row("SELECT COUNT(*) FROM projects WHERE id = ?1 AND workspace_id = ?2 AND deleted = 0", params![project, ws], |r| r.get(0)))?;
                if n == 0 {
                    return Err(not_found());
                }
                if existing.is_some() {
                    return Err(fail(409, "id_taken"));
                }
                validate(c, ws, project, kind, f, true)?;
                let mut fields = f.clone();
                if FILE_KINDS.contains(&kind) {
                    let sha = f["sha256"].as_str().unwrap_or("");
                    // Only a file uploaded into this same project counts; a hash seen elsewhere grants nothing.
                    let size: Option<i64> = db(c
                        .query_row("SELECT size FROM blobs WHERE sha256 = ?1 AND workspace_id = ?2 AND project_id = ?3", params![sha, ws, project], |r| r.get(0))
                        .optional())?;
                    let on_disk = self.media.as_ref().is_some_and(|d| d.join(sha).exists());
                    let Some(size) = size.filter(|_| on_disk) else { return Err(bad("upload the file (PUT …/blobs/<sha256>) to this project before creating its record")) };
                    fields.insert("size".into(), json!(size));
                }
                (Write::Insert(project.to_string(), fields), None)
            }
            Some(action @ ("update" | "delete")) => {
                let row = existing.filter(|r| r.ws == ws).ok_or_else(not_found)?;
                if row.kind != kind {
                    return Err(bad("kind does not match the record"));
                }
                // Claiming (D035): anyone who may add tasks can put themselves on an unclaimed task or take
                // themselves off; assigning other people or taking over a claim still needs task.assign.
                let held = row.fields.get("assignee").and_then(Value::as_str);
                let claim = kind == "task"
                    && action == "update"
                    && f.len() == 1
                    && f.get("assignee").is_some_and(|v| (v.as_str() == Some(who.user.as_str()) && held.is_none()) || (v.is_null() && held == Some(who.user.as_str())));
                if claim {
                    self.check(c, who, m.as_ref(), Perm::TaskCreate, Target::Item { workspace: ws, project: &row.project, created_by: &row.created_by }, "task.claim")?;
                } else {
                    self.authorize(c, who, m.as_ref(), ws, &row.project, kind, Some(&row.created_by), if action == "delete" { &empty } else { f })?;
                }
                if action == "delete" {
                    (if row.deleted { Write::Nothing } else { Write::Tombstone(row.project) }, None)
                } else if row.deleted {
                    (Write::Nothing, Some(json!({ "error": "deleted" })))
                } else if kind == "board_el"
                    && !row.fields.get("board").and_then(Value::as_str).is_some_and(|b| load(c, b).ok().flatten().is_some_and(|p| p.kind == "board" && !p.deleted))
                {
                    // Its board was deleted: the element is gone too (and its tombstone is on the way).
                    (Write::Nothing, Some(json!({ "error": "deleted" })))
                } else {
                    let base = b["base_version"].as_i64().filter(|v| (1..=row.version).contains(v)).ok_or_else(|| bad("base_version must be a version this record had"))?;
                    validate(c, ws, &row.project, kind, f, false)?;
                    // A field clashes when another device changed it after the version this edit started from. Ops
                    // queued on one device are sent in order, so a device never conflicts with its own earlier edits.
                    // Canvas elements are last-writer-wins: two people moving one shape is not worth a conflict screen.
                    let clash: Vec<&String> = f
                        .keys()
                        .filter(|_| kind != "board_el")
                        .filter(|k| row.fv.get(*k).is_some_and(|e| e[0].as_i64().unwrap_or(0) > base && e[1].as_str() != Some(&who.device)))
                        .collect();
                    if !clash.is_empty() {
                        (Write::Nothing, Some(json!({ "error": "conflict", "fields": clash })))
                    } else {
                        let (mut fields, mut fv, v) = (row.fields, row.fv, row.version + 1);
                        for (k, val) in f {
                            fields.insert(k.clone(), val.clone());
                            fv.insert(k.clone(), json!([v, who.device]));
                        }
                        (Write::Update(fields, fv, v), None)
                    }
                }
            }
            _ => return Err(bad("action must be create, update or delete")),
        };
        // Phase 2: the write, its audit entry and the remembered reply commit together, so a crash can't apply an op
        // without recording it (a retry would then conflict with itself).
        let tx = db(c.transaction())?;
        let seq: i64 = db(tx.query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM records", [], |r| r.get(0)))?;
        let t = now();
        match write {
            Write::Insert(project, fields) => {
                let fv: Map<String, Value> = fields.keys().map(|k| (k.clone(), json!([1, who.device]))).collect();
                db(tx.execute(
                    "INSERT INTO records(id, workspace_id, project_id, kind, fields, field_versions, version, created_by, updated_by, updated_at, seq) VALUES(?1,?2,?3,?4,?5,?6,1,?7,?7,?8,?9)",
                    params![id, ws, project, kind, Value::Object(fields).to_string(), Value::Object(fv).to_string(), who.user, t, seq],
                ))?;
            }
            Write::Update(fields, fv, v) => {
                db(tx.execute(
                    "UPDATE records SET fields = ?1, field_versions = ?2, version = ?3, updated_by = ?4, updated_at = ?5, seq = ?6 WHERE id = ?7",
                    params![Value::Object(fields).to_string(), Value::Object(fv).to_string(), v, who.user, t, seq, id],
                ))?;
            }
            Write::Tombstone(project) => {
                // No content stays readable through the feed.
                db(tx.execute(
                    "UPDATE records SET fields = '{}', field_versions = '{}', deleted = 1, version = version + 1, updated_by = ?1, updated_at = ?2, seq = ?3 WHERE id = ?4",
                    params![who.user, t, seq, id],
                ))?;
                log(&tx, Some(ws), who, "record.deleted", Some(id), json!({ "kind": kind, "project": project }))?;
                // A deleted board takes its canvas elements with it: none of their content stays readable.
                if kind == "board" {
                    let els: Vec<String> = db(tx
                        .prepare("SELECT id FROM records WHERE kind = 'board_el' AND deleted = 0 AND json_extract(fields, '$.board') = ?1")
                        .and_then(|mut st| st.query_map([id], |r| r.get(0))?.collect()))?;
                    for (i, el) in els.iter().enumerate() {
                        db(tx.execute(
                            "UPDATE records SET fields = '{}', field_versions = '{}', deleted = 1, version = version + 1, updated_by = ?1, updated_at = ?2, seq = ?3 WHERE id = ?4",
                            params![who.user, t, seq + 1 + i as i64, el],
                        ))?;
                    }
                }
            }
            Write::Nothing => {}
        }
        let record = current(&tx, id)?;
        let reply = match conflict {
            Some(mut body) => {
                body["record"] = record;
                Reply { code: 409, body, raw: None }
            }
            None => ok(json!({ "record": record })),
        };
        // Results that describe state are replayed to retries; refusals are re-evaluated (rights can change).
        db(tx.execute(
            "INSERT INTO applied_ops(op_id, user_id, workspace_id, code, reply, at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![op_id, who.user, ws, reply.code, reply.body.to_string(), t],
        ))?;
        db(tx.commit())?;
        Ok(reply)
    }

    /// `GET …/changes?since=SEQ&limit=N`: records (latest state, tombstones included) changed after `since` in projects
    /// and kinds the caller may read, plus the authorized project ids. A client purges cached projects missing from
    /// that list and resyncs from 0 when a new one appears (§7.5).
    pub(crate) fn changes(&self, c: &Connection, who: &Caller, ws: &str, query: &str) -> Res {
        let m = membership(c, ws, &who.user);
        self.check(c, who, m.as_ref(), Perm::WorkspaceRead, Target::Workspace(ws), "changes.read")?;
        let m = m.expect("allowed implies membership");
        let q = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(k)?.strip_prefix('=')).and_then(|v| v.parse::<i64>().ok());
        let since = q("since").unwrap_or(0).max(0);
        let limit = q("limit").unwrap_or(200).clamp(1, FEED_MAX);
        let mut kinds = Vec::new();
        if m.has(Perm::ProjectRead) && m.has(Perm::TaskRead) {
            kinds.extend(["task", "milestone"]);
        }
        if m.has(Perm::ProjectRead) && m.has(Perm::MediaRead) {
            kinds.extend(["media", "board", "board_el"]);
        }
        if m.has(Perm::ProjectRead) && m.has(Perm::DocRead) {
            kinds.push("snapshot");
        }
        let mut st = db(c.prepare(&format!(
            "SELECT {RECORD_COLS} FROM records r
             JOIN memberships m ON m.workspace_id = r.workspace_id AND m.user_id = ?2 AND m.revoked = 0
             JOIN projects p ON p.id = r.project_id AND p.deleted = 0
             WHERE r.workspace_id = ?1 AND r.seq > ?3 AND r.kind IN (SELECT value FROM json_each(?4))
               AND (m.scope IS NULL OR r.project_id IN (SELECT value FROM json_each(m.scope)))
             ORDER BY r.seq LIMIT ?5"
        )))?;
        let mut list: Vec<Value> = db(st.query_map(params![ws, who.user, since, json!(kinds).to_string(), limit + 1], record_json).and_then(Iterator::collect))?;
        let more = list.len() as i64 > limit;
        list.truncate(limit as usize);
        let next = list.last().and_then(|r| r["seq"].as_i64()).unwrap_or(since);
        let projects = self.projects(c, who, ws)?.body["projects"].clone();
        Ok(ok(json!({ "changes": list, "next": next, "more": more, "projects": projects })))
    }

    /// `PUT …/projects/{p}/blobs/{sha256}`: raw bytes, stored once under their hash. Only reachable through a media
    /// record in a project the caller can read.
    pub(crate) fn put_blob(&self, c: &Connection, who: &Caller, ws: &str, project: &str, sha: &str, body: &[u8]) -> Res {
        let m = membership(c, ws, &who.user);
        self.check(c, who, m.as_ref(), Perm::MediaUpload, Target::Project { workspace: ws, project }, "media.upload")?;
        let n: i64 = db(c.query_row("SELECT COUNT(*) FROM projects WHERE id = ?1 AND workspace_id = ?2 AND deleted = 0", params![project, ws], |r| r.get(0)))?;
        if n == 0 {
            return Err(not_found());
        }
        let dir = self.media.as_ref().ok_or_else(|| fail(503, "media_off"))?;
        // "auto": the hub fingerprints the upload itself (phones on plain http have no crypto.subtle).
        let computed;
        let sha = if sha == "auto" {
            computed = hash_bytes(body);
            computed.as_str()
        } else {
            sha
        };
        if !sha_ok(sha) {
            return Err(bad("blob name must be a lowercase sha256"));
        }
        if body.len() > MAX_BLOB {
            return Err(fail(413, "too_large"));
        }
        if hash_bytes(body) != sha {
            return Err(bad("hash_mismatch"));
        }
        let path = dir.join(sha);
        if !path.exists() {
            std::fs::create_dir_all(dir).map_err(|e| internal(e.to_string()))?;
            let part = dir.join(format!("{sha}.part-{}", random_hex(4).map_err(internal)?));
            std::fs::write(&part, body).and_then(|_| std::fs::rename(&part, &path)).map_err(|e| {
                let _ = std::fs::remove_file(&part);
                internal(e.to_string())
            })?;
        }
        db(c.execute(
            "INSERT OR IGNORE INTO blobs(sha256, workspace_id, project_id, size, uploaded_by, uploaded_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![sha, ws, project, body.len() as i64, who.user, now()],
        ))?;
        Ok(ok(json!({ "sha256": sha, "size": body.len() })))
    }

    /// `GET …/media/{record_id}`: the file of a live media record (media.read) or snapshot (doc.read) on its project.
    pub(crate) fn get_media(&self, c: &Connection, who: &Caller, ws: &str, id: &str) -> Res {
        let m = membership(c, ws, &who.user);
        let row = load(c, id)?.filter(|r| r.ws == ws && FILE_KINDS.contains(&r.kind.as_str()));
        let perm = if row.as_ref().is_some_and(|r| r.kind == "snapshot") { Perm::DocRead } else { Perm::MediaRead };
        // Unknown ids are checked against the workspace so they refuse exactly like hidden ones.
        let project = row.as_ref().map(|r| r.project.clone()).unwrap_or_default();
        self.check(c, who, m.as_ref(), perm, Target::Project { workspace: ws, project: &project }, "media.read")?;
        let row = row.filter(|r| !r.deleted).ok_or_else(not_found)?;
        let sha = row.fields.get("sha256").and_then(Value::as_str).filter(|s| sha_ok(s)).ok_or_else(not_found)?;
        // Defence in depth: the file must have been uploaded into the record's own project.
        let linked: i64 = db(c.query_row("SELECT COUNT(*) FROM blobs WHERE sha256 = ?1 AND workspace_id = ?2 AND project_id = ?3", params![sha, ws, row.project], |r| r.get(0)))?;
        if linked == 0 {
            return Err(not_found());
        }
        let bytes = self.media.as_ref().and_then(|d| std::fs::read(d.join(sha)).ok()).ok_or_else(not_found)?;
        Ok(Reply { code: 200, body: json!({}), raw: Some(bytes) })
    }
}
