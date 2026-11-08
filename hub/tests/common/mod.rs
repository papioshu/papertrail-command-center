//! Shared fixture: workspace "FLY WING" with projects p1 (site) and p2 (contracts), one member per role, a member
//! scoped to p1 and an outsider who owns another workspace.
#![allow(dead_code)]
use ptcc_hub::api::{new_workspace, Hub, Reply};
use ptcc_hub::store;
use serde_json::{json, Value};

/// One media folder for every test hub in this process (content-addressed, so sharing is harmless).
pub fn media_dir() -> std::path::PathBuf {
    static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| std::env::temp_dir().join(format!("ptcc-hub-media-{}", std::process::id()))).clone()
}

pub const ACTORS: [&str; 7] = ["owner", "admin", "editor", "contributor", "viewer", "scoped", "outsider"];

pub struct Fx {
    pub hub: Hub,
    pub ws: String,
    pub other_ws: String,
    pub p1: String,
    pub p2: String,
    /// actor name → (user id, bearer)
    pub who: Vec<(&'static str, String, String)>,
    pub invite_viewer: String,
}

impl Fx {
    pub fn call(&self, method: &str, path: &str, as_: Option<&str>, body: Value) -> Reply {
        let auth = as_.map(|a| format!("Bearer {}", self.token(a)));
        self.hub.handle(method, path, auth.as_deref(), body.to_string().as_bytes())
    }
    pub fn token(&self, a: &str) -> &str {
        &self.who.iter().find(|w| w.0 == a).unwrap().2
    }
    pub fn user(&self, a: &str) -> &str {
        &self.who.iter().find(|w| w.0 == a).unwrap().1
    }
}

pub fn redeem(hub: &Hub, token: &str, name: &str) -> (String, String) {
    let r = hub.handle("POST", "/v1/redeem", None, json!({ "token": token, "name": name, "device_name": "pc" }).to_string().as_bytes());
    assert_eq!(r.code, 200, "{r:?}");
    (r.body["user_id"].as_str().unwrap().into(), r.body["device"]["token"].as_str().unwrap().into())
}

/// Workspace with projects p1, p2 and one member per role, a member scoped to p1 and an outsider (owner elsewhere).
pub fn fixture() -> Fx {
    let hub = Hub::new(store::open(None).unwrap(), Some(media_dir()));
    let (ws, owner_inv) = new_workspace(&hub.db.lock().unwrap(), "FLY WING").unwrap();
    let (other_ws, other_inv) = new_workspace(&hub.db.lock().unwrap(), "OARA").unwrap();
    let mut who = Vec::new();
    let (u, t) = redeem(&hub, &owner_inv, "Rich");
    who.push(("owner", u, t.clone()));
    let auth = format!("Bearer {t}");
    let post = |path: &str, b: Value| hub.handle("POST", path, Some(&auth), b.to_string().as_bytes());
    let p1 = post(&format!("/v1/workspaces/{ws}/projects"), json!({ "name": "site" })).body["id"].as_str().unwrap().to_string();
    let p2 = post(&format!("/v1/workspaces/{ws}/projects"), json!({ "name": "contracts" })).body["id"].as_str().unwrap().to_string();
    for (actor, role, scope) in [("admin", "admin", None), ("editor", "editor", None), ("contributor", "contributor", None), ("viewer", "viewer", None), ("scoped", "editor", Some(json!([p1])))] {
        let inv = post(&format!("/v1/workspaces/{ws}/invitations"), json!({ "role": role, "projects": scope }));
        assert_eq!(inv.code, 200, "{inv:?}");
        let (u, t) = redeem(&hub, inv.body["token"].as_str().unwrap(), actor);
        who.push((actor, u, t));
    }
    let (u, t) = redeem(&hub, &other_inv, "Outsider");
    who.push(("outsider", u, t));
    let invite_viewer = post(&format!("/v1/workspaces/{ws}/invitations"), json!({ "role": "viewer" })).body["id"].as_str().unwrap().to_string();
    Fx { hub, ws, other_ws, p1, p2, who, invite_viewer }
}

