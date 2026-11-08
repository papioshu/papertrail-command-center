//! Workspace hub client, M3 (D030, docs/workspaces/ARCHITECTURE.md §2, §4, §7, §9, §12).
//! Until the user joins a hub nothing here runs: no thread, no file, no table, no request.
//! After joining, `hub/` in the app data folder holds:
//!   hub.json        hub URL, account, workspaces, local→hub project links (no secrets)
//!   device.key      the device token, encrypted for this Windows user with DPAPI
//!   <ws>.db         per workspace: cached records, the outbox of unsent edits, conflict copies (P2 data)
//!   unsent/*.json   the user's own unsent edits, saved when a workspace is revoked (P1)
//! The app's main database is a rebuildable cache, so unsent work never lives there.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::Duration;

type R<T> = Result<T, String>;

/* ---------- device secret (DPAPI, current Windows user) ---------- */

#[cfg(windows)]
mod dpapi {
    use std::ffi::c_void;
    #[repr(C)]
    struct Blob {
        len: u32,
        data: *mut u8,
    }
    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(i: *const Blob, d: *const u16, e: *const Blob, r: *mut c_void, p: *mut c_void, f: u32, o: *mut Blob) -> i32;
        fn CryptUnprotectData(i: *const Blob, d: *mut *mut u16, e: *const Blob, r: *mut c_void, p: *mut c_void, f: u32, o: *mut Blob) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(h: *mut c_void) -> *mut c_void;
    }
    const UI_FORBIDDEN: u32 = 1;

    fn run(input: &[u8], protect: bool) -> Result<Vec<u8>, String> {
        let i = Blob { len: input.len() as u32, data: input.as_ptr() as *mut u8 };
        let mut o = Blob { len: 0, data: std::ptr::null_mut() };
        let null = std::ptr::null_mut();
        // SAFETY: valid input blob; on success Windows allocates o.data, which we copy and free with LocalFree.
        let ok = unsafe {
            if protect {
                CryptProtectData(&i, std::ptr::null(), std::ptr::null(), null, null, UI_FORBIDDEN, &mut o)
            } else {
                CryptUnprotectData(&i, std::ptr::null_mut(), std::ptr::null(), null, null, UI_FORBIDDEN, &mut o)
            }
        };
        if ok == 0 || o.data.is_null() {
            return Err(format!("Windows data protection failed ({})", std::io::Error::last_os_error()));
        }
        let out = unsafe { std::slice::from_raw_parts(o.data, o.len as usize).to_vec() };
        unsafe { LocalFree(o.data as *mut c_void) };
        Ok(out)
    }
    pub fn protect(b: &[u8]) -> Result<Vec<u8>, String> {
        run(b, true)
    }
    pub fn unprotect(b: &[u8]) -> Result<Vec<u8>, String> {
        run(b, false)
    }
}

#[cfg(not(windows))]
mod dpapi {
    pub fn protect(_: &[u8]) -> Result<Vec<u8>, String> {
        Err("the hub client needs Windows data protection".into())
    }
    pub fn unprotect(_: &[u8]) -> Result<Vec<u8>, String> {
        Err("the hub client needs Windows data protection".into())
    }
}

/* ---------- helpers ---------- */

fn now() -> String {
    crate::util::now_iso()
}

pub fn uuid() -> R<String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).map_err(|e| e.to_string())?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]))
}

fn write_atomic(path: &Path, data: &[u8]) -> R<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("{}: {e}", path.display()))
}

/// The bearer only travels over HTTPS or to this PC / the Tailscale network (§9).
/// The address is `scheme://host[:port]` and nothing else: no user info (`a@b`), path, query or fragment, so no
/// parser can read a different host than the one checked here.
pub fn check_url(url: &str) -> R<String> {
    let url = url.trim().trim_end_matches('/');
    let bad = || -> String { "The hub address must look like http://100.x.y.z:8770 or https://name.ts.net".into() };
    let (scheme, auth) = url.split_once("://").ok_or_else(bad)?;
    if auth.is_empty() || !auth.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']')) {
        return Err(bad());
    }
    let (host, port) = if let Some(v6) = auth.strip_prefix('[') {
        let (h, rest) = v6.split_once(']').ok_or_else(bad)?;
        (h, rest.strip_prefix(':'))
    } else {
        match auth.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (auth, None),
        }
    };
    if host.is_empty() || port.is_some_and(|p| p.parse::<u16>().map_or(true, |n| n == 0)) || (!auth.starts_with('[') && host.contains(['[', ']'])) {
        return Err(bad());
    }
    let ip = host.parse::<std::net::IpAddr>();
    if auth.starts_with('[') != matches!(ip, Ok(std::net::IpAddr::V6(_))) {
        return Err(bad());
    }
    let private = host.eq_ignore_ascii_case("localhost")
        || match ip {
            Ok(std::net::IpAddr::V4(v)) => v.is_loopback() || (v.octets()[0] == 100 && (64..128).contains(&v.octets()[1])),
            Ok(std::net::IpAddr::V6(v)) => v.is_loopback() || v.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
            Err(_) => false,
        };
    match scheme {
        "https" => Ok(url.to_string()),
        "http" if private => Ok(url.to_string()),
        "http" => Err("Plain http is only allowed to this PC or a Tailscale address (100.x). Use https for anything else.".into()),
        _ => Err(bad()),
    }
}

/// An invite link is `<hub url>#<token>` (the token stays in the fragment, out of logs).
pub fn parse_invite(link: &str) -> R<(String, String)> {
    let (url, token) = link.trim().split_once('#').ok_or("That isn't an invite link (expected http://…#code).")?;
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("The invite code in that link is incomplete.".into());
    }
    Ok((check_url(url)?, token.to_string()))
}

enum Net {
    Reply(u16, Value),
    Down(String),
}

/* ---------- client ---------- */

pub struct Client {
    dir: PathBuf,
    agent: ureq::Agent,
}

#[derive(Default)]
pub struct Status {
    pub online: Option<bool>,
    pub last_sync: Option<String>,
    pub last_error: Option<String>,
}

const WS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS records(id TEXT PRIMARY KEY, kind TEXT NOT NULL, project_id TEXT NOT NULL, version INTEGER NOT NULL,
  fields TEXT NOT NULL, deleted INTEGER NOT NULL DEFAULT 0, created_by TEXT, updated_by TEXT, updated_at TEXT, seq INTEGER);
CREATE TABLE IF NOT EXISTS outbox(ord INTEGER PRIMARY KEY AUTOINCREMENT, op_id TEXT NOT NULL UNIQUE, record_id TEXT NOT NULL,
  op TEXT NOT NULL, created_at TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, last_error TEXT);
CREATE TABLE IF NOT EXISTS conflicts(id INTEGER PRIMARY KEY, record_id TEXT NOT NULL, kind TEXT NOT NULL, mine TEXT NOT NULL,
  theirs TEXT, reason TEXT, at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
"#;

fn meta(c: &Connection, k: &str) -> Option<Value> {
    c.query_row("SELECT value FROM meta WHERE key = ?1", [k], |r| r.get::<_, String>(0)).ok().and_then(|s| serde_json::from_str(&s).ok())
}

fn set_meta(c: &Connection, k: &str, v: &Value) -> R<()> {
    c.execute("INSERT INTO meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![k, v.to_string()]).map(|_| ()).map_err(|e| e.to_string())
}

fn e(x: impl std::fmt::Display) -> String {
    x.to_string()
}

fn obj(s: &str) -> Map<String, Value> {
    serde_json::from_str(s).unwrap_or_default()
}

impl Client {
    pub fn new(data_dir: &Path) -> Client {
        Client { dir: data_dir.join("hub"), agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build() }
    }

    fn config_path(&self) -> PathBuf {
        self.dir.join("hub.json")
    }

    pub fn config(&self) -> Option<Value> {
        std::fs::read(self.config_path()).ok().and_then(|b| serde_json::from_slice(&b).ok())
    }

    fn save_config(&self, v: &Value) -> R<()> {
        std::fs::create_dir_all(&self.dir).map_err(e)?;
        write_atomic(&self.config_path(), v.to_string().as_bytes())
    }

    fn token(&self) -> Option<String> {
        let b = std::fs::read(self.dir.join("device.key")).ok()?;
        String::from_utf8(dpapi::unprotect(&b).ok()?).ok()
    }

    fn workspaces(&self) -> Vec<String> {
        let cfg = self.config().unwrap_or_default();
        cfg["workspaces"].as_array().into_iter().flatten().filter_map(|w| w["id"].as_str().map(String::from)).collect()
    }

    fn request(&self, method: &str, path: &str) -> Result<ureq::Request, Net> {
        let Some(url) = self.config().and_then(|c| c["url"].as_str().map(String::from)) else { return Err(Net::Down("no hub".into())) };
        // Re-checked on every call: hub.json is a plain file, and the bearer must never go anywhere else.
        let Ok(url) = check_url(&url) else { return Err(Net::Down("the saved hub address is not allowed".into())) };
        let mut req = self.agent.request(method, &format!("{url}{path}"));
        if let Some(t) = self.token() {
            req = req.set("Authorization", &format!("Bearer {t}"));
        }
        Ok(req)
    }

    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Net {
        let req = match self.request(method, path) {
            Ok(r) => r,
            Err(n) => return n,
        };
        let res = match body {
            Some(b) => req.send_json(b.clone()),
            None => req.call(),
        };
        Self::reply(res)
    }

    fn reply(res: Result<ureq::Response, ureq::Error>) -> Net {
        match res {
            Ok(r) => Net::Reply(r.status(), r.into_json().unwrap_or_default()),
            // 5xx = the hub is having trouble; retry later like a network error.
            Err(ureq::Error::Status(code, _)) if code >= 500 => Net::Down(format!("hub error {code}")),
            Err(ureq::Error::Status(code, r)) => Net::Reply(code, r.into_json().unwrap_or_default()),
            Err(ureq::Error::Transport(t)) => Net::Down(t.to_string()),
        }
    }

    fn need(&self, method: &str, path: &str, body: Option<&Value>) -> R<Value> {
        match self.call(method, path, body) {
            Net::Reply(200, v) => Ok(v),
            Net::Reply(code, v) => Err(format!("The hub refused ({code}: {})", v["error"].as_str().unwrap_or("error"))),
            Net::Down(d) => Err(format!("Can't reach the hub ({d}).")),
        }
    }

    /// Joins with an invite link. A first join creates the account and this device; joining another workspace on the
    /// same hub adds it to the same account. One hub per PC for now.
    pub fn join(&self, link: &str, name: &str, device_name: &str) -> R<Value> {
        let (url, token) = parse_invite(link)?;
        let existing = self.config();
        if let Some(cfg) = &existing {
            if cfg["url"].as_str() != Some(url.as_str()) {
                return Err(format!("This PC is already joined to the hub at {}. Leave it first.", cfg["url"].as_str().unwrap_or("?")));
            }
        } else {
            self.save_config(&json!({ "url": url, "workspaces": [], "links": {} }))?;
        }
        let body = json!({ "token": token, "name": name.trim(), "device_name": device_name });
        let r = match self.call("POST", "/v1/redeem", Some(&body)) {
            Net::Reply(200, v) => v,
            other => {
                if existing.is_none() {
                    let _ = std::fs::remove_file(self.config_path());
                }
                return Err(match other {
                    Net::Reply(409, _) => "You're already in that workspace.".into(),
                    Net::Reply(_, v) if v["error"] == "invalid_invitation" => "That invite has expired, was already used, or was cancelled. Ask for a new one.".into(),
                    Net::Reply(code, v) => format!("The hub refused ({code}: {})", v["error"].as_str().unwrap_or("error")),
                    Net::Down(d) => format!("Can't reach the hub ({d})."),
                });
            }
        };
        if let Some(t) = r["device"]["token"].as_str() {
            write_atomic(&self.dir.join("device.key"), &dpapi::protect(t.as_bytes())?)?;
        }
        let mut cfg = self.config().unwrap_or_default();
        cfg["user_id"] = r["user_id"].clone();
        self.save_config(&cfg)?;
        self.refresh_me()?;
        Ok(json!({ "workspace_id": r["workspace_id"], "role": r["role"] }))
    }

    /// Updates the workspace list from the hub; any workspace that vanished was revoked and is purged.
    fn refresh_me(&self) -> R<()> {
        match self.call("GET", "/v1/me", None) {
            Net::Reply(200, me) => {
                let mut cfg = self.config().unwrap_or_default();
                let now_ids: Vec<String> = me["workspaces"].as_array().into_iter().flatten().filter_map(|w| w["id"].as_str().map(String::from)).collect();
                for gone in self.workspaces().into_iter().filter(|w| !now_ids.contains(w)) {
                    self.purge(&gone)?;
                }
                cfg["name"] = me["name"].clone();
                cfg["device_id"] = me["device_id"].clone();
                cfg["workspaces"] = me["workspaces"].clone();
                self.save_config(&cfg)
            }
            Net::Reply(401, _) => self.device_revoked(),
            Net::Reply(code, _) => Err(format!("hub error {code}")),
            Net::Down(d) => Err(d),
        }
    }

    fn device_revoked(&self) -> R<()> {
        for ws in self.workspaces() {
            self.purge(&ws)?;
        }
        let mut cfg = self.config().unwrap_or_default();
        cfg["revoked"] = json!(true);
        cfg["workspaces"] = json!([]);
        self.save_config(&cfg)?;
        let _ = std::fs::remove_file(self.dir.join("device.key"));
        Err("This device was signed out of the hub.".into())
    }

    fn ws_path(&self, ws: &str) -> R<PathBuf> {
        if ws.is_empty() || !ws.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err("bad workspace id".into());
        }
        Ok(self.dir.join(format!("{ws}.db")))
    }

    fn open_ws(&self, ws: &str) -> R<Connection> {
        if !self.workspaces().iter().any(|w| w == ws) {
            return Err("You're not in that workspace.".into());
        }
        let c = Connection::open(self.ws_path(ws)?).map_err(e)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA busy_timeout=5000;").map_err(e)?;
        c.execute_batch(WS_SCHEMA).map_err(e)?;
        Ok(c)
    }

    /// §7.5: the user's own unsent edits go to a private file, then every cached byte of the workspace is deleted.
    pub fn purge(&self, ws: &str) -> R<()> {
        let path = self.ws_path(ws)?;
        if path.exists() {
            let ops: Vec<Value> = Connection::open(&path)
                .ok()
                .and_then(|c| {
                    let mut st = c.prepare("SELECT op FROM outbox ORDER BY ord").ok()?;
                    let v = st.query_map([], |r| r.get::<_, String>(0)).ok()?.filter_map(|s| s.ok()?.parse().ok()).collect();
                    Some(v)
                })
                .unwrap_or_default();
            if !ops.is_empty() {
                let dir = self.dir.join("unsent");
                std::fs::create_dir_all(&dir).map_err(e)?;
                let f = dir.join(format!("{ws}-{}.json", now().replace(':', "")));
                write_atomic(&f, serde_json::to_string_pretty(&json!({ "workspace": ws, "saved_at": now(), "ops": ops })).map_err(e)?.as_bytes())?;
            }
            for ext in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
            }
        }
        let _ = std::fs::remove_dir_all(self.dir.join("files").join(ws));
        let mut cfg = self.config().unwrap_or_default();
        if let Some(list) = cfg["workspaces"].as_array_mut() {
            list.retain(|w| w["id"] != ws);
        }
        if let Some(links) = cfg["links"].as_object_mut() {
            links.retain(|_, l| l["workspace"] != ws);
        }
        self.save_config(&cfg)
    }

    /// Leaves the hub on this PC: tells the hub to forget this device (best effort), keeps unsent work, deletes the rest.
    pub fn leave(&self) -> R<()> {
        if let Some(dev) = self.config().and_then(|c| c["device_id"].as_str().map(String::from)) {
            let _ = self.call("DELETE", &format!("/v1/devices/{dev}"), None);
        }
        for ws in self.workspaces() {
            self.purge(&ws)?;
        }
        for f in ["device.key", "hub.json"] {
            let _ = std::fs::remove_file(self.dir.join(f));
        }
        Ok(())
    }

    /* ----- local edits: cache + outbox in one transaction (§7.2) ----- */

    pub fn edit(&self, ws: &str, action: &str, kind: &str, record_id: Option<&str>, project: &str, fields: &Value) -> R<String> {
        if !matches!(action, "create" | "update" | "delete") || !matches!(kind, "task" | "milestone" | "media" | "snapshot") {
            return Err("unsupported edit".into());
        }
        let mut c = self.open_ws(ws)?;
        let tx = c.transaction().map_err(e)?;
        let id = match record_id {
            Some(id) => id.to_string(),
            None if action == "create" => uuid()?,
            None => return Err("record_id is required".into()),
        };
        let row: Option<(i64, String, String)> = tx.query_row("SELECT version, fields, project_id FROM records WHERE id = ?1", [&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().map_err(e)?;
        let f = fields.as_object().cloned().unwrap_or_default();
        let (base, project) = match (&row, action) {
            (None, "create") => (0, project.to_string()),
            (Some(_), "create") => return Err("that record already exists".into()),
            (None, _) => return Err("That record isn't here (it may have been removed).".into()),
            (Some((v, _, p)), _) => (*v, p.clone()),
        };
        let op = json!({ "op_id": uuid()?, "action": action, "kind": kind, "record_id": id, "project_id": project, "base_version": base, "fields": if action == "delete" { Value::Null } else { Value::Object(f.clone()) } });
        let mut cached = row.as_ref().map(|r| obj(&r.1)).unwrap_or_default();
        for (k, v) in &f {
            cached.insert(k.clone(), v.clone());
        }
        tx.execute(
            "INSERT INTO records(id, kind, project_id, version, fields, deleted) VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(id) DO UPDATE SET fields = excluded.fields, deleted = excluded.deleted",
            params![id, kind, project, base, Value::Object(cached).to_string(), action == "delete"],
        )
        .map_err(e)?;
        tx.execute("INSERT INTO outbox(op_id, record_id, op, created_at) VALUES(?1,?2,?3,?4)", params![op["op_id"].as_str(), id, op.to_string(), now()]).map_err(e)?;
        tx.commit().map_err(e)?;
        Ok(id)
    }

    /* ----- sync (§7.2–7.5) ----- */

    /// Sends queued ops in order, then pulls the feed. Err(Down) = hub unreachable, try again later.
    /// Ok(true) when something changed locally.
    pub fn sync_all(&self) -> Result<bool, String> {
        if self.config().is_none_or(|c| c["revoked"] == true) {
            return Ok(false);
        }
        if let Err(d) = self.refresh_me() {
            if self.config().is_none_or(|c| c["revoked"] == true) {
                return Ok(true);
            }
            return Err(d);
        }
        let mut changed = false;
        for ws in self.workspaces() {
            changed |= self.sync_ws(&ws)?;
        }
        Ok(changed)
    }

    fn revoked(&self, ws: &str) -> bool {
        matches!(self.call("GET", &format!("/v1/workspaces/{ws}"), None), Net::Reply(404 | 401, _))
    }

    fn sync_ws(&self, ws: &str) -> Result<bool, String> {
        let c = self.open_ws(ws)?;
        let mut changed = false;
        let mut resync = false;
        loop {
            let next: Option<(i64, String)> = c.query_row("SELECT ord, op FROM outbox ORDER BY ord LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(e)?;
            let Some((ord, op)) = next else { break };
            let opv: Value = serde_json::from_str(&op).unwrap_or_default();
            let rid = opv["record_id"].as_str().unwrap_or("").to_string();
            match self.call("POST", &format!("/v1/workspaces/{ws}/ops"), Some(&opv)) {
                Net::Down(d) => {
                    let _ = c.execute("UPDATE outbox SET attempts = attempts + 1, last_error = ?1 WHERE ord = ?2", params![d, ord]);
                    return Err(d);
                }
                Net::Reply(200, v) => {
                    apply(&c, &v["record"])?;
                }
                Net::Reply(409, v) => {
                    let kind = if v["error"] == "deleted" { "deleted" } else { "conflict" };
                    c.execute(
                        "INSERT INTO conflicts(record_id, kind, mine, theirs, reason, at) VALUES(?1,?2,?3,?4,?5,?6)",
                        params![rid, kind, opv["fields"].to_string(), v["record"].to_string(), v["fields"].to_string(), now()],
                    )
                    .map_err(e)?;
                    apply(&c, &v["record"])?;
                }
                Net::Reply(code, v) => {
                    if self.revoked(ws) {
                        drop(c);
                        self.purge(ws)?;
                        return Ok(true);
                    }
                    // Refused (rights changed, invalid field): keep the text as a copy, then restore the hub's state.
                    c.execute(
                        "INSERT INTO conflicts(record_id, kind, mine, theirs, reason, at) VALUES(?1,'rejected',?2,NULL,?3,?4)",
                        params![rid, opv.to_string(), format!("{code} {}", v["error"].as_str().unwrap_or("")), now()],
                    )
                    .map_err(e)?;
                    if opv["action"] == "create" {
                        c.execute("DELETE FROM records WHERE id = ?1 AND version = 0", [&rid]).map_err(e)?;
                    }
                    resync = true;
                }
            }
            c.execute("DELETE FROM outbox WHERE ord = ?1", [ord]).map_err(e)?;
            changed = true;
        }
        if resync {
            set_meta(&c, "cursor", &json!(0))?;
        }
        changed |= self.pull(ws, &c)?;
        if let Net::Reply(200, m) = self.call("GET", &format!("/v1/workspaces/{ws}/members"), None) {
            set_meta(&c, "members", &m["members"])?;
        }
        set_meta(&c, "last_sync", &json!(now()))?;
        Ok(changed)
    }

    fn pull(&self, ws: &str, c: &Connection) -> Result<bool, String> {
        let mut changed = false;
        loop {
            let cursor = meta(c, "cursor").and_then(|v| v.as_i64()).unwrap_or(0);
            let r = match self.call("GET", &format!("/v1/workspaces/{ws}/changes?since={cursor}&limit=500"), None) {
                Net::Down(d) => return Err(d),
                Net::Reply(200, v) => v,
                Net::Reply(_, _) => {
                    if self.revoked(ws) {
                        self.purge(ws)?;
                        return Ok(true);
                    }
                    return Err("change feed refused".into());
                }
            };
            let projects: Vec<String> = r["projects"].as_array().into_iter().flatten().filter_map(|p| p["id"].as_str().map(String::from)).collect();
            let known: Vec<String> = meta(c, "projects").and_then(|v| serde_json::from_value::<Vec<Value>>(v).ok()).unwrap_or_default().iter().filter_map(|p| p["id"].as_str().map(String::from)).collect();
            set_meta(c, "projects", &r["projects"])?;
            // A project appearing or disappearing is a change the UI must show, even with no records.
            changed |= projects.len() != known.len() || projects.iter().any(|p| !known.contains(p));
            if cursor > 0 && projects.iter().any(|p| !known.contains(p)) {
                // A project became visible; its older records are behind the cursor.
                set_meta(c, "cursor", &json!(0))?;
                continue;
            }
            // Projects no longer visible: drop their cached records (scope removal, §7.5).
            let n = c.execute("DELETE FROM records WHERE project_id NOT IN (SELECT value FROM json_each(?1))", [json!(projects).to_string()]).map_err(e)?;
            changed |= n > 0;
            for rec in r["changes"].as_array().into_iter().flatten() {
                apply(c, rec)?;
                changed = true;
            }
            set_meta(c, "cursor", &r["next"])?;
            if r["more"] != true {
                return Ok(changed);
            }
        }
    }

    /* ----- reads and actions for the UI ----- */

    pub fn board(&self, ws: &str) -> R<Value> {
        let c = self.open_ws(ws)?;
        let rows = |sql: &str| -> R<Vec<Value>> {
            let mut st = c.prepare(sql).map_err(e)?;
            let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
            let out = st
                .query_map([], |r| {
                    let mut m = Map::new();
                    for (i, n) in names.iter().enumerate() {
                        let v: rusqlite::types::Value = r.get(i)?;
                        m.insert(n.clone(), match v {
                            rusqlite::types::Value::Integer(x) => json!(x),
                            rusqlite::types::Value::Text(s) if matches!(n.as_str(), "fields" | "mine" | "theirs" | "reason") => serde_json::from_str(&s).unwrap_or(json!(s)),
                            rusqlite::types::Value::Text(s) => json!(s),
                            _ => Value::Null,
                        });
                    }
                    Ok(Value::Object(m))
                })
                .map_err(e)?
                .filter_map(|r| r.ok())
                .collect();
            Ok(out)
        };
        let cfg = self.config().unwrap_or_default();
        let role = cfg["workspaces"].as_array().into_iter().flatten().find(|w| w["id"] == ws).map(|w| w["role"].clone()).unwrap_or(Value::Null);
        Ok(json!({
            "workspace": ws, "role": role,
            "projects": meta(&c, "projects").unwrap_or(json!([])),
            "members": meta(&c, "members").unwrap_or(json!([])),
            "records": rows("SELECT r.id, r.kind, r.project_id, r.version, r.fields, r.created_by, r.updated_by, r.updated_at,
                              (SELECT count(*) FROM outbox o WHERE o.record_id = r.id) pending
                              FROM records r WHERE r.deleted = 0 ORDER BY r.kind, r.updated_at DESC")?,
            "conflicts": rows("SELECT id, record_id, kind, mine, theirs, reason, at FROM conflicts ORDER BY id")?,
            "pending": c.query_row("SELECT count(*) FROM outbox", [], |r| r.get::<_, i64>(0)).map_err(e)?,
            // hub project id -> local project id, for projects shared from this PC (their local chats/activity show).
            "links": cfg["links"].as_object().map(|l| l.iter().filter(|(_, v)| v["workspace"] == ws).map(|(k, v)| (v["project"].as_str().unwrap_or("").to_string(), json!(k))).collect::<Map<String, Value>>()).unwrap_or_default(),
            "last_sync": meta(&c, "last_sync"),
        }))
    }

    /// "Keep mine" re-queues the user's fields against the version they now see; "keep theirs" drops the copy.
    pub fn resolve(&self, ws: &str, conflict: i64, keep_mine: bool) -> R<()> {
        let c = self.open_ws(ws)?;
        let row: Option<(String, String, String)> = c.query_row("SELECT record_id, kind, mine FROM conflicts WHERE id = ?1", [conflict], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().map_err(e)?;
        let (rid, kind, mine) = row.ok_or("That conflict is already resolved.")?;
        c.execute("DELETE FROM conflicts WHERE id = ?1", [conflict]).map_err(e)?;
        drop(c);
        if keep_mine && kind == "conflict" {
            let rec_kind: String = self.open_ws(ws)?.query_row("SELECT kind FROM records WHERE id = ?1", [&rid], |r| r.get(0)).map_err(e)?;
            self.edit(ws, "update", &rec_kind, Some(&rid), "", &serde_json::from_str(&mine).unwrap_or_default())?;
        }
        Ok(())
    }

    /// `role` is a built-in level (viewer…admin) or the name of one of the workspace's roles (e.g. "Artist").
    pub fn invite(&self, ws: &str, role: &str, projects: Option<Vec<String>>) -> R<String> {
        let builtin = ["admin", "editor", "contributor", "viewer"].contains(&role);
        let body = if builtin { json!({ "role": role, "projects": projects }) } else { json!({ "title": role, "projects": projects }) };
        let v = self.need("POST", &format!("/v1/workspaces/{ws}/invitations"), Some(&body))?;
        let url = self.config().and_then(|c| c["url"].as_str().map(String::from)).unwrap_or_default();
        Ok(format!("{}#{}", shareable(&url, crate::phone::tailscale_ip().as_deref()), v["token"].as_str().unwrap_or("")))
    }

    /// Role and member administration (D036). Only these fixed calls; the hub checks every one.
    pub fn admin(&self, ws: &str, action: &str, body: &Value) -> R<Value> {
        if action != "people" {
            self.ws_path(ws)?;
        }
        let user = || -> R<String> {
            let u = body["user_id"].as_str().unwrap_or("");
            if u.is_empty() || !u.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err("bad member id".into());
            }
            Ok(u.to_string())
        };
        let (method, path, send) = match action {
            "roles" => ("GET", format!("/v1/workspaces/{ws}/roles"), None),
            "people" => ("GET", "/v1/people".to_string(), None),
            "member_add" => {
                let b = if body["title"].is_string() { json!({ "user_id": user()?, "title": body["title"] }) } else { json!({ "user_id": user()?, "role": body["role"] }) };
                ("POST", format!("/v1/workspaces/{ws}/members"), Some(b))
            }
            "role_save" => ("POST", format!("/v1/workspaces/{ws}/roles"), Some(json!({ "name": body["name"], "base": body["base"], "grant": body["grant"], "deny": body["deny"] }))),
            "role_delete" => ("DELETE", format!("/v1/workspaces/{ws}/roles"), Some(json!({ "name": body["name"] }))),
            "member_set" => {
                let b = if body["title"].is_string() { json!({ "title": body["title"] }) } else { json!({ "role": body["role"] }) };
                ("PATCH", format!("/v1/workspaces/{ws}/members/{}", user()?), Some(b))
            }
            "member_remove" => ("DELETE", format!("/v1/workspaces/{ws}/members/{}", user()?), None),
            _ => return Err("unknown admin action".into()),
        };
        let r = match self.call(method, &path, send.as_ref()) {
            Net::Reply(200, v) => v,
            Net::Reply(409, v) if v["error"] == "role_in_use" => return Err("Someone still has that role. Give them another role first.".into()),
            Net::Reply(409, v) if v["error"] == "already_member" => return Err("They're already in that workspace.".into()),
            Net::Reply(code, v) => return Err(format!("The hub refused ({code}: {})", v["error"].as_str().unwrap_or("error"))),
            Net::Down(d) => return Err(format!("Can't reach the hub ({d}).")),
        };
        if method != "GET" {
            let _ = self.sync_ws(ws); // members/roles show the change at once
        }
        Ok(r)
    }

    pub fn new_project(&self, ws: &str, name: &str) -> R<Value> {
        let v = self.need("POST", &format!("/v1/workspaces/{ws}/projects"), Some(&json!({ "name": name })))?;
        let _ = self.sync_ws(ws);
        Ok(v)
    }

    /// Exactly what linking a local project would send (§6.1): its name and, optionally, its action items as tasks.
    /// Assignees (local team names, not hub accounts), history, paths and everything else stay on this PC.
    pub fn link_preview(name: &str, items: &[crate::team::Item]) -> Value {
        let tasks: Vec<Value> = items.iter().map(import_fields).collect();
        json!({ "project_name": name, "tasks": tasks, "kept_private": ["assignees", "item history", "who/when stamps", "file paths", "everything else in the project"] })
    }

    pub fn link(&self, ws: &str, local_id: &str, name: &str, items: Option<&[crate::team::Item]>) -> R<Value> {
        let cfg = self.config().ok_or("Join a hub first.")?;
        if cfg["links"][local_id].is_object() {
            return Err("That project is already linked.".into());
        }
        let p = self.need("POST", &format!("/v1/workspaces/{ws}/projects"), Some(&json!({ "name": name })))?;
        let pid = p["id"].as_str().ok_or("hub gave no project id")?.to_string();
        let mut cfg = self.config().unwrap_or_default();
        if !cfg["links"].is_object() {
            cfg["links"] = json!({});
        }
        cfg["links"][local_id] = json!({ "workspace": ws, "project": pid, "linked_at": now() });
        self.save_config(&cfg)?;
        let mut n = 0;
        for it in items.unwrap_or_default() {
            self.edit(ws, "create", "task", None, &pid, &import_fields(it))?;
            n += 1;
        }
        // Sync now so the new project (and its imported tasks) show at once.
        let _ = self.sync_ws(ws);
        Ok(json!({ "project_id": pid, "imported": n }))
    }
}

/// Action item → shared task fields. Only title, notes and status cross over.
fn import_fields(it: &crate::team::Item) -> Value {
    let status = match it.status.as_str() {
        "doing" => "doing",
        "done" => "done",
        _ => "todo",
    };
    let mut f = json!({ "title": it.title.chars().take(200).collect::<String>(), "status": status });
    if !it.notes.trim().is_empty() {
        f["notes"] = json!(it.notes.chars().take(4000).collect::<String>());
    }
    f
}

/// Stores a record from the hub, then lays any still-queued local edits back on top so the user keeps seeing them.
fn apply(c: &Connection, rec: &Value) -> R<()> {
    let Some(id) = rec["id"].as_str() else { return Ok(()) };
    let mut fields = rec["fields"].as_object().cloned().unwrap_or_default();
    let mut deleted = rec["deleted"] == true;
    let mut st = c.prepare("SELECT op FROM outbox WHERE record_id = ?1 ORDER BY ord").map_err(e)?;
    for op in st.query_map([id], |r| r.get::<_, String>(0)).map_err(e)?.filter_map(|r| r.ok()) {
        let op: Value = serde_json::from_str(&op).unwrap_or_default();
        if op["action"] == "delete" {
            deleted = true;
        }
        for (k, v) in op["fields"].as_object().into_iter().flatten() {
            fields.insert(k.clone(), v.clone());
        }
    }
    c.execute(
        "INSERT INTO records(id, kind, project_id, version, fields, deleted, created_by, updated_by, updated_at, seq) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
         ON CONFLICT(id) DO UPDATE SET kind=excluded.kind, project_id=excluded.project_id, version=excluded.version, fields=excluded.fields,
           deleted=excluded.deleted, created_by=excluded.created_by, updated_by=excluded.updated_by, updated_at=excluded.updated_at, seq=excluded.seq",
        params![id, rec["kind"].as_str(), rec["project_id"].as_str(), rec["version"].as_i64(), Value::Object(fields).to_string(), deleted,
                rec["created_by"].as_str(), rec["updated_by"].as_str(), rec["updated_at"].as_str(), rec["seq"].as_i64()],
    )
    .map_err(e)?;
    Ok(())
}

/* ---------- files: media and project snapshots (D035) ---------- */

/// A link teammates can open: a loopback hub address is swapped for this PC's Tailscale address.
pub fn shareable(url: &str, tailscale: Option<&str>) -> String {
    match (url.split_once("://"), tailscale) {
        (Some((scheme, rest)), Some(ip)) if rest.starts_with("127.0.0.1") || rest.starts_with("localhost") => {
            let port = rest.rsplit_once(':').map(|(_, p)| format!(":{p}")).unwrap_or_default();
            format!("{scheme}://{ip}{port}")
        }
        _ => url.to_string(),
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { B64[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

pub fn b64_decode(s: &str) -> R<Vec<u8>> {
    let s = s.split_once(',').map_or(s, |(_, d)| d); // accepts a data: URL
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for ch in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        let v = B64.iter().position(|&x| x == ch).ok_or("bad base64")? as u32;
        acc = (acc << 6 | v) & 0xffffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits & 0xff) as u8);
        }
    }
    Ok(out)
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// File names that may hold secrets never go into a snapshot, even if git tracks them.
pub fn sensitive(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    name.starts_with(".env")
        || ["id_rsa", "id_ed25519", ".npmrc", ".pypirc", ".netrc", "credentials", ".git-credentials"].iter().any(|n| name.starts_with(n))
        || [".pem", ".key", ".p12", ".pfx", ".kdbx", ".keystore", ".jks"].iter().any(|x| name.ends_with(x))
        || name.contains("secret")
        || name.contains("password")
}

/// The project's git-tracked file list (sizes, folder counts) and its map: folders as nodes, parent->child edges.
/// Only names and sizes; never contents, never the folder's location on this PC.
pub fn build_snapshot(repo: &Path) -> R<(Value, usize)> {
    let out = crate::util::run("git", &["-c", "core.quotepath=off", "ls-files", "-z"], Some(repo), Duration::from_secs(30))?;
    if !out.ok {
        return Err("This project has no git repository to snapshot.".into());
    }
    let mut files = Vec::new();
    let mut excluded = 0;
    let mut dirs: std::collections::BTreeMap<String, (u64, u64)> = std::collections::BTreeMap::new();
    for p in out.stdout.split('\0').filter(|p| !p.is_empty()) {
        if sensitive(p) {
            excluded += 1;
            continue;
        }
        let size = std::fs::metadata(repo.join(p)).map(|m| m.len()).unwrap_or(0);
        let mut d = p.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_string());
        loop {
            let e = dirs.entry(d.clone()).or_default();
            e.0 += 1;
            e.1 += size;
            match d.rsplit_once('/') {
                Some((up, _)) => d = up.to_string(),
                None if !d.is_empty() => d = String::new(),
                None => break,
            }
        }
        files.push(json!([p, size]));
        if files.len() >= 50_000 {
            break;
        }
    }
    let nodes: Vec<Value> = dirs.iter().map(|(d, (n, b))| json!({ "id": d, "files": n, "bytes": b })).collect();
    let edges: Vec<Value> = dirs.keys().filter(|d| !d.is_empty()).map(|d| json!([d.rsplit_once('/').map_or("", |(u, _)| u), d])).collect();
    Ok((json!({ "version": 1, "made_at": now(), "files": files, "map": { "nodes": nodes, "edges": edges } }), excluded))
}

impl Client {
    /// Uploads a file into a shared project, then queues its record. Needs the hub to be reachable.
    pub fn publish_file(&self, ws: &str, project: &str, kind: &str, bytes: &[u8], fields: Value) -> R<String> {
        if bytes.len() > 25 * 1024 * 1024 {
            return Err("Files can be at most 25 MB.".into());
        }
        let sha = sha256_hex(bytes);
        let req = self.request("PUT", &format!("/v1/workspaces/{ws}/projects/{project}/blobs/{sha}")).map_err(|_| "Can't reach the hub.".to_string())?;
        match Self::reply(req.send_bytes(bytes)) {
            Net::Reply(200, _) => {}
            Net::Reply(code, v) => return Err(format!("The hub refused the upload ({code}: {})", v["error"].as_str().unwrap_or("error"))),
            Net::Down(d) => return Err(format!("Can't reach the hub ({d}).")),
        }
        let mut f = fields.as_object().cloned().unwrap_or_default();
        f.insert("sha256".into(), json!(sha));
        let dir = self.dir.join("files").join(ws);
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = write_atomic(&dir.join(&sha), bytes);
        }
        self.edit(ws, "create", kind, None, project, &Value::Object(f))
    }

    /// A media or snapshot file: from this PC's cache if it still matches its hash, else downloaded and cached.
    pub fn file(&self, ws: &str, record_id: &str) -> R<Vec<u8>> {
        let c = self.open_ws(ws)?;
        let fields: String = c.query_row("SELECT fields FROM records WHERE id = ?1 AND deleted = 0", [record_id], |r| r.get(0)).map_err(|_| "That file is gone.")?;
        let sha = obj(&fields).get("sha256").and_then(Value::as_str).filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())).map(String::from).ok_or("That record has no file.")?;
        let path = self.dir.join("files").join(ws).join(&sha);
        if let Ok(b) = std::fs::read(&path) {
            if sha256_hex(&b) == sha {
                return Ok(b);
            }
        }
        let req = self.request("GET", &format!("/v1/workspaces/{ws}/media/{record_id}")).map_err(|_| "Can't reach the hub.".to_string())?;
        let res = req.call().map_err(|e| format!("Couldn't download it ({e})."))?;
        let mut b = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(res.into_reader(), 26 * 1024 * 1024), &mut b).map_err(e)?;
        if sha256_hex(&b) != sha {
            return Err("The downloaded file didn't match its fingerprint.".into());
        }
        std::fs::create_dir_all(path.parent().unwrap_or(&self.dir)).map_err(e)?;
        write_atomic(&path, &b)?;
        Ok(b)
    }

    /// Publishes a fresh snapshot for a shared project and retires this project's older ones.
    pub fn publish_snapshot(&self, ws: &str, project: &str, snap: &Value) -> R<String> {
        let files = snap["files"].as_array().map_or(0, Vec::len);
        let dirs = snap["map"]["nodes"].as_array().map_or(0, Vec::len);
        let old: Vec<String> = {
            let c = self.open_ws(ws)?;
            let mut st = c.prepare("SELECT id FROM records WHERE kind = 'snapshot' AND project_id = ?1 AND deleted = 0").map_err(e)?;
            let v = st.query_map([project], |r| r.get(0)).map_err(e)?.filter_map(|r| r.ok()).collect();
            v
        };
        let id = self.publish_file(ws, project, "snapshot", snap.to_string().as_bytes(), json!({ "files": files, "dirs": dirs }))?;
        for o in old {
            let _ = self.edit(ws, "delete", "snapshot", Some(&o), project, &Value::Null);
        }
        Ok(id)
    }
}

/// The hub installed on this PC (D035): `%LOCALAPPDATA%\ptcc-hub\ptcc-hub.exe`, its data in the same folder.
pub fn local_hub_exe() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("ptcc-hub").join("ptcc-hub.exe");
    p.is_file().then_some(p)
}

/* ---------- background sync (only once joined) ---------- */

pub struct HubState {
    pub client: Client,
    wake: Mutex<Option<Sender<()>>>,
    pub status: Mutex<Status>,
}

impl HubState {
    pub fn new(data_dir: &Path) -> HubState {
        HubState { client: Client::new(data_dir), wake: Mutex::new(None), status: Mutex::new(Status::default()) }
    }
    pub fn wake(&self) {
        if let Some(tx) = self.wake.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            let _ = tx.send(());
        }
    }
}

/// Starts the sync thread if this PC has joined a hub and it isn't running yet. No hub = nothing happens.
pub fn start(app: &tauri::AppHandle) {
    use tauri::Manager;
    let st = app.state::<HubState>();
    let mut slot = st.wake.lock().unwrap_or_else(|p| p.into_inner());
    if slot.is_some() || st.client.config().is_none_or(|c| c["revoked"] == true) {
        return;
    }
    let (tx, rx) = channel();
    *slot = Some(tx);
    let app = app.clone();
    std::thread::spawn(move || {
        use tauri::Emitter;
        let mut failures = 0u32;
        let mut was_online: Option<bool> = None;
        loop {
            let st = app.state::<HubState>();
            if st.client.config().is_none() {
                *st.wake.lock().unwrap_or_else(|p| p.into_inner()) = None;
                return;
            }
            let result = st.client.sync_all();
            {
                let mut s = st.status.lock().unwrap_or_else(|p| p.into_inner());
                match &result {
                    Ok(_) => {
                        failures = 0;
                        *s = Status { online: Some(true), last_sync: Some(now()), last_error: None };
                    }
                    Err(d) => {
                        failures += 1;
                        s.online = Some(false);
                        s.last_error = Some(d.clone());
                    }
                }
            }
            // Tell the UI when data changed or the hub went online/offline.
            let online = Some(result.is_ok());
            if result.as_ref().is_ok_and(|c| *c) || online != was_online {
                was_online = online;
                let _ = app.emit("hub-changed", ());
            }
            // §9: 60 s while the window is visible, 10 min in the tray; offline retries back off 5 s → 15 min.
            let visible = app.get_webview_window("main").and_then(|w| w.is_visible().ok()).unwrap_or(false);
            let wait = if failures > 0 {
                let jitter = u64::from(getrandom_u8()) % 5;
                (5 * 3u64.saturating_pow(failures - 1)).min(900) + jitter
            } else if visible {
                60
            } else {
                600
            };
            match rx.recv_timeout(Duration::from_secs(wait)) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
}

fn getrandom_u8() -> u8 {
    let mut b = [0u8; 1];
    let _ = getrandom::getrandom(&mut b);
    b[0]
}

/* ---------- commands ---------- */

use crate::AppState;
use tauri::{AppHandle, State};

#[tauri::command(async)]
pub fn hub_status(h: State<HubState>) -> R<Value> {
    let Some(cfg) = h.client.config() else { return Ok(json!({ "joined": false })) };
    let s = h.status.lock().unwrap_or_else(|p| p.into_inner());
    Ok(json!({ "joined": true, "url": cfg["url"], "name": cfg["name"], "user_id": cfg["user_id"], "revoked": cfg["revoked"] == true, "workspaces": cfg["workspaces"],
               "links": cfg["links"], "online": s.online, "last_sync": s.last_sync, "last_error": s.last_error }))
}

#[tauri::command(async)]
pub fn hub_join(app: AppHandle, h: State<HubState>, link: String, name: String) -> R<Value> {
    if name.trim().is_empty() {
        return Err("Enter the name your team will see.".into());
    }
    let device = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into());
    let r = h.client.join(&link, &name, &device)?;
    start(&app);
    h.wake();
    Ok(r)
}

#[tauri::command(async)]
pub fn hub_leave(h: State<HubState>) -> R<()> {
    let r = h.client.leave();
    h.wake(); // the thread sees no config and exits
    r
}

#[tauri::command(async)]
pub fn hub_sync_now(h: State<HubState>) {
    h.wake();
}

#[tauri::command(async)]
pub fn hub_board(h: State<HubState>, workspace: String) -> R<Value> {
    h.client.board(&workspace)
}

#[tauri::command(async)]
pub fn hub_edit(h: State<HubState>, workspace: String, action: String, kind: String, record_id: Option<String>, project_id: String, fields: Value) -> R<String> {
    let id = h.client.edit(&workspace, &action, &kind, record_id.as_deref(), &project_id, &fields)?;
    h.wake();
    Ok(id)
}

#[tauri::command(async)]
pub fn hub_resolve(h: State<HubState>, workspace: String, conflict: i64, keep_mine: bool) -> R<()> {
    h.client.resolve(&workspace, conflict, keep_mine)?;
    h.wake();
    Ok(())
}

#[tauri::command(async)]
pub fn hub_invite(h: State<HubState>, workspace: String, role: String, projects: Option<Vec<String>>) -> R<String> {
    h.client.invite(&workspace, &role, projects)
}

#[tauri::command(async)]
pub fn hub_admin(h: State<HubState>, workspace: String, action: String, body: Option<Value>) -> R<Value> {
    h.client.admin(&workspace, &action, &body.unwrap_or(Value::Null))
}

#[tauri::command(async)]
pub fn hub_new_project(h: State<HubState>, workspace: String, name: String) -> R<Value> {
    h.client.new_project(&workspace, &name)
}

fn local_project(st: &State<AppState>, id: &str) -> R<(String, Vec<crate::team::Item>)> {
    let c = st.db.lock().map_err(|_| "database lock poisoned")?;
    let (name, vault): (String, Option<String>) = c.query_row("SELECT name, vault_dir FROM projects WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|_| "unknown project")?;
    let items = crate::team::file_for(vault.as_deref()).and_then(|p| crate::team::read(&p)).unwrap_or_default();
    Ok((name, items))
}

#[tauri::command(async)]
pub fn hub_link_preview(st: State<AppState>, project: String) -> R<Value> {
    let (name, items) = local_project(&st, &project)?;
    Ok(Client::link_preview(&name, &items))
}

#[tauri::command(async)]
pub fn hub_link(st: State<AppState>, h: State<HubState>, project: String, workspace: String, import: bool) -> R<Value> {
    let (name, items) = local_project(&st, &project)?;
    let r = h.client.link(&workspace, &project, &name, import.then_some(&items[..]))?;
    h.wake();
    Ok(r)
}

fn mime(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

#[tauri::command(async)]
pub fn hub_upload(h: State<HubState>, workspace: String, project_id: String, name: String, caption: Option<String>, data: String) -> R<String> {
    let bytes = b64_decode(&data)?;
    let mut f = json!({ "name": name.chars().take(200).collect::<String>() });
    if let Some(c) = caption.filter(|c| !c.trim().is_empty()) {
        f["caption"] = json!(c.chars().take(1000).collect::<String>());
    }
    let id = h.client.publish_file(&workspace, &project_id, "media", &bytes, f)?;
    h.wake();
    Ok(id)
}

/// Media as a data: URL for <img>/<video> (never opened as a page); a snapshot (no name) as parsed JSON.
#[tauri::command(async)]
pub fn hub_file(h: State<HubState>, workspace: String, record_id: String, name: Option<String>) -> R<Value> {
    let b = h.client.file(&workspace, &record_id)?;
    match name {
        None => serde_json::from_slice(&b).map_err(|_| "That snapshot is unreadable.".to_string()),
        Some(n) => Ok(json!(format!("data:{};base64,{}", mime(&n), b64_encode(&b)))),
    }
}

fn repo_of(st: &State<AppState>, id: &str) -> R<PathBuf> {
    let c = st.db.lock().map_err(|_| "database lock poisoned")?;
    let repo: Option<String> = c.query_row("SELECT repo_path FROM projects WHERE id = ?1", [id], |r| r.get(0)).map_err(|_| "unknown project")?;
    repo.map(PathBuf::from).filter(|p| p.is_dir()).ok_or_else(|| "This project has no git repository to snapshot.".into())
}

/// Exactly what a snapshot would send: counts, top-level folders, and how many secret-looking files are left out.
#[tauri::command(async)]
pub fn hub_snapshot_preview(st: State<AppState>, project: String) -> R<Value> {
    let (snap, excluded) = build_snapshot(&repo_of(&st, &project)?)?;
    let top: Vec<Value> = snap["map"]["nodes"].as_array().into_iter().flatten().filter(|n| n["id"].as_str().is_some_and(|d| !d.is_empty() && !d.contains('/'))).cloned().collect();
    Ok(json!({ "files": snap["files"].as_array().map_or(0, Vec::len), "dirs": snap["map"]["nodes"].as_array().map_or(0, Vec::len), "excluded": excluded, "top": top,
               "sent": ["file paths and sizes tracked by git", "the folder structure (the map)"],
               "never": ["file contents", "untracked or ignored files", "secret-looking files (.env, keys, credentials)", "chats", "where the folder is on this PC"] }))
}

#[tauri::command(async)]
pub fn hub_snapshot_publish(st: State<AppState>, h: State<HubState>, project: String) -> R<String> {
    let link = h.client.config().map(|c| c["links"][&project].clone()).filter(|l| l.is_object()).ok_or("Share this project with a workspace first.")?;
    let (snap, _) = build_snapshot(&repo_of(&st, &project)?)?;
    let id = h.client.publish_snapshot(link["workspace"].as_str().unwrap_or(""), link["project"].as_str().unwrap_or(""), &snap)?;
    h.wake();
    Ok(id)
}

/// `%LOCALAPPDATA%\ptcc-hub\share`: the Command Center installer to send teammates.
fn share_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("ptcc-hub").join("share"))
}

/// The hub installed on this PC, if any; whether it answers; this PC's Tailscale address; the teammate installer.
#[tauri::command(async)]
pub fn hub_host_status() -> Value {
    let up = ureq::AgentBuilder::new().timeout(Duration::from_secs(2)).build().get("http://127.0.0.1:8770/v1/health").call().is_ok();
    let installer = share_dir().and_then(|d| std::fs::read_dir(d).ok()).into_iter().flatten().filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy().ends_with(".exe"));
    json!({ "installed": local_hub_exe().is_some(), "running": up, "tailscale": crate::phone::tailscale_ip(), "installer": installer })
}

/// Opens the folder holding the teammate installer (fixed path; nothing else can be opened this way).
#[tauri::command(async)]
pub fn hub_open_share_folder() -> R<()> {
    let d = share_dir().ok_or("no app data folder")?;
    std::fs::create_dir_all(&d).map_err(e)?;
    crate::commands::shell_open(&d.to_string_lossy())
}

/// Creates a workspace on the hub installed on this PC and joins it as its owner.
#[tauri::command(async)]
pub fn hub_host_create_workspace(app: AppHandle, h: State<HubState>, name: String, me: Option<String>) -> R<Value> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err("Give the workspace a name (up to 80 characters).".into());
    }
    let exe = local_hub_exe().ok_or("The hub isn't installed on this PC.")?;
    let out = crate::util::run(&exe.to_string_lossy(), &["new-workspace", name], None, Duration::from_secs(30))?;
    let token: String = out.stdout.split('#').nth(1).map(|t| t.chars().take_while(|c| c.is_ascii_hexdigit()).collect()).unwrap_or_default();
    if token.len() != 64 {
        return Err("The hub didn't create the workspace.".into());
    }
    // The hub on this PC serves on 8770; joining it while joined elsewhere fails with a clear message.
    let url = "http://127.0.0.1:8770".to_string();
    let who = me.filter(|m| !m.trim().is_empty()).or_else(|| h.client.config().and_then(|c| c["name"].as_str().map(String::from))).unwrap_or_else(|| "Owner".into());
    let device = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into());
    let r = h.client.join(&format!("{url}#{token}"), &who, &device)?;
    start(&app);
    h.wake();
    Ok(r)
}

/* ---------- tests: against a real hub on loopback ---------- */

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Hub {
        url: String,
        data: PathBuf,
        server: Arc<tiny_http::Server>,
        hub: Arc<ptcc_hub::api::Hub>,
    }
    impl Drop for Hub {
        fn drop(&mut self) {
            self.server.unblock();
            let _ = std::fs::remove_dir_all(&self.data);
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ptcc-hubclient-{tag}-{}", uuid().unwrap()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn hub() -> Hub {
        let data = tmp("hub");
        let hub = Arc::new(ptcc_hub::api::Hub::new(ptcc_hub::store::open(Some(&data)).unwrap(), Some(data.join("media"))));
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let url = format!("http://127.0.0.1:{}", server.server_addr().to_ip().unwrap().port());
        ptcc_hub::serve(hub.clone(), server.clone(), 4);
        Hub { url, data, server, hub }
    }

    fn owner_invite(h: &Hub) -> (String, String) {
        let (ws, token) = ptcc_hub::api::new_workspace(&h.hub.db.lock().unwrap(), "FLY WING").unwrap();
        (ws, format!("{}#{token}", h.url))
    }

    fn set_url(c: &Client, url: &str) {
        let mut cfg = c.config().unwrap();
        cfg["url"] = json!(url);
        c.save_config(&cfg).unwrap();
    }

    #[test]
    fn nothing_exists_until_joined() {
        let d = tmp("idle");
        let c = Client::new(&d);
        assert!(c.config().is_none());
        assert_eq!(c.sync_all(), Ok(false), "no hub, no requests");
        assert!(!d.join("hub").exists(), "no files or folders are created without a hub");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn urls_and_invites_are_checked() {
        for ok in ["http://127.0.0.1:8770", "http://100.101.1.2:8770/", "https://hub.tail1234.ts.net", "http://localhost:8770", "http://[fd7a:115c:a1e0::1]:8770"] {
            assert!(check_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://192.168.1.4:8770", "http://example.com", "ftp://x", "hub", "http://100.200.1.1",
            // Parser tricks: user info, paths, queries, fragments, backslashes, bad ports, stray brackets.
            "http://100.64.0.1:80@evil.com", "http://100.64.0.1@evil.com", "http://user:pw@127.0.0.1:8770", "https://a@b.ts.net",
            "http://127.0.0.1:8770/path", "http://127.0.0.1:8770?x", r"http://127.0.0.1\@evil.com", "http://127.0.0.1:99999",
            "http://127.0.0.1:0", "http://127.0.0.1:80:81", "http://[127.0.0.1]:80", "http://[::1", "http://127.0.0.1]", "http://",
            "http://127.0.0.1 :80", "http://evil.com#127.0.0.1",
        ] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
        assert!(parse_invite("http://127.0.0.1:1#abc").is_err());
        assert!(parse_invite(&format!("http://100.64.0.1:80@evil.com#{}", "a".repeat(64))).is_err());
        assert!(parse_invite(&format!("http://127.0.0.1:1#{}", "a".repeat(64))).is_ok());
    }

    #[test]
    fn a_tampered_saved_address_never_gets_the_bearer() {
        let h = hub();
        let (_, link) = owner_invite(&h);
        let d = tmp("tamper");
        let c = Client::new(&d);
        c.join(&link, "Rich", "PC").unwrap();
        set_url(&c, "http://100.64.0.1:80@127.0.0.1:9");
        assert!(matches!(c.call("GET", "/v1/me", None), Net::Down(m) if m.contains("not allowed")));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn base64_round_trips_and_links_use_tailscale() {
        for b in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0u8, 255, 128, 7][..]] {
            assert_eq!(b64_decode(&b64_encode(b)).unwrap(), b);
        }
        let big: Vec<u8> = (0..100_000u32).map(|i| (i * 7 % 251) as u8).collect();
        assert_eq!(b64_decode(&b64_encode(&big)).unwrap(), big);
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64_decode("data:image/png;base64,Zm9vYg==").unwrap(), b"foob");
        assert_eq!(shareable("http://127.0.0.1:8770", Some("100.1.2.3")), "http://100.1.2.3:8770");
        assert_eq!(shareable("http://127.0.0.1:8770", None), "http://127.0.0.1:8770");
        assert_eq!(shareable("https://hub.ts.net", Some("100.1.2.3")), "https://hub.ts.net");
    }

    #[test]
    fn snapshots_hold_tracked_names_only_and_skip_secrets() {
        let repo = tmp("repo");
        let git = |args: &[&str]| crate::util::run("git", args, Some(&repo), Duration::from_secs(30)).unwrap();
        git(&["init", "-q"]);
        for (p, body) in [("src/main.rs", "fn main(){}"), ("src/ui/app.tsx", "x"), (".env", "KEY=supersecret"), ("keys/server.pem", "-----"), ("README.md", "hi")] {
            let f = repo.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        }
        std::fs::write(repo.join("untracked.txt"), "x").unwrap();
        git(&["add", "src", ".env", "keys", "README.md"]);
        let (snap, excluded) = build_snapshot(&repo).unwrap();
        let text = snap.to_string();
        assert_eq!(excluded, 2);
        let folder = repo.file_name().unwrap().to_string_lossy().to_string();
        for leak in [".env", "server.pem", "supersecret", "untracked", "fn main", folder.as_str()] {
            assert!(!text.contains(leak), "{leak} leaked");
        }
        assert_eq!(snap["files"].as_array().unwrap().len(), 3);
        let dirs: Vec<&str> = snap["map"]["nodes"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap()).collect();
        assert_eq!(dirs, ["", "src", "src/ui"]);
        assert_eq!(snap["map"]["nodes"][0]["files"], 3);
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn media_and_snapshots_round_trip_through_the_hub() {
        let h = hub();
        let (ws, link) = owner_invite(&h);
        let (ad, bd) = (tmp("a"), tmp("b"));
        let a = Client::new(&ad);
        a.join(&link, "Rich", "PC").unwrap();
        let p = a.new_project(&ws, "site").unwrap()["id"].as_str().unwrap().to_string();
        let png = b"\x89PNG\r\n fake".to_vec();
        let id = a.publish_file(&ws, &p, "media", &png, json!({ "name": "cover.png" })).unwrap();
        a.sync_all().unwrap();
        let b = Client::new(&bd);
        b.join(&a.invite(&ws, "viewer", None).unwrap(), "Sam", "Laptop").unwrap();
        b.sync_all().unwrap();
        assert_eq!(b.file(&ws, &id).unwrap(), png, "downloaded");
        assert_eq!(b.file(&ws, &id).unwrap(), png, "from cache");
        // A tampered cached copy is ignored and fetched again.
        std::fs::write(bd.join("hub/files").join(&ws).join(sha256_hex(&png)), b"tampered").unwrap();
        assert_eq!(b.file(&ws, &id).unwrap(), png);
        let s1 = a.publish_snapshot(&ws, &p, &json!({ "files": [["a.rs", 1]], "map": { "nodes": [{ "id": "" }], "edges": [] } })).unwrap();
        a.sync_all().unwrap();
        let s2 = a.publish_snapshot(&ws, &p, &json!({ "files": [], "map": { "nodes": [], "edges": [] } })).unwrap();
        a.sync_all().unwrap();
        b.sync_all().unwrap();
        let recs = b.board(&ws).unwrap()["records"].to_string();
        assert!(recs.contains(&s2) && !recs.contains(&s1), "older snapshot retired");
        // Removal deletes cached files too.
        let sam = b.config().unwrap()["user_id"].as_str().unwrap().to_string();
        a.need("DELETE", &format!("/v1/workspaces/{ws}/members/{sam}"), None).unwrap();
        b.sync_all().unwrap();
        assert!(!bd.join("hub/files").join(&ws).exists());
        for d in [ad, bd] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn secret_is_encrypted_at_rest() {
        let s = b"d_x.0123456789abcdef";
        let p = dpapi::protect(s).unwrap();
        assert!(!p.windows(s.len()).any(|w| w == s));
        assert_eq!(dpapi::unprotect(&p).unwrap(), s);
    }

    #[test]
    fn join_edit_offline_queue_sync_and_second_member() {
        let h = hub();
        let (ws, link) = owner_invite(&h);
        let a_dir = tmp("a");
        let a = Client::new(&a_dir);
        a.join(&link, "Rich", "PC").unwrap();
        assert_eq!(a.workspaces(), std::slice::from_ref(&ws));
        let key = std::fs::read(a_dir.join("hub/device.key")).unwrap();
        assert!(!String::from_utf8_lossy(&key).contains("d_"), "token not stored in clear");
        assert!(!std::fs::read_to_string(a_dir.join("hub/hub.json")).unwrap().contains(&link[link.len() - 64..]));
        let p = a.new_project(&ws, "site").unwrap()["id"].as_str().unwrap().to_string();
        // Invite a teammate; they join from their own PC.
        let invite = a.invite(&ws, "editor", None).unwrap();
        let b_dir = tmp("b");
        let b = Client::new(&b_dir);
        b.join(&invite, "Sam", "Laptop").unwrap();
        assert!(b.join(&invite, "Sam", "Laptop").is_err(), "invite is single use");
        // Rich adds a task while the hub is unreachable: it shows locally and waits in the outbox.
        set_url(&a, "http://127.0.0.1:9");
        let t = a.edit(&ws, "create", "task", None, &p, &json!({ "title": "Mint page copy", "status": "todo" })).unwrap();
        assert!(a.sync_all().is_err());
        let board = a.board(&ws).unwrap();
        assert_eq!(board["pending"], 1);
        assert_eq!(board["records"][0]["fields"]["title"], "Mint page copy");
        // Back online: it reaches the hub and Sam sees it.
        set_url(&a, &h.url);
        a.sync_all().unwrap();
        assert_eq!(a.board(&ws).unwrap()["pending"], 0);
        b.sync_all().unwrap();
        let bb = b.board(&ws).unwrap();
        assert_eq!(bb["records"][0]["fields"]["title"], "Mint page copy");
        assert_eq!(bb["records"][0]["version"], 1);
        assert!(bb["members"].as_array().unwrap().len() == 2);
        // Disjoint edits merge.
        a.edit(&ws, "update", "task", Some(&t), "", &json!({ "status": "doing" })).unwrap();
        b.edit(&ws, "update", "task", Some(&t), "", &json!({ "notes": "use the new logo" })).unwrap();
        a.sync_all().unwrap();
        b.sync_all().unwrap();
        a.sync_all().unwrap();
        for c in [&a, &b] {
            let f = &c.board(&ws).unwrap()["records"][0]["fields"];
            assert_eq!((f["status"].as_str(), f["notes"].as_str()), (Some("doing"), Some("use the new logo")));
        }
        for d in [a_dir, b_dir] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn conflicts_keep_both_sides_and_resolve() {
        let h = hub();
        let (ws, link) = owner_invite(&h);
        let (ad, bd) = (tmp("a"), tmp("b"));
        let a = Client::new(&ad);
        a.join(&link, "Rich", "PC").unwrap();
        let p = a.new_project(&ws, "site").unwrap()["id"].as_str().unwrap().to_string();
        let b = Client::new(&bd);
        b.join(&a.invite(&ws, "editor", None).unwrap(), "Sam", "Laptop").unwrap();
        let t = a.edit(&ws, "create", "task", None, &p, &json!({ "title": "Old" })).unwrap();
        a.sync_all().unwrap();
        b.sync_all().unwrap();
        a.edit(&ws, "update", "task", Some(&t), "", &json!({ "title": "Rich's" })).unwrap();
        b.edit(&ws, "update", "task", Some(&t), "", &json!({ "title": "Sam's" })).unwrap();
        b.sync_all().unwrap();
        a.sync_all().unwrap();
        let board = a.board(&ws).unwrap();
        assert_eq!(board["records"][0]["fields"]["title"], "Sam's");
        let cf = &board["conflicts"][0];
        assert_eq!((cf["kind"].as_str(), cf["mine"]["title"].as_str()), (Some("conflict"), Some("Rich's")));
        a.resolve(&ws, cf["id"].as_i64().unwrap(), true).unwrap();
        a.sync_all().unwrap();
        b.sync_all().unwrap();
        assert_eq!(b.board(&ws).unwrap()["records"][0]["fields"]["title"], "Rich's");
        assert_eq!(a.board(&ws).unwrap()["conflicts"], json!([]));
        for d in [ad, bd] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn removal_saves_unsent_work_and_deletes_the_cache() {
        let h = hub();
        let (ws, link) = owner_invite(&h);
        let (ad, bd) = (tmp("a"), tmp("b"));
        let a = Client::new(&ad);
        a.join(&link, "Rich", "PC").unwrap();
        let p = a.new_project(&ws, "site").unwrap()["id"].as_str().unwrap().to_string();
        let b = Client::new(&bd);
        b.join(&a.invite(&ws, "contributor", None).unwrap(), "Sam", "Laptop").unwrap();
        b.edit(&ws, "create", "task", None, &p, &json!({ "title": "synced" })).unwrap();
        b.sync_all().unwrap();
        // Sam goes offline with an edit queued; Rich removes Sam.
        set_url(&b, "http://127.0.0.1:9");
        b.edit(&ws, "create", "task", None, &p, &json!({ "title": "Sam's unsent idea" })).unwrap();
        let sam = b.config().unwrap()["user_id"].as_str().unwrap().to_string();
        a.need("DELETE", &format!("/v1/workspaces/{ws}/members/{sam}"), None).unwrap();
        set_url(&b, &h.url);
        b.sync_all().unwrap();
        assert!(b.workspaces().is_empty());
        assert!(!bd.join(format!("hub/{ws}.db")).exists(), "workspace cache deleted");
        let unsent: Vec<_> = std::fs::read_dir(bd.join("hub/unsent")).unwrap().collect();
        assert_eq!(unsent.len(), 1);
        let saved = std::fs::read_to_string(unsent[0].as_ref().unwrap().path()).unwrap();
        assert!(saved.contains("Sam's unsent idea") && !saved.contains("\"synced\""), "only their own unsent work");
        assert!(b.board(&ws).is_err());
        // Rich's view never got the unsent task.
        a.sync_all().unwrap();
        assert_eq!(a.board(&ws).unwrap()["records"].as_array().unwrap().len(), 1);
        for d in [ad, bd] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn refused_edits_are_kept_as_copies_and_the_hub_state_restored() {
        let h = hub();
        let (ws, link) = owner_invite(&h);
        let (ad, bd) = (tmp("a"), tmp("b"));
        let a = Client::new(&ad);
        a.join(&link, "Rich", "PC").unwrap();
        let p = a.new_project(&ws, "site").unwrap()["id"].as_str().unwrap().to_string();
        let t = a.edit(&ws, "create", "task", None, &p, &json!({ "title": "Rich's task" })).unwrap();
        a.sync_all().unwrap();
        let v = Client::new(&bd);
        v.join(&a.invite(&ws, "viewer", None).unwrap(), "Viewer", "Tablet").unwrap();
        v.sync_all().unwrap();
        v.edit(&ws, "update", "task", Some(&t), "", &json!({ "title": "vandalised" })).unwrap();
        v.edit(&ws, "create", "task", None, &p, &json!({ "title": "not allowed" })).unwrap();
        v.sync_all().unwrap();
        let board = v.board(&ws).unwrap();
        assert_eq!(board["records"].as_array().unwrap().len(), 1, "refused create removed locally");
        assert_eq!(board["records"][0]["fields"]["title"], "Rich's task", "refused edit rolled back to the hub's state");
        assert_eq!(board["conflicts"].as_array().unwrap().iter().filter(|c| c["kind"] == "rejected").count(), 2);
        assert_eq!(v.workspaces(), std::slice::from_ref(&ws), "a refusal isn't a revocation");
        for d in [ad, bd] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn link_preview_and_import_send_only_allowed_fields() {
        let h = hub();
        let (ws, link) = owner_invite(&h);
        let ad = tmp("a");
        let a = Client::new(&ad);
        a.join(&link, "Rich", "PC").unwrap();
        let items = vec![
            crate::team::Item { id: "1".into(), title: "Order merch".into(), notes: "C:\\Users\\private\\notes.txt".into(), status: "doing".into(), assignee: Some("Rich".into()), ..Default::default() },
            crate::team::Item { id: "2".into(), title: "Done thing".into(), status: "done".into(), ..Default::default() },
        ];
        let preview = Client::link_preview("BOMBSQUAD", &items);
        assert_eq!(preview["tasks"][0], json!({ "title": "Order merch", "status": "doing", "notes": "C:\\Users\\private\\notes.txt" }));
        assert!(preview["tasks"][0].get("assignee").is_none());
        let r = a.link(&ws, "bombsquad", "BOMBSQUAD", Some(&items)).unwrap();
        assert_eq!(r["imported"], 2);
        let board = a.board(&ws).unwrap();
        assert_eq!(board["projects"][0]["name"], "BOMBSQUAD", "shows at once, without waiting for the sync thread");
        assert_eq!(board["links"][r["project_id"].as_str().unwrap()], "bombsquad");
        assert_eq!(board["pending"], 0);
        assert!(a.link(&ws, "bombsquad", "BOMBSQUAD", None).is_err(), "linked once");
        a.sync_all().unwrap();
        let recs = a.board(&ws).unwrap()["records"].clone();
        assert_eq!(recs.as_array().unwrap().len(), 2);
        assert!(recs.to_string().contains("Done thing"));
        let _ = std::fs::remove_dir_all(ad);
    }

    #[test]
    fn losing_a_project_drops_its_cached_tasks_and_regaining_it_resyncs() {
        let h = hub();
        let (ws, link) = owner_invite(&h);
        let (ad, bd) = (tmp("a"), tmp("b"));
        let a = Client::new(&ad);
        a.join(&link, "Rich", "PC").unwrap();
        let p1 = a.new_project(&ws, "site").unwrap()["id"].as_str().unwrap().to_string();
        let p2 = a.new_project(&ws, "contracts").unwrap()["id"].as_str().unwrap().to_string();
        a.edit(&ws, "create", "task", None, &p1, &json!({ "title": "site task" })).unwrap();
        a.edit(&ws, "create", "task", None, &p2, &json!({ "title": "contracts task" })).unwrap();
        a.sync_all().unwrap();
        let b = Client::new(&bd);
        b.join(&a.invite(&ws, "editor", None).unwrap(), "Sam", "Laptop").unwrap();
        b.sync_all().unwrap();
        assert_eq!(b.board(&ws).unwrap()["records"].as_array().unwrap().len(), 2);
        let sam = b.config().unwrap()["user_id"].as_str().unwrap().to_string();
        a.need("PATCH", &format!("/v1/workspaces/{ws}/members/{sam}"), Some(&json!({ "projects": [p1] }))).unwrap();
        b.sync_all().unwrap();
        let recs = b.board(&ws).unwrap()["records"].to_string();
        assert!(recs.contains("site task") && !recs.contains("contracts task"), "{recs}");
        a.need("PATCH", &format!("/v1/workspaces/{ws}/members/{sam}"), Some(&json!({ "projects": null }))).unwrap();
        b.sync_all().unwrap();
        assert!(b.board(&ws).unwrap()["records"].to_string().contains("contracts task"));
        for d in [ad, bd] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn paper_sets_roles_and_profit_joins_as_ops() {
        let h = hub();
        let (ws, link) = owner_invite(&h);
        let (ad, bd) = (tmp("a"), tmp("b"));
        let a = Client::new(&ad);
        a.join(&link, "Paper", "PC").unwrap();
        let roles = a.admin(&ws, "roles", &Value::Null).unwrap();
        assert_eq!(roles["roles"].as_array().unwrap().len(), 2);
        assert!(roles["bases"]["editor"].as_array().unwrap().iter().any(|p| p == "task.assign"));
        a.admin(&ws, "role_save", &json!({ "name": "Mod", "base": "contributor", "grant": ["milestone.edit"], "deny": [] })).unwrap();
        let b = Client::new(&bd);
        b.join(&a.invite(&ws, "Ops", None).unwrap(), "Profit", "Laptop").unwrap();
        a.sync_all().unwrap();
        let members = a.board(&ws).unwrap()["members"].clone();
        let profit = members.as_array().unwrap().iter().find(|m| m["name"] == "Profit").unwrap().clone();
        assert_eq!((profit["title"].as_str(), profit["role"].as_str()), (Some("Ops"), Some("editor")));
        let uid = profit["user_id"].as_str().unwrap().to_string();
        a.admin(&ws, "member_set", &json!({ "user_id": uid, "title": "Mod" })).unwrap();
        assert!(a.board(&ws).unwrap()["members"].to_string().contains("\"Mod\""), "shows at once");
        assert!(a.admin(&ws, "role_delete", &json!({ "name": "Mod" })).unwrap_err().contains("still has that role"));
        assert!(a.admin(&ws, "member_set", &json!({ "user_id": "../x", "role": "viewer" })).is_err());
        assert!(a.admin(&ws, "drop_table", &Value::Null).is_err());
        // Paper puts Profit into a second workspace from the People view, then takes them out of the first.
        let (ws2, inv2) = ptcc_hub::api::new_workspace(&h.hub.db.lock().unwrap(), "BOMBSQUAD").unwrap();
        a.join(&format!("{}#{inv2}", h.url), "Paper", "PC").unwrap();
        let people = a.admin("", "people", &Value::Null).unwrap();
        assert_eq!(people["workspaces"].as_array().unwrap().len(), 2);
        a.admin(&ws2, "member_add", &json!({ "user_id": uid, "title": "Artist" })).unwrap();
        b.sync_all().unwrap();
        assert_eq!(b.workspaces().len(), 2);
        a.admin(&ws, "member_remove", &json!({ "user_id": uid })).unwrap();
        b.sync_all().unwrap();
        assert_eq!(b.workspaces(), std::slice::from_ref(&ws2), "out of the first, still in the second");
        for d in [ad, bd] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn leaving_revokes_the_device_and_deletes_hub_data() {
        let h = hub();
        let (ws, link) = owner_invite(&h);
        let ad = tmp("a");
        let a = Client::new(&ad);
        a.join(&link, "Rich", "PC").unwrap();
        let token = a.token().unwrap();
        a.leave().unwrap();
        assert!(a.config().is_none() && !ad.join("hub/device.key").exists() && !ad.join(format!("hub/{ws}.db")).exists());
        let r = h.hub.handle("GET", "/v1/me", Some(&format!("Bearer {token}")), b"");
        assert_eq!(r.code, 401, "the hub forgot this device");
        let _ = std::fs::remove_dir_all(ad);
    }
}
