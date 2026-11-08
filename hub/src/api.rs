//! Hub HTTP API (ARCHITECTURE.md §4, §5, §11; records and sync in sync.rs): accounts by invitation, device sessions, workspaces, projects,
//! members, invitations and the audit log. Every route except health and redeem needs a device bearer, and every
//! workspace route asks `access::decide` before touching data. NotFound and "doesn't exist" give the same reply.
use crate::store::{err, hash, new_id, now, random_hex, R};
use ptcc_access::{decide, Decision, Membership, Perm, Role, Target};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Hub {
    /// shortcut: one connection for reads and writes; add a read pool if a team ever outgrows it (§10).
    pub db: Mutex<Connection>,
    /// Content-addressed media files (sha256 names); None = media uploads off.
    pub media: Option<std::path::PathBuf>,
    denials: Mutex<HashMap<String, Instant>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub code: u16,
    pub body: Value,
    /// A file download instead of JSON.
    pub raw: Option<Vec<u8>>,
}

pub(crate) fn ok(body: Value) -> Reply {
    Reply { code: 200, body, raw: None }
}
pub fn fail(code: u16, error: &str) -> Reply {
    Reply { code, body: json!({ "error": error }), raw: None }
}
pub(crate) fn not_found() -> Reply {
    fail(404, "not_found")
}
pub(crate) fn bad(msg: &str) -> Reply {
    fail(400, msg)
}
pub(crate) fn internal(e: String) -> Reply {
    eprintln!("ptcc-hub: internal error: {e}");
    fail(500, "internal")
}

pub(crate) type Res = Result<Reply, Reply>;

pub(crate) struct Caller {
    pub(crate) user: String,
    pub(crate) device: String,
}

impl Hub {
    pub fn new(c: Connection, media: Option<std::path::PathBuf>) -> Hub {
        Hub { db: Mutex::new(c), media, denials: Mutex::new(HashMap::new()) }
    }
}

const MAX_NAME: usize = 80;

fn name_arg(b: &Value, k: &str) -> Result<String, Reply> {
    let s = b[k].as_str().map(str::trim).unwrap_or("");
    if s.is_empty() || s.chars().count() > MAX_NAME || s.chars().any(char::is_control) {
        return Err(bad(&format!("{k} must be 1-{MAX_NAME} characters")));
    }
    Ok(s.to_string())
}

pub(crate) fn db<T>(r: rusqlite::Result<T>) -> Result<T, Reply> {
    r.map_err(|e| internal(e.to_string()))
}

pub fn audit(c: &Connection, ws: Option<&str>, caller: Option<(&str, &str)>, action: &str, target: Option<&str>, detail: Value) -> R<()> {
    c.execute(
        "INSERT INTO audit(ts, workspace_id, actor_user, actor_device, action, target, detail) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![now(), ws, caller.map(|c| c.0), caller.map(|c| c.1), action, target, detail.to_string()],
    )
    .map(|_| ())
    .map_err(err)
}

pub(crate) fn log(c: &Connection, ws: Option<&str>, who: &Caller, action: &str, target: Option<&str>, detail: Value) -> Result<(), Reply> {
    audit(c, ws, Some((&who.user, &who.device)), action, target, detail).map_err(internal)
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Bearer `<device id>.<secret>`; revoked devices and revoked users get nothing.
fn authenticate(c: &Connection, auth: Option<&str>) -> Option<Caller> {
    let (id, secret) = auth?.strip_prefix("Bearer ")?.trim().split_once('.')?;
    let (user, stored): (String, String) = c
        .query_row(
            "SELECT d.user_id, d.secret_hash FROM devices d JOIN users u ON u.id = d.user_id WHERE d.id = ?1 AND d.revoked = 0 AND u.revoked = 0",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()?;
    if !ct_eq(&hash(secret), &stored) {
        return None;
    }
    let _ = c.execute("UPDATE devices SET last_seen = ?1 WHERE id = ?2", params![now(), id]);
    Some(Caller { user, device: id.to_string() })
}

fn json_set<T, F: Fn(&str) -> Option<T>>(s: &str, parse: F) -> BTreeSet<T>
where
    T: Ord,
{
    serde_json::from_str::<Vec<String>>(s).unwrap_or_default().iter().filter_map(|x| parse(x)).collect()
}

/// The caller's active membership in a live workspace.
pub(crate) fn membership(c: &Connection, ws: &str, user: &str) -> Option<Membership> {
    // A named role adds its own extra/removed permissions to the member's (deny still beats grant).
    c.query_row(
        "SELECT m.role, m.grants, m.denies, m.scope, coalesce(r.grants, '[]'), coalesce(r.denies, '[]')
         FROM memberships m JOIN workspaces w ON w.id = m.workspace_id AND w.deleted = 0
         LEFT JOIN roles r ON r.workspace_id = m.workspace_id AND r.name = m.title
         WHERE m.workspace_id = ?1 AND m.user_id = ?2 AND m.revoked = 0",
        params![ws, user],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?)),
    )
    .ok()
    .and_then(|(role, g, d, scope, rg, rd)| {
        let mut m = Membership::new(user, ws, Role::parse(&role)?);
        m.grant = json_set(&g, Perm::parse);
        m.grant.extend(json_set(&rg, Perm::parse));
        m.deny = json_set(&d, Perm::parse);
        m.deny.extend(json_set(&rd, Perm::parse));
        m.projects = scope.map(|s| json_set(&s, |x| Some(x.to_string())));
        Some(m)
    })
}

/// A member row as the caller's `decide` target needs it (active members only).
pub(crate) fn member_role(c: &Connection, ws: &str, user: &str) -> Result<Option<Role>, Reply> {
    let r: Option<String> = db(c
        .query_row("SELECT role FROM memberships WHERE workspace_id = ?1 AND user_id = ?2 AND revoked = 0", params![ws, user], |r| r.get(0))
        .optional())?;
    Ok(r.and_then(|r| Role::parse(&r)))
}

impl Hub {
    /// Refusal reply; denials are audited at most once a minute per device and action (§11).
    pub(crate) fn refuse(&self, c: &Connection, who: &Caller, ws: &str, action: &str, d: Decision) -> Reply {
        let key = format!("{}|{ws}|{action}", who.device);
        let mut seen = self.denials.lock().unwrap_or_else(|e| e.into_inner());
        if seen.get(&key).is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
            seen.insert(key, Instant::now());
            seen.retain(|_, t| t.elapsed() < Duration::from_secs(60));
            let _ = audit(c, Some(ws), Some((&who.user, &who.device)), "access.denied", None, json!({ "action": action }));
        }
        match d {
            Decision::Forbidden => fail(403, "forbidden"),
            _ => not_found(),
        }
    }

    pub(crate) fn check(&self, c: &Connection, who: &Caller, m: Option<&Membership>, p: Perm, t: Target, action: &str) -> Result<(), Reply> {
        match decide(m, p, t) {
            Decision::Allow => Ok(()),
            d => Err(self.refuse(c, who, t_ws(&t), action, d)),
        }
    }
}

fn t_ws<'a>(t: &Target<'a>) -> &'a str {
    match *t {
        Target::Workspace(w) | Target::Project { workspace: w, .. } | Target::Item { workspace: w, .. } | Target::Member { workspace: w, .. } => w,
    }
}

/// `projects` from a request: absent/null = every project; else ids that must all exist in the workspace.
fn scope_arg(c: &Connection, ws: &str, v: &Value) -> Result<Option<String>, Reply> {
    if v.is_null() {
        return Ok(None);
    }
    let ids: Vec<String> = serde_json::from_value(v.clone()).map_err(|_| bad("projects must be a list of project ids or null"))?;
    if ids.is_empty() {
        return Err(bad("projects must not be empty (use null for every project)"));
    }
    for id in &ids {
        let n: i64 = db(c.query_row("SELECT COUNT(*) FROM projects WHERE id = ?1 AND workspace_id = ?2 AND deleted = 0", params![id, ws], |r| r.get(0)))?;
        if n == 0 {
            return Err(bad("unknown project id"));
        }
    }
    let set: BTreeSet<String> = ids.into_iter().collect();
    Ok(Some(json!(set).to_string()))
}

fn perms_arg(v: &Value, grant: bool) -> Result<String, Reply> {
    let names: Vec<String> = serde_json::from_value(v.clone()).map_err(|_| bad("permissions must be a list of names"))?;
    let mut set = BTreeSet::new();
    for n in names {
        let p = Perm::parse(&n).ok_or_else(|| bad("unknown permission"))?;
        if grant && !p.grantable() {
            return Err(bad("that permission comes only from the role and can't be granted"));
        }
        set.insert(p.name());
    }
    Ok(json!(set).to_string())
}

pub fn new_device(c: &Connection, user: &str, name: &str) -> R<(String, String)> {
    let id = new_id("d")?;
    let secret = random_hex(32)?;
    c.execute("INSERT INTO devices(id, user_id, name, secret_hash, created_at) VALUES(?1,?2,?3,?4,?5)", params![id, user, name, hash(&secret), now()]).map_err(err)?;
    Ok((id.clone(), format!("{id}.{secret}")))
}

/// Every new workspace starts with these named roles (owner's choice, D036): Ops runs the work, Artist makes it.
/// id, workspace, role, scope, title of a redeemable invitation.
type InviteRow = (String, String, String, Option<String>, Option<String>);

pub const STARTER_ROLES: [(&str, &str, &str); 2] = [("Ops", "editor", "[\"project.link\"]"), ("Artist", "contributor", "[]")];

/// Named role lookup: (base level, whether it exists).
fn role_preset(c: &Connection, ws: &str, name: &str) -> Result<Option<Role>, Reply> {
    let base: Option<String> = db(c.query_row("SELECT base FROM roles WHERE workspace_id = ?1 AND name = ?2", params![ws, name], |r| r.get(0)).optional())?;
    Ok(base.and_then(|b| Role::parse(&b)))
}

fn role_name_arg(b: &Value) -> Result<String, Reply> {
    let n = b["name"].as_str().map(str::trim).unwrap_or("");
    let ok = (1..=40).contains(&n.chars().count())
        && n.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        && Role::parse(&n.to_lowercase()).is_none();
    if !ok {
        return Err(bad("a role name is 1-40 letters, digits, spaces, - or _, and not one of the built-in roles"));
    }
    Ok(n.to_string())
}

/// Host-side bootstrap (CLI on the hub machine): a workspace plus a single-use owner invitation, token shown once.
pub fn new_workspace(c: &Connection, name: &str) -> R<(String, String)> {
    let ws = new_id("w")?;
    c.execute("INSERT INTO workspaces(id, name, created_at) VALUES(?1,?2,?3)", params![ws, name, now()]).map_err(err)?;
    for (role, base, grants) in STARTER_ROLES {
        c.execute("INSERT INTO roles(workspace_id, name, base, grants, created_at) VALUES(?1,?2,?3,?4,?5)", params![ws, role, base, grants, now()]).map_err(err)?;
    }
    let token = create_invitation(c, &ws, Role::Owner, None, 72, None)?.1;
    audit(c, Some(&ws), None, "workspace.created", Some(&ws), json!({ "by": "host" }))?;
    Ok((ws, token))
}

fn create_invitation(c: &Connection, ws: &str, role: Role, scope: Option<String>, hours: i64, by: Option<&str>) -> R<(String, String, String)> {
    create_invitation_titled(c, ws, role, None, scope, hours, by)
}

fn create_invitation_titled(c: &Connection, ws: &str, role: Role, title: Option<&str>, scope: Option<String>, hours: i64, by: Option<&str>) -> R<(String, String, String)> {
    let id = new_id("i")?;
    let token = random_hex(32)?;
    let expires = (chrono::Utc::now() + chrono::Duration::hours(hours)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
    c.execute(
        "INSERT INTO invitations(id, token_hash, workspace_id, role, scope, expires_at, created_by, created_at, title) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![id, hash(&token), ws, role.name(), scope, expires, by, now(), title],
    )
    .map_err(err)?;
    Ok((id, token, expires))
}

/// Host-side: revoking a user revokes every device they have (§4).
pub fn revoke_user(c: &Connection, user: &str) -> R<bool> {
    let n = c.execute("UPDATE users SET revoked = 1 WHERE id = ?1", [user]).map_err(err)?;
    c.execute("UPDATE devices SET revoked = 1 WHERE user_id = ?1", [user]).map_err(err)?;
    if n > 0 {
        audit(c, None, None, "user.revoked", Some(user), json!({ "by": "host" }))?;
    }
    Ok(n > 0)
}

impl Hub {
    pub fn handle(&self, method: &str, path: &str, auth: Option<&str>, body: &[u8]) -> Reply {
        let mut c = self.db.lock().unwrap_or_else(|e| e.into_inner());
        if method == "PUT" {
            let seg: Vec<&str> = path.trim_matches('/').split('/').collect();
            if let ["v1", "workspaces", ws, "projects", p, "blobs", sha] = seg[..] {
                let Some(who) = authenticate(&c, auth) else { return fail(401, "unauthorized") };
                return self.put_blob(&c, &who, ws, p, sha, body).unwrap_or_else(|r| r);
            }
        }
        let b: Value = if body.iter().all(u8::is_ascii_whitespace) {
            json!({})
        } else {
            match serde_json::from_slice(body) {
                Ok(v @ Value::Object(_)) => v,
                _ => return bad("body must be a JSON object"),
            }
        };
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let seg: Vec<&str> = path.trim_matches('/').split('/').collect();
        if seg.first() != Some(&"v1") {
            return not_found();
        }
        let seg = &seg[1..];
        match (method, seg) {
            ("GET", ["health"]) => return ok(json!({ "ok": true })),
            ("POST", ["redeem"]) => {
                let who = authenticate(&c, auth);
                return self.redeem(&mut c, who, &b).unwrap_or_else(|r| r);
            }
            _ => {}
        }
        let Some(who) = authenticate(&c, auth) else { return fail(401, "unauthorized") };
        let r = match (method, seg) {
            ("GET", ["me"]) => self.me(&c, &who),
            ("GET", ["devices"]) => self.devices(&c, &who),
            ("POST", ["devices"]) => self.add_device(&c, &who, &b),
            ("DELETE", ["devices", id]) => self.revoke_device(&c, &who, id),
            ("GET", ["workspaces", ws]) => self.workspace(&c, &who, ws),
            ("PATCH", ["workspaces", ws]) => self.rename_workspace(&c, &who, ws, &b),
            ("DELETE", ["workspaces", ws]) => self.delete_workspace(&c, &who, ws),
            ("GET", ["workspaces", ws, "projects"]) => self.projects(&c, &who, ws),
            ("POST", ["workspaces", ws, "projects"]) => self.add_project(&c, &who, ws, &b),
            ("GET", ["workspaces", ws, "projects", p]) => self.project(&c, &who, ws, p),
            ("GET", ["workspaces", ws, "members"]) => self.members(&c, &who, ws),
            ("PATCH", ["workspaces", ws, "members", u]) => self.update_member(&c, &who, ws, u, &b),
            ("DELETE", ["workspaces", ws, "members", u]) => self.remove_member(&c, &who, ws, u),
            ("POST", ["workspaces", ws, "transfer"]) => self.transfer(&mut c, &who, ws, &b),
            ("GET", ["workspaces", ws, "invitations"]) => self.invitations(&c, &who, ws),
            ("POST", ["workspaces", ws, "invitations"]) => self.invite(&c, &who, ws, &b),
            ("DELETE", ["workspaces", ws, "invitations", id]) => self.revoke_invitation(&c, &who, ws, id),
            ("GET", ["workspaces", ws, "audit"]) => self.audit_log(&c, &who, ws, query),
            ("GET", ["workspaces", ws, "roles"]) => self.roles(&c, &who, ws),
            ("POST", ["workspaces", ws, "members"]) => self.add_member(&c, &who, ws, &b),
            ("GET", ["people"]) => self.people(&c, &who),
            ("POST", ["workspaces", ws, "roles"]) => self.save_role(&c, &who, ws, &b),
            ("DELETE", ["workspaces", ws, "roles"]) => self.delete_role(&c, &who, ws, &b),
            ("POST", ["workspaces", ws, "ops"]) => self.op(&mut c, &who, ws, &b),
            ("GET", ["workspaces", ws, "changes"]) => self.changes(&c, &who, ws, query),
            ("GET", ["workspaces", ws, "media", id]) => self.get_media(&c, &who, ws, id),
            _ => Err(not_found()),
        };
        r.unwrap_or_else(|r| r)
    }

    /// Single-use, atomic: the use is counted and the membership written in one transaction. Expired, used,
    /// revoked and unknown tokens all get the same reply. A signed-in caller adds the workspace to their account.
    fn redeem(&self, c: &mut Connection, who: Option<Caller>, b: &Value) -> Res {
        let token = b["token"].as_str().unwrap_or("");
        let invalid = || bad("invalid_invitation");
        let tx = db(c.transaction())?;
        let inv: Option<InviteRow> = db(tx
            .query_row(
                "SELECT i.id, i.workspace_id, i.role, i.scope, i.title FROM invitations i JOIN workspaces w ON w.id = i.workspace_id AND w.deleted = 0
                 WHERE i.token_hash = ?1 AND i.revoked = 0 AND i.uses < i.max_uses AND i.expires_at > ?2",
                params![hash(token), now()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional())?;
        let Some((inv_id, ws, role, scope, title)) = inv else { return Err(invalid()) };
        let (user, device) = match &who {
            Some(w) => (w.user.clone(), None),
            None => {
                let name = name_arg(b, "name")?;
                let dev_name = b["device_name"].as_str().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("device");
                let user = new_id("u").map_err(internal)?;
                db(tx.execute("INSERT INTO users(id, name, created_at) VALUES(?1,?2,?3)", params![user, name, now()]))?;
                let d = new_device(&tx, &user, &dev_name.chars().take(MAX_NAME).collect::<String>()).map_err(internal)?;
                (user, Some(d))
            }
        };
        if member_role(&tx, &ws, &user)?.is_some() {
            return Err(fail(409, "already_member"));
        }
        let used = db(tx.execute("UPDATE invitations SET uses = uses + 1 WHERE id = ?1 AND uses < max_uses", [&inv_id]))?;
        if used != 1 {
            return Err(invalid());
        }
        db(tx.execute(
            "INSERT INTO memberships(workspace_id, user_id, role, scope, created_at, title) VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(workspace_id, user_id) DO UPDATE SET role = excluded.role, scope = excluded.scope, title = excluded.title, grants = '[]', denies = '[]', revoked = 0",
            params![ws, user, role, scope, now(), title],
        ))?;
        let actor_dev = device.as_ref().map(|d| d.0.clone()).or(who.as_ref().map(|w| w.device.clone())).unwrap_or_default();
        audit(&tx, Some(&ws), Some((&user, &actor_dev)), "invitation.redeemed", Some(&inv_id), json!({ "role": role })).map_err(internal)?;
        if device.is_some() {
            audit(&tx, None, Some((&user, &actor_dev)), "device.created", Some(&actor_dev), json!({})).map_err(internal)?;
        }
        db(tx.commit())?;
        Ok(ok(json!({ "user_id": user, "workspace_id": ws, "role": role, "title": title, "device": device.map(|(id, token)| json!({ "id": id, "token": token })) })))
    }

    fn me(&self, c: &Connection, who: &Caller) -> Res {
        let name: String = db(c.query_row("SELECT name FROM users WHERE id = ?1", [&who.user], |r| r.get(0)))?;
        let mut st = db(c.prepare(
            "SELECT w.id, w.name, m.role, m.scope IS NOT NULL, m.title FROM memberships m JOIN workspaces w ON w.id = m.workspace_id AND w.deleted = 0
             WHERE m.user_id = ?1 AND m.revoked = 0 ORDER BY w.name",
        ))?;
        let ws: Vec<Value> = db(st
            .query_map([&who.user], |r| Ok(json!({ "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "role": r.get::<_, String>(2)?, "scoped": r.get::<_, bool>(3)?, "title": r.get::<_, Option<String>>(4)? })))
            .and_then(Iterator::collect))?;
        Ok(ok(json!({ "user_id": who.user, "name": name, "device_id": who.device, "workspaces": ws })))
    }

    fn devices(&self, c: &Connection, who: &Caller) -> Res {
        let mut st = db(c.prepare("SELECT id, name, created_at, last_seen, revoked FROM devices WHERE user_id = ?1 ORDER BY created_at"))?;
        let list: Vec<Value> = db(st
            .query_map([&who.user], |r| {
                let id: String = r.get(0)?;
                Ok(json!({ "current": id == who.device, "id": id, "name": r.get::<_, String>(1)?, "created_at": r.get::<_, String>(2)?,
                           "last_seen": r.get::<_, Option<String>>(3)?, "revoked": r.get::<_, bool>(4)? }))
            })
            .and_then(Iterator::collect))?;
        Ok(ok(json!({ "devices": list })))
    }

    fn add_device(&self, c: &Connection, who: &Caller, b: &Value) -> Res {
        let name = name_arg(b, "name")?;
        let (id, token) = new_device(c, &who.user, &name).map_err(internal)?;
        log(c, None, who, "device.created", Some(&id), json!({ "from": who.device }))?;
        Ok(ok(json!({ "id": id, "token": token })))
    }

    fn revoke_device(&self, c: &Connection, who: &Caller, id: &str) -> Res {
        let n = db(c.execute("UPDATE devices SET revoked = 1 WHERE id = ?1 AND user_id = ?2", params![id, who.user]))?;
        if n == 0 {
            return Err(not_found());
        }
        log(c, None, who, "device.revoked", Some(id), json!({}))?;
        Ok(ok(json!({ "revoked": id })))
    }

    fn workspace(&self, c: &Connection, who: &Caller, ws: &str) -> Res {
        let m = membership(c, ws, &who.user);
        self.check(c, who, m.as_ref(), Perm::WorkspaceRead, Target::Workspace(ws), "workspace.read")?;
        let name: String = db(c.query_row("SELECT name FROM workspaces WHERE id = ?1", [ws], |r| r.get(0)))?;
        let m = m.expect("allowed implies membership");
        Ok(ok(json!({ "id": ws, "name": name, "role": m.role.name(), "scoped": m.projects.is_some() })))
    }

    fn rename_workspace(&self, c: &Connection, who: &Caller, ws: &str, b: &Value) -> Res {
        let m = membership(c, ws, &who.user);
        self.check(c, who, m.as_ref(), Perm::WorkspaceManage, Target::Workspace(ws), "workspace.manage")?;
        let name = name_arg(b, "name")?;
        db(c.execute("UPDATE workspaces SET name = ?1 WHERE id = ?2", params![name, ws]))?;
        log(c, Some(ws), who, "workspace.renamed", Some(ws), json!({ "name": name }))?;
        Ok(ok(json!({ "id": ws, "name": name })))
    }

    /// Soft delete: the workspace disappears for everyone; rows stay for the audit trail and backups.
    fn delete_workspace(&self, c: &Connection, who: &Caller, ws: &str) -> Res {
        let m = membership(c, ws, &who.user);
        self.check(c, who, m.as_ref(), Perm::WorkspaceDelete, Target::Workspace(ws), "workspace.delete")?;
        db(c.execute("UPDATE workspaces SET deleted = 1 WHERE id = ?1", [ws]))?;
        db(c.execute("UPDATE invitations SET revoked = 1 WHERE workspace_id = ?1", [ws]))?;
        log(c, Some(ws), who, "workspace.deleted", Some(ws), json!({}))?;
        Ok(ok(json!({ "deleted": ws })))
    }

    /// Scoped in SQL by the caller's own membership row (a join, not a post-filter, §5).
    pub(crate) fn projects(&self, c: &Connection, who: &Caller, ws: &str) -> Res {
        let m = membership(c, ws, &who.user);
        self.check(c, who, m.as_ref(), Perm::WorkspaceRead, Target::Workspace(ws), "project.list")?;
        if !m.as_ref().is_some_and(|m| m.has(Perm::ProjectRead)) {
            return Ok(ok(json!({ "projects": [] })));
        }
        let mut st = db(c.prepare(
            "SELECT p.id, p.name FROM projects p
             JOIN memberships m ON m.workspace_id = p.workspace_id AND m.user_id = ?2 AND m.revoked = 0
             WHERE p.workspace_id = ?1 AND p.deleted = 0 AND (m.scope IS NULL OR p.id IN (SELECT value FROM json_each(m.scope)))
             ORDER BY p.name, p.id",
        ))?;
        let list: Vec<Value> = db(st
            .query_map(params![ws, who.user], |r| Ok(json!({ "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)? })))
            .and_then(Iterator::collect))?;
        Ok(ok(json!({ "projects": list })))
    }

    fn add_project(&self, c: &Connection, who: &Caller, ws: &str, b: &Value) -> Res {
        let m = membership(c, ws, &who.user);
        self.check(c, who, m.as_ref(), Perm::ProjectLink, Target::Workspace(ws), "project.link")?;
        let name = name_arg(b, "name")?;
        let id = new_id("p").map_err(internal)?;
        db(c.execute("INSERT INTO projects(id, workspace_id, name, created_by, created_at) VALUES(?1,?2,?3,?4,?5)", params![id, ws, name, who.user, now()]))?;
        log(c, Some(ws), who, "project.created", Some(&id), json!({ "name": name }))?;
        Ok(ok(json!({ "id": id, "name": name })))
    }

    fn project(&self, c: &Connection, who: &Caller, ws: &str, p: &str) -> Res {
        let m = membership(c, ws, &who.user);
        self.check(c, who, m.as_ref(), Perm::ProjectRead, Target::Project { workspace: ws, project: p }, "project.read")?;
        let row: Option<(String, String, String)> = db(c
            .query_row("SELECT name, created_by, created_at FROM projects WHERE id = ?1 AND workspace_id = ?2 AND deleted = 0", params![p, ws], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional())?;
        let (name, by, at) = row.ok_or_else(not_found)?;
        Ok(ok(json!({ "id": p, "name": name, "created_by": by, "created_at": at })))
    }

    /// Everyone in the workspace sees who is in it; scopes and custom permissions only those who manage members.
    fn members(&self, c: &Connection, who: &Caller, ws: &str) -> Res {
        let m = membership(c, ws, &who.user);
        self.check(c, who, m.as_ref(), Perm::WorkspaceRead, Target::Workspace(ws), "members.read")?;
        let manager = m.as_ref().is_some_and(|m| m.has(Perm::MembersManage) && m.projects.is_none());
        let mut st = db(c.prepare(
            "SELECT u.id, u.name, m.role, m.grants, m.denies, m.scope, m.title FROM memberships m JOIN users u ON u.id = m.user_id
             WHERE m.workspace_id = ?1 AND m.revoked = 0 AND u.revoked = 0 ORDER BY u.name, u.id",
        ))?;
        let list: Vec<Value> = db(st
            .query_map([ws], |r| {
                let mut v = json!({ "user_id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "role": r.get::<_, String>(2)?, "title": r.get::<_, Option<String>>(6)? });
                if manager {
                    let parse = |s: Option<String>| s.and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null);
                    v["grant"] = parse(r.get(3)?);
                    v["deny"] = parse(r.get(4)?);
                    v["projects"] = parse(r.get(5)?);
                }
                Ok(v)
            })
            .and_then(Iterator::collect))?;
        Ok(ok(json!({ "members": list })))
    }

    fn update_member(&self, c: &Connection, who: &Caller, ws: &str, user: &str, b: &Value) -> Res {
        let m = membership(c, ws, &who.user);
        // An unknown member looks like one the caller may not manage, so ids can't be probed.
        let current = member_role(c, ws, user)?.unwrap_or(Role::Viewer);
        // {"title": "Artist"} assigns a named role (its base level is checked); {"role": "editor"} a built-in one.
        let title = match b.get("title") {
            Some(Value::String(t)) => Some(t.clone()),
            Some(_) => return Err(bad("title must be a role name")),
            None => None,
        };
        let new_role = match (&title, b.get("role")) {
            (Some(t), _) => Some(role_preset(c, ws, t)?.ok_or_else(|| bad("unknown role"))?),
            (None, Some(r)) => Some(r.as_str().and_then(Role::parse).ok_or_else(|| bad("unknown role"))?),
            (None, None) => None,
        };
        self.check(c, who, m.as_ref(), Perm::MembersManage, Target::Member { workspace: ws, role: current, new_role }, "members.manage")?;
        if member_role(c, ws, user)?.is_none() {
            return Err(not_found());
        }
        let mut changed = serde_json::Map::new();
        if let Some(r) = new_role {
            db(c.execute("UPDATE memberships SET role = ?1, title = ?2 WHERE workspace_id = ?3 AND user_id = ?4", params![r.name(), title, ws, user]))?;
            changed.insert("role".into(), json!(r.name()));
            changed.insert("title".into(), json!(title));
        }
        for (k, col, grant) in [("grant", "grants", true), ("deny", "denies", false)] {
            if let Some(v) = b.get(k) {
                let s = perms_arg(v, grant)?;
                db(c.execute(&format!("UPDATE memberships SET {col} = ?1 WHERE workspace_id = ?2 AND user_id = ?3"), params![s, ws, user]))?;
                changed.insert(k.into(), serde_json::from_str(&s).unwrap_or_default());
            }
        }
        if let Some(v) = b.get("projects") {
            let s = scope_arg(c, ws, v)?;
            db(c.execute("UPDATE memberships SET scope = ?1 WHERE workspace_id = ?2 AND user_id = ?3", params![s, ws, user]))?;
            changed.insert("projects".into(), s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null));
        }
        log(c, Some(ws), who, "member.updated", Some(user), Value::Object(changed.clone()))?;
        Ok(ok(json!({ "user_id": user, "changed": changed })))
    }

    fn remove_member(&self, c: &Connection, who: &Caller, ws: &str, user: &str) -> Res {
        let m = membership(c, ws, &who.user);
        let current = member_role(c, ws, user)?.unwrap_or(Role::Viewer);
        // Taking someone out of a workspace is an access decision: owner only (D037).
        self.check(c, who, m.as_ref(), Perm::MembersInvite, Target::Member { workspace: ws, role: current, new_role: None }, "members.access")?;
        let n = db(c.execute("UPDATE memberships SET revoked = 1 WHERE workspace_id = ?1 AND user_id = ?2 AND revoked = 0", params![ws, user]))?;
        if n == 0 {
            return Err(not_found());
        }
        log(c, Some(ws), who, "member.removed", Some(user), json!({}))?;
        Ok(ok(json!({ "removed": user })))
    }

    /// The only way to make an owner. The caller steps down to admin, so the workspace keeps exactly its owners.
    fn transfer(&self, c: &mut Connection, who: &Caller, ws: &str, b: &Value) -> Res {
        let m = membership(c, ws, &who.user);
        self.check(c, who, m.as_ref(), Perm::OwnershipTransfer, Target::Workspace(ws), "ownership.transfer")?;
        let to = b["user_id"].as_str().unwrap_or("");
        if to == who.user || member_role(c, ws, to)?.is_none() {
            return Err(bad("user_id must be another active member"));
        }
        let tx = db(c.transaction())?;
        db(tx.execute("UPDATE memberships SET role = 'owner', title = NULL, scope = NULL, denies = '[]' WHERE workspace_id = ?1 AND user_id = ?2", params![ws, to]))?;
        db(tx.execute("UPDATE memberships SET role = 'admin', title = NULL WHERE workspace_id = ?1 AND user_id = ?2", params![ws, who.user]))?;
        log(&tx, Some(ws), who, "ownership.transferred", Some(to), json!({}))?;
        db(tx.commit())?;
        Ok(ok(json!({ "owner": to })))
    }

    fn invitations(&self, c: &Connection, who: &Caller, ws: &str) -> Res {
        let m = membership(c, ws, &who.user);
        // Listing needs the right to invite at least a viewer.
        self.check(c, who, m.as_ref(), Perm::MembersInvite, Target::Member { workspace: ws, role: Role::Viewer, new_role: None }, "members.invite")?;
        let mut st = db(c.prepare(
            "SELECT id, role, scope, expires_at, uses, max_uses, revoked, created_by, created_at FROM invitations WHERE workspace_id = ?1 ORDER BY created_at DESC LIMIT 200",
        ))?;
        let t = now();
        let list: Vec<Value> = db(st
            .query_map([ws], |r| {
                let (exp, uses, max, revoked): (String, i64, i64, bool) = (r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?);
                let status = if revoked { "revoked" } else if uses >= max { "used" } else if exp <= t { "expired" } else { "open" };
                Ok(json!({ "id": r.get::<_, String>(0)?, "role": r.get::<_, String>(1)?, "projects": r.get::<_, Option<String>>(2)?.and_then(|s| serde_json::from_str::<Value>(&s).ok()),
                           "expires_at": exp, "status": status, "created_by": r.get::<_, Option<String>>(7)?, "created_at": r.get::<_, String>(8)? }))
            })
            .and_then(Iterator::collect))?;
        Ok(ok(json!({ "invitations": list })))
    }

    fn invite(&self, c: &Connection, who: &Caller, ws: &str, b: &Value) -> Res {
        let m = membership(c, ws, &who.user);
        let title = b["title"].as_str().map(String::from);
        let role = match &title {
            Some(t) => role_preset(c, ws, t)?.ok_or_else(|| bad("unknown role"))?,
            None => b["role"].as_str().and_then(Role::parse).ok_or_else(|| bad("unknown role"))?,
        };
        self.check(c, who, m.as_ref(), Perm::MembersInvite, Target::Member { workspace: ws, role, new_role: None }, "members.invite")?;
        let hours = b["hours"].as_i64().unwrap_or(72);
        if !(1..=168).contains(&hours) {
            return Err(bad("hours must be 1-168 (7 days max)"));
        }
        let scope = scope_arg(c, ws, &b["projects"])?;
        let (id, token, expires) = create_invitation_titled(c, ws, role, title.as_deref(), scope, hours, Some(&who.user)).map_err(internal)?;
        log(c, Some(ws), who, "invitation.created", Some(&id), json!({ "role": role.name(), "title": title, "hours": hours }))?;
        Ok(ok(json!({ "id": id, "token": token, "role": role.name(), "title": title, "expires_at": expires })))
    }

    fn revoke_invitation(&self, c: &Connection, who: &Caller, ws: &str, id: &str) -> Res {
        let m = membership(c, ws, &who.user);
        let role: Option<String> = db(c.query_row("SELECT role FROM invitations WHERE id = ?1 AND workspace_id = ?2", params![id, ws], |r| r.get(0)).optional())?;
        let r = role.as_deref().and_then(Role::parse).unwrap_or(Role::Viewer);
        self.check(c, who, m.as_ref(), Perm::MembersInvite, Target::Member { workspace: ws, role: r, new_role: None }, "members.invite")?;
        if role.is_none() {
            return Err(not_found());
        }
        db(c.execute("UPDATE invitations SET revoked = 1 WHERE id = ?1", [id]))?;
        log(c, Some(ws), who, "invitation.revoked", Some(id), json!({}))?;
        Ok(ok(json!({ "revoked": id })))
    }

    /// Owner only (D037): put someone who is already in one of the caller's workspaces into this one, with a role.
    fn add_member(&self, c: &Connection, who: &Caller, ws: &str, b: &Value) -> Res {
        let m = membership(c, ws, &who.user);
        let title = b["title"].as_str().map(String::from);
        let role = match &title {
            Some(t) => role_preset(c, ws, t)?.ok_or_else(|| bad("unknown role"))?,
            None => b["role"].as_str().and_then(Role::parse).ok_or_else(|| bad("unknown role"))?,
        };
        self.check(c, who, m.as_ref(), Perm::MembersInvite, Target::Member { workspace: ws, role, new_role: None }, "members.access")?;
        let user = b["user_id"].as_str().unwrap_or("");
        // Only people the caller already shares a workspace with as its owner; anyone else joins by invitation.
        let known: i64 = db(c.query_row(
            "SELECT count(*) FROM memberships them JOIN memberships mine ON mine.workspace_id = them.workspace_id
             JOIN users u ON u.id = them.user_id AND u.revoked = 0
             WHERE them.user_id = ?1 AND them.revoked = 0 AND mine.user_id = ?2 AND mine.revoked = 0 AND mine.role = 'owner'",
            params![user, who.user],
            |r| r.get(0),
        ))?;
        if known == 0 || user == who.user {
            return Err(not_found());
        }
        if member_role(c, ws, user)?.is_some() {
            return Err(fail(409, "already_member"));
        }
        db(c.execute(
            "INSERT INTO memberships(workspace_id, user_id, role, title, created_at) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(workspace_id, user_id) DO UPDATE SET role = excluded.role, title = excluded.title, scope = NULL, grants = '[]', denies = '[]', revoked = 0",
            params![ws, user, role.name(), title, now()],
        ))?;
        log(c, Some(ws), who, "member.added", Some(user), json!({ "role": role.name(), "title": title }))?;
        Ok(ok(json!({ "user_id": user, "role": role.name(), "title": title })))
    }

    /// Everyone in the workspaces the caller owns, with where they are and as what (empty for non-owners).
    fn people(&self, c: &Connection, who: &Caller) -> Res {
        let mut st = db(c.prepare(
            "SELECT w.id, w.name FROM memberships m JOIN workspaces w ON w.id = m.workspace_id AND w.deleted = 0
             WHERE m.user_id = ?1 AND m.revoked = 0 AND m.role = 'owner' ORDER BY w.name",
        ))?;
        let owned: Vec<(String, String)> = db(st.query_map([&who.user], |r| Ok((r.get(0)?, r.get(1)?))).and_then(Iterator::collect))?;
        let ids = json!(owned.iter().map(|o| o.0.clone()).collect::<Vec<_>>()).to_string();
        let mut st = db(c.prepare(
            "SELECT u.id, u.name, m.workspace_id, m.role, m.title FROM memberships m JOIN users u ON u.id = m.user_id AND u.revoked = 0
             WHERE m.revoked = 0 AND m.user_id != ?1 AND m.workspace_id IN (SELECT value FROM json_each(?2)) ORDER BY u.name, u.id",
        ))?;
        let rows: Vec<(String, String, String, String, Option<String>)> =
            db(st.query_map(params![who.user, ids], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).and_then(Iterator::collect))?;
        let mut people: Vec<Value> = Vec::new();
        for (id, name, ws, role, title) in rows {
            if people.last().is_none_or(|p| p["user_id"] != id.as_str()) {
                people.push(json!({ "user_id": id, "name": name, "workspaces": {} }));
            }
            people.last_mut().expect("just pushed")["workspaces"][ws] = json!({ "role": role, "title": title });
        }
        Ok(ok(json!({ "workspaces": owned.iter().map(|(id, n)| json!({ "id": id, "name": n })).collect::<Vec<_>>(), "people": people })))
    }

    /// Named roles of the workspace, with how many active members hold each.
    fn roles(&self, c: &Connection, who: &Caller, ws: &str) -> Res {
        let m = membership(c, ws, &who.user);
        self.check(c, who, m.as_ref(), Perm::WorkspaceRead, Target::Workspace(ws), "roles.read")?;
        let mut st = db(c.prepare(
            "SELECT r.name, r.base, r.grants, r.denies, (SELECT count(*) FROM memberships m WHERE m.workspace_id = r.workspace_id AND m.title = r.name AND m.revoked = 0)
             FROM roles r WHERE r.workspace_id = ?1 ORDER BY r.rowid",
        ))?;
        let list: Vec<Value> = db(st
            .query_map([ws], |r| {
                let arr = |s: String| serde_json::from_str::<Value>(&s).unwrap_or(json!([]));
                Ok(json!({ "name": r.get::<_, String>(0)?, "base": r.get::<_, String>(1)?, "grant": arr(r.get(2)?), "deny": arr(r.get(3)?), "members": r.get::<_, i64>(4)? }))
            })
            .and_then(Iterator::collect))?;
        let perms: Vec<Value> = Perm::ALL.iter().map(|p| json!({ "name": p.name(), "grantable": p.grantable() })).collect();
        let bases: serde_json::Map<String, Value> = Role::ALL.iter().map(|r| (r.name().to_string(), json!(Perm::ALL.iter().filter(|p| r.allows(**p)).map(|p| p.name()).collect::<Vec<_>>()))).collect();
        Ok(ok(json!({ "roles": list, "builtin": Role::ALL.iter().map(|r| r.name()).collect::<Vec<_>>(), "permissions": perms, "bases": bases })))
    }

    /// Creates or edits a named role. Needs workspace.manage, and the role's base (old and new) must rank below the
    /// caller: owners can make admin-level roles, admins up to editor. Members holding it move with a base change.
    fn save_role(&self, c: &Connection, who: &Caller, ws: &str, b: &Value) -> Res {
        let m = membership(c, ws, &who.user);
        self.check(c, who, m.as_ref(), Perm::WorkspaceManage, Target::Workspace(ws), "roles.manage")?;
        let caller = m.expect("allowed implies membership").role;
        let name = role_name_arg(b)?;
        let base = b["base"].as_str().and_then(Role::parse).filter(|r| *r != Role::Owner).ok_or_else(|| bad("base must be admin, editor, contributor or viewer"))?;
        let old = role_preset(c, ws, &name)?;
        if base <= caller || old.is_some_and(|o| o <= caller) {
            return Err(self.refuse(c, who, ws, "roles.manage", Decision::Forbidden));
        }
        let grant = perms_arg(&b.get("grant").cloned().unwrap_or(json!([])), true)?;
        let deny = perms_arg(&b.get("deny").cloned().unwrap_or(json!([])), false)?;
        db(c.execute(
            "INSERT INTO roles(workspace_id, name, base, grants, denies, created_at) VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(workspace_id, name) DO UPDATE SET base = excluded.base, grants = excluded.grants, denies = excluded.denies",
            params![ws, name, base.name(), grant, deny, now()],
        ))?;
        let moved = db(c.execute("UPDATE memberships SET role = ?1 WHERE workspace_id = ?2 AND title = ?3", params![base.name(), ws, name]))?;
        log(c, Some(ws), who, if old.is_some() { "role.updated" } else { "role.created" }, Some(&name), json!({ "base": base.name(), "grant": serde_json::from_str::<Value>(&grant).unwrap_or_default(), "deny": serde_json::from_str::<Value>(&deny).unwrap_or_default(), "members_moved": moved }))?;
        Ok(ok(json!({ "name": name, "base": base.name(), "members": moved })))
    }

    fn delete_role(&self, c: &Connection, who: &Caller, ws: &str, b: &Value) -> Res {
        let m = membership(c, ws, &who.user);
        self.check(c, who, m.as_ref(), Perm::WorkspaceManage, Target::Workspace(ws), "roles.manage")?;
        let caller = m.expect("allowed implies membership").role;
        let name = b["name"].as_str().unwrap_or("");
        let base = role_preset(c, ws, name)?.ok_or_else(not_found)?;
        if base <= caller {
            return Err(self.refuse(c, who, ws, "roles.manage", Decision::Forbidden));
        }
        let held: i64 = db(c.query_row("SELECT count(*) FROM memberships WHERE workspace_id = ?1 AND title = ?2 AND revoked = 0", params![ws, name], |r| r.get(0)))?;
        if held > 0 {
            return Err(fail(409, "role_in_use"));
        }
        db(c.execute("DELETE FROM roles WHERE workspace_id = ?1 AND name = ?2", params![ws, name]))?;
        log(c, Some(ws), who, "role.deleted", Some(name), json!({}))?;
        Ok(ok(json!({ "deleted": name })))
    }

    fn audit_log(&self, c: &Connection, who: &Caller, ws: &str, query: &str) -> Res {
        let m = membership(c, ws, &who.user);
        self.check(c, who, m.as_ref(), Perm::AuditRead, Target::Workspace(ws), "audit.read")?;
        let since: i64 = query.split('&').find_map(|kv| kv.strip_prefix("since=")).and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut st = db(c.prepare(
            "SELECT seq, ts, actor_user, actor_device, action, target, detail FROM audit WHERE workspace_id = ?1 AND seq > ?2 ORDER BY seq LIMIT 200",
        ))?;
        let list: Vec<Value> = db(st
            .query_map(params![ws, since], |r| {
                Ok(json!({ "seq": r.get::<_, i64>(0)?, "ts": r.get::<_, String>(1)?, "actor_user": r.get::<_, Option<String>>(2)?, "actor_device": r.get::<_, Option<String>>(3)?,
                           "action": r.get::<_, String>(4)?, "target": r.get::<_, Option<String>>(5)?, "detail": serde_json::from_str::<Value>(&r.get::<_, String>(6)?).unwrap_or_default() }))
            })
            .and_then(Iterator::collect))?;
        Ok(ok(json!({ "entries": list })))
    }
}
