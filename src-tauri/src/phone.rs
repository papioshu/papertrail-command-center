//! Phone access (D027): the same UI, read-only, served to paired phones over Tailscale.
//! Off until switched on in Settings. Binds 127.0.0.1 and this PC's Tailscale address only, never 0.0.0.0.
//! Every /api call needs a paired device's bearer key and may only run the read commands in `call`.
use crate::{commands, db, AppState};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager, State};

// shortcut: fixed port, make it a setting if something else on this PC ever needs 8765.
pub const PORT: u16 = 8765;
const MAX_BODY: u64 = 64 * 1024;
/// Requests handled at once; past this the server answers 503 and the phone waits and repeats (D028).
const MAX_INFLIGHT: usize = 32;
static INFLIGHT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Takes a request slot, or None when all MAX_INFLIGHT are busy. The slot frees itself when dropped.
struct Slot;
impl Slot {
    fn take() -> Option<Slot> {
        use std::sync::atomic::Ordering::SeqCst;
        INFLIGHT.fetch_update(SeqCst, SeqCst, |n| (n < MAX_INFLIGHT).then_some(n + 1)).ok().map(|_| Slot)
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        INFLIGHT.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[derive(Default)]
pub struct Status {
    pub listening: Vec<String>,
    pub tailscale_ip: Option<String>,
    pub error: Option<String>,
    pub last_seen: HashMap<String, String>,
}

pub struct Phone {
    pub status: Mutex<Status>,
    wake: Mutex<Sender<()>>,
}

type R<T> = Result<T, String>;

fn hash(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn random_hex(n: usize) -> R<String> {
    let mut b = vec![0u8; n];
    getrandom::getrandom(&mut b).map_err(|e| e.to_string())?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn devices(c: &rusqlite::Connection) -> Vec<Value> {
    db::setting(c, "phone_devices").and_then(|v| v.as_array().cloned()).unwrap_or_default()
}

/// The paired device a bearer header belongs to, if any.
pub(crate) fn device_for(devs: &[Value], authorization: Option<&str>) -> Option<String> {
    let token = authorization?.strip_prefix("Bearer ")?.trim();
    if token.len() < 32 {
        return None;
    }
    let h = hash(token);
    devs.iter().find(|d| d["hash"].as_str() == Some(h.as_str())).and_then(|d| d["id"].as_str().map(String::from))
}

pub(crate) fn tailscale_ip() -> Option<String> {
    for exe in ["tailscale", r"C:\Program Files\Tailscale\tailscale.exe"] {
        if let Ok(o) = crate::util::run(exe, &["ip", "-4"], None, Duration::from_secs(5)) {
            if let Some(ip) = o.stdout.lines().map(str::trim).find(|l| l.starts_with("100.")) {
                return Some(ip.to_string());
            }
        }
    }
    None
}

/// This PC's tailnet HTTPS address (https://name.tailnet.ts.net) when the tailnet has HTTPS certificates turned on.
/// Phones need HTTPS for the microphone (Ghost voice, D052).
pub(crate) fn tailscale_https() -> Option<String> {
    for exe in ["tailscale", r"C:\Program Files\Tailscale\tailscale.exe"] {
        if let Ok(o) = crate::util::run(exe, &["status", "--json"], None, Duration::from_secs(5)) {
            let v: serde_json::Value = serde_json::from_str(&o.stdout).ok()?;
            let name = v["Self"]["DNSName"].as_str()?.trim_end_matches('.').to_string();
            let certs = v["CertDomains"].as_array().is_some_and(|c| c.iter().any(|d| d == name.as_str()));
            return (certs && name.ends_with(".ts.net")).then(|| format!("https://{name}"));
        }
    }
    None
}

fn enabled(app: &AppHandle) -> bool {
    let st = app.state::<AppState>();
    let on = st.db.lock().ok().and_then(|c| db::setting(&c, "phone_enabled")).and_then(|v| v.as_bool());
    on.unwrap_or(false)
}

/// Starts the supervisor: (re)binds when the switch or the Tailscale address changes, checked every 20 s.
pub fn spawn(app: AppHandle) -> Phone {
    let (tx, rx) = channel();
    let handle = app.clone();
    std::thread::spawn(move || supervise(handle, rx));
    Phone { status: Mutex::new(Status::default()), wake: Mutex::new(tx) }
}

fn supervise(app: AppHandle, rx: Receiver<()>) {
    let mut servers: Vec<Arc<tiny_http::Server>> = Vec::new();
    let mut bound: Vec<String> = Vec::new();
    loop {
        let on = enabled(&app);
        let ip = if on { tailscale_ip() } else { None };
        let mut want = Vec::new();
        if on {
            want.push(format!("127.0.0.1:{PORT}"));
            if let Some(ip) = &ip {
                want.push(format!("{ip}:{PORT}"));
            }
        }
        let mut error = None;
        if want != bound {
            for s in servers.drain(..) {
                s.unblock();
            }
            bound.clear();
            for addr in &want {
                match tiny_http::Server::http(addr.as_str()) {
                    Ok(s) => {
                        let s = Arc::new(s);
                        let (srv, app) = (s.clone(), app.clone());
                        // One thread per request, capped: a phone that vanishes mid-reply can't hold up anyone else,
                        // and a flood of connections can't start unbounded threads.
                        std::thread::spawn(move || {
                            while let Ok(req) = srv.recv() {
                                let Some(slot) = Slot::take() else {
                                    json_reply(req, 503, json!({"error": "busy, try again"}));
                                    continue;
                                };
                                let app = app.clone();
                                std::thread::spawn(move || {
                                    let _slot = slot;
                                    handle(&app, req);
                                });
                            }
                        });
                        servers.push(s);
                        bound.push(addr.clone());
                    }
                    Err(e) => error = Some(format!("{addr}: {e}")),
                }
            }
            crate::util::log("info", format!("phone access listening on {bound:?}"));
        }
        if let Some(p) = app.try_state::<Phone>() {
            let mut s = p.status.lock().unwrap();
            s.listening = bound.clone();
            s.tailscale_ip = ip;
            if want == bound || error.is_some() {
                s.error = error;
            }
        }
        // Retry a failed bind next round rather than holding a partial set.
        if bound != want {
            bound.clear();
        }
        let retry = if bound.is_empty() && !want.is_empty() { 2 } else { 20 };
        let _ = rx.recv_timeout(Duration::from_secs(retry));
    }
}

pub(crate) fn header(req: &tiny_http::Request, name: &'static str) -> Option<String> {
    req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str().to_string())
}

pub(crate) fn respond(req: tiny_http::Request, code: u16, mime: &str, body: Vec<u8>, extra: &[(&str, &str)]) {
    let mut r = tiny_http::Response::from_data(body).with_status_code(code);
    let mut add = |k: &str, v: &str| {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            r.add_header(h);
        }
    };
    add("Content-Type", mime);
    add("X-Content-Type-Options", "nosniff");
    add("Referrer-Policy", "no-referrer");
    add("X-Frame-Options", "DENY");
    for (k, v) in extra {
        add(k, v);
    }
    let _ = req.respond(r);
}

pub(crate) fn json_reply(req: tiny_http::Request, code: u16, v: Value) {
    respond(req, code, "application/json", v.to_string().into_bytes(), &[("Cache-Control", "no-store")]);
}

fn handle(app: &AppHandle, mut req: tiny_http::Request) {
    let path = req.url().split(['?', '#']).next().unwrap_or("/").to_string();
    let post = *req.method() == tiny_http::Method::Post;
    if let Some(cmd) = path.strip_prefix("/api/") {
        if !post {
            return json_reply(req, 405, json!({"error": "POST only"}));
        }
        let st = app.state::<AppState>();
        let Ok(devs) = st.db.lock().map(|c| devices(&c)) else {
            return json_reply(req, 503, json!({"error": "busy, try again"}));
        };
        let Some(device) = device_for(&devs, header(&req, "Authorization").as_deref()) else {
            return json_reply(req, 401, json!({"error": "This phone isn't paired. Pair it from Settings on the PC."}));
        };
        let mut body = String::new();
        if req.as_reader().take(MAX_BODY + 1).read_to_string(&mut body).is_err() || body.len() as u64 > MAX_BODY {
            return json_reply(req, 413, json!({"error": "request too large"}));
        }
        let args: Value = if body.trim().is_empty() { json!({}) } else { serde_json::from_str(&body).unwrap_or(Value::Null) };
        if !args.is_object() {
            return json_reply(req, 400, json!({"error": "arguments must be a JSON object"}));
        }
        if let Some(p) = app.try_state::<Phone>() {
            p.status.lock().unwrap().last_seen.insert(device, crate::util::now_iso());
        }
        return match call(app, cmd, &args) {
            Ok(v) => json_reply(req, 200, v),
            Err(e) => json_reply(req, 400, json!({"error": e})),
        };
    }
    if *req.method() != tiny_http::Method::Get {
        return respond(req, 405, "text/plain", b"GET only".to_vec(), &[]);
    }
    let file = if path == "/" { "index.html".to_string() } else { path.trim_start_matches('/').to_string() };
    match app.asset_resolver().get(file.clone()) {
        Some(a) => {
            let cache = if file.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
            let csp = "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'";
            let mime = a.mime_type().to_string();
            respond(req, 200, &mime, a.bytes, &[("Cache-Control", cache), ("Content-Security-Policy", csp)]);
        }
        None => respond(req, 404, "text/plain", b"not found".to_vec(), &[]),
    }
}

fn arg<T: DeserializeOwned>(a: &Value, k: &str) -> R<T> {
    serde_json::from_value(a.get(k).cloned().unwrap_or(Value::Null)).map_err(|e| format!("{k}: {e}"))
}

fn out<T: serde::Serialize>(r: R<T>) -> R<Value> {
    serde_json::to_value(r?).map_err(|e| e.to_string())
}

/// Commands a phone may run. Read-only by design: nothing here writes files, launches programs or changes settings.
pub const READ_ONLY: &[&str] = &[
    "get_status", "get_projects", "get_briefing", "get_project", "get_activity", "get_timeline", "get_sessions",
    "get_session_files", "get_graph", "search", "get_sparks", "get_folders", "chat_projects", "chat_list", "chat_page",
    "chat_tail", "chat_entry", "actions_list", "get_discoveries", "get_settings",
];

fn call(app: &AppHandle, cmd: &str, a: &Value) -> R<Value> {
    if !READ_ONLY.contains(&cmd) {
        return Err("Not available on the phone (read-only).".into());
    }
    let st: State<AppState> = app.state();
    match cmd {
        "get_status" => out(commands::get_status(st)),
        "get_projects" => out(commands::get_projects(st, arg(a, "includeHidden")?)),
        "get_briefing" => out(commands::get_briefing(st)),
        "get_project" => out(commands::get_project(st, arg(a, "id")?)),
        "get_activity" => out(commands::get_activity(st, arg(a, "project")?, arg(a, "from")?, arg(a, "to")?, arg(a, "workspace")?)),
        "get_timeline" => out(commands::get_timeline(st, arg(a, "f")?)),
        "get_sessions" => out(commands::get_sessions(st, arg(a, "agent")?)),
        "get_session_files" => out(commands::get_session_files(st, arg(a, "id")?)),
        "get_graph" => out(commands::get_graph(st)),
        "search" => out(commands::search(st, arg(a, "q")?)),
        "get_sparks" => out(commands::get_sparks(st, arg(a, "days")?)),
        "get_folders" => out(commands::get_folders(st, arg(a, "id")?)),
        "chat_projects" => out(commands::chat_projects(st)),
        "chat_list" => out(commands::chat_list(st, arg(a, "agent")?, arg(a, "project")?)),
        "chat_page" => out(commands::chat_page(st, arg(a, "id")?, arg(a, "before")?, None)),
        "chat_tail" => out(commands::chat_tail(st, arg(a, "id")?, arg(a, "from")?)),
        "chat_entry" => out(commands::chat_entry(st, arg(a, "id")?, arg(a, "off")?)),
        "actions_list" => out(commands::actions_list(st, arg(a, "projectId")?)),
        "get_discoveries" => out(commands::get_discoveries(st)),
        // Only what read views use; paths and launch presets stay on the PC.
        "get_settings" => {
            let c = st.db.lock().map_err(|_| "database lock poisoned")?;
            Ok(json!({"workstream_aliases": crate::registry::settings(&c)["workstream_aliases"]}))
        }
        _ => Err("unknown command".into()),
    }
}

pub(crate) fn qr_svg(text: &str) -> R<String> {
    let q = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium).map_err(|e| format!("{e:?}"))?;
    let (n, border) = (q.size(), 4);
    let mut d = String::new();
    for y in 0..n {
        for x in 0..n {
            if q.get_module(x, y) {
                d += &format!("M{},{}h1v1h-1z", x + border, y + border);
            }
        }
    }
    let s = n + border * 2;
    Ok(format!(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {s} {s}" shape-rendering="crispEdges"><rect width="100%" height="100%" fill="#fff"/><path d="{d}" fill="#000"/></svg>"##))
}

/* ---------- desktop-only commands (Settings › Phone access) ---------- */

#[tauri::command(async)]
pub fn phone_status(app: AppHandle) -> R<Value> {
    let p = app.state::<Phone>();
    let s = p.status.lock().unwrap();
    let st = app.state::<AppState>();
    let devs = st.db.lock().map(|c| devices(&c)).unwrap_or_default();
    let list: Vec<Value> = devs
        .iter()
        .map(|d| json!({"id": d["id"], "name": d["name"], "paired": d["paired"], "last_seen": s.last_seen.get(d["id"].as_str().unwrap_or(""))}))
        .collect();
    Ok(json!({
        "enabled": enabled(&app), "port": PORT, "listening": s.listening, "tailscale_ip": s.tailscale_ip,
        "error": s.error, "devices": list,
        "url": s.tailscale_ip.as_ref().map(|ip| format!("http://{ip}:{PORT}/")),
    }))
}

fn wake(app: &AppHandle) {
    let _ = app.state::<Phone>().wake.lock().unwrap().send(());
}

#[tauri::command(async)]
pub fn phone_set_enabled(app: AppHandle, enabled: bool) -> R<()> {
    {
        let st = app.state::<AppState>();
        let c = st.db.lock().map_err(|_| "database lock poisoned")?;
        db::set_setting(&c, "phone_enabled", &json!(enabled)).map_err(|e| e.to_string())?;
    }
    crate::standby::register(enabled);
    wake(&app);
    Ok(())
}

/// New device key. The key itself is only ever in the returned link / QR; the PC keeps its hash.
#[tauri::command(async)]
pub fn phone_pair(app: AppHandle, name: String) -> R<Value> {
    let ip = app.state::<Phone>().status.lock().unwrap().tailscale_ip.clone().ok_or("Sign in to Tailscale on this PC first, then try again.")?;
    if !enabled(&app) {
        return Err("Turn phone access on first.".into());
    }
    let name = name.trim();
    let name = if name.is_empty() { "Phone" } else { name };
    let token = random_hex(32)?;
    let device = json!({"id": random_hex(8)?, "name": name.chars().take(40).collect::<String>(), "hash": hash(&token), "paired": crate::util::now_iso()});
    {
        let st = app.state::<AppState>();
        let c = st.db.lock().map_err(|_| "database lock poisoned")?;
        let mut devs = devices(&c);
        devs.push(device);
        db::set_setting(&c, "phone_devices", &Value::Array(devs)).map_err(|e| e.to_string())?;
    }
    let url = format!("http://{ip}:{PORT}/#pair={token}");
    Ok(json!({"url": url, "qr_svg": qr_svg(&url)?}))
}

#[tauri::command(async)]
pub fn phone_revoke(app: AppHandle, id: String) -> R<()> {
    let st = app.state::<AppState>();
    let c = st.db.lock().map_err(|_| "database lock poisoned")?;
    let devs: Vec<Value> = devices(&c).into_iter().filter(|d| d["id"].as_str() != Some(id.as_str())).collect();
    db::set_setting(&c, "phone_devices", &Value::Array(devs)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_paired_key_unlocks() {
        let token = "a".repeat(64);
        let devs = vec![json!({"id": "d1", "hash": hash(&token)})];
        assert_eq!(device_for(&devs, Some(&format!("Bearer {token}"))), Some("d1".into()));
        assert_eq!(device_for(&devs, Some(&format!("Bearer {}", "b".repeat(64)))), None);
        assert_eq!(device_for(&devs, Some(&token)), None, "scheme required");
        assert_eq!(device_for(&devs, None), None);
        // A device with no hash can't be matched by an empty or short key.
        assert_eq!(device_for(&[json!({"id": "x"})], Some("Bearer ")), None);
    }

    #[test]
    fn write_commands_are_not_on_the_phone_list() {
        for w in ["launch_agent", "open_path", "save_settings", "task_toggle", "action_op", "bos_append", "refresh_now", "team_login", "phone_pair", "get_diagnostics"] {
            assert!(!READ_ONLY.contains(&w), "{w}");
        }
    }

    #[test]
    fn request_slots_are_capped_and_freed() {
        let held: Vec<Slot> = std::iter::from_fn(Slot::take).take(MAX_INFLIGHT + 5).collect();
        assert_eq!(held.len(), MAX_INFLIGHT);
        assert!(Slot::take().is_none());
        drop(held);
        assert!(Slot::take().is_some());
    }

    #[test]
    fn qr_is_an_svg() {
        let s = qr_svg("http://100.64.0.1:8765/#pair=abc").unwrap();
        assert!(s.starts_with("<svg") && s.contains("<path d=\"M"));
    }
}
