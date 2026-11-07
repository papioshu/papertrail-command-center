//! Phone standby (D029): the same exe run as `--phone-standby` at Windows login, no window, no Tauri.
//! While the dashboard is closed it holds the phone port and lets a paired phone start the dashboard;
//! nothing else is served (no data, no commands). It steps aside within 3 s whenever the dashboard runs.
use crate::phone::{device_for, header, json_reply, respond, tailscale_ip, PORT};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RUN_NAME: &str = "Papertrail Phone Standby";

/// Same folder Tauri's app_data_dir resolves to for this identifier.
pub fn data_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("os.papertrail.commandcenter"))
}

fn lock_file(dir: &Path, name: &str) -> Option<File> {
    OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(name)).ok()
}

/// The dashboard holds this for its whole life (the OS lets go if it crashes), so standby can tell it is up.
pub fn hold_app_lock(dir: &Path) {
    if let Some(f) = lock_file(dir, "app.lock") {
        std::thread::spawn(move || {
            if f.lock().is_ok() {
                Box::leak(Box::new(f));
            }
        });
    }
}

fn app_running(dir: &Path) -> bool {
    let Some(f) = lock_file(dir, "app.lock") else { return false };
    match f.try_lock() {
        Ok(()) => {
            let _ = f.unlock();
            false
        }
        Err(_) => true,
    }
}

fn setting(db: &Path, key: &str) -> Option<Value> {
    let c = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    crate::db::setting(&c, key)
}

/// Start standby at login while phone access is on; called by the dashboard at start and on the switch.
pub fn register(on: bool) {
    let Ok(exe) = std::env::current_exe() else { return };
    let Ok(al) = auto_launch::AutoLaunchBuilder::new()
        .set_app_name(RUN_NAME)
        .set_app_path(&exe.to_string_lossy())
        .set_args(&["--phone-standby"])
        .build()
    else {
        return;
    };
    let r = if on { al.enable() } else { al.disable() };
    if let Err(e) = r {
        crate::util::log("warn", format!("phone standby autostart: {e}"));
    }
    if on {
        // Running now too, not only from the next login; a second copy exits on its own.
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--phone-standby");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let _ = cmd.spawn();
    }
}

pub fn main() {
    let Some(dir) = data_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let Some(me) = lock_file(&dir, "standby.lock") else { return };
    if me.try_lock().is_err() {
        return; // one standby per user
    }
    crate::util::init_log(&dir.join("logs"));
    crate::util::log("info", format!("phone standby start pid {}", std::process::id()));
    let db = dir.join("papertrail.db");
    let (tx, rx) = channel::<()>();
    let last_launch = Arc::new(Mutex::new(None::<Instant>));
    let mut servers: Vec<Arc<tiny_http::Server>> = Vec::new();
    let (mut ip, mut ip_checked) = (None, None::<Instant>);
    let mut hold_off = Instant::now();
    loop {
        if setting(&db, "phone_enabled").and_then(|v| v.as_bool()) != Some(true) {
            crate::util::log("info", "phone standby: phone access is off, exiting");
            return;
        }
        if ip_checked.is_none_or(|t: Instant| t.elapsed() > Duration::from_secs(20)) {
            let now = tailscale_ip();
            if now != ip {
                servers.drain(..).for_each(|s| s.unblock()); // rebind on the new address
            }
            (ip, ip_checked) = (now, Some(Instant::now()));
        }
        let serve = Instant::now() >= hold_off && !app_running(&dir);
        if !serve {
            servers.drain(..).for_each(|s| s.unblock());
        } else if servers.is_empty() {
            let addrs = std::iter::once("127.0.0.1".to_string()).chain(ip.clone());
            for addr in addrs.map(|a| format!("{a}:{PORT}")) {
                if let Ok(s) = tiny_http::Server::http(addr.as_str()) {
                    let s = Arc::new(s);
                    let (srv, db, tx, last) = (s.clone(), db.clone(), tx.clone(), last_launch.clone());
                    std::thread::spawn(move || {
                        while let Ok(req) = srv.recv() {
                            handle(req, &db, &tx, &last);
                        }
                    });
                    servers.push(s);
                }
            }
        }
        if rx.recv_timeout(Duration::from_secs(3)).is_ok() {
            // Launching: hand the port over to the dashboard and keep off it while it starts.
            servers.drain(..).for_each(|s| s.unblock());
            hold_off = Instant::now() + Duration::from_secs(20); // the dashboard takes its lock within ~1 s
        }
    }
}

const PAGE: &str = include_str!("standby.html");

fn handle(req: tiny_http::Request, db: &Path, launch: &Sender<()>, last: &Mutex<Option<Instant>>) {
    let path = req.url().split(['?', '#']).next().unwrap_or("/").to_string();
    let post = *req.method() == tiny_http::Method::Post;
    if path == "/api/launch" && post {
        let devs = setting(db, "phone_devices").and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let Some(device) = device_for(&devs, header(&req, "Authorization").as_deref()) else {
            return json_reply(req, 401, json!({"error": "This phone isn't paired. Pair it from Settings on the PC."}));
        };
        let mut last = last.lock().unwrap();
        // Skip only while a launch is actually under way; repeating it would only queue more windows.
        let starting = last.is_some_and(|t| t.elapsed() < Duration::from_secs(10));
        if !starting && !db.parent().is_some_and(app_running) {
            *last = Some(Instant::now());
            crate::util::log("info", format!("phone standby: {device} asked to open the dashboard"));
            let _ = launch.send(());
            std::thread::sleep(Duration::from_millis(300)); // let the main loop release the port first
            if let Ok(exe) = std::env::current_exe() {
                if let Err(e) = std::process::Command::new(exe).spawn() {
                    return json_reply(req, 500, json!({"error": format!("couldn't start the dashboard: {e}")}));
                }
            }
        }
        return json_reply(req, 202, json!({"launching": true}));
    }
    let tag = [("X-PT-Standby", "1"), ("Cache-Control", "no-store")];
    if path.starts_with("/api/") {
        // The real page is open and the dashboard just closed: the phone waits (D028) and offers to open it.
        return respond(req, 503, "application/json", json!({"error": "closed", "standby": true}).to_string().into_bytes(), &tag);
    }
    if (path == "/" || path == "/index.html") && !post {
        let csp = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'";
        return respond(req, 200, "text/html; charset=utf-8", PAGE.as_bytes().to_vec(), &[tag[0], tag[1], ("Content-Security-Policy", csp)]);
    }
    respond(req, 404, "text/plain", b"not found".to_vec(), &tag);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standby_sees_whether_the_dashboard_holds_its_lock() {
        let dir = std::env::temp_dir().join(format!("ptcc-standby-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!app_running(&dir));
        let held = lock_file(&dir, "app.lock").unwrap();
        held.lock().unwrap();
        assert!(app_running(&dir));
        drop(held); // app quit or crashed
        assert!(!app_running(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
