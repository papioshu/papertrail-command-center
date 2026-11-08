//! ptcc-hub: the optional, self-hosted workspace hub (D030, docs/workspaces/ARCHITECTURE.md). Milestone 1:
//! accounts, devices, workspaces, projects, members, invitations, audit and backups (M1); shared records,
//! change feed, conflicts, idempotent ops and media (M2).
pub mod api;
pub mod store;
pub mod sync;

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

fn serve_one(hub: &api::Hub, mut req: tiny_http::Request) {
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
