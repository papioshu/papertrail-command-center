//! ptcc-hub: the optional, self-hosted workspace hub (D030, docs/workspaces/ARCHITECTURE.md). Milestone 1:
//! accounts, devices, workspaces, projects, members, invitations, audit and backups. No sync yet (M2).
pub mod api;
pub mod store;

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
    let mut resp = tiny_http::Response::from_data(r.body.to_string().into_bytes()).with_status_code(r.code);
    for (k, v) in [("Content-Type", "application/json"), ("Cache-Control", "no-store"), ("X-Content-Type-Options", "nosniff")] {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp.add_header(h);
        }
    }
    let _ = req.respond(resp);
}

fn serve_one(hub: &api::Hub, mut req: tiny_http::Request) {
    let mut body = Vec::new();
    if req.as_reader().take(MAX_BODY as u64 + 1).read_to_end(&mut body).is_err() || body.len() > MAX_BODY {
        return respond(req, api::Reply { code: 413, body: serde_json::json!({ "error": "too_large" }) });
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
