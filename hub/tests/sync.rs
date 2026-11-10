//! M2 gate (ARCHITECTURE.md §7, §13): two clients with outboxes against one hub — offline edits, conflicting edits,
//! duplicate retries, revocation mid-queue — plus per-op authorization, field allowlists, the scoped feed and media.
mod common;
use common::*;
use ptcc_hub::store;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn uuid() -> String {
    let h = store::random_hex(16).unwrap();
    format!("{}-{}-4{}-8{}-{}", &h[0..8], &h[8..12], &h[13..16], &h[17..20], &h[20..32])
}

/// The protocol a desktop client follows (M3 builds the real one): edits go to the cache and an ordered outbox;
/// sync sends ops in order, then pulls the feed. 409 = keep a conflict copy; a workspace that vanishes = revoked.
struct Client<'a> {
    fx: &'a Fx,
    actor: &'static str,
    online: bool,
    outbox: Vec<Value>,
    cache: BTreeMap<String, Value>,
    cursor: i64,
    projects: Vec<String>,
    conflicts: Vec<(Value, Value)>,
    rejected: Vec<Value>,
    unsent: Vec<Value>,
    revoked: bool,
}

impl<'a> Client<'a> {
    fn new(fx: &'a Fx, actor: &'static str) -> Self {
        let mut c = Client { fx, actor, online: true, outbox: vec![], cache: BTreeMap::new(), cursor: 0, projects: vec![], conflicts: vec![], rejected: vec![], unsent: vec![], revoked: false };
        c.sync();
        c
    }
    fn path(&self, tail: &str) -> String {
        format!("/v1/workspaces/{}/{tail}", self.fx.ws)
    }
    fn queue(&mut self, action: &str, kind: &str, id: &str, project: &str, fields: Value) {
        let base = self.cache.get(id).and_then(|r| r["version"].as_i64()).unwrap_or(0);
        self.outbox.push(json!({ "op_id": uuid(), "action": action, "kind": kind, "record_id": id, "project_id": project, "base_version": base, "fields": fields }));
        // Local cache shows the edit at once (offline-first).
        let r = self.cache.entry(id.to_string()).or_insert_with(|| json!({ "id": id, "kind": kind, "project_id": project, "version": 0, "fields": {} }));
        for (k, v) in fields.as_object().into_iter().flatten() {
            r["fields"][k] = v.clone();
        }
    }
    fn create(&mut self, kind: &str, project: &str, fields: Value) -> String {
        let id = uuid();
        self.queue("create", kind, &id, project, fields);
        id
    }
    fn send(&self, op: &Value) -> ptcc_hub::api::Reply {
        self.fx.call("POST", &self.path("ops"), Some(self.actor), op.clone())
    }
    fn sync(&mut self) {
        if !self.online || self.revoked {
            return;
        }
        while !self.outbox.is_empty() {
            let op = self.outbox[0].clone();
            let r = self.send(&op);
            match r.code {
                200 => {
                    let rec = r.body["record"].clone();
                    self.cache.insert(rec["id"].as_str().unwrap().into(), rec);
                }
                409 => self.conflicts.push((op, r.body.clone())),
                _ if self.fx.call("GET", &format!("/v1/workspaces/{}", self.fx.ws), Some(self.actor), json!({})).code == 404 => return self.purge(),
                _ => self.rejected.push(op),
            }
            self.outbox.remove(0);
        }
        self.pull();
    }
    fn pull(&mut self) {
        loop {
            let r = self.fx.call("GET", &self.path(&format!("changes?since={}&limit=2", self.cursor)), Some(self.actor), json!({}));
            if r.code == 404 {
                return self.purge();
            }
            let projects: Vec<String> = r.body["projects"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap().into()).collect();
            if projects.iter().any(|p| !self.projects.contains(p)) && self.cursor > 0 {
                // A project became visible: its older records are behind our cursor, so resync from the start.
                self.projects = projects;
                self.cursor = 0;
                continue;
            }
            self.cache.retain(|_, rec| projects.iter().any(|p| rec["project_id"] == *p));
            self.projects = projects;
            for rec in r.body["changes"].as_array().unwrap() {
                self.cache.insert(rec["id"].as_str().unwrap().into(), rec.clone());
            }
            self.cursor = r.body["next"].as_i64().unwrap();
            if !r.body["more"].as_bool().unwrap() {
                return;
            }
        }
    }
    /// §7.5: keep our own unsent work as a private file's worth of ops, drop every cached workspace record.
    fn purge(&mut self) {
        self.unsent.append(&mut self.outbox);
        self.cache.clear();
        self.projects.clear();
        self.revoked = true;
    }
    fn field(&self, id: &str, k: &str) -> Value {
        self.cache.get(id).map(|r| r["fields"][k].clone()).unwrap_or(Value::Null)
    }
}

#[test]
fn offline_edits_merge_with_disjoint_edits_from_another_client() {
    let fx = fixture();
    let mut a = Client::new(&fx, "editor");
    let mut b = Client::new(&fx, "admin");
    let t = a.create("task", &fx.p1, json!({ "title": "Ship the mint page", "status": "todo" }));
    a.sync();
    b.sync();
    assert_eq!(b.field(&t, "title"), "Ship the mint page");
    // A goes offline and edits the same task twice; B changes a different field meanwhile.
    a.online = false;
    a.queue("update", "task", &t, &fx.p1, json!({ "status": "doing" }));
    a.queue("update", "task", &t, &fx.p1, json!({ "status": "blocked", "notes": "waiting on art" }));
    b.queue("update", "task", &t, &fx.p1, json!({ "due": "2026-12-01" }));
    b.sync();
    a.online = true;
    a.sync();
    b.sync();
    assert!(a.conflicts.is_empty(), "{:?}", a.conflicts);
    for c in [&a, &b] {
        assert_eq!(c.field(&t, "status"), "blocked");
        assert_eq!(c.field(&t, "notes"), "waiting on art");
        assert_eq!(c.field(&t, "due"), "2026-12-01");
        assert_eq!(c.cache[&t]["version"], 4, "create + B's edit + A's two edits");
    }
}

#[test]
fn same_field_edits_conflict_and_keep_both_sides() {
    let fx = fixture();
    let mut a = Client::new(&fx, "editor");
    let mut b = Client::new(&fx, "admin");
    let t = a.create("task", &fx.p1, json!({ "title": "Old title" }));
    a.sync();
    b.sync();
    a.queue("update", "task", &t, &fx.p1, json!({ "title": "A's title" }));
    b.queue("update", "task", &t, &fx.p1, json!({ "title": "B's title" }));
    b.sync();
    a.sync();
    assert_eq!(a.conflicts.len(), 1);
    let (mine, theirs) = &a.conflicts[0];
    assert_eq!(mine["fields"]["title"], "A's title", "nothing is silently dropped");
    assert_eq!(theirs["error"], "conflict");
    assert_eq!(theirs["fields"], json!(["title"]));
    assert_eq!(theirs["record"]["fields"]["title"], "B's title");
    assert_eq!(a.field(&t, "title"), "B's title", "the hub's value wins until the author chooses");
    // "Keep mine": re-send against the version they've now seen.
    let (mine, _) = a.conflicts.remove(0);
    a.queue("update", "task", &t, &fx.p1, mine["fields"].clone());
    a.sync();
    b.sync();
    assert!(a.conflicts.is_empty());
    assert_eq!(b.field(&t, "title"), "A's title");
    // The same person's other device still conflicts (conflicts are per device, not per user).
    let other = fx.call("POST", "/v1/devices", Some("editor"), json!({ "name": "laptop" }));
    let laptop = format!("Bearer {}", other.body["token"].as_str().unwrap());
    let v = a.cache[&t]["version"].as_i64().unwrap();
    a.queue("update", "task", &t, &fx.p1, json!({ "title": "from the PC" }));
    a.sync();
    let r = fx.hub.handle("POST", &a.path("ops"), Some(&laptop), json!({ "op_id": uuid(), "action": "update", "kind": "task", "record_id": t, "base_version": v, "fields": { "title": "from the laptop" } }).to_string().as_bytes());
    assert_eq!(r.code, 409);
}

#[test]
fn duplicate_retries_apply_once() {
    let fx = fixture();
    let mut a = Client::new(&fx, "editor");
    let t = a.create("task", &fx.p1, json!({ "title": "x" }));
    a.sync();
    a.queue("update", "task", &t, &fx.p1, json!({ "status": "done" }));
    let op = a.outbox[0].clone();
    // The reply is lost on the way back: the client retries the same op three times.
    let first = a.send(&op);
    let second = a.send(&op);
    let third = a.send(&op);
    assert_eq!(first.code, 200);
    assert_eq!((&first, &first), (&second, &third));
    assert_eq!(first.body["record"]["version"], 2, "applied once");
    // Creates too: a retried create doesn't become id_taken.
    let id = uuid();
    let create = json!({ "op_id": uuid(), "action": "create", "kind": "task", "record_id": id, "project_id": fx.p1, "fields": { "title": "y" } });
    assert_eq!(a.send(&create), a.send(&create));
    // Someone else can't replay or probe another user's op id.
    assert_eq!(fx.call("POST", &a.path("ops"), Some("admin"), op).code, 409);
    // A conflicting retry returns the same 409, not a fresh evaluation.
    let mut b = Client::new(&fx, "admin");
    b.queue("update", "task", &t, &fx.p1, json!({ "status": "todo" }));
    b.sync();
    let stale = json!({ "op_id": uuid(), "action": "update", "kind": "task", "record_id": t, "base_version": 1, "fields": { "status": "doing" } });
    let r1 = a.send(&stale);
    assert_eq!(r1.code, 409);
    b.queue("update", "task", &t, &fx.p1, json!({ "notes": "moved on" }));
    b.sync();
    assert_eq!(a.send(&stale), r1);
    // Idempotency records expire after 30 days.
    fx.hub.db.lock().unwrap().execute("UPDATE applied_ops SET at = '2000-01-01T00:00:00Z'", []).unwrap();
    assert!(store::prune_ops(&fx.hub.db.lock().unwrap()).unwrap() >= 4);
}

#[test]
fn revocation_mid_queue_rejects_ops_keeps_own_work_and_purges_cache() {
    let fx = fixture();
    let mut a = Client::new(&fx, "contributor");
    let t = a.create("task", &fx.p1, json!({ "title": "mine" }));
    a.sync();
    assert!(!a.cache.is_empty());
    a.online = false;
    a.queue("update", "task", &t, &fx.p1, json!({ "notes": "offline note" }));
    a.create("task", &fx.p1, json!({ "title": "offline task" }));
    // Removed while their edits are still queued.
    let uid = fx.user("contributor").to_string();
    assert_eq!(fx.call("DELETE", &format!("/v1/workspaces/{}/members/{uid}", fx.ws), Some("owner"), json!({})).code, 200);
    a.online = true;
    a.sync();
    assert!(a.revoked);
    assert_eq!(a.unsent.len(), 2, "their own unsent work is kept locally");
    assert!(a.cache.is_empty(), "no removed workspace data stays readable");
    let mut b = Client::new(&fx, "admin");
    b.sync();
    assert_eq!(b.cache.len(), 1, "nothing from the queue reached the hub");
    assert_eq!(b.field(&t, "notes"), Value::Null);
}

#[test]
fn delete_beats_edit_and_tombstones_carry_no_content() {
    let fx = fixture();
    let mut a = Client::new(&fx, "editor");
    let mut b = Client::new(&fx, "admin");
    let t = a.create("task", &fx.p1, json!({ "title": "secret plan", "notes": "details" }));
    a.sync();
    b.sync();
    b.queue("delete", "task", &t, &fx.p1, json!(null));
    a.queue("update", "task", &t, &fx.p1, json!({ "notes": "more details" }));
    b.sync();
    a.sync();
    assert_eq!(a.conflicts.len(), 1);
    assert_eq!(a.conflicts[0].1["error"], "deleted");
    assert_eq!(a.conflicts[0].0["fields"]["notes"], "more details", "the editor's text is kept as a conflict copy");
    assert_eq!(a.cache[&t]["deleted"], true);
    assert_eq!(a.cache[&t]["fields"], json!({}));
    let audit = fx.call("GET", &a.path("audit"), Some("owner"), json!({}));
    assert!(audit.body.to_string().contains("record.deleted"));
    assert!(!audit.body.to_string().contains("secret plan"));
}

#[test]
fn ops_are_authorized_at_apply_time_per_kind_and_field() {
    let fx = fixture();
    let mut ed = Client::new(&fx, "editor");
    let theirs = ed.create("task", &fx.p1, json!({ "title": "editor's" }));
    ed.sync();
    let mut con = Client::new(&fx, "contributor");
    let own = con.create("task", &fx.p1, json!({ "title": "contributor's" }));
    con.sync();
    con.queue("update", "task", &own, &fx.p1, json!({ "status": "done" }));
    con.queue("update", "task", &theirs, &fx.p1, json!({ "status": "done" }));
    con.queue("update", "task", &own, &fx.p1, json!({ "assignee": fx.user("viewer") }));
    con.queue("delete", "task", &theirs, &fx.p1, json!(null));
    con.sync();
    assert_eq!(con.field(&own, "status"), "done", "edit own");
    assert_eq!(con.rejected.len(), 3, "others' tasks, assigning and deleting others' are refused");
    // Viewers write nothing; scoped members can't touch or even see out-of-scope records.
    let mut v = Client::new(&fx, "viewer");
    v.create("task", &fx.p1, json!({ "title": "no" }));
    v.sync();
    assert_eq!(v.rejected.len(), 1);
    let mut adm = Client::new(&fx, "admin");
    let hidden = adm.create("task", &fx.p2, json!({ "title": "contracts audit" }));
    adm.sync();
    let mut s = Client::new(&fx, "scoped");
    s.sync();
    assert!(!s.cache.contains_key(&hidden));
    // Check the hub's raw feed, not the client's cache (the client also purges, which would mask a hub leak).
    let raw = fx.call("GET", &s.path("changes?since=0&limit=500"), Some("scoped"), json!({}));
    assert!(raw.body["changes"].as_array().unwrap().iter().all(|r| r["project_id"] == fx.p1.as_str()), "feed is scoped in SQL: {}", raw.body);
    assert!(!raw.body.to_string().contains("contracts audit"));
    assert!(s.cache.contains_key(&own));
    let r = fx.call("POST", &s.path("ops"), Some("scoped"), json!({ "op_id": uuid(), "action": "update", "kind": "task", "record_id": hidden, "base_version": 1, "fields": { "title": "x" } }));
    let missing = fx.call("POST", &s.path("ops"), Some("scoped"), json!({ "op_id": uuid(), "action": "update", "kind": "task", "record_id": uuid(), "base_version": 1, "fields": { "title": "x" } }));
    assert_eq!((r.code, &r.body), (404, &missing.body), "hidden record = missing record");
    // A denied task.read empties the feed for that member.
    fx.call("PATCH", &format!("/v1/workspaces/{}/members/{}", fx.ws, fx.user("viewer")), Some("admin"), json!({ "deny": ["task.read"] }));
    let feed = fx.call("GET", &v.path("changes"), Some("viewer"), json!({}));
    assert_eq!(feed.body["changes"], json!([]));
}

#[test]
fn fields_are_allowlisted_and_validated() {
    let fx = fixture();
    let send = |kind: &str, fields: Value| {
        fx.call("POST", &format!("/v1/workspaces/{}/ops", fx.ws), Some("owner"), json!({ "op_id": uuid(), "action": "create", "kind": kind, "record_id": uuid(), "project_id": fx.p1, "fields": fields })).code
    };
    assert_eq!(send("task", json!({ "title": "ok", "status": "doing", "due": "2026-12-01", "assignee": fx.user("viewer") })), 200);
    for bad in [
        json!({}),
        json!({ "title": "" }),
        json!({ "title": "x", "path": "C:\\secret" }),
        json!({ "title": "x", "commit": "abc" }),
        json!({ "title": "x", "status": "maybe" }),
        json!({ "title": "x", "due": "tomorrow" }),
        json!({ "title": "x", "assignee": fx.user("outsider") }),
        json!({ "title": "x", "milestone": uuid() }),
        json!({ "title": "x".repeat(201) }),
    ] {
        assert_eq!(send("task", bad.clone()), 400, "{bad}");
    }
    assert_eq!(send("milestone", json!({ "title": "v1", "progress": 101 })), 400);
    assert_eq!(send("session", json!({ "title": "x" })), 400, "private kinds can't be published");
    assert_eq!(send("transcript", json!({ "title": "x" })), 400);
    let base = json!({ "op_id": uuid(), "action": "create", "kind": "task", "project_id": fx.p1, "fields": { "title": "x" } });
    let mut no_uuid = base.clone();
    no_uuid["record_id"] = json!("not-a-uuid");
    assert_eq!(fx.call("POST", &format!("/v1/workspaces/{}/ops", fx.ws), Some("owner"), no_uuid).code, 400);
    // A task can point at a milestone in the same project.
    let mut c = Client::new(&fx, "owner");
    let m = c.create("milestone", &fx.p1, json!({ "title": "Launch", "progress": 40 }));
    let t = c.create("task", &fx.p1, json!({ "title": "Art", "milestone": m }));
    c.sync();
    assert!(c.rejected.is_empty() && c.field(&t, "milestone") == m.as_str());
    let elsewhere = c.create("task", &fx.p2, json!({ "title": "x", "milestone": m }));
    c.sync();
    assert!(!c.cache.get(&elsewhere).is_some_and(|r| r["version"].as_i64() > Some(0)), "milestone must be in the same project");
}

#[test]
fn scope_changes_purge_and_resync_projects() {
    let fx = fixture();
    let mut adm = Client::new(&fx, "admin");
    let t1 = adm.create("task", &fx.p1, json!({ "title": "site" }));
    let t2 = adm.create("task", &fx.p2, json!({ "title": "contracts" }));
    adm.sync();
    let mut e = Client::new(&fx, "editor");
    assert!(e.cache.contains_key(&t1) && e.cache.contains_key(&t2));
    let uid = fx.user("editor").to_string();
    fx.call("PATCH", &format!("/v1/workspaces/{}/members/{uid}", fx.ws), Some("admin"), json!({ "projects": [fx.p1] }));
    e.sync();
    assert!(e.cache.contains_key(&t1) && !e.cache.contains_key(&t2), "out-of-scope project purged");
    fx.call("PATCH", &format!("/v1/workspaces/{}/members/{uid}", fx.ws), Some("admin"), json!({ "projects": null }));
    e.sync();
    assert!(e.cache.contains_key(&t2), "re-added project's older records come back via resync");
}

#[test]
fn media_upload_record_and_download_are_checked() {
    let fx = fixture();
    let png = b"\x89PNG fake image bytes".to_vec();
    let sha = store::hash_bytes(&png);
    let put = |actor: &str, project: &str, name: &str, body: &[u8]| {
        let auth = format!("Bearer {}", fx.token(actor));
        fx.hub.handle("PUT", &format!("/v1/workspaces/{}/projects/{project}/blobs/{name}", fx.ws), Some(&auth), body).code
    };
    assert_eq!(put("contributor", &fx.p1, &store::hash_bytes(b"other"), &png), 400, "hash mismatch");
    assert_eq!(put("contributor", &fx.p1, "NOTAHASH", &png), 400);
    // Phones upload with "auto" and get the fingerprint back.
    let auth = format!("Bearer {}", fx.token("contributor"));
    let auto = fx.hub.handle("PUT", &format!("/v1/workspaces/{}/projects/{}/blobs/auto", fx.ws, fx.p1), Some(&auth), b"from a phone");
    assert_eq!((auto.code, auto.body["sha256"].as_str()), (200, Some(store::hash_bytes(b"from a phone").as_str())));
    let auth_v = format!("Bearer {}", fx.token("viewer"));
    assert_eq!(fx.hub.handle("PUT", &format!("/v1/workspaces/{}/projects/{}/blobs/auto", fx.ws, fx.p1), Some(&auth_v), b"x").code, 403);
    assert_eq!(put("viewer", &fx.p1, &sha, &png), 403);
    let mut c = Client::new(&fx, "contributor");
    let early = c.create("media", &fx.p1, json!({ "sha256": store::hash_bytes(b"never uploaded"), "name": "x.png" }));
    c.sync();
    assert!(c.rejected.len() == 1 && !c.cache.get(&early).is_some_and(|r| r["version"].as_i64() > Some(0)), "record needs its file first");
    assert_eq!(put("contributor", &fx.p1, &sha, &png), 200);
    let id = c.create("media", &fx.p1, json!({ "sha256": sha, "name": "cover.png", "caption": "draft" }));
    c.sync();
    assert_eq!(c.field(&id, "size"), png.len());
    let get = |actor: &str| fx.call("GET", &format!("/v1/workspaces/{}/media/{id}", fx.ws), Some(actor), json!({}));
    assert_eq!(get("viewer").raw.as_deref(), Some(&png[..]));
    assert_eq!(get("scoped").raw.as_deref(), Some(&png[..]), "p1 is in scope");
    assert_eq!(get("outsider").code, 404);
    // sha256 is fixed once created.
    c.queue("update", "media", &id, &fx.p1, json!({ "sha256": store::hash_bytes(b"swap") }));
    c.sync();
    assert_eq!(c.rejected.len(), 2);
    // A media record in p2 is invisible to the p1-scoped member, exactly like a missing one.
    assert_eq!(put("admin", &fx.p2, &sha, &png), 200);
    let mut a = Client::new(&fx, "admin");
    let hidden = a.create("media", &fx.p2, json!({ "sha256": sha, "name": "contract.png" }));
    a.sync();
    let h = fx.call("GET", &format!("/v1/workspaces/{}/media/{hidden}", fx.ws), Some("scoped"), json!({}));
    let m = fx.call("GET", &format!("/v1/workspaces/{}/media/{}", fx.ws, uuid()), Some("scoped"), json!({}));
    assert_eq!((h.code, &h.body), (404, &m.body));
    // Knowing a hash grants nothing: the outsider (owner of another workspace) and the p1-scoped member can't
    // publish a record naming a file that was uploaded somewhere else.
    let steal = |actor: &str, ws: &str, project: &str| {
        fx.call("POST", &format!("/v1/workspaces/{ws}/ops"), Some(actor), json!({ "op_id": uuid(), "action": "create", "kind": "media", "record_id": uuid(), "project_id": project, "fields": { "sha256": sha, "name": "x" } })).code
    };
    let out_proj = fx.call("POST", &format!("/v1/workspaces/{}/projects", fx.other_ws), Some("outsider"), json!({ "name": "mine" })).body["id"].as_str().unwrap().to_string();
    assert_eq!(steal("outsider", &fx.other_ws, &out_proj), 400, "cross-workspace hash reuse refused");
    let p3 = fx.call("POST", &format!("/v1/workspaces/{}/projects", fx.ws), Some("owner"), json!({ "name": "p3" })).body["id"].as_str().unwrap().to_string();
    assert_eq!(steal("owner", &fx.ws, &p3), 400, "cross-project hash reuse refused, even for the owner");
    // Deleted media can't be downloaded.
    c.queue("delete", "media", &id, &fx.p1, json!(null));
    c.sync();
    assert_eq!(get("viewer").code, 404);
}

#[test]
fn feed_pages_and_is_ordered() {
    let fx = fixture();
    let mut a = Client::new(&fx, "owner");
    let ids: Vec<String> = (0..7).map(|i| a.create("task", &fx.p1, json!({ "title": format!("t{i}") }))).collect();
    a.sync();
    let mut b = Client::new(&fx, "viewer"); // pulls in pages of 2
    b.sync();
    assert!(ids.iter().all(|i| b.cache.contains_key(i)));
    let r = fx.call("GET", &format!("/v1/workspaces/{}/changes?since=0&limit=3", fx.ws), Some("viewer"), json!({}));
    let seqs: Vec<i64> = r.body["changes"].as_array().unwrap().iter().map(|c| c["seq"].as_i64().unwrap()).collect();
    assert_eq!(seqs.len(), 3);
    assert!(seqs.windows(2).all(|w| w[0] < w[1]) && r.body["more"] == true && r.body["next"] == seqs[2]);
}

#[test]
fn migrating_a_v1_hub_backs_it_up_first() {
    let data = std::env::temp_dir().join(format!("ptcc-hub-test-migrate-{}", store::random_hex(4).unwrap()));
    {
        let c = store::open(Some(&data)).unwrap();
        ptcc_hub::api::new_workspace(&c, "Old").unwrap();
        c.execute_batch("DROP TABLE records; DROP TABLE applied_ops; DROP TABLE blobs; DROP TABLE roles; DROP TABLE device_links; ALTER TABLE memberships DROP COLUMN title; ALTER TABLE invitations DROP COLUMN title; PRAGMA user_version = 1;").unwrap();
    }
    let c = store::open(Some(&data)).unwrap();
    assert_eq!(c.query_row("SELECT COUNT(*) FROM records", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    let backups: Vec<String> = std::fs::read_dir(store::backups_dir(&data)).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert!(backups.iter().any(|b| b.starts_with("pre-migrate-v1-")), "{backups:?}");
    drop(c);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn contributors_claim_and_unclaim_but_only_editors_assign_others() {
    let fx = fixture();
    let mut ed = Client::new(&fx, "editor");
    let t = ed.create("task", &fx.p1, json!({ "title": "Design the flyer" }));
    ed.sync();
    let con = fx.user("contributor").to_string();
    let viewer = fx.user("viewer").to_string();
    let mut c = Client::new(&fx, "contributor");
    c.queue("update", "task", &t, &fx.p1, json!({ "assignee": con }));
    c.sync();
    assert!(c.rejected.is_empty(), "claim an unclaimed task");
    assert_eq!(c.field(&t, "assignee"), con.as_str());
    // A second contributor-level claim on a claimed task, or assigning someone else, is refused.
    let mut v = Client::new(&fx, "viewer");
    v.queue("update", "task", &t, &fx.p1, json!({ "assignee": viewer }));
    v.sync();
    assert_eq!(v.rejected.len(), 1, "viewers can't claim");
    c.queue("update", "task", &t, &fx.p1, json!({ "assignee": viewer }));
    c.sync();
    assert_eq!(c.rejected.len(), 1, "contributors can't assign others");
    // Claim + another field at once isn't a plain claim: it needs edit rights on someone else's task.
    c.queue("update", "task", &t, &fx.p1, json!({ "assignee": null, "title": "mine now" }));
    c.sync();
    assert_eq!(c.rejected.len(), 2);
    // Unclaim their own claim.
    c.queue("update", "task", &t, &fx.p1, json!({ "assignee": null }));
    c.sync();
    assert_eq!(c.rejected.len(), 2);
    assert_eq!(c.field(&t, "assignee"), Value::Null);
    // Editors can take over or assign anyone.
    ed.sync();
    ed.queue("update", "task", &t, &fx.p1, json!({ "assignee": viewer }));
    ed.sync();
    assert!(ed.rejected.is_empty() && ed.field(&t, "assignee") == viewer.as_str());
    // ...and a contributor can't claim it away from them.
    c.sync();
    c.queue("update", "task", &t, &fx.p1, json!({ "assignee": con }));
    c.sync();
    assert_eq!(c.rejected.len(), 3);
}

#[test]
fn snapshots_need_doc_publish_and_are_read_with_doc_read() {
    let fx = fixture();
    let snap = br#"{"files":[{"path":"src/main.rs","size":10}],"map":{"nodes":[],"edges":[]}}"#.to_vec();
    let sha = store::hash_bytes(&snap);
    let put = |actor: &str, project: &str| {
        let auth = format!("Bearer {}", fx.token(actor));
        fx.hub.handle("PUT", &format!("/v1/workspaces/{}/projects/{project}/blobs/{sha}", fx.ws), Some(&auth), &snap).code
    };
    assert_eq!(put("contributor", &fx.p1), 200);
    let mut c = Client::new(&fx, "contributor");
    c.create("snapshot", &fx.p1, json!({ "sha256": sha, "files": 1, "dirs": 1 }));
    c.sync();
    assert_eq!(c.rejected.len(), 1, "publishing a snapshot needs doc.publish (editor+)");
    let mut e = Client::new(&fx, "editor");
    let id = e.create("snapshot", &fx.p1, json!({ "sha256": sha, "files": 1, "dirs": 1, "path": "C:/secret" }));
    e.sync();
    assert_eq!(e.rejected.len(), 1, "fields are allowlisted");
    let id2 = e.create("snapshot", &fx.p1, json!({ "sha256": sha, "files": 1, "dirs": 1 }));
    e.sync();
    assert!(e.cache[&id2]["version"] == 1 && !e.cache.get(&id).is_some_and(|r| r["version"].as_i64() > Some(0)));
    let get = |actor: &str| fx.call("GET", &format!("/v1/workspaces/{}/media/{id2}", fx.ws), Some(actor), json!({}));
    assert_eq!(get("viewer").raw.as_deref(), Some(&snap[..]));
    assert_eq!(get("outsider").code, 404);
    // Denying doc.read hides snapshots from the feed and the download.
    fx.call("PATCH", &format!("/v1/workspaces/{}/members/{}", fx.ws, fx.user("viewer")), Some("admin"), json!({ "deny": ["doc.read"] }));
    assert_eq!(get("viewer").code, 404);
    let feed = fx.call("GET", &format!("/v1/workspaces/{}/changes", fx.ws), Some("viewer"), json!({}));
    assert!(!feed.body.to_string().contains(&id2));
}

#[test]
fn boards_share_elements_last_writer_wins_and_only_contributors_draw() {
    let fx = fixture();
    let mut a = Client::new(&fx, "editor");
    let mut b = Client::new(&fx, "contributor");
    let mut v = Client::new(&fx, "viewer");
    let board = a.create("board", &fx.p1, json!({ "title": "Moodboard", "bg": "grid" }));
    a.sync();
    let el = b.create("board_el", &fx.p1, json!({ "board": board, "data": "{\"id\":\"x\",\"version\":1}" }));
    b.sync();
    a.sync();
    assert_eq!(a.field(&el, "board"), json!(board), "the editor sees the contributor's element");
    // Both move the same shape: no conflict, the later write wins.
    a.queue("update", "board_el", &el, &fx.p1, json!({ "data": "{\"id\":\"x\",\"version\":2}" }));
    b.queue("update", "board_el", &el, &fx.p1, json!({ "data": "{\"id\":\"x\",\"version\":3}" }));
    a.sync();
    b.sync();
    a.sync();
    assert!(a.conflicts.is_empty() && b.conflicts.is_empty());
    assert_eq!(a.field(&el, "data"), json!("{\"id\":\"x\",\"version\":3}"));
    // A viewer reads the board but can't draw on it.
    v.sync();
    assert_eq!(v.field(&el, "board"), json!(board));
    let n = v.rejected.len();
    v.queue("update", "board_el", &el, &fx.p1, json!({ "data": "{}" }));
    v.sync();
    assert_eq!(v.rejected.len(), n + 1, "viewer edit refused");
    // Elements must be JSON objects and belong to a board of the same project.
    let n = a.rejected.len();
    a.create("board_el", &fx.p1, json!({ "board": board, "data": "not json" }));
    a.create("board_el", &fx.p2, json!({ "board": board, "data": "{}" }));
    a.sync();
    assert_eq!(a.rejected.len(), n + 2);
    // A saved version (media) names its board; only a board of the same project.
    let sha = store::hash_bytes(b"version");
    let auth = format!("Bearer {}", fx.token("admin"));
    let put = |p: &str| fx.hub.handle("PUT", &format!("/v1/workspaces/{}/projects/{p}/blobs/{sha}", fx.ws), Some(&auth), b"version").code;
    assert_eq!((put(&fx.p1), put(&fx.p2)), (200, 200));
    let ver = a.create("media", &fx.p1, json!({ "sha256": sha, "name": "v.excalidraw", "board": board }));
    a.create("media", &fx.p2, json!({ "sha256": sha, "name": "v.excalidraw", "board": board }));
    a.sync();
    assert_eq!((a.field(&ver, "board"), a.rejected.len()), (json!(board), n + 3));
    // Deleting the board tombstones its elements: their content leaves every cache, and they can't be edited.
    a.queue("delete", "board", &board, &fx.p1, json!(null));
    a.sync();
    b.sync();
    assert_eq!(b.field(&el, "data"), Value::Null, "element content is gone from the other member's cache");
    let feed = fx.call("GET", &format!("/v1/workspaces/{}/changes?since=0", fx.ws), Some("viewer"), json!({}));
    let rec = feed.body["changes"].as_array().unwrap().iter().find(|r| r["id"] == json!(el)).unwrap();
    assert_eq!((rec["deleted"].clone(), rec["fields"].clone()), (json!(true), json!({})));
}

#[test]
fn project_files_are_published_by_owners_and_changed_only_through_accepted_proposals() {
    let fx = fixture();
    let put = |actor: &str, body: &[u8]| {
        let auth = format!("Bearer {}", fx.token(actor));
        let sha = store::hash_bytes(body);
        assert_eq!(fx.hub.handle("PUT", &format!("/v1/workspaces/{}/projects/{}/blobs/{sha}", fx.ws, fx.p1), Some(&auth), body).code, 200);
        sha
    };
    let mut o = Client::new(&fx, "owner");
    let mut c = Client::new(&fx, "contributor");
    let v1 = put("owner", b"logo v1");
    let file = o.create("file", &fx.p1, json!({ "path": "art/logo.txt", "sha256": v1 }));
    o.sync();
    c.sync();
    assert_eq!((c.field(&file, "path"), c.field(&file, "size")), (json!("art/logo.txt"), json!(7)), "teammate sees the shared file");
    let get = |actor: &str, id: &str| fx.call("GET", &format!("/v1/workspaces/{}/media/{id}", fx.ws), Some(actor), json!({}));
    assert_eq!(get("contributor", &file).raw.as_deref(), Some(&b"logo v1"[..]), "and can download it");
    // Teammates can't publish or change shared files directly, and paths stay inside the project.
    let n = c.rejected.len();
    c.create("file", &fx.p1, json!({ "path": "x.txt", "sha256": v1 }));
    c.queue("update", "file", &file, &fx.p1, json!({ "sha256": v1 }));
    for bad in ["../escape.txt", "/abs.txt", "C:/x.txt", ".git/config", "a//b", "a\\b", ".git./hooks/x", ".git /config", "GIT~1/config", "con.txt"] {
        c.create("proposal", &fx.p1, json!({ "path": bad, "sha256": v1, "base": null }));
    }
    c.sync();
    assert_eq!(c.rejected.len(), n + 12);
    // A proposal waits; the proposer can't accept it, the owner can.
    let v2 = put("contributor", b"logo v2, brighter");
    let prop = c.create("proposal", &fx.p1, json!({ "path": "art/logo.txt", "sha256": v2, "base": v1, "note": "brighter" }));
    c.sync();
    let n = c.rejected.len();
    c.queue("update", "proposal", &prop, &fx.p1, json!({ "status": "accepted" }));
    c.sync();
    assert_eq!(c.rejected.len(), n + 1, "proposer can't decide");
    o.sync();
    o.queue("update", "proposal", &prop, &fx.p1, json!({ "status": "accepted" }));
    o.queue("update", "file", &file, &fx.p1, json!({ "sha256": v2 }));
    o.sync();
    c.sync();
    assert_eq!((c.field(&prop, "status"), c.field(&file, "size")), (json!("accepted"), json!(17)), "accepted, and the shared copy moved on");
    // A file can only point at content uploaded into its own project.
    let n = o.rejected.len();
    o.queue("update", "file", &file, &fx.p1, json!({ "sha256": store::hash_bytes(b"never uploaded") }));
    o.sync();
    assert_eq!(o.rejected.len(), n + 1);
    // The proposer may withdraw an open proposal of their own.
    let p2 = c.create("proposal", &fx.p1, json!({ "path": "new/readme.md", "sha256": v2, "base": null }));
    c.sync();
    c.queue("delete", "proposal", &p2, &fx.p1, json!(null));
    c.sync();
    assert_eq!(c.field(&p2, "path"), Value::Null, "withdrawn");
}

#[test]
fn ghost_voice_uses_the_callers_own_rights() {
    let fx = fixture();
    let say = |who: &str, text: &str| {
        let r = fx.call("POST", "/v1/voice", Some(who), json!({ "text": text }));
        assert_eq!(r.code, 200, "{r:?}");
        r.body["say"].as_str().unwrap().to_string()
    };
    let added = say("editor", "Hey Ghost, add task order sample tees for contracts due tomorrow");
    assert!(added.starts_with("Added Order sample tees to FLY WING contracts, due"), "{added}");
    let feed = fx.call("GET", &format!("/v1/workspaces/{}/changes", fx.ws), Some("owner"), json!({}));
    let t = feed.body["changes"].as_array().unwrap().iter().find(|x| x["fields"]["title"] == "Order sample tees").expect("task created").clone();
    assert_eq!(t["project_id"], fx.p2.as_str());
    assert!(t["fields"]["due"].is_string());
    assert!(say("editor", "what's due this week").contains("Order sample tees"));
    assert_eq!(say("editor", "claim the sample tees"), "Order sample tees is yours.");
    assert!(say("editor", "what are my tasks").contains("Order sample tees"));
    assert_eq!(say("viewer", "mark sample tees done"), "Your role can't do that.");
    assert_eq!(say("scoped", "mark sample tees done"), "I couldn't find an open task like sample tees.", "a task outside your scope stays invisible");
    assert_eq!(say("editor", "mark sample tees as done"), "Marked Order sample tees done.");
    assert_eq!(say("editor", "what's blocked"), "Nothing is blocked.");
    assert!(say("editor", "note the drop moves to December").starts_with("Saved your note"));
    assert!(say("editor", "sing").starts_with("I heard: sing."));
    assert_eq!(fx.hub.handle("POST", "/v1/voice", None, b"{\"text\":\"catch me up\"}").code, 401);
}
