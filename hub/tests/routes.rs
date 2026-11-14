//! M1 gate (ARCHITECTURE.md §13): every route × role × scope, revoked device 401, hidden project 404, restore drill.
mod common;
use common::*;
use ptcc_hub::api::{new_workspace, revoke_user, Hub};
use ptcc_hub::{private_bind, serve, store};
use serde_json::{json, Value};
use std::sync::Arc;

fn op(kind: &str, project: &str, fields: Value) -> Value {
    json!({ "op_id": "11111111-1111-4111-8111-111111111111", "record_id": "22222222-2222-4222-8222-222222222222", "kind": kind, "action": "create", "project_id": project, "fields": fields })
}

#[test]
fn every_route_by_every_role_and_scope() {
    // (label, method, path template, body, expected status for ACTORS in order). {ws} {p1} {p2} {viewer} {contributor} {admin} {inv}
    let cases: Vec<(&str, &str, &str, Value, [u16; 7])> = vec![
        ("read workspace", "GET", "/v1/workspaces/{ws}", json!({}), [200, 200, 200, 200, 200, 200, 404]),
        ("rename workspace", "PATCH", "/v1/workspaces/{ws}", json!({ "name": "New" }), [200, 200, 403, 403, 403, 403, 404]),
        ("delete workspace", "DELETE", "/v1/workspaces/{ws}", json!({}), [200, 403, 403, 403, 403, 403, 404]),
        ("list projects", "GET", "/v1/workspaces/{ws}/projects", json!({}), [200, 200, 200, 200, 200, 200, 404]),
        ("create project", "POST", "/v1/workspaces/{ws}/projects", json!({ "name": "media" }), [200, 200, 403, 403, 403, 403, 404]),
        ("read in-scope project", "GET", "/v1/workspaces/{ws}/projects/{p1}", json!({}), [200, 200, 200, 200, 200, 200, 404]),
        ("read out-of-scope project", "GET", "/v1/workspaces/{ws}/projects/{p2}", json!({}), [200, 200, 200, 200, 200, 404, 404]),
        ("list members", "GET", "/v1/workspaces/{ws}/members", json!({}), [200, 200, 200, 200, 200, 200, 404]),
        ("change a viewer's role", "PATCH", "/v1/workspaces/{ws}/members/{viewer}", json!({ "role": "contributor" }), [200, 200, 403, 403, 403, 403, 404]),
        ("promote a viewer to admin", "PATCH", "/v1/workspaces/{ws}/members/{viewer}", json!({ "role": "admin" }), [200, 403, 403, 403, 403, 403, 404]),
        ("demote an admin", "PATCH", "/v1/workspaces/{ws}/members/{admin}", json!({ "role": "viewer" }), [200, 403, 403, 403, 403, 403, 404]),
        ("remove a contributor", "DELETE", "/v1/workspaces/{ws}/members/{contributor}", json!({}), [200, 403, 403, 403, 403, 403, 404]),
        ("invite an editor", "POST", "/v1/workspaces/{ws}/invitations", json!({ "role": "editor" }), [200, 403, 403, 403, 403, 403, 404]),
        ("invite an admin", "POST", "/v1/workspaces/{ws}/invitations", json!({ "role": "admin" }), [200, 403, 403, 403, 403, 403, 404]),
        ("invite an owner", "POST", "/v1/workspaces/{ws}/invitations", json!({ "role": "owner" }), [403, 403, 403, 403, 403, 403, 404]),
        ("list invitations", "GET", "/v1/workspaces/{ws}/invitations", json!({}), [200, 403, 403, 403, 403, 403, 404]),
        ("revoke an invitation", "DELETE", "/v1/workspaces/{ws}/invitations/{inv}", json!({}), [200, 403, 403, 403, 403, 403, 404]),
        ("read audit", "GET", "/v1/workspaces/{ws}/audit", json!({}), [200, 200, 403, 403, 403, 403, 404]),
        ("transfer ownership", "POST", "/v1/workspaces/{ws}/transfer", json!({ "user_id": "{admin}" }), [200, 403, 403, 403, 403, 403, 404]),
        ("other workspace", "GET", "/v1/workspaces/{other}", json!({}), [404, 404, 404, 404, 404, 404, 200]),
        ("unknown workspace", "GET", "/v1/workspaces/w_nope/projects", json!({}), [404, 404, 404, 404, 404, 404, 404]),
        ("change feed", "GET", "/v1/workspaces/{ws}/changes", json!({}), [200, 200, 200, 200, 200, 200, 404]),
        ("create task in p1", "POST", "/v1/workspaces/{ws}/ops", op("task", "{p1}", json!({ "title": "x" })), [200, 200, 200, 200, 403, 200, 404]),
        ("create task in p2", "POST", "/v1/workspaces/{ws}/ops", op("task", "{p2}", json!({ "title": "x" })), [200, 200, 200, 200, 403, 404, 404]),
        ("create assigned task", "POST", "/v1/workspaces/{ws}/ops", op("task", "{p1}", json!({ "title": "x", "assignee": "{viewer}" })), [200, 200, 200, 403, 403, 200, 404]),
        ("create milestone", "POST", "/v1/workspaces/{ws}/ops", op("milestone", "{p1}", json!({ "title": "v1" })), [200, 200, 200, 403, 403, 200, 404]),
        // The test body is the JSON text "{}", so the blob name is sha256("{}").
        ("upload blob to p1", "PUT", "/v1/workspaces/{ws}/projects/{p1}/blobs/44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a", json!({}), [200, 200, 200, 200, 403, 200, 404]),
        ("upload blob to p2", "PUT", "/v1/workspaces/{ws}/projects/{p2}/blobs/44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a", json!({}), [200, 200, 200, 200, 403, 404, 404]),
        ("download unknown media", "GET", "/v1/workspaces/{ws}/media/00000000-0000-4000-8000-000000000000", json!({}), [404; 7]),
        ("list roles", "GET", "/v1/workspaces/{ws}/roles", json!({}), [200, 200, 200, 200, 200, 200, 404]),
        ("create a contributor-level role", "POST", "/v1/workspaces/{ws}/roles", json!({ "name": "Mod", "base": "contributor" }), [200, 200, 403, 403, 403, 403, 404]),
        ("create an admin-level role", "POST", "/v1/workspaces/{ws}/roles", json!({ "name": "Co-lead", "base": "admin" }), [200, 403, 403, 403, 403, 403, 404]),
        ("delete an unused role", "DELETE", "/v1/workspaces/{ws}/roles", json!({ "name": "Artist" }), [200, 200, 403, 403, 403, 403, 404]),
        ("give the viewer the Artist role", "PATCH", "/v1/workspaces/{ws}/members/{viewer}", json!({ "title": "Artist" }), [200, 200, 403, 403, 403, 403, 404]),
        ("invite as Ops", "POST", "/v1/workspaces/{ws}/invitations", json!({ "title": "Ops" }), [200, 403, 403, 403, 403, 403, 404]),
        ("people across my workspaces", "GET", "/v1/people", json!({}), [200; 7]),
        ("me", "GET", "/v1/me", json!({}), [200; 7]),
        ("own devices", "GET", "/v1/devices", json!({}), [200; 7]),
    ];
    let mut failures = Vec::new();
    for (label, method, tpl, body, want) in &cases {
        for (i, actor) in ACTORS.iter().enumerate() {
            let fx = fixture();
            let fill = |s: &str| {
                s.replace("{ws}", &fx.ws).replace("{other}", &fx.other_ws).replace("{p1}", &fx.p1).replace("{p2}", &fx.p2)
                    .replace("{viewer}", fx.user("viewer")).replace("{contributor}", fx.user("contributor")).replace("{admin}", fx.user("admin"))
                    .replace("{inv}", &fx.invite_viewer)
            };
            let body: Value = serde_json::from_str(&fill(&body.to_string())).unwrap();
            let r = fx.call(method, &fill(tpl), Some(actor), body);
            if r.code != want[i] {
                failures.push(format!("{label} as {actor}: got {} want {} ({})", r.code, want[i], r.body));
            }
            // No anonymous access anywhere but health and redeem.
            if i == 0 {
                assert_eq!(fx.call(method, &fill(tpl), None, json!({})).code, 401, "{label} anonymous");
            }
        }
    }
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn scoped_member_lists_only_their_projects_and_hidden_looks_like_missing() {
    let fx = fixture();
    let list = fx.call("GET", &format!("/v1/workspaces/{}/projects", fx.ws), Some("scoped"), json!({}));
    let ids: Vec<&str> = list.body["projects"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [fx.p1.as_str()]);
    assert_eq!(fx.call("GET", &format!("/v1/workspaces/{}/projects", fx.ws), Some("viewer"), json!({})).body["projects"].as_array().unwrap().len(), 2);
    let hidden = fx.call("GET", &format!("/v1/workspaces/{}/projects/{}", fx.ws, fx.p2), Some("scoped"), json!({}));
    let missing = fx.call("GET", &format!("/v1/workspaces/{}/projects/p_nope", fx.ws), Some("scoped"), json!({}));
    assert_eq!((hidden.code, &hidden.body), (404, &missing.body), "a hidden project is indistinguishable from a missing one");
    // Scopes and custom permissions are visible only to those who manage members.
    let members = fx.call("GET", &format!("/v1/workspaces/{}/members", fx.ws), Some("scoped"), json!({}));
    assert!(members.body["members"].as_array().unwrap().iter().all(|m| m.get("projects").is_none()), "{}", members.body);
    let members = fx.call("GET", &format!("/v1/workspaces/{}/members", fx.ws), Some("admin"), json!({}));
    assert!(members.body.to_string().contains(&fx.p1));
}

#[test]
fn revoked_devices_users_and_members_lose_access() {
    let fx = fixture();
    let me = fx.call("GET", "/v1/me", Some("editor"), json!({}));
    let dev = me.body["device_id"].as_str().unwrap().to_string();
    assert_eq!(fx.call("DELETE", &format!("/v1/devices/{dev}"), Some("editor"), json!({})).code, 200);
    assert_eq!(fx.call("GET", "/v1/me", Some("editor"), json!({})).code, 401, "revoked device");
    // Another user's device can't be revoked (and looks missing).
    let vdev = fx.call("GET", "/v1/me", Some("viewer"), json!({})).body["device_id"].as_str().unwrap().to_string();
    assert_eq!(fx.call("DELETE", &format!("/v1/devices/{vdev}"), Some("admin"), json!({})).code, 404);
    // Revoking a user (host CLI) revokes all their devices.
    let extra = fx.call("POST", "/v1/devices", Some("viewer"), json!({ "name": "laptop" }));
    let extra_token = extra.body["token"].as_str().unwrap().to_string();
    assert_eq!(fx.hub.handle("GET", "/v1/me", Some(&format!("Bearer {extra_token}")), b"").code, 200);
    assert!(revoke_user(&fx.hub.db.lock().unwrap(), fx.user("viewer")).unwrap());
    assert_eq!(fx.call("GET", "/v1/me", Some("viewer"), json!({})).code, 401);
    assert_eq!(fx.hub.handle("GET", "/v1/me", Some(&format!("Bearer {extra_token}")), b"").code, 401);
    // A removed member sees the workspace as missing, then can rejoin only by a new invitation.
    assert_eq!(fx.call("DELETE", &format!("/v1/workspaces/{}/members/{}", fx.ws, fx.user("contributor")), Some("owner"), json!({})).code, 200);
    assert_eq!(fx.call("GET", &format!("/v1/workspaces/{}", fx.ws), Some("contributor"), json!({})).code, 404);
    // Garbage and forged bearers.
    for bad in ["Bearer", "Bearer x", "Bearer d_x.y", &format!("Bearer {dev}.00"), "Basic abc"] {
        assert_eq!(fx.hub.handle("GET", "/v1/me", Some(bad), b"").code, 401, "{bad}");
    }
}

#[test]
fn invitations_are_single_use_expire_and_fail_the_same_way() {
    let fx = fixture();
    let inv = fx.call("POST", &format!("/v1/workspaces/{}/invitations", fx.ws), Some("owner"), json!({ "role": "viewer" }));
    let token = inv.body["token"].as_str().unwrap().to_string();
    redeem(&fx.hub, &token, "First");
    let again = fx.hub.handle("POST", "/v1/redeem", None, json!({ "token": token, "name": "Second" }).to_string().as_bytes());
    let unknown = fx.hub.handle("POST", "/v1/redeem", None, json!({ "token": "00", "name": "X" }).to_string().as_bytes());
    assert_eq!((again.code, &again.body), (400, &unknown.body));
    // Expired.
    let inv = fx.call("POST", &format!("/v1/workspaces/{}/invitations", fx.ws), Some("owner"), json!({ "role": "viewer", "hours": 1 }));
    fx.hub.db.lock().unwrap().execute("UPDATE invitations SET expires_at = '2000-01-01T00:00:00Z' WHERE id = ?1", [inv.body["id"].as_str().unwrap()]).unwrap();
    let r = fx.hub.handle("POST", "/v1/redeem", None, json!({ "token": inv.body["token"], "name": "Late" }).to_string().as_bytes());
    assert_eq!((r.code, &r.body), (400, &unknown.body));
    // Revoked.
    let inv = fx.call("POST", &format!("/v1/workspaces/{}/invitations", fx.ws), Some("owner"), json!({ "role": "viewer" }));
    fx.call("DELETE", &format!("/v1/workspaces/{}/invitations/{}", fx.ws, inv.body["id"].as_str().unwrap()), Some("owner"), json!({}));
    let r = fx.hub.handle("POST", "/v1/redeem", None, json!({ "token": inv.body["token"], "name": "Gone" }).to_string().as_bytes());
    assert_eq!((r.code, &r.body), (400, &unknown.body));
    // Over 7 days is refused; an existing member can't redeem into the same workspace and doesn't burn the invite.
    assert_eq!(fx.call("POST", &format!("/v1/workspaces/{}/invitations", fx.ws), Some("owner"), json!({ "role": "viewer", "hours": 169 })).code, 400);
    let inv = fx.call("POST", &format!("/v1/workspaces/{}/invitations", fx.ws), Some("owner"), json!({ "role": "viewer" }));
    let r = fx.call("POST", "/v1/redeem", Some("editor"), json!({ "token": inv.body["token"] }));
    assert_eq!(r.code, 409);
    // A signed-in user joining a second workspace keeps one account and gets no new device.
    let r = fx.call("POST", "/v1/redeem", Some("outsider"), json!({ "token": inv.body["token"] }));
    assert_eq!(r.code, 200, "{r:?}");
    assert!(r.body["device"].is_null());
    assert_eq!(fx.call("GET", "/v1/me", Some("outsider"), json!({})).body["workspaces"].as_array().unwrap().len(), 2);
    // A new user needs a name.
    let inv = fx.call("POST", &format!("/v1/workspaces/{}/invitations", fx.ws), Some("owner"), json!({ "role": "viewer" }));
    assert_eq!(fx.hub.handle("POST", "/v1/redeem", None, json!({ "token": inv.body["token"] }).to_string().as_bytes()).code, 400);
}

#[test]
fn custom_permissions_and_ownership_rules() {
    let fx = fixture();
    let ws = &fx.ws;
    let viewer = fx.user("viewer").to_string();
    // Grant task.create to a viewer works; access-controlling permissions can't be granted.
    assert_eq!(fx.call("PATCH", &format!("/v1/workspaces/{ws}/members/{viewer}"), Some("admin"), json!({ "grant": ["task.create"] })).code, 200);
    assert_eq!(fx.call("PATCH", &format!("/v1/workspaces/{ws}/members/{viewer}"), Some("admin"), json!({ "grant": ["members.invite"] })).code, 400);
    assert_eq!(fx.call("PATCH", &format!("/v1/workspaces/{ws}/members/{viewer}"), Some("admin"), json!({ "grant": ["nope"] })).code, 400);
    // Denying project.read empties their project list; scoping to an unknown project is refused.
    fx.call("PATCH", &format!("/v1/workspaces/{ws}/members/{viewer}"), Some("admin"), json!({ "deny": ["project.read"] }));
    assert_eq!(fx.call("GET", &format!("/v1/workspaces/{ws}/projects"), Some("viewer"), json!({})).body["projects"], json!([]));
    assert_eq!(fx.call("PATCH", &format!("/v1/workspaces/{ws}/members/{viewer}"), Some("admin"), json!({ "projects": ["p_nope"] })).code, 400);
    // Scoping a member hides the other project at once.
    let editor = fx.user("editor").to_string();
    fx.call("PATCH", &format!("/v1/workspaces/{ws}/members/{editor}"), Some("owner"), json!({ "projects": [fx.p2] }));
    assert_eq!(fx.call("GET", &format!("/v1/workspaces/{ws}/projects/{}", fx.p1), Some("editor"), json!({})).code, 404);
    // Nobody can demote or remove the owner; transfer swaps owner and admin, and the old owner loses owner rights.
    let owner = fx.user("owner").to_string();
    assert_eq!(fx.call("DELETE", &format!("/v1/workspaces/{ws}/members/{owner}"), Some("admin"), json!({})).code, 403);
    assert_eq!(fx.call("DELETE", &format!("/v1/workspaces/{ws}/members/{owner}"), Some("owner"), json!({})).code, 403, "not even themselves");
    assert_eq!(fx.call("POST", &format!("/v1/workspaces/{ws}/transfer"), Some("owner"), json!({ "user_id": owner })).code, 400);
    assert_eq!(fx.call("POST", &format!("/v1/workspaces/{ws}/transfer"), Some("owner"), json!({ "user_id": fx.user("admin") })).code, 200);
    assert_eq!(fx.call("GET", &format!("/v1/workspaces/{ws}"), Some("owner"), json!({})).body["role"], "admin");
    assert_eq!(fx.call("DELETE", &format!("/v1/workspaces/{ws}"), Some("owner"), json!({})).code, 403);
    let owners = fx.hub.db.lock().unwrap().query_row("SELECT COUNT(*) FROM memberships WHERE workspace_id = ?1 AND role = 'owner' AND revoked = 0", [ws], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(owners, 1);
    // A deleted workspace is gone for everyone, including its owner.
    assert_eq!(fx.call("DELETE", &format!("/v1/workspaces/{ws}"), Some("admin"), json!({})).code, 200);
    assert_eq!(fx.call("GET", &format!("/v1/workspaces/{ws}"), Some("admin"), json!({})).code, 404);
}

#[test]
fn audit_records_actions_and_denials_and_is_append_only() {
    let fx = fixture();
    let ws = &fx.ws;
    fx.call("PATCH", &format!("/v1/workspaces/{ws}"), Some("viewer"), json!({ "name": "x" }));
    fx.call("PATCH", &format!("/v1/workspaces/{ws}"), Some("viewer"), json!({ "name": "x" }));
    let log = fx.call("GET", &format!("/v1/workspaces/{ws}/audit"), Some("owner"), json!({}));
    let actions: Vec<&str> = log.body["entries"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap()).collect();
    for a in ["workspace.created", "project.created", "invitation.created", "invitation.redeemed"] {
        assert!(actions.contains(&a), "{a} in {actions:?}");
    }
    assert_eq!(actions.iter().filter(|a| **a == "access.denied").count(), 1, "denials are rate-limited");
    let last = log.body["entries"].as_array().unwrap().last().unwrap()["seq"].as_i64().unwrap();
    assert_eq!(fx.call("GET", &format!("/v1/workspaces/{ws}/audit?since={last}"), Some("owner"), json!({})).body["entries"], json!([]));
    // No token ever lands in the audit log.
    assert!(!log.body.to_string().contains(fx.token("owner").split('.').nth(1).unwrap()));
    let c = fx.hub.db.lock().unwrap();
    assert!(c.execute("UPDATE audit SET action = 'x'", []).is_err());
    assert!(c.execute("DELETE FROM audit", []).is_err());
}

#[test]
fn bad_requests_are_refused_cleanly() {
    let fx = fixture();
    let auth = format!("Bearer {}", fx.token("owner"));
    assert_eq!(fx.hub.handle("POST", &format!("/v1/workspaces/{}/projects", fx.ws), Some(&auth), b"not json").code, 400);
    assert_eq!(fx.hub.handle("POST", &format!("/v1/workspaces/{}/projects", fx.ws), Some(&auth), b"[1]").code, 400);
    assert_eq!(fx.call("POST", &format!("/v1/workspaces/{}/projects", fx.ws), Some("owner"), json!({ "name": "" })).code, 400);
    assert_eq!(fx.call("POST", &format!("/v1/workspaces/{}/projects", fx.ws), Some("owner"), json!({ "name": "x".repeat(81) })).code, 400);
    assert_eq!(fx.call("GET", "/v1/nothing", Some("owner"), json!({})).code, 404);
    assert_eq!(fx.call("GET", "/", Some("owner"), json!({})).code, 404);
    assert_eq!(fx.hub.handle("GET", "/v1/health", None, b"").code, 200);
}

#[test]
fn bind_policy_is_loopback_and_tailscale_only() {
    for ok in ["127.0.0.1", "100.64.0.1", "100.101.102.103", "100.127.255.255", "::1", "fd7a:115c:a1e0::1"] {
        assert!(private_bind(ok.parse().unwrap()), "{ok}");
    }
    for no in ["0.0.0.0", "192.168.1.5", "100.63.0.1", "100.128.0.1", "8.8.8.8", "::", "fd00::1"] {
        assert!(!private_bind(no.parse().unwrap()), "{no}");
    }
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ptcc-hub-test-{name}-{}", store::random_hex(4).unwrap()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn restore_drill() {
    let data = temp_dir("restore");
    let (ws, backup) = {
        let c = store::open(Some(&data)).unwrap();
        let (ws, _) = new_workspace(&c, "Before").unwrap();
        let b = store::daily_backup(&c, &store::backups_dir(&data)).unwrap().expect("first backup of the day");
        assert!(store::daily_backup(&c, &store::backups_dir(&data)).unwrap().is_none(), "once a day");
        new_workspace(&c, "After").unwrap();
        (ws, b)
    };
    // Can't restore under a running hub.
    let lock = store::lock(&data).unwrap();
    assert!(store::restore(&data, &backup).unwrap_err().contains("running"));
    drop(lock);
    // A file that isn't a hub database is refused and the live one is untouched.
    let junk = data.join("junk.db");
    std::fs::write(&junk, b"not sqlite").unwrap();
    assert!(store::restore(&data, &junk).is_err());
    let saved = store::restore(&data, &backup).unwrap();
    let c = store::open(Some(&data)).unwrap();
    let names: Vec<String> = c.prepare("SELECT name FROM workspaces ORDER BY name").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(names, ["Before"], "back to the backup's state");
    let id: String = c.query_row("SELECT id FROM workspaces", [], |r| r.get(0)).unwrap();
    assert_eq!(id, ws);
    // The pre-restore copy still has both workspaces.
    let old = rusqlite::Connection::open(&saved).unwrap();
    assert_eq!(old.query_row("SELECT COUNT(*) FROM workspaces", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
    drop((c, old));
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn daily_backups_rotate_keeping_14_and_monthlies() {
    let data = temp_dir("rotate");
    let dir = store::backups_dir(&data);
    std::fs::create_dir_all(&dir).unwrap();
    for d in 1..=28 {
        std::fs::write(dir.join(format!("hub-2020-02-{d:02}.db")), b"").unwrap();
    }
    std::fs::write(dir.join("hub-2020-01-01.db"), b"").unwrap();
    std::fs::write(dir.join("hub-2020-01-09.db"), b"").unwrap();
    std::fs::write(dir.join("pre-migrate-v1-x.db"), b"").unwrap();
    let c = store::open(None).unwrap();
    store::daily_backup(&c, &dir).unwrap();
    let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    left.sort();
    assert!(left.contains(&"hub-2020-01-01.db".into()) && left.contains(&"hub-2020-02-01.db".into()), "monthlies kept");
    assert!(!left.contains(&"hub-2020-01-09.db".into()) && !left.contains(&"hub-2020-02-02.db".into()));
    assert!(left.contains(&"pre-migrate-v1-x.db".into()), "other backups untouched");
    assert_eq!(left.iter().filter(|n| n.starts_with("hub-") && !n.ends_with("-01.db")).count(), 14);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn real_http_round_trip() {
    let hub = Arc::new(Hub::new(store::open(None).unwrap(), None));
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let port = server.server_addr().to_ip().unwrap().port();
    serve(hub, server.clone(), 2);
    let get = |path: &str, extra: &str| {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n{extra}\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    };
    assert!(get("/v1/health", "").starts_with("HTTP/1.1 200"));
    // The phone board is served with a strict CSP and no inline script.
    let page = get("/", "");
    assert!(page.starts_with("HTTP/1.1 200") && page.contains("text/html") && page.contains("Content-Security-Policy") && page.contains("<script src=\"/app.js\">"), "{page}");
    assert!(!page.contains("<script>"), "no inline script");
    assert!(get("/app.js?v=1", "").contains("text/javascript"));
    assert!(get("/nope", "").starts_with("HTTP/1.1 404"));
    let r = get("/v1/me", "Authorization: Bearer d_x.y\r\n");
    assert!(r.starts_with("HTTP/1.1 401") && r.contains("application/json") && r.contains("no-store"), "{r}");
    server.unblock();
}

#[test]
fn named_roles_start_with_ops_and_artist_and_carry_their_permissions() {
    let fx = fixture();
    let ws = &fx.ws;
    let roles = fx.call("GET", &format!("/v1/workspaces/{ws}/roles"), Some("viewer"), json!({}));
    let names: Vec<&str> = roles.body["roles"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Ops", "Artist"], "every workspace starts with Ops and Artist");
    // Profit joins as Ops through an invite carrying the role; Ops can share projects (project.link) though its base is editor.
    let inv = fx.call("POST", &format!("/v1/workspaces/{ws}/invitations"), Some("owner"), json!({ "title": "Ops" }));
    assert_eq!(inv.body["role"], "editor");
    let r = fx.hub.handle("POST", "/v1/redeem", None, json!({ "token": inv.body["token"], "name": "Profit" }).to_string().as_bytes());
    assert_eq!((r.code, r.body["title"].as_str()), (200, Some("Ops")));
    let profit = format!("Bearer {}", r.body["device"]["token"].as_str().unwrap());
    let call = |auth: &str, m: &str, path: String, b: Value| fx.hub.handle(m, &path, Some(auth), b.to_string().as_bytes());
    assert_eq!(call(&profit, "POST", format!("/v1/workspaces/{ws}/projects"), json!({ "name": "ops board" })).code, 200, "Ops has project.link");
    assert_eq!(fx.call("POST", &format!("/v1/workspaces/{ws}/projects"), Some("editor"), json!({ "name": "x" })).code, 403, "a plain editor doesn't");
    let me = call(&profit, "GET", "/v1/me".into(), json!({}));
    assert_eq!(me.body["workspaces"][0]["title"], "Ops");
    // Rich becomes the Artist: contributor-level, so he can claim and add but not assign others.
    let viewer = fx.user("viewer").to_string();
    assert_eq!(fx.call("PATCH", &format!("/v1/workspaces/{ws}/members/{viewer}"), Some("owner"), json!({ "title": "Artist" })).code, 200);
    let members = fx.call("GET", &format!("/v1/workspaces/{ws}/members"), Some("viewer"), json!({}));
    let rich = members.body["members"].as_array().unwrap().iter().find(|m| m["user_id"] == viewer.as_str()).unwrap().clone();
    assert_eq!((rich["role"].as_str(), rich["title"].as_str()), (Some("contributor"), Some("Artist")));
    // Paper edits Artist: removing media.upload takes effect for every Artist at once.
    assert_eq!(fx.call("POST", &format!("/v1/workspaces/{ws}/roles"), Some("owner"), json!({ "name": "Artist", "base": "contributor", "deny": ["media.upload"] })).code, 200);
    let auth = format!("Bearer {}", fx.token("viewer"));
    let png = b"img".to_vec();
    let up = fx.hub.handle("PUT", &format!("/v1/workspaces/{ws}/projects/{}/blobs/{}", fx.p1, ptcc_hub::store::hash_bytes(&png)), Some(&auth), &png);
    assert_eq!(up.code, 403);
    // Raising Artist's base moves its holders; admins can't touch roles at or above their own level.
    assert_eq!(fx.call("POST", &format!("/v1/workspaces/{ws}/roles"), Some("owner"), json!({ "name": "Artist", "base": "editor" })).code, 200);
    assert_eq!(fx.call("GET", &format!("/v1/workspaces/{ws}"), Some("viewer"), json!({})).body["role"], "editor");
    assert_eq!(fx.call("POST", &format!("/v1/workspaces/{ws}/roles"), Some("admin"), json!({ "name": "Artist", "base": "admin" })).code, 403);
    // In-use roles can't be deleted; names can't shadow built-ins or carry odd characters; grants stay limited.
    assert_eq!(fx.call("DELETE", &format!("/v1/workspaces/{ws}/roles"), Some("owner"), json!({ "name": "Artist" })).code, 409);
    for bad in [json!({ "name": "Admin", "base": "viewer" }), json!({ "name": "<b>x</b>", "base": "viewer" }), json!({ "name": "", "base": "viewer" }), json!({ "name": "Boss", "base": "owner" }),
                json!({ "name": "Gate", "base": "viewer", "grant": ["members.invite"] })] {
        assert_eq!(fx.call("POST", &format!("/v1/workspaces/{ws}/roles"), Some("owner"), bad.clone()).code, 400, "{bad}");
    }
    assert_eq!(fx.call("PATCH", &format!("/v1/workspaces/{ws}/members/{viewer}"), Some("owner"), json!({ "title": "Nope" })).code, 400);
    // Back to a built-in role clears the name.
    fx.call("PATCH", &format!("/v1/workspaces/{ws}/members/{viewer}"), Some("owner"), json!({ "role": "viewer" }));
    let members = fx.call("GET", &format!("/v1/workspaces/{ws}/members"), Some("owner"), json!({}));
    assert!(members.body["members"].as_array().unwrap().iter().any(|m| m["user_id"] == viewer.as_str() && m["title"].is_null()));
    let audit = fx.call("GET", &format!("/v1/workspaces/{ws}/audit"), Some("owner"), json!({})).body.to_string();
    assert!(audit.contains("role.updated"));
}

#[test]
fn only_the_owner_puts_people_in_or_takes_them_out() {
    let fx = fixture();
    let ws = &fx.ws;
    let viewer = fx.user("viewer").to_string();
    // A second workspace Paper owns, with nobody else in it yet.
    let (ws2, inv) = ptcc_hub::api::new_workspace(&fx.hub.db.lock().unwrap(), "BOMBSQUAD").unwrap();
    assert_eq!(fx.call("POST", "/v1/redeem", Some("owner"), json!({ "token": inv })).code, 200);
    let people = fx.call("GET", "/v1/people", Some("owner"), json!({}));
    assert_eq!(people.body["workspaces"].as_array().unwrap().len(), 2);
    let v = people.body["people"].as_array().unwrap().iter().find(|p| p["user_id"] == viewer.as_str()).unwrap().clone();
    assert_eq!(v["workspaces"][ws]["role"], "viewer");
    assert!(v["workspaces"].get(&ws2).is_none());
    assert!(!people.body.to_string().contains(fx.user("outsider")), "only people in the owner's workspaces");
    assert_eq!(fx.call("GET", "/v1/people", Some("admin"), json!({})).body["people"], json!([]), "non-owners see nobody");
    // Paper adds the viewer to BOMBSQUAD as Artist; nobody else can.
    let add = |actor: &str, body: Value| fx.call("POST", &format!("/v1/workspaces/{ws2}/members"), Some(actor), body).code;
    assert_eq!(add("admin", json!({ "user_id": viewer, "title": "Artist" })), 404, "the admin isn't in BOMBSQUAD at all");
    assert_eq!(add("owner", json!({ "user_id": viewer, "title": "Artist" })), 200);
    assert_eq!(add("owner", json!({ "user_id": viewer, "role": "viewer" })), 409);
    assert_eq!(add("owner", json!({ "user_id": fx.user("outsider"), "role": "viewer" })), 404, "strangers join only by invitation");
    assert_eq!(add("owner", json!({ "user_id": "u_nope", "role": "viewer" })), 404);
    assert_eq!(fx.call("GET", &format!("/v1/workspaces/{ws2}"), Some("viewer"), json!({})).body["role"], "contributor");
    // In FLY WING the admin can still change roles below them but can't add or remove anyone.
    assert_eq!(fx.call("POST", &format!("/v1/workspaces/{ws}/members"), Some("admin"), json!({ "user_id": fx.user("outsider"), "role": "viewer" })).code, 403);
    assert_eq!(fx.call("DELETE", &format!("/v1/workspaces/{ws}/members/{viewer}"), Some("admin"), json!({})).code, 403);
    assert_eq!(fx.call("PATCH", &format!("/v1/workspaces/{ws}/members/{viewer}"), Some("admin"), json!({ "role": "contributor" })).code, 200);
    // Paper takes the viewer out of BOMBSQUAD; FLY WING is untouched.
    assert_eq!(fx.call("DELETE", &format!("/v1/workspaces/{ws2}/members/{viewer}"), Some("owner"), json!({})).code, 200);
    assert_eq!(fx.call("GET", &format!("/v1/workspaces/{ws2}"), Some("viewer"), json!({})).code, 404);
    assert_eq!(fx.call("GET", &format!("/v1/workspaces/{ws}"), Some("viewer"), json!({})).code, 200);
    // Denying workspace.read (to a member or through a role) would lock someone out without removing them: refused.
    assert_eq!(fx.call("PATCH", &format!("/v1/workspaces/{ws}/members/{viewer}"), Some("admin"), json!({ "deny": ["workspace.read"] })).code, 400);
    assert_eq!(fx.call("POST", &format!("/v1/workspaces/{ws}/roles"), Some("admin"), json!({ "name": "Ghost", "base": "viewer", "deny": ["workspace.read"] })).code, 400);
    assert_eq!(fx.call("GET", &format!("/v1/workspaces/{ws}"), Some("viewer"), json!({})).code, 200);
    // And can add them back later.
    assert_eq!(add("owner", json!({ "user_id": viewer, "role": "viewer" })), 200);
}

#[test]
fn a_device_link_signs_in_another_device_as_the_same_person_once() {
    let fx = fixture();
    let link = fx.call("POST", "/v1/devices/link", Some("editor"), json!({}));
    assert_eq!(link.code, 200);
    let token = link.body["token"].as_str().unwrap().to_string();
    let claim = |t: &str| fx.hub.handle("POST", "/v1/devices/claim", None, json!({ "token": t, "device_name": "iPad" }).to_string().as_bytes());
    let peek = fx.hub.handle("POST", "/v1/devices/claim", None, json!({ "token": token, "preview": true }).to_string().as_bytes());
    assert_eq!(peek.body["name"], "editor", "preview names the account and uses nothing up");
    let r = claim(&token);
    assert_eq!((r.code, r.body["user_id"].as_str()), (200, Some(fx.user("editor"))), "same account");
    let ipad = format!("Bearer {}", r.body["device"]["token"].as_str().unwrap());
    let me = fx.hub.handle("GET", "/v1/me", Some(&ipad), b"");
    assert_eq!(me.body["workspaces"][0]["role"], "editor", "same workspaces and role");
    // Single use; unknown and expired links fail the same way; anonymous callers can't make links.
    let again = claim(&token);
    assert_eq!((again.code, &again.body), (400, &claim("00").body));
    let l2 = fx.call("POST", "/v1/devices/link", Some("editor"), json!({}));
    fx.hub.db.lock().unwrap().execute("UPDATE device_links SET expires_at = '2000-01-01T00:00:00Z'", []).unwrap();
    assert_eq!(claim(l2.body["token"].as_str().unwrap()).code, 400);
    assert_eq!(fx.hub.handle("POST", "/v1/devices/link", None, b"").code, 401);
    // A link dies with the device that made it.
    let l3 = fx.call("POST", "/v1/devices/link", Some("editor"), json!({}));
    let maker = fx.call("GET", "/v1/me", Some("editor"), json!({})).body["device_id"].as_str().unwrap().to_string();
    assert_eq!(fx.call("DELETE", &format!("/v1/devices/{maker}"), Some("editor"), json!({})).code, 200);
    assert_eq!(claim(l3.body["token"].as_str().unwrap()).code, 400);
    // Revoking the person revokes every device, the new one included.
    ptcc_hub::api::revoke_user(&fx.hub.db.lock().unwrap(), fx.user("editor")).unwrap();
    assert_eq!(fx.hub.handle("GET", "/v1/me", Some(&ipad), b"").code, 401);
}

#[test]
fn app_updates_come_from_the_share_folder_to_signed_in_devices_only() {
    let base = std::env::temp_dir().join(format!("ptcc-hub-test-share-{}", store::random_hex(4).unwrap()));
    let hub = Hub::new(store::open(None).unwrap(), Some(base.join("media")));
    let (_, inv) = new_workspace(&hub.db.lock().unwrap(), "W").unwrap();
    let r = hub.handle("POST", "/v1/redeem", None, json!({ "token": inv, "name": "Rich" }).to_string().as_bytes());
    let auth = format!("Bearer {}", r.body["device"]["token"].as_str().unwrap());
    assert_eq!(hub.handle("GET", "/v1/app/latest", Some(&auth), b"").code, 404, "nothing published yet");
    std::fs::create_dir_all(base.join("share")).unwrap();
    std::fs::write(base.join("share").join(ptcc_hub::api::INSTALLER), b"MZ installer").unwrap();
    std::fs::write(base.join("share/latest.json"), json!({ "exe_sha256": "ab", "installer_sha256": store::hash_bytes(b"MZ installer"), "commit": "8f24663", "secret": "no" }).to_string()).unwrap();
    let latest = hub.handle("GET", "/v1/app/latest", Some(&auth), b"");
    assert_eq!((latest.code, latest.body["commit"].as_str()), (200, Some("8f24663")));
    assert!(latest.body.get("secret").is_none(), "only the known fields");
    assert_eq!(hub.handle("GET", "/v1/app/installer", Some(&auth), b"").raw.as_deref(), Some(&b"MZ installer"[..]));
    assert_eq!(hub.handle("GET", "/v1/app/installer", None, b"").code, 401);
    assert_eq!(hub.handle("GET", "/v1/app/latest", Some("Bearer d_x.y"), b"").code, 401);
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn the_owner_issues_a_signin_link_for_a_teammate_and_nobody_else_can() {
    let fx = fixture();
    let path = |a: &str| format!("/v1/people/{}/signin-link", fx.user(a));
    let r = fx.call("POST", &path("viewer"), Some("owner"), json!({}));
    assert_eq!(r.code, 200);
    let token = r.body["token"].as_str().unwrap().to_string();
    let claim = |t: &str| fx.hub.handle("POST", "/v1/devices/claim", None, json!({ "token": t, "device_name": "iPhone" }).to_string().as_bytes());
    let c = claim(&token);
    assert_eq!((c.code, c.body["user_id"].as_str()), (200, Some(fx.user("viewer"))), "signs into the teammate's existing account");
    assert_eq!(claim(&token).code, 400, "once");
    // Only an owner of a shared workspace, never for themselves, another owner or a stranger.
    for (who, target) in [("admin", "viewer"), ("editor", "viewer"), ("outsider", "viewer"), ("owner", "owner"), ("owner", "outsider")] {
        assert_eq!(fx.call("POST", &path(target), Some(who), json!({})).code, 403, "{who} for {target}");
    }
    assert_eq!(fx.hub.handle("POST", &path("viewer"), None, b"").code, 401);
}
