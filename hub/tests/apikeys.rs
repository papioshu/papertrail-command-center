//! D060: API keys for a teammate's own dashboard. Owner-made (or self-made), limited to read or tasks and to chosen
//! workspaces, checked before the person's role; revocable; cross-origin only on the paths a key can reach.
mod common;
use common::*;
use ptcc_hub::api::Hub;
use ptcc_hub::{serve, store};
use serde_json::{json, Value};
use std::sync::Arc;

fn uuid() -> String {
    let h = store::random_hex(16).unwrap();
    format!("{}-{}-4{}-8{}-{}", &h[0..8], &h[8..12], &h[13..16], &h[17..20], &h[20..32])
}

fn with_key(fx: &Fx, key: &str, method: &str, path: &str, body: Value) -> ptcc_hub::api::Reply {
    fx.hub.handle(method, path, Some(&format!("Bearer {key}")), body.to_string().as_bytes())
}

fn make(fx: &Fx, by: &str, for_: &str, access: &str, wss: &[&str]) -> ptcc_hub::api::Reply {
    fx.call("POST", &format!("/v1/people/{}/api-keys", fx.user(for_)), Some(by), json!({ "name": "Dash", "access": access, "workspaces": wss }))
}

fn task(fx: &Fx, kind: &str) -> Value {
    json!({ "op_id": uuid(), "record_id": uuid(), "kind": kind, "action": "create", "project_id": fx.p1, "fields": { "title": "from the dashboard", "status": "todo" } })
}

#[test]
fn read_keys_only_read_and_tasks_keys_only_write_tasks() {
    let fx = fixture();
    let ws = fx.ws.clone();
    let r = make(&fx, "owner", "contributor", "read", &[&ws]);
    assert_eq!(r.code, 200, "{r:?}");
    let read = r.body["token"].as_str().unwrap().to_string();
    for p in [format!("/v1/workspaces/{ws}"), format!("/v1/workspaces/{ws}/projects"), format!("/v1/workspaces/{ws}/members"), format!("/v1/workspaces/{ws}/changes"), "/v1/me".into()] {
        assert_eq!(with_key(&fx, &read, "GET", &p, json!({})).code, 200, "read {p}");
    }
    assert_eq!(with_key(&fx, &read, "POST", &format!("/v1/workspaces/{ws}/ops"), task(&fx, "task")).code, 403, "read key can't write");
    for (m, p) in [("GET", "/v1/devices".to_string()), ("GET", "/v1/people".into()), ("POST", "/v1/devices/link".into()), ("POST", format!("/v1/workspaces/{ws}/invitations")),
                   ("GET", format!("/v1/workspaces/{ws}/audit")), ("GET", format!("/v1/people/{}/api-keys", fx.user("contributor"))), ("POST", "/v1/voice".into())] {
        assert_eq!(with_key(&fx, &read, m, &p, json!({})).code, 403, "{m} {p}");
    }
    assert_eq!(fx.hub.handle("PUT", &format!("/v1/workspaces/{ws}/projects/{}/blobs/{}", fx.p1, "a".repeat(64)), Some(&format!("Bearer {read}")), b"x").code, 403, "no uploads");

    let tasks = make(&fx, "owner", "contributor", "tasks", &[&ws]).body["token"].as_str().unwrap().to_string();
    assert_eq!(with_key(&fx, &tasks, "POST", &format!("/v1/workspaces/{ws}/ops"), task(&fx, "task")).code, 200, "tasks key writes a task");
    assert_eq!(with_key(&fx, &tasks, "POST", &format!("/v1/workspaces/{ws}/ops"), task(&fx, "board")).code, 403, "but not boards");
    // The person's own role still applies: a viewer's tasks key can't write.
    let viewer = make(&fx, "owner", "viewer", "tasks", &[&ws]).body["token"].as_str().unwrap().to_string();
    assert_eq!(with_key(&fx, &viewer, "POST", &format!("/v1/workspaces/{ws}/ops"), task(&fx, "task")).code, 403);
    // Keys never show in the person's device list, and refusals are audited.
    let devs = fx.call("GET", "/v1/devices", Some("contributor"), json!({}));
    assert!(devs.body["devices"].as_array().unwrap().iter().all(|d| !d["id"].as_str().unwrap().starts_with("k_")));
    let audit = fx.call("GET", &format!("/v1/workspaces/{ws}/audit"), Some("owner"), json!({}));
    assert!(audit.body.to_string().contains("apikey.created"));
}

#[test]
fn keys_stay_in_their_workspaces_and_only_owners_or_the_person_make_them() {
    let fx = fixture();
    let (ws, other) = (fx.ws.clone(), fx.other_ws.clone());
    // editor also joins OARA (owned by the outsider).
    let inv = fx.call("POST", &format!("/v1/workspaces/{other}/invitations"), Some("outsider"), json!({ "role": "viewer" }));
    assert_eq!(fx.call("POST", "/v1/redeem", Some("editor"), json!({ "token": inv.body["token"] })).code, 200);
    assert_eq!(make(&fx, "owner", "editor", "read", &[&ws, &other]).code, 403, "owner can't grant a workspace they don't own");
    let key = make(&fx, "owner", "editor", "read", &[&ws]).body["token"].as_str().unwrap().to_string();
    assert_eq!(with_key(&fx, &key, "GET", &format!("/v1/workspaces/{other}"), json!({})).code, 403, "limited to its workspaces");
    let me = with_key(&fx, &key, "GET", "/v1/me", json!({}));
    assert_eq!(me.body["workspaces"].as_array().unwrap().len(), 1, "me lists only the key's workspaces");
    // Not admins, not strangers; the person themself may.
    assert_eq!(make(&fx, "admin", "viewer", "read", &[&ws]).code, 403);
    assert_eq!(make(&fx, "outsider", "viewer", "read", &[&ws]).code, 403);
    assert_eq!(make(&fx, "viewer", "viewer", "read", &[&ws]).code, 200);
    assert_eq!(make(&fx, "owner", "viewer", "admin", &[&ws]).code, 400, "only read or tasks");
    assert_eq!(make(&fx, "owner", "viewer", "read", &[]).code, 403, "at least one workspace");
    // Listing shows name/access/workspaces, never the secret.
    let list = fx.call("GET", &format!("/v1/people/{}/api-keys", fx.user("editor")), Some("owner"), json!({}));
    assert_eq!(list.body["keys"].as_array().unwrap().len(), 1);
    assert!(!list.body.to_string().contains(key.split_once('.').unwrap().1));
}

#[test]
fn revoked_keys_and_removed_people_lose_access() {
    let fx = fixture();
    let ws = fx.ws.clone();
    let r = make(&fx, "owner", "contributor", "read", &[&ws]);
    let (id, key) = (r.body["id"].as_str().unwrap().to_string(), r.body["token"].as_str().unwrap().to_string());
    assert_eq!(fx.call("DELETE", &format!("/v1/people/{}/api-keys/{id}", fx.user("contributor")), Some("admin"), json!({})).code, 403);
    assert_eq!(fx.call("DELETE", &format!("/v1/people/{}/api-keys/{id}", fx.user("contributor")), Some("owner"), json!({})).code, 200);
    assert_eq!(with_key(&fx, &key, "GET", "/v1/me", json!({})).code, 401);
    let key2 = make(&fx, "owner", "contributor", "read", &[&ws]).body["token"].as_str().unwrap().to_string();
    assert_eq!(fx.call("DELETE", &format!("/v1/workspaces/{ws}/members/{}", fx.user("contributor")), Some("owner"), json!({})).code, 200);
    assert_ne!(with_key(&fx, &key2, "GET", &format!("/v1/workspaces/{ws}"), json!({})).code, 200, "removal ends the key's reach");
}

#[test]
fn browsers_may_call_only_key_paths_cross_origin() {
    let hub = Arc::new(Hub::new(store::open(None).unwrap(), None));
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let port = server.server_addr().to_ip().unwrap().port();
    serve(hub, server.clone(), 2);
    let send = |method: &str, path: &str| {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "{method} {path} HTTP/1.1\r\nHost: x\r\nOrigin: http://localhost:3000\r\nConnection: close\r\n\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    };
    let pre = send("OPTIONS", "/v1/workspaces/w_x/changes");
    assert!(pre.starts_with("HTTP/1.1 204") && pre.contains("Access-Control-Allow-Origin: *") && pre.contains("Authorization"), "{pre}");
    assert!(send("GET", "/v1/me").contains("Access-Control-Allow-Origin: *"), "401 replies readable too");
    for p in ["/v1/login", "/v1/redeem", "/v1/devices/claim"] {
        let r = send("OPTIONS", p);
        assert!(r.starts_with("HTTP/1.1 404") && !r.contains("Access-Control-Allow-Origin"), "{p}: {r}");
    }
    server.unblock();
}
