//! ptcc-hub: the optional, self-hosted workspace hub (D030, docs/workspaces/ARCHITECTURE.md). Milestone 1:
//! accounts, devices, workspaces, projects, members, invitations, audit and backups (M1); shared records,
//! change feed, conflicts, idempotent ops and media (M2).
pub mod api;
pub mod store;
pub mod sync;
pub mod voice;

use std::io::Read;
use std::net::IpAddr;
use std::sync::Arc;

pub const MAX_BODY: usize = 64 * 1024;

/// Default network policy (§9): loopback and Tailscale (100.64.0.0/10, fd7a:115c:a1e0::/48) only.
pub fn private_bind(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback() || (v.octets()[0] == 100 && (64..128).contains(&v.octets()[1])),
        IpAddr::V6(v) => v.is_loopback() || v.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

fn respond(req: tiny_http::Request, r: api::Reply) {
    // Downloads are never rendered: a fixed binary type, attachment, nosniff.
    let (data, kind, disp) = match r.raw {
        Some(raw) => (raw, "application/octet-stream", "attachment"),
        None => (r.body.to_string().into_bytes(), "application/json", "inline"),
    };
    let mut resp = tiny_http::Response::from_data(data).with_status_code(r.code);
    for (k, v) in [("Content-Type", kind), ("Content-Disposition", disp), ("Cache-Control", "no-store"), ("X-Content-Type-Options", "nosniff")] {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp.add_header(h);
        }
    }
    let _ = req.respond(resp);
}

/// The phone board (D040): two static files, same API and rules as the desktop app. No inline script (strict CSP).
fn static_file(url: &str) -> Option<(&'static [u8], &'static str)> {
    match url.split(['?', '#']).next().unwrap_or("") {
        "/" | "/index.html" => Some((include_bytes!("phone.html"), "text/html; charset=utf-8")),
        "/app.js" => Some((include_bytes!("phone.js"), "text/javascript; charset=utf-8")),
        _ => None,
    }
}

/// Boards on iPhone/iPad (D050): the board page (a separate web build, `npm run build:board`) is served from the
/// `board` folder next to the hub exe. Only plain names are looked up, so no path can leave that folder.
pub fn board_path(root: &std::path::Path, url: &str) -> Option<(std::path::PathBuf, &'static str)> {
    let rest = url.split(['?', '#']).next()?.strip_prefix("/board")?;
    let rest = if rest.is_empty() { rest } else { rest.strip_prefix('/')? };
    let rel = if rest.is_empty() { "board.html" } else { rest };
    let safe = |s: &str| !s.is_empty() && !s.starts_with('.') && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if !rel.split('/').all(safe) {
        return None;
    }
    let kind = match rel.rsplit('.').next()? {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "png" => "image/png",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "json" => "application/json",
        _ => return None,
    };
    let path = root.join(rel);
    path.is_file().then_some((path, kind))
}

fn serve_board(req: tiny_http::Request) -> Result<(), tiny_http::Request> {
    let Some(root) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("board"))) else { return Err(req) };
    let Some((path, kind)) = board_path(&root, req.url()) else { return Err(req) };
    let Ok(file) = std::fs::File::open(&path) else { return Err(req) };
    let hashed = req.url().starts_with("/board/assets/");
    let mut resp = tiny_http::Response::from_file(file);
    for (k, v) in [
        ("Content-Type", kind),
        // Excalidraw shows images as data:/blob: URLs and runs the PDF preview in a worker.
        ("Content-Security-Policy", "default-src 'self'; img-src 'self' data: blob:; font-src 'self' data:; style-src 'self' 'unsafe-inline'; worker-src 'self' blob:; connect-src 'self' data: blob:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        ("Cache-Control", if hashed { "max-age=31536000, immutable" } else { "no-cache" }),
    ] {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp.add_header(h);
        }
    }
    let _ = req.respond(resp);
    Ok(())
}

fn serve_one(hub: &api::Hub, mut req: tiny_http::Request) {
    if *req.method() == tiny_http::Method::Get && req.url().starts_with("/board") {
        match serve_board(req) {
            Ok(()) => return,
            Err(r) => return respond(r, api::fail(404, "not_found")),
        }
    }
    if *req.method() == tiny_http::Method::Get {
        if let Some((body, kind)) = static_file(req.url()) {
            let mut resp = tiny_http::Response::from_data(body.to_vec());
            for (k, v) in [
                ("Content-Type", kind),
                ("Content-Security-Policy", "default-src 'self'; img-src 'self' blob:; media-src 'self' blob:; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"),
                ("X-Content-Type-Options", "nosniff"),
                ("Referrer-Policy", "no-referrer"),
                ("Cache-Control", "no-cache"),
            ] {
                if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                    resp.add_header(h);
                }
            }
            let _ = req.respond(resp);
            return;
        }
    }
    let max = if *req.method() == tiny_http::Method::Put && req.url().contains("/blobs/") { sync::MAX_BLOB } else { MAX_BODY };
    let mut body = Vec::new();
    if req.as_reader().take(max as u64 + 1).read_to_end(&mut body).is_err() || body.len() > max {
        return respond(req, api::fail(413, "too_large"));
    }
    let auth = req.headers().iter().find(|h| h.field.equiv("Authorization")).map(|h| h.value.as_str().to_string());
    let r = hub.handle(req.method().as_str(), req.url(), auth.as_deref(), &body);
    respond(req, r);
}

/// Runs `workers` threads answering requests until the server is dropped/unblocked.
pub fn serve(hub: Arc<api::Hub>, server: Arc<tiny_http::Server>, workers: usize) -> Vec<std::thread::JoinHandle<()>> {
    (0..workers)
        .map(|_| {
            let (hub, server) = (hub.clone(), server.clone());
            std::thread::spawn(move || {
                while let Ok(req) = server.recv() {
                    serve_one(&hub, req);
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn board_page_paths_stay_in_the_board_folder() {
        let root = std::env::temp_dir().join(format!("ptcc-board-{}", std::process::id()));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("board.html"), "x").unwrap();
        std::fs::write(root.join("assets").join("a-1.js"), "x").unwrap();
        assert_eq!(super::board_path(&root, "/board/").unwrap().0, root.join("board.html"));
        assert_eq!(super::board_path(&root, "/board#ws/b").unwrap().0, root.join("board.html"));
        assert!(super::board_path(&root, "/board/assets/a-1.js?v=2").is_some());
        for bad in ["/board/../hub.json", "/board/assets/..%2fx.js", "/board/.git/x.js", "/board//etc.js", "/board/assets\\a-1.js", "/boardx.html", "/board/missing.js", "/board/hub.db"] {
            assert!(super::board_path(&root, bad).is_none(), "{bad}");
        }
        std::fs::remove_dir_all(root).ok();
    }
}
